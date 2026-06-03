//! 剪切板同步桥接：本机变化 → 广播到所有 peer；远端 ClipboardText → apply_remote。
//!
//! **严格防回环** 的两道防线：
//! 1. `clipboard` crate 内部：写入前先登记 hash，监听器命中相同 hash 不发出本机变化；
//! 2. 本模块的 `parse_remote_clipboard`：收到的 `ClipboardText` 若 `origin == self_id` → 忽略（兜底，防御性）。

use std::sync::Arc;

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use bytes::Bytes;
use lanclip_clipboard::ClipboardService;
use lanclip_domain::{ClipboardPayload, DeviceId};
use lanclip_proto::Msg;
use tokio::sync::RwLock;
use tokio::sync::{broadcast, mpsc};
use tracing::{debug, info, warn};

use crate::clipboard_history::{ClipboardHistory, HistoryEntry};
use crate::config::AppConfig;
use crate::connections::{ConnEvent, ConnectionManager};

/// 启动同步任务（spawn 两个常驻 task）。返回时 sync 已经在跑。
pub fn spawn(
    self_id: DeviceId,
    clipboard: Arc<ClipboardService>,
    local_rx: mpsc::Receiver<ClipboardPayload>,
    conn_mgr: Arc<ConnectionManager>,
    history: Arc<ClipboardHistory>,
    config: Arc<RwLock<AppConfig>>,
) {
    // 出口：本机变化 → broadcast + 记入 history
    tokio::spawn(broadcast_local_changes(
        self_id.clone(),
        local_rx,
        conn_mgr.clone(),
        history.clone(),
        config.clone(),
    ));

    // 入口：远端 ClipboardText/Image → apply_remote + 记入 history
    let events = conn_mgr.subscribe();
    tokio::spawn(apply_remote_clipboards(
        self_id, clipboard, events, history, config,
    ));
}

// ============================================================================
// 出口
// ============================================================================

async fn broadcast_local_changes(
    self_id: DeviceId,
    mut local_rx: mpsc::Receiver<ClipboardPayload>,
    conn_mgr: Arc<ConnectionManager>,
    history: Arc<ClipboardHistory>,
    config: Arc<RwLock<AppConfig>>,
) {
    while let Some(payload) = local_rx.recv().await {
        let cfg = config.read().await.clone();
        if matches!(payload, ClipboardPayload::FileRefs { .. }) && !cfg.show_file_refs {
            continue;
        }
        // 先记 history（即使没 peer 连着也要记）
        history.push(HistoryEntry::new_local(payload.clone()));

        if !cfg.clipboard_sync_enabled || !payload_allowed_by_config(&payload, &cfg) {
            continue;
        }

        let msg = match build_clipboard_msg(&self_id, &payload) {
            Some(m) => m,
            None => continue,
        };
        let peers = conn_mgr.connected_peers().await;
        let mut n = 0usize;
        for peer_id in peers {
            if !cfg.trusted_peers.iter().any(|id| id == &peer_id) {
                continue;
            }
            match conn_mgr.send_control(&peer_id, &msg).await {
                Ok(()) => n += 1,
                Err(e) => warn!("[sync] send to trusted peer {peer_id} failed: {e}"),
            }
        }
        info!(
            "[sync] local clipboard ({} bytes) -> {} peer(s)",
            payload.size(),
            n
        );
    }
    debug!("[sync] local_rx closed");
}

/// 把 `ClipboardPayload` 包成可广播的 `Msg`。
fn build_clipboard_msg(self_id: &DeviceId, payload: &ClipboardPayload) -> Option<Msg> {
    let content_hash = payload.hash().0;
    match payload {
        ClipboardPayload::Text { plain, .. } => Some(Msg::ClipboardText {
            origin: self_id.0.clone(),
            content_hash,
            text: plain.clone(),
        }),
        ClipboardPayload::ImagePng {
            width,
            height,
            data,
        } => Some(Msg::ClipboardImage {
            origin: self_id.0.clone(),
            content_hash,
            width: *width,
            height: *height,
            png_b64: B64.encode(data),
        }),
        ClipboardPayload::FileRefs { .. } => None,
    }
}

fn payload_allowed_by_config(payload: &ClipboardPayload, config: &AppConfig) -> bool {
    match payload {
        ClipboardPayload::Text { .. } => config.sync_text,
        ClipboardPayload::ImagePng { .. } => config.sync_images,
        ClipboardPayload::FileRefs { .. } => false,
    }
}

// ============================================================================
// 入口
// ============================================================================

async fn apply_remote_clipboards(
    self_id: DeviceId,
    clipboard: Arc<ClipboardService>,
    mut events: broadcast::Receiver<ConnEvent>,
    history: Arc<ClipboardHistory>,
    config: Arc<RwLock<AppConfig>>,
) {
    loop {
        match events.recv().await {
            Ok(ConnEvent::ControlMessage { peer_id, msg }) => {
                let cfg = config.read().await.clone();
                if !cfg.clipboard_sync_enabled || !cfg.trusted_peers.iter().any(|id| id == &peer_id)
                {
                    continue;
                }
                let Some(payload) = parse_remote_clipboard(&self_id, msg) else {
                    continue;
                };
                if !payload_allowed_by_config(&payload, &cfg) {
                    continue;
                }
                // 先记 history（即使后面 apply_remote 失败，历史中也保留一份供查阅）
                history.push(HistoryEntry::new_remote(peer_id.clone(), payload.clone()));

                match clipboard.apply_remote(payload).await {
                    Ok(()) => info!("[sync] applied clipboard from {peer_id}"),
                    Err(e) => warn!("[sync] apply_remote from {peer_id} failed: {e}"),
                }
            }
            Ok(_) => {}
            Err(broadcast::error::RecvError::Lagged(n)) => {
                warn!("[sync] event channel lagged: {n}");
            }
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
    debug!("[sync] conn events closed");
}

/// 从收到的 `Msg` 提取需要写入本机剪切板的 payload。
///
/// **关键防回环规则**：`origin == self_id` 的消息一律忽略（兜底，正常情况下不会发生，
/// 因为 cm 不会把消息回送给发出方；但路由错乱或测试场景下保护一下）。
pub(crate) fn parse_remote_clipboard(self_id: &DeviceId, msg: Msg) -> Option<ClipboardPayload> {
    match msg {
        Msg::ClipboardText { origin, text, .. } => {
            if origin == self_id.0 {
                debug!("[sync] ignored self-origin clipboard text");
                return None;
            }
            Some(ClipboardPayload::plain_text(text))
        }
        Msg::ClipboardImage {
            origin,
            width,
            height,
            png_b64,
            ..
        } => {
            if origin == self_id.0 {
                debug!("[sync] ignored self-origin clipboard image");
                return None;
            }
            match B64.decode(png_b64.as_bytes()) {
                Ok(data) => Some(ClipboardPayload::ImagePng {
                    width,
                    height,
                    data: Bytes::from(data),
                }),
                Err(e) => {
                    warn!("[sync] invalid base64 in ClipboardImage: {e}");
                    None
                }
            }
        }
        _ => None,
    }
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn dev_id(s: &str) -> DeviceId {
        DeviceId(s.to_string())
    }

    #[test]
    fn parse_remote_text_other_origin() {
        let me = dev_id("me");
        let msg = Msg::ClipboardText {
            origin: "peer".into(),
            content_hash: "h".into(),
            text: "hi".into(),
        };
        let p = parse_remote_clipboard(&me, msg).expect("should accept");
        match p {
            ClipboardPayload::Text { plain, .. } => assert_eq!(plain, "hi"),
            _ => panic!("expected text"),
        }
    }

    #[test]
    fn parse_remote_text_self_origin_ignored() {
        let me = dev_id("me");
        let msg = Msg::ClipboardText {
            origin: "me".into(),
            content_hash: "h".into(),
            text: "loop?".into(),
        };
        assert!(parse_remote_clipboard(&me, msg).is_none());
    }

    #[test]
    fn parse_remote_image_other_origin() {
        let me = dev_id("me");
        let payload_bytes: Vec<u8> = vec![137, 80, 78, 71, 13, 10, 26, 10]; // PNG magic
        let msg = Msg::ClipboardImage {
            origin: "peer".into(),
            content_hash: "h".into(),
            width: 1,
            height: 1,
            png_b64: B64.encode(&payload_bytes),
        };
        let p = parse_remote_clipboard(&me, msg).expect("should accept");
        match p {
            ClipboardPayload::ImagePng {
                width,
                height,
                data,
            } => {
                assert_eq!(width, 1);
                assert_eq!(height, 1);
                assert_eq!(&data[..], &payload_bytes[..]);
            }
            _ => panic!("expected image"),
        }
    }

    #[test]
    fn parse_remote_image_self_origin_ignored() {
        let me = dev_id("me");
        let msg = Msg::ClipboardImage {
            origin: "me".into(),
            content_hash: "h".into(),
            width: 1,
            height: 1,
            png_b64: B64.encode(b"x"),
        };
        assert!(parse_remote_clipboard(&me, msg).is_none());
    }

    #[test]
    fn parse_remote_image_invalid_base64() {
        let me = dev_id("me");
        let msg = Msg::ClipboardImage {
            origin: "peer".into(),
            content_hash: "h".into(),
            width: 1,
            height: 1,
            png_b64: "!!!not base64!!!".into(),
        };
        assert!(parse_remote_clipboard(&me, msg).is_none());
    }

    #[test]
    fn build_msg_image_roundtrip() {
        let me = dev_id("me");
        let png: Vec<u8> = vec![137, 80, 78, 71, 13, 10, 26, 10, 1, 2, 3, 4];
        let payload = ClipboardPayload::ImagePng {
            width: 2,
            height: 3,
            data: Bytes::from(png.clone()),
        };
        let msg = build_clipboard_msg(&me, &payload).expect("build");
        // 发送端消息不能被自己识别为 remote（origin == me）
        assert!(parse_remote_clipboard(&me, msg.clone()).is_none());
        // 但对端能还原
        let other = dev_id("other");
        let back = parse_remote_clipboard(&other, msg).expect("parse");
        match back {
            ClipboardPayload::ImagePng {
                width,
                height,
                data,
            } => {
                assert_eq!(width, 2);
                assert_eq!(height, 3);
                assert_eq!(&data[..], &png[..]);
            }
            _ => panic!("expected image"),
        }
    }

    #[test]
    fn parse_non_clipboard_msg_returns_none() {
        let me = dev_id("me");
        assert!(parse_remote_clipboard(&me, Msg::Ping { ts: 1 }).is_none());
        assert!(parse_remote_clipboard(
            &me,
            Msg::TransferDone {
                task_id: Uuid::new_v4(),
            }
        )
        .is_none());
    }

    #[test]
    fn build_msg_text() {
        let me = dev_id("me");
        let payload = ClipboardPayload::plain_text("hi");
        let msg = build_clipboard_msg(&me, &payload).expect("should build");
        match msg {
            Msg::ClipboardText {
                origin,
                content_hash,
                text,
            } => {
                assert_eq!(origin, "me");
                assert_eq!(text, "hi");
                assert_eq!(content_hash, payload.hash().0);
            }
            _ => panic!("expected ClipboardText"),
        }
    }
}
