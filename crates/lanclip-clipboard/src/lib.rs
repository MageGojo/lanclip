//! 剪切板服务 —— 事件驱动监听 + 严格防回环。
//!
//! 关键不变量（与设计文档 5.4.2 一致）：
//! 1. 写入系统剪切板前 **必须先** 更新 `current_hash`；
//! 2. 监听器命中时若 `new_hash == current_hash` → 跳过广播（含自写入回灌）；
//! 3. 支持文本与 PNG 图片；类型优先级 image > text。
//!
//! macOS 平台使用 `NSPasteboard` 直接调用（Maccy 同款方式）；
//! 其他平台使用 `clipboard-rs`。

use std::sync::{Arc, Mutex as StdMutex};

use lanclip_domain::{ClipboardPayload, ContentHash};
use thiserror::Error;

/// 文本最大 1 MiB。
pub const MAX_TEXT_BYTES: usize = 1024 * 1024;

/// 图片最大 8 MiB。
pub const MAX_IMAGE_BYTES: usize = 8 * 1024 * 1024;

// ============================================================================
// 错误
// ============================================================================

#[derive(Debug, Error)]
pub enum ClipError {
    #[error("clipboard backend: {0}")]
    Backend(String),

    #[error("payload too large: {kind} {size} bytes")]
    TooLarge { kind: &'static str, size: usize },
}

pub type Result<T> = std::result::Result<T, ClipError>;

#[cfg(not(target_os = "macos"))]
fn cb<E: std::fmt::Display>(e: E) -> ClipError {
    ClipError::Backend(e.to_string())
}

// ============================================================================
// 共享状态
// ============================================================================

type SharedHash = Arc<StdMutex<Option<ContentHash>>>;

// ============================================================================
// 公共工具函数
// ============================================================================

/// 防回环核心判定（纯函数，便于单测）：
/// - 若 `new_hash` 与 `current` 相同 → 返回 false（跳过，含自写入回灌）；
/// - 否则更新 `current` 为 `new_hash` 并返回 true。
fn should_emit(new_hash: &ContentHash, current: &mut Option<ContentHash>) -> bool {
    if current.as_ref() == Some(new_hash) {
        false
    } else {
        *current = Some(new_hash.clone());
        true
    }
}

fn check_size(payload: &ClipboardPayload) -> Result<()> {
    let size = payload.size();
    match payload {
        ClipboardPayload::Text { .. } if size > MAX_TEXT_BYTES => {
            Err(ClipError::TooLarge { kind: "text", size })
        }
        ClipboardPayload::ImagePng { .. } if size > MAX_IMAGE_BYTES => Err(ClipError::TooLarge {
            kind: "image",
            size,
        }),
        _ => Ok(()),
    }
}

// ============================================================================
// 平台条件模块
// ============================================================================

#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "macos")]
pub use macos::ClipboardService;

#[cfg(not(target_os = "macos"))]
mod cliprs;

#[cfg(not(target_os = "macos"))]
pub use cliprs::ClipboardService;

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_check_text_ok() {
        check_size(&ClipboardPayload::plain_text("hi")).unwrap();
    }

    #[test]
    fn size_check_text_too_large() {
        let p = ClipboardPayload::plain_text("x".repeat(MAX_TEXT_BYTES + 1));
        assert!(matches!(check_size(&p), Err(ClipError::TooLarge { .. })));
    }

    #[test]
    fn size_check_image_ok() {
        let p = ClipboardPayload::ImagePng {
            width: 10,
            height: 10,
            data: bytes::Bytes::from(vec![0u8; 1024]),
        };
        check_size(&p).unwrap();
    }

    #[test]
    fn size_check_image_too_large() {
        let p = ClipboardPayload::ImagePng {
            width: 100,
            height: 100,
            data: bytes::Bytes::from(vec![0u8; MAX_IMAGE_BYTES + 1]),
        };
        assert!(matches!(
            check_size(&p),
            Err(ClipError::TooLarge { kind: "image", .. })
        ));
    }

    #[test]
    fn should_emit_after_apply_remote_blocks_echo() {
        let h_remote = ContentHash::of(b"hello");
        let mut current: Option<ContentHash> = Some(h_remote.clone());
        let h_observed = ContentHash::of(b"hello");
        assert!(!should_emit(&h_observed, &mut current), "must skip echo");
    }

    #[test]
    fn should_emit_new_content_passes() {
        let mut current: Option<ContentHash> = Some(ContentHash::of(b"old"));
        let new_h = ContentHash::of(b"new");
        assert!(should_emit(&new_h, &mut current));
        assert_eq!(current, Some(new_h));
    }

    #[test]
    fn should_emit_first_change_passes() {
        let mut current: Option<ContentHash> = None;
        let new_h = ContentHash::of(b"first");
        assert!(should_emit(&new_h, &mut current));
        assert_eq!(current, Some(new_h));
    }

    #[test]
    fn should_emit_dedup_same_content_twice() {
        let mut current: Option<ContentHash> = None;
        let h = ContentHash::of(b"same");
        assert!(should_emit(&h, &mut current));
        assert!(!should_emit(&h, &mut current));
    }

    #[test]
    fn png_dimensions_ok() {
        // 最小合法 PNG：8 字节签名 + IHDR（13 字节数据 + 12 字节开销）
        let mut data = vec![0u8; 24];
        data[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
        // width = 100 (0x00000064), height = 200 (0x000000C8)
        data[16..20].copy_from_slice(&100u32.to_be_bytes());
        data[20..24].copy_from_slice(&200u32.to_be_bytes());

        #[cfg(target_os = "macos")]
        {
            assert_eq!(macos::png_dimensions(&data), Some((100, 200)));
        }
    }

    #[test]
    fn png_dimensions_bad_magic() {
        let data = vec![0u8; 24];
        #[cfg(target_os = "macos")]
        {
            assert_eq!(macos::png_dimensions(&data), None);
        }
    }
}
