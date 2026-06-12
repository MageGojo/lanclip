//! macOS 原生剪切板实现 —— 直接调用 NSPasteboard（Maccy 同款方式）。
//!
//! macOS 没有剪切板变化的通知 API，所有剪切板管理器（Maccy、Hammerspoon 等）
//! 都通过轮询 `NSPasteboard.changeCount` 来检测变化。本模块直接使用 `objc2-app-kit`
//! 调用 NSPasteboard，去掉 `clipboard-rs` / `cocoa` 中间层，获得最原生的体验。

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use bytes::Bytes;
use lanclip_domain::{ClipboardPayload, ContentHash, FileClipboardEntry};
use objc2::rc::Retained;
use objc2::ClassType;
use objc2_app_kit::{NSPasteboard, NSPasteboardItem};
use objc2_foundation::{NSData, NSString};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::{check_size, should_emit, ClipError, Result, SharedHash};

/// 轮询间隔，与 Maccy 默认值一致（500 ms）。
const POLL_INTERVAL: Duration = Duration::from_millis(500);

// ============================================================================
// Pasteboard type UTI 常量
// ============================================================================

const TYPE_TEXT: &str = "public.utf8-plain-text";
const TYPE_RTF: &str = "public.rtf";
const TYPE_HTML: &str = "public.html";
const TYPE_PNG: &str = "public.png";
const TYPE_TIFF: &str = "public.tiff";
const TYPE_FILE_URL: &str = "public.file-url";

// ============================================================================
// NSData 读取 helpers
// ============================================================================

/// 从 `Retained<NSData>` 安全地提取 `&[u8]` 切片。
unsafe fn nsdata_bytes(data: &NSData) -> &[u8] {
    let ptr: *const u8 = objc2::msg_send![data, bytes];
    let len: usize = objc2::msg_send![data, length];
    if ptr.is_null() || len == 0 {
        &[]
    } else {
        std::slice::from_raw_parts(ptr, len)
    }
}

/// 从 `&[u8]` 创建 `Retained<NSData>`（通过 `+dataWithBytes:length:` 拷贝数据）。
fn nsdata_from_slice(bytes: &[u8]) -> Retained<NSData> {
    let ptr = bytes.as_ptr();
    let len = bytes.len();
    unsafe { objc2::msg_send![NSData::class(), dataWithBytes: ptr, length: len] }
}

// ============================================================================
// 服务
// ============================================================================

pub struct ClipboardService {
    current_hash: SharedHash,
    running: Arc<StdMutex<bool>>,
    _watcher_thread: Option<std::thread::JoinHandle<()>>,
}

impl ClipboardService {
    /// 启动监听。返回 `(service, local_change_rx)`。
    pub fn start() -> Result<(Self, mpsc::Receiver<ClipboardPayload>)> {
        let current_hash: SharedHash = Arc::new(StdMutex::new(None));
        let (tx, rx) = mpsc::channel::<ClipboardPayload>(8);
        let running = Arc::new(StdMutex::new(true));

        let hash = current_hash.clone();
        let run = running.clone();

        let thread = std::thread::Builder::new()
            .name("lanclip-clipboard-watcher".into())
            .spawn(move || watch_loop(hash, tx, run))
            .map_err(|e| ClipError::Backend(format!("spawn watcher: {e}")))?;

        info!("clipboard service started (macOS native, polling changeCount)");

        Ok((
            Self {
                current_hash,
                running,
                _watcher_thread: Some(thread),
            },
            rx,
        ))
    }

    /// 写入系统剪切板（来自远端 peer）。**严格防回环**：先登记 hash，再写入。
    pub async fn apply_remote(&self, payload: ClipboardPayload) -> Result<()> {
        check_size(&payload)?;

        let new_hash = payload.hash();
        {
            let mut guard = self
                .current_hash
                .lock()
                .expect("clipboard hash mutex poisoned");
            *guard = Some(new_hash);
        }

        tokio::task::spawn_blocking(move || write_to_pasteboard(payload))
            .await
            .map_err(|e| ClipError::Backend(format!("join: {e}")))?
    }

    pub fn shutdown(&mut self) {
        if let Ok(mut r) = self.running.lock() {
            *r = false;
        }
    }

    pub fn current_hash(&self) -> Option<ContentHash> {
        self.current_hash
            .lock()
            .expect("clipboard hash mutex poisoned")
            .clone()
    }
}

impl Drop for ClipboardService {
    fn drop(&mut self) {
        self.shutdown();
    }
}

// ============================================================================
// 轮询循环（在独立 std thread 上执行）
// ============================================================================

fn watch_loop(
    current_hash: SharedHash,
    tx: mpsc::Sender<ClipboardPayload>,
    running: Arc<StdMutex<bool>>,
) {
    let pb = NSPasteboard::generalPasteboard();
    let mut last_count = pb.changeCount();

    loop {
        if !*running.lock().expect("running mutex poisoned") {
            break;
        }

        std::thread::sleep(POLL_INTERVAL);

        let count = pb.changeCount();
        if count == last_count {
            continue;
        }
        last_count = count;

        let payload = match read_from_pasteboard(&pb) {
            Some(p) => p,
            None => continue,
        };

        if let Err(e) = check_size(&payload) {
            warn!("clipboard payload skipped: {e}");
            continue;
        }

        let new_hash = payload.hash();
        let emit = {
            let mut guard = current_hash.lock().expect("clipboard hash poisoned");
            should_emit(&new_hash, &mut guard)
        };

        if emit {
            if let Err(e) = tx.blocking_send(payload) {
                warn!("clipboard tx send failed: {e}");
            }
        }
    }

    debug!("clipboard watcher thread exit");
}

// ============================================================================
// 读取 pasteboard
// ============================================================================

fn read_from_pasteboard(pb: &NSPasteboard) -> Option<ClipboardPayload> {
    // 优先级：image > text（与设计文档 5.4.3 一致）
    read_image(pb)
        .or_else(|| read_files(pb))
        .or_else(|| read_text(pb))
}

fn read_files(pb: &NSPasteboard) -> Option<ClipboardPayload> {
    let type_str = NSString::from_str(TYPE_FILE_URL);
    let entries = read_file_items(pb, &type_str);
    if !entries.is_empty() {
        return Some(ClipboardPayload::FileRefs { entries });
    }

    let url: Option<Retained<NSString>> =
        unsafe { objc2::msg_send![pb, stringForType: &*type_str] };
    let path = file_url_to_path(&url?.to_string())?;
    let entry = file_entry_for_path(&path);
    Some(ClipboardPayload::FileRefs {
        entries: vec![entry],
    })
}

fn read_file_items(pb: &NSPasteboard, type_str: &NSString) -> Vec<FileClipboardEntry> {
    let Some(items) = pb.pasteboardItems() else {
        return Vec::new();
    };
    let mut entries = Vec::new();
    for index in 0..items.count() {
        let item: Retained<NSPasteboardItem> = items.objectAtIndex(index);
        let Some(url) = item.stringForType(type_str) else {
            continue;
        };
        let Some(path) = file_url_to_path(&url.to_string()) else {
            continue;
        };
        entries.push(file_entry_for_path(&path));
    }
    entries
}

fn file_url_to_path(raw: &str) -> Option<PathBuf> {
    let rest = raw.strip_prefix("file://")?;
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    let decoded = percent_decode(rest);
    Some(PathBuf::from(decoded))
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(a), Some(b)) = (hex_value(bytes[i + 1]), hex_value(bytes[i + 2])) {
                out.push((a << 4) | b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_value(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn file_entry_for_path(path: &Path) -> FileClipboardEntry {
    let metadata = std::fs::metadata(path).ok();
    let is_dir = metadata.as_ref().map(|m| m.is_dir()).unwrap_or(false);
    let size = metadata
        .as_ref()
        .and_then(|m| (!m.is_dir()).then_some(m.len()));
    let child_count = if is_dir {
        std::fs::read_dir(path).ok().map(|entries| {
            entries
                .filter_map(std::result::Result::ok)
                .take(10_001)
                .count()
        })
    } else {
        None
    };
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| path.to_string_lossy().into_owned());
    FileClipboardEntry {
        path: path.to_path_buf(),
        name,
        is_dir,
        size,
        child_count,
    }
}

/// 读取文本，同时抓取 RTF / HTML（参照 Maccy：保存所有可用格式）。
fn read_text(pb: &NSPasteboard) -> Option<ClipboardPayload> {
    let type_str = NSString::from_str(TYPE_TEXT);
    let text: Option<Retained<NSString>> =
        unsafe { objc2::msg_send![pb, stringForType: &*type_str] };
    let plain = text.map(|s| s.to_string()).filter(|s| !s.is_empty())?;

    // 尝试读取 RTF
    let rtf = read_data_for_type(pb, TYPE_RTF).map(|d| d.to_vec());

    // 尝试读取 HTML
    let html_type = NSString::from_str(TYPE_HTML);
    let html_ns: Option<Retained<NSString>> =
        unsafe { objc2::msg_send![pb, stringForType: &*html_type] };
    let html = html_ns.map(|s| s.to_string()).filter(|s| !s.is_empty());

    Some(ClipboardPayload::Text { plain, rtf, html })
}

/// 从 pasteboard 读取指定 UTI 类型的原始数据。
fn read_data_for_type(pb: &NSPasteboard, uti: &str) -> Option<Retained<NSData>> {
    let type_str = NSString::from_str(uti);
    let data: Option<Retained<NSData>> = unsafe { objc2::msg_send![pb, dataForType: &*type_str] };
    data.filter(|d| unsafe { nsdata_bytes(d).len() } > 0)
}

fn read_image(pb: &NSPasteboard) -> Option<ClipboardPayload> {
    // 优先尝试 PNG（无损、体积小）
    let png_type = NSString::from_str(TYPE_PNG);
    let png_data: Option<Retained<NSData>> =
        unsafe { objc2::msg_send![pb, dataForType: &*png_type] };
    if let Some(ref data) = png_data {
        let bytes = unsafe { nsdata_bytes(data) };
        if !bytes.is_empty() {
            if let Some((w, h)) = png_dimensions(bytes) {
                return Some(ClipboardPayload::ImagePng {
                    width: w,
                    height: h,
                    data: Bytes::copy_from_slice(bytes),
                });
            }
        }
    }

    // 退而求其次：TIFF → 转 PNG
    let tiff_type = NSString::from_str(TYPE_TIFF);
    let tiff_data: Option<Retained<NSData>> =
        unsafe { objc2::msg_send![pb, dataForType: &*tiff_type] };
    if let Some(ref data) = tiff_data {
        let bytes = unsafe { nsdata_bytes(data) };
        if !bytes.is_empty() {
            return tiff_to_png(bytes);
        }
    }

    None
}

/// 从 PNG 文件头解析宽高（IHDR chunk，偏移 16-23）。
pub(crate) fn png_dimensions(data: &[u8]) -> Option<(u32, u32)> {
    if data.len() < 24 || &data[..8] != b"\x89PNG\r\n\x1a\n" {
        return None;
    }
    let w = u32::from_be_bytes([data[16], data[17], data[18], data[19]]);
    let h = u32::from_be_bytes([data[20], data[21], data[22], data[23]]);
    Some((w, h))
}

/// 将 TIFF 数据解码再编码为 PNG。
fn tiff_to_png(data: &[u8]) -> Option<ClipboardPayload> {
    let img = image::load_from_memory(data).ok()?;
    let (w, h) = (img.width(), img.height());
    let mut buf = std::io::Cursor::new(Vec::new());
    img.write_to(&mut buf, image::ImageFormat::Png).ok()?;
    Some(ClipboardPayload::ImagePng {
        width: w,
        height: h,
        data: Bytes::from(buf.into_inner()),
    })
}

// ============================================================================
// 写入 pasteboard
// ============================================================================

/// 写入系统剪切板（参照 Maccy：写入所有可用的格式表示，确保在各类 App 中粘贴正常）。
///
/// 策略：
/// - 文本：同时写入 `public.utf8-plain-text` + `public.rtf`（无 RTF 时自动生成）+ `public.html`（如有）
/// - 图片：同时写入 `public.png` + `public.tiff`（从 PNG 自动生成）
fn write_to_pasteboard(payload: ClipboardPayload) -> Result<()> {
    let pb = NSPasteboard::generalPasteboard();
    pb.clearContents();

    match payload {
        ClipboardPayload::Text { plain, rtf, html } => {
            // 1. 写入纯文本（永远有）
            let text_ns = NSString::from_str(&plain);
            let text_type_ns = NSString::from_str(TYPE_TEXT);
            let ok: bool =
                unsafe { objc2::msg_send![&*pb, setString: &*text_ns, forType: &*text_type_ns] };
            if !ok {
                return Err(ClipError::Backend("setString for plain text failed".into()));
            }

            // 2. 写入 RTF（从存储中取，或从纯文本生成）
            let rtf_bytes = match rtf {
                Some(data) => data,
                None => generate_rtf_from_plain(&plain),
            };
            let rtf_ns = nsdata_from_slice(&rtf_bytes);
            let rtf_type_ns = NSString::from_str(TYPE_RTF);
            let _: bool =
                unsafe { objc2::msg_send![&*pb, setData: &*rtf_ns, forType: &*rtf_type_ns] };

            // 3. 写入 HTML（如有）
            if let Some(html_str) = html {
                let html_ns = nsdata_from_slice(html_str.as_bytes());
                let html_type_ns = NSString::from_str(TYPE_HTML);
                let _: bool =
                    unsafe { objc2::msg_send![&*pb, setData: &*html_ns, forType: &*html_type_ns] };
            }
        }
        ClipboardPayload::ImagePng { data, .. } => {
            // 写入 PNG
            let png_ns = nsdata_from_slice(&data);
            let png_type_ns = NSString::from_str(TYPE_PNG);
            let ok: bool =
                unsafe { objc2::msg_send![&*pb, setData: &*png_ns, forType: &*png_type_ns] };
            if !ok {
                return Err(ClipError::Backend("setData for PNG failed".into()));
            }

            // 写入 TIFF（从 PNG 生成，确保所有 App 都能读取）
            if let Ok(tiff_bytes) = png_to_tiff(&data) {
                let tiff_ns = nsdata_from_slice(&tiff_bytes);
                let tiff_type_ns = NSString::from_str(TYPE_TIFF);
                let _: bool =
                    unsafe { objc2::msg_send![&*pb, setData: &*tiff_ns, forType: &*tiff_type_ns] };
            }
        }
        ClipboardPayload::FileRefs { entries } => {
            let plain = entries
                .iter()
                .map(|entry| entry.path.to_string_lossy())
                .collect::<Vec<_>>()
                .join("\n");
            let text_ns = NSString::from_str(&plain);
            let text_type_ns = NSString::from_str(TYPE_TEXT);
            let ok: bool =
                unsafe { objc2::msg_send![&*pb, setString: &*text_ns, forType: &*text_type_ns] };
            if !ok {
                return Err(ClipError::Backend("setString for file paths failed".into()));
            }
        }
    }

    Ok(())
}

/// 从纯文本生成最小 RTF 文档，使得在 TextEdit / Mail / Pages 等 App 中都能正常粘贴。
/// 参照 Maccy 的兼容策略。
fn generate_rtf_from_plain(plain: &str) -> Vec<u8> {
    let mut rtf = String::from("{\\rtf1\\ansi\\deff0 {\\fonttbl {\\f0 Helvetica;}}\n\\f0\\fs24 ");
    for c in plain.chars() {
        match c {
            '\\' => rtf.push_str("\\\\"),
            '{' => rtf.push_str("\\{"),
            '}' => rtf.push_str("\\}"),
            '\n' => rtf.push_str("\\line\n"),
            '\r' => {} // 忽略 CR
            c if (c as u32) <= 0x7F => rtf.push(c),
            c => {
                // Unicode: \uNNNN?
                let cp = c as u32;
                if cp <= 0xFFFF {
                    rtf.push_str(&format!("\\u{cp}?"));
                }
            }
        }
    }
    rtf.push('}');
    rtf.into_bytes()
}

/// 将 PNG 数据转换为 TIFF（使用 image crate）。
fn png_to_tiff(png_data: &[u8]) -> std::result::Result<Vec<u8>, image::ImageError> {
    let img = image::load_from_memory(png_data)?;
    let mut buf = std::io::Cursor::new(Vec::new());
    img.write_to(&mut buf, image::ImageFormat::Tiff)?;
    Ok(buf.into_inner())
}
