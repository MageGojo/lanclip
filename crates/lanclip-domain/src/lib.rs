//! lanclip 领域模型：纯数据，不依赖 IO/网络。
//!
//! 这一层是协议层和应用层的公共词汇，应该尽量稳定。

use std::fmt;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ============================================================================
// Device
// ============================================================================

/// 设备唯一标识。启动时生成，持久化到 config，跨重启稳定。
#[derive(Debug, Clone, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DeviceId(pub String);

impl DeviceId {
    pub fn new() -> Self {
        Self(Uuid::new_v4().to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// 操作系统类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OsKind {
    Mac,
    Windows,
    Linux,
    Android,
    Unknown,
}

impl OsKind {
    /// 当前编译目标的 OS。
    pub const fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::Mac
        } else if cfg!(target_os = "windows") {
            Self::Windows
        } else if cfg!(target_os = "linux") {
            Self::Linux
        } else if cfg!(target_os = "android") {
            Self::Android
        } else {
            Self::Unknown
        }
    }
}

impl fmt::Display for OsKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Mac => "mac",
            Self::Windows => "windows",
            Self::Linux => "linux",
            Self::Android => "android",
            Self::Unknown => "unknown",
        };
        f.write_str(s)
    }
}

/// 设备公开信息（可在网络上广播）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Device {
    pub id: DeviceId,
    pub name: String,
    pub os: OsKind,
}

// ============================================================================
// Peer
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerStatus {
    /// mDNS 已发现，未连接。
    Online,
    /// 已建立控制连接，可以传剪切板/发起文件传输。
    Connected,
    /// TTL 过期未刷新。
    Offline,
}

#[derive(Debug, Clone)]
pub struct Peer {
    pub device: Device,
    pub addrs: Vec<SocketAddr>,
    pub status: PeerStatus,
    pub last_seen: Instant,
}

impl Peer {
    pub fn is_stale(&self, ttl: Duration) -> bool {
        self.last_seen.elapsed() > ttl
    }
}

// ============================================================================
// Clipboard
// ============================================================================

/// 剪切板内容指纹（blake3）。用 32 字节十六进制字符串表示，便于跨进程/跨网络比较。
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentHash(pub String);

impl ContentHash {
    /// 计算字节串的 blake3 指纹。
    pub fn of(bytes: &[u8]) -> Self {
        Self(blake3_hex(bytes))
    }
}

/// 剪切板内容（领域层；不带 origin/hash，这是协议层的事）。
///
/// 参照 Maccy：同时保存 plain text、RTF、HTML 等多种表示，
/// 写入剪切板时同时写入所有可用的表示，确保在各类 App 中粘贴都能获得原生体验。
#[derive(Debug, Clone)]
pub enum ClipboardPayload {
    /// 文本内容，可携带 RTF / HTML 格式（从剪切板读取时带回，写入时一起写入）。
    Text {
        plain: String,
        /// 可选：富文本（RTF 格式的原始字节，来自 `public.rtf`）。
        rtf: Option<Vec<u8>>,
        /// 可选：HTML 格式（来自 `public.html`）。
        html: Option<String>,
    },
    ImagePng {
        width: u32,
        height: u32,
        data: Bytes,
    },
    /// Local file/folder references captured from the system pasteboard.
    ///
    /// These are display-only in the current app version: they are saved to
    /// history so the user can inspect what was copied, but they are not sent
    /// over the LAN as clipboard payloads.
    FileRefs { entries: Vec<FileClipboardEntry> },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileClipboardEntry {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    pub size: Option<u64>,
    pub child_count: Option<usize>,
}

impl ClipboardPayload {
    /// 快捷构造纯文本 payload（无富文本）。
    pub fn plain_text(s: impl Into<String>) -> Self {
        Self::Text {
            plain: s.into(),
            rtf: None,
            html: None,
        }
    }

    /// 快捷构造带 RTF 的文本 payload。
    pub fn rich_text(plain: impl Into<String>, rtf: Vec<u8>) -> Self {
        Self::Text {
            plain: plain.into(),
            rtf: Some(rtf),
            html: None,
        }
    }

    /// 用于 hash 比较与防回环的"规范字节流"。
    /// 仅使用 `plain` 文本或 `data` 图片数据，保证 hash 稳定。
    pub fn canonical_bytes(&self) -> Bytes {
        match self {
            Self::Text { plain, .. } => Bytes::copy_from_slice(plain.as_bytes()),
            Self::ImagePng { data, .. } => data.clone(),
            Self::FileRefs { entries } => {
                let mut out = Vec::new();
                for entry in entries {
                    out.extend_from_slice(entry.path.to_string_lossy().as_bytes());
                    out.push(0);
                    out.extend_from_slice(if entry.is_dir { b"dir" } else { b"file" });
                    out.push(0);
                }
                Bytes::from(out)
            }
        }
    }

    pub fn hash(&self) -> ContentHash {
        ContentHash::of(&self.canonical_bytes())
    }

    pub fn size(&self) -> usize {
        match self {
            Self::Text { plain, .. } => plain.len(),
            Self::ImagePng { data, .. } => data.len(),
            Self::FileRefs { entries } => entries
                .iter()
                .filter_map(|entry| entry.size)
                .fold(0usize, |acc, size| acc.saturating_add(size as usize)),
        }
    }

    pub fn as_path_text(&self) -> Option<String> {
        match self {
            Self::FileRefs { entries } if !entries.is_empty() => Some(
                entries
                    .iter()
                    .map(|entry| entry.path.to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
            _ => None,
        }
    }
}

// ============================================================================
// Transfer
// ============================================================================

pub type TaskId = Uuid;
pub type EntryId = u32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Send,
    Recv,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferState {
    Pending,
    Accepted,
    Running,
    Done,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone)]
pub struct FileEntry {
    pub entry_id: EntryId,
    /// 相对于用户选择根的路径，已规范化（无 `..`、无绝对前缀）。
    pub rel_path: String,
    /// 发送端的源文件绝对路径（仅 Direction::Send 有效）。
    pub source_path: Option<PathBuf>,
    pub size: u64,
}

#[derive(Debug, Clone, Default)]
pub struct TransferProgress {
    pub bytes_done: u64,
    /// 最近一次速度估计（B/s）。
    pub speed_bps: u64,
}

#[derive(Debug, Clone)]
pub struct TransferTask {
    pub id: TaskId,
    pub peer: DeviceId,
    pub direction: Direction,
    pub entries: Vec<FileEntry>,
    pub total_bytes: u64,
    pub state: TransferState,
    pub progress: TransferProgress,
    pub created_at: Instant,
}

// ============================================================================
// helpers
// ============================================================================

fn blake3_hex(bytes: &[u8]) -> String {
    let hash = blake3::hash(bytes);
    hash.to_hex().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_id_unique() {
        let a = DeviceId::new();
        let b = DeviceId::new();
        assert_ne!(a, b);
    }

    #[test]
    fn clipboard_hash_stable() {
        let p = ClipboardPayload::plain_text("hello");
        assert_eq!(p.hash(), p.hash());
    }

    #[test]
    fn clipboard_hash_differs() {
        let a = ClipboardPayload::plain_text("hello");
        let b = ClipboardPayload::plain_text("world");
        assert_ne!(a.hash(), b.hash());
    }
}
