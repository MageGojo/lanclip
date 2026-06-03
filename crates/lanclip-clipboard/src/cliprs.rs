//! 非 macOS 平台的剪切板实现 —— 基于 `clipboard-rs`。

use clipboard_rs::common::{RustImage, RustImageData};
use clipboard_rs::{
    Clipboard, ClipboardContext, ClipboardHandler, ClipboardWatcher, ClipboardWatcherContext,
    ContentFormat,
};
use lanclip_domain::ClipboardPayload;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::{cb, check_size, should_emit, ClipError, Result, SharedHash};

// ============================================================================
// 服务
// ============================================================================

pub struct ClipboardService {
    current_hash: SharedHash,
    shutdown: Option<clipboard_rs::WatcherShutdown>,
    _watcher_thread: Option<std::thread::JoinHandle<()>>,
}

impl ClipboardService {
    pub fn start() -> Result<(Self, mpsc::Receiver<ClipboardPayload>)> {
        let current_hash: SharedHash = std::sync::Arc::new(std::sync::Mutex::new(None));
        let (tx, rx) = mpsc::channel::<ClipboardPayload>(8);

        let watcher_ctx = ClipboardContext::new().map_err(cb)?;
        let handler = Handler {
            ctx: watcher_ctx,
            current_hash: current_hash.clone(),
            tx,
        };

        let mut watcher = ClipboardWatcherContext::new().map_err(cb)?;
        let shutdown_raw = watcher.add_handler(handler).get_shutdown_channel();

        let watcher_thread = std::thread::Builder::new()
            .name("lanclip-clipboard-watcher".into())
            .spawn(move || {
                watcher.start_watch();
                debug!("clipboard watcher thread exit");
            })
            .map_err(|e| ClipError::Backend(format!("spawn watcher: {e}")))?;

        info!("clipboard service started (text + image)");

        Ok((
            Self {
                current_hash,
                shutdown: Some(shutdown_raw),
                _watcher_thread: Some(watcher_thread),
            },
            rx,
        ))
    }

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

        tokio::task::spawn_blocking(move || -> Result<()> {
            let ctx = ClipboardContext::new().map_err(cb)?;
            match payload {
                ClipboardPayload::Text { plain, .. } => ctx.set_text(plain).map_err(cb)?,
                ClipboardPayload::ImagePng { data, .. } => {
                    let img = RustImageData::from_bytes(&data).map_err(cb)?;
                    ctx.set_image(img).map_err(cb)?;
                }
                ClipboardPayload::FileRefs { entries } => {
                    let text = entries
                        .iter()
                        .map(|entry| entry.path.to_string_lossy())
                        .collect::<Vec<_>>()
                        .join("\n");
                    ctx.set_text(text).map_err(cb)?;
                }
            }
            Ok(())
        })
        .await
        .map_err(|e| ClipError::Backend(format!("join: {e}")))?
    }

    pub fn shutdown(&mut self) {
        if let Some(s) = self.shutdown.take() {
            s.stop();
        }
    }

    pub fn current_hash(&self) -> Option<lanclip_domain::ContentHash> {
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
// Handler
// ============================================================================

struct Handler {
    ctx: ClipboardContext,
    current_hash: SharedHash,
    tx: mpsc::Sender<ClipboardPayload>,
}

impl ClipboardHandler for Handler {
    fn on_clipboard_change(&mut self) {
        let payload = match read_clipboard(&self.ctx) {
            Some(p) => p,
            None => return,
        };

        if let Err(e) = check_size(&payload) {
            warn!("clipboard payload skipped: {e}");
            return;
        }

        let new_hash = payload.hash();
        let emit = {
            let mut guard = self
                .current_hash
                .lock()
                .expect("clipboard hash mutex poisoned");
            should_emit(&new_hash, &mut guard)
        };

        if emit {
            if let Err(e) = self.tx.blocking_send(payload) {
                warn!("clipboard tx send failed: {e}");
            }
        }
    }
}

// ============================================================================
// 读取 helpers
// ============================================================================

fn read_clipboard(ctx: &ClipboardContext) -> Option<ClipboardPayload> {
    if ctx.has(ContentFormat::Image) {
        if let Ok(img) = ctx.get_image() {
            if !img.is_empty() {
                match img.to_png() {
                    Ok(buf) => {
                        let (width, height) = img.get_size();
                        let data = bytes::Bytes::copy_from_slice(buf.get_bytes());
                        return Some(ClipboardPayload::ImagePng {
                            width,
                            height,
                            data,
                        });
                    }
                    Err(e) => warn!("image to_png failed: {e}"),
                }
            }
        }
    }
    if ctx.has(ContentFormat::Text) {
        if let Ok(s) = ctx.get_text() {
            if !s.is_empty() {
                return Some(ClipboardPayload::plain_text(s));
            }
        }
    }
    None
}
