//! lanclip 线缆协议：控制 / 数据消息的 JSON 编解码。
//!
//! 这一层 **独立于 domain**，所有线缆字段都用基础类型（String、u32…），
//! 以便协议演化时不影响 domain。app 层负责 wire ↔ domain 的转换。

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

/// 当前协议版本。
pub const PROTOCOL_VERSION: u16 = 1;

/// 最低兼容版本。低于这个直接拒连。
pub const MIN_PROTOCOL_VERSION: u16 = 1;

/// WebSocket 子协议名。
pub const WS_SUBPROTOCOL: &str = "lanclip.v1";

/// 控制连接路径。
pub const WS_PATH_CONTROL: &str = "/lanclip/control";

/// 数据连接路径。
pub const WS_PATH_DATA: &str = "/lanclip/data";

/// 单个 Binary 帧最大字节数（与 WS 默认友好）。
pub const MAX_BINARY_CHUNK: usize = 64 * 1024;

// ============================================================================
// Msg 枚举
// ============================================================================

/// 所有 JSON 控制消息。
///
/// - 控制连接使用：Hello / Ping / Pong / Clipboard* / Transfer*
/// - 数据连接使用：Hello / FileBegin / FileEnd（中间夹 Binary 帧）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Msg {
    /// 每条连接第一帧。
    Hello {
        version: u16,
        role: ConnRole,
        device: DevicePublic,
    },

    /// 应用层心跳（除 WS Ping/Pong 外的双向 keepalive）。
    Ping {
        ts: u64,
    },
    Pong {
        ts: u64,
    },

    // -------- 剪切板（仅控制连接） --------
    /// 文本剪切板更新。
    /// - `origin`：来源 DeviceId（接收方据此防回环）。
    /// - `content_hash`：blake3 hex；接收方写入系统剪切板前先登记此 hash。
    ClipboardText {
        origin: String,
        content_hash: String,
        text: String,
    },

    /// 图片剪切板（PNG，base64 编码）。
    ClipboardImage {
        origin: String,
        content_hash: String,
        width: u32,
        height: u32,
        png_b64: String,
    },

    // -------- 配对（仅控制连接） --------
    PairRequest {
        origin: String,
        code: String,
    },
    PairConfirm {
        origin: String,
        code: String,
    },
    PairCancel {
        origin: String,
    },

    // -------- 文件传输元数据（仅控制连接） --------
    TransferOffer {
        task_id: Uuid,
        entries: Vec<FileEntryMeta>,
        total: u64,
    },
    TransferAccept {
        task_id: Uuid,
    },
    TransferReject {
        task_id: Uuid,
        reason: String,
    },
    TransferDone {
        task_id: Uuid,
    },
    TransferCancel {
        task_id: Uuid,
    },
    /// 接收方上报进度。
    TransferProgress {
        task_id: Uuid,
        bytes_done: u64,
    },

    // -------- 文件流锚定（仅数据连接） --------
    FileBegin {
        task_id: Uuid,
        entry_id: u32,
        rel_path: String,
        size: u64,
    },
    FileEnd {
        task_id: Uuid,
        entry_id: u32,
    },
}

// ============================================================================
// 子类型
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnRole {
    Control,
    Data,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DevicePublic {
    pub id: String,
    pub name: String,
    /// "mac" | "windows" | "linux" | "android" | "unknown"
    /// 协议层用字符串，向前兼容未知值。
    pub os: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntryMeta {
    pub entry_id: u32,
    pub rel_path: String,
    pub size: u64,
}

// ============================================================================
// 编解码
// ============================================================================

#[derive(Debug, Error)]
pub enum ProtoError {
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
}

impl Msg {
    /// 序列化为 JSON 字符串（用于 WS Text 帧）。
    pub fn encode(&self) -> Result<String, ProtoError> {
        Ok(serde_json::to_string(self)?)
    }

    /// 反序列化。
    pub fn decode(s: &str) -> Result<Self, ProtoError> {
        Ok(serde_json::from_str(s)?)
    }
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(msg: &Msg) {
        let s = msg.encode().expect("encode");
        let back = Msg::decode(&s).expect("decode");
        assert_eq!(format!("{msg:?}"), format!("{back:?}"));
    }

    #[test]
    fn hello_roundtrip() {
        roundtrip(&Msg::Hello {
            version: PROTOCOL_VERSION,
            role: ConnRole::Control,
            device: DevicePublic {
                id: "abc".into(),
                name: "MagegojoMac".into(),
                os: "mac".into(),
            },
        });
    }

    #[test]
    fn clipboard_text_roundtrip() {
        roundtrip(&Msg::ClipboardText {
            origin: "device-a".into(),
            content_hash: "deadbeef".into(),
            text: "hello world".into(),
        });
    }

    #[test]
    fn pair_roundtrip() {
        roundtrip(&Msg::PairRequest {
            origin: "device-a".into(),
            code: "123456".into(),
        });
        roundtrip(&Msg::PairConfirm {
            origin: "device-a".into(),
            code: "123456".into(),
        });
        roundtrip(&Msg::PairCancel {
            origin: "device-a".into(),
        });
    }

    #[test]
    fn transfer_offer_roundtrip() {
        roundtrip(&Msg::TransferOffer {
            task_id: Uuid::new_v4(),
            entries: vec![
                FileEntryMeta {
                    entry_id: 0,
                    rel_path: "a.txt".into(),
                    size: 12,
                },
                FileEntryMeta {
                    entry_id: 1,
                    rel_path: "dir/b.bin".into(),
                    size: 1024,
                },
            ],
            total: 1036,
        });
    }

    #[test]
    fn file_begin_end_roundtrip() {
        let task_id = Uuid::new_v4();
        roundtrip(&Msg::FileBegin {
            task_id,
            entry_id: 7,
            rel_path: "x.bin".into(),
            size: 100,
        });
        roundtrip(&Msg::FileEnd {
            task_id,
            entry_id: 7,
        });
    }

    #[test]
    fn tag_naming_is_snake_case() {
        let json = Msg::Hello {
            version: 1,
            role: ConnRole::Data,
            device: DevicePublic {
                id: "1".into(),
                name: "n".into(),
                os: "linux".into(),
            },
        }
        .encode()
        .unwrap();
        // 注意是 hello / data 而不是 Hello / Data
        assert!(json.contains("\"type\":\"hello\""), "got: {json}");
        assert!(json.contains("\"role\":\"data\""), "got: {json}");
    }
}
