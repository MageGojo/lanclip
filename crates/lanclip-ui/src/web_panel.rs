//! Maccy-style clipboard panel backed by a Tao window + Wry WebView.

use std::collections::HashMap;

use base64::Engine;
use lanclip_domain::ClipboardPayload;
use serde::Serialize;
use tao::{
    dpi::{LogicalPosition, LogicalSize, Position},
    event_loop::{EventLoop, EventLoopProxy},
    window::{Window, WindowBuilder, WindowId},
};
use tracing::warn;
use wry::{Rect, WebView, WebViewBuilder};

use crate::UserEvent;

type C = f64;

const PANEL_W: C = 780.0;
const PANEL_H: C = 520.0;
const MENU_W: C = 392.0;
const COLLAPSED_W: C = MENU_W + 8.0;
const IMAGE_PREVIEW_LIMIT: usize = 6 * 1024 * 1024;

#[derive(Debug, Clone)]
pub enum PanelAction {
    Select(String),
    Clear,
    Hide,
    PreviewVisible(bool),
    PreviewRequest(String),
    OpenControl,
    Quit,
    /// 搜索：空串表示展示全部历史。后端用 SQLite 全文检索返回完整结果。
    Search(String),
}

#[derive(Clone)]
pub struct PanelEntry {
    pub hash: String,
    pub title: String,
    pub subtitle: String,
    pub preview: PanelPreview,
}

#[derive(Clone)]
pub enum PanelPreview {
    Text(String),
    Image {
        data_url: Option<String>,
        label: String,
    },
    Files {
        text: String,
        label: String,
    },
}

pub struct WebPanel {
    webview: WebView,
    window: Window,
    visible: bool,
    preview_expanded: bool,
    previews: HashMap<String, PanelPreview>,
}

impl WebPanel {
    pub fn new(
        event_loop: &EventLoop<UserEvent>,
        proxy: EventLoopProxy<UserEvent>,
    ) -> anyhow::Result<Self> {
        let window = WindowBuilder::new()
            .with_title("lanclip")
            .with_inner_size(LogicalSize::new(PANEL_W, PANEL_H))
            .with_resizable(false)
            .with_visible(false)
            .with_decorations(false)
            .with_always_on_top(true)
            .with_transparent(true)
            .build(event_loop)?;

        #[cfg(target_os = "macos")]
        {
            use tao::platform::macos::WindowExtMacOS;
            window.set_has_shadow(false);
        }

        let proxy_for_ipc = proxy.clone();
        let webview = WebViewBuilder::new()
            .with_html(panel_html())
            .with_transparent(true)
            .with_background_color((0, 0, 0, 0))
            .with_bounds(Rect {
                position: wry::dpi::LogicalPosition::new(0, 0).into(),
                size: wry::dpi::LogicalSize::new(PANEL_W, PANEL_H).into(),
            })
            .with_accept_first_mouse(true)
            .with_ipc_handler(move |request| {
                if let Some(action) = parse_panel_action(request.body()) {
                    let _ = proxy_for_ipc.send_event(UserEvent::Panel(action));
                }
            })
            .build(&window)?;

        Ok(Self {
            webview,
            window,
            visible: false,
            preview_expanded: true,
            previews: HashMap::new(),
        })
    }

    pub fn window_id(&self) -> WindowId {
        self.window.id()
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    pub fn show(&mut self, entries: Vec<PanelEntry>, anchor: Option<(f64, f64, f64, f64)>) {
        self.set_preview_expanded(false);
        let monitor_bounds = self.window.primary_monitor().map(|monitor| {
            let scale = monitor.scale_factor().max(1.0);
            let position = monitor.position();
            let size = monitor.size();
            (
                position.x as C / scale,
                position.y as C / scale,
                size.width as C / scale,
                size.height as C / scale,
            )
        });
        if let Some((x, y)) = panel_position(anchor, self.window.scale_factor(), monitor_bounds) {
            self.window
                .set_outer_position(Position::Logical(LogicalPosition::new(x, y)));
        }

        self.set_entries(entries);
        self.visible = true;
        self.window.set_visible(true);
        self.window.set_focus();
        self.focus_search();
    }

    pub fn hide(&mut self) {
        self.visible = false;
        self.set_preview_expanded(false);
        self.window.set_visible(false);
    }

    pub fn set_preview_expanded(&mut self, expanded: bool) {
        if self.preview_expanded == expanded {
            return;
        }
        self.preview_expanded = expanded;
        let width = if expanded { PANEL_W } else { COLLAPSED_W };
        self.window.set_inner_size(LogicalSize::new(width, PANEL_H));
        if let Err(e) = self.webview.set_bounds(Rect {
            position: wry::dpi::LogicalPosition::new(0, 0).into(),
            size: wry::dpi::LogicalSize::new(width, PANEL_H).into(),
        }) {
            warn!("panel resize failed: {e}");
        }
    }

    pub fn set_entries(&mut self, entries: Vec<PanelEntry>) {
        // 常规刷新：以最新列表为准重建预览缓存（保持有界）。
        self.previews = entries
            .iter()
            .map(|entry| (entry.hash.clone(), entry.preview.clone()))
            .collect();
        self.push_entries(&entries, "window.lanclipSetEntries");
    }

    /// 用后端搜索结果刷新列表（已由 SQLite 过滤，前端直接渲染）。
    pub fn set_search_results(&mut self, entries: Vec<PanelEntry>) {
        // 搜索结果可能含未在主列表里的旧条目，合并进预览缓存以便 hover 预览。
        for entry in &entries {
            self.previews
                .entry(entry.hash.clone())
                .or_insert_with(|| entry.preview.clone());
        }
        self.push_entries(&entries, "window.lanclipSetSearchResults");
    }

    fn push_entries(&mut self, entries: &[PanelEntry], js_fn: &str) {
        let js_entries: Vec<JsEntry> = entries.iter().map(JsEntry::from).collect();
        match serde_json::to_string(&js_entries) {
            Ok(json) => {
                let script = format!("{js_fn}({json});");
                if let Err(e) = self.webview.evaluate_script(&script) {
                    warn!("panel update failed: {e}");
                }
            }
            Err(e) => warn!("panel json failed: {e}"),
        }
    }

    pub fn send_preview(&self, hash: &str) {
        let Some(preview) = self.previews.get(hash) else {
            return;
        };
        let Ok(json) = serde_json::to_string(&JsPreview::from((hash, preview))) else {
            return;
        };
        let script = format!("window.lanclipSetPreview && window.lanclipSetPreview({json});");
        if let Err(e) = self.webview.evaluate_script(&script) {
            warn!("panel preview failed: {e}");
        }
    }

    pub fn focus_search(&self) {
        if let Err(e) = self
            .webview
            .evaluate_script("window.lanclipFocusSearch && window.lanclipFocusSearch();")
        {
            warn!("panel focus failed: {e}");
        }
    }
}

pub fn preview_from_payload(payload: &ClipboardPayload) -> PanelPreview {
    match payload {
        ClipboardPayload::Text { plain, .. } => PanelPreview::Text(plain.clone()),
        ClipboardPayload::ImagePng {
            width,
            height,
            data,
            ..
        } => {
            let label = format!(
                "Image {}x{} - {:.0} KB PNG",
                width,
                height,
                data.len() as f64 / 1024.0
            );
            let data_url = if data.len() <= IMAGE_PREVIEW_LIMIT {
                let encoded = base64::engine::general_purpose::STANDARD.encode(data);
                Some(format!("data:image/png;base64,{encoded}"))
            } else {
                None
            };
            PanelPreview::Image { data_url, label }
        }
        ClipboardPayload::FileRefs { entries } => {
            let text = entries
                .iter()
                .map(|entry| {
                    let kind = if entry.is_dir { "Folder" } else { "File" };
                    let size = entry
                        .size
                        .map(format_bytes)
                        .or_else(|| entry.child_count.map(|n| format!("{n} items")))
                        .unwrap_or_else(|| "unknown size".to_string());
                    format!("{kind}: {}\n{}\n{}", entry.name, entry.path.display(), size)
                })
                .collect::<Vec<_>>()
                .join("\n\n");
            let label = if entries.len() == 1 {
                if entries[0].is_dir {
                    "Folder reference".to_string()
                } else {
                    "File reference".to_string()
                }
            } else {
                format!("{} file references", entries.len())
            };
            PanelPreview::Files { text, label }
        }
    }
}

fn parse_panel_action(body: &str) -> Option<PanelAction> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    match v.get("type")?.as_str()? {
        "select" => v
            .get("hash")
            .and_then(|h| h.as_str())
            .map(|h| PanelAction::Select(h.to_string())),
        "clear" => Some(PanelAction::Clear),
        "hide" => Some(PanelAction::Hide),
        "open_control" => Some(PanelAction::OpenControl),
        "quit" => Some(PanelAction::Quit),
        "preview" => v
            .get("visible")
            .and_then(|visible| visible.as_bool())
            .map(PanelAction::PreviewVisible),
        "preview_request" => v
            .get("hash")
            .and_then(|h| h.as_str())
            .map(|h| PanelAction::PreviewRequest(h.to_string())),
        "search" => Some(PanelAction::Search(
            v.get("query")
                .and_then(|q| q.as_str())
                .unwrap_or("")
                .to_string(),
        )),
        _ => None,
    }
}

fn format_bytes(bytes: u64) -> String {
    if bytes >= 1024 * 1024 * 1024 {
        format!("{:.1} GB", bytes as f64 / 1024.0 / 1024.0 / 1024.0)
    } else if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / 1024.0 / 1024.0)
    } else if bytes >= 1024 {
        format!("{:.0} KB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

fn panel_position(
    anchor: Option<(f64, f64, f64, f64)>,
    scale_factor: f64,
    monitor_bounds: Option<(C, C, C, C)>,
) -> Option<(C, C)> {
    let scale_factor = scale_factor.max(1.0);
    let (preferred_x, preferred_y) = if let Some((ax, ay, aw, ah)) = anchor {
        let ax = ax / scale_factor;
        let ay = ay / scale_factor;
        let aw = aw / scale_factor;
        let ah = ah / scale_factor;
        (ax + aw / 2.0 - MENU_W / 2.0, ay + ah + 8.0)
    } else if let Some((mx, my, mw, _mh)) = monitor_bounds {
        (mx + mw / 2.0 - MENU_W / 2.0, my + 42.0)
    } else {
        (12.0, 42.0)
    };
    let x = if let Some((mx, _my, mw, _mh)) = monitor_bounds {
        let min_x = mx + 8.0;
        let max_x = (mx + mw - PANEL_W - 8.0).max(min_x);
        preferred_x.clamp(min_x, max_x)
    } else {
        preferred_x.max(12.0)
    };
    let y = preferred_y.max(28.0);
    Some((x, y))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct JsEntry {
    hash: String,
    title: String,
    subtitle: String,
    kind: &'static str,
    search_text: String,
    preview_label: Option<String>,
}

impl From<&PanelEntry> for JsEntry {
    fn from(value: &PanelEntry) -> Self {
        match &value.preview {
            PanelPreview::Text(text) => Self {
                hash: value.hash.clone(),
                title: value.title.clone(),
                subtitle: value.subtitle.clone(),
                kind: "text",
                search_text: truncate_string(text, 8192),
                preview_label: None,
            },
            PanelPreview::Image { label, .. } => Self {
                hash: value.hash.clone(),
                title: value.title.clone(),
                subtitle: value.subtitle.clone(),
                kind: "image",
                search_text: label.clone(),
                preview_label: Some(label.clone()),
            },
            PanelPreview::Files { text, label } => Self {
                hash: value.hash.clone(),
                title: value.title.clone(),
                subtitle: value.subtitle.clone(),
                kind: "files",
                search_text: truncate_string(text, 8192),
                preview_label: Some(label.clone()),
            },
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct JsPreview {
    hash: String,
    kind: &'static str,
    text: Option<String>,
    image_src: Option<String>,
    label: Option<String>,
}

impl From<(&str, &PanelPreview)> for JsPreview {
    fn from((hash, preview): (&str, &PanelPreview)) -> Self {
        match preview {
            PanelPreview::Text(text) => Self {
                hash: hash.to_string(),
                kind: "text",
                text: Some(text.clone()),
                image_src: None,
                label: None,
            },
            PanelPreview::Image { data_url, label } => Self {
                hash: hash.to_string(),
                kind: "image",
                text: None,
                image_src: data_url.clone(),
                label: Some(label.clone()),
            },
            PanelPreview::Files { text, label } => Self {
                hash: hash.to_string(),
                kind: "files",
                text: Some(text.clone()),
                image_src: None,
                label: Some(label.clone()),
            },
        }
    }
}

fn truncate_string(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    let mut end = max_bytes;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

fn panel_html() -> String {
    r#"<!doctype html>
<html>
<head>
  <meta charset="utf-8">
  <style>
    :root {
      color-scheme: light dark;
      font-family: -apple-system, BlinkMacSystemFont, "SF Pro Text", "Segoe UI", sans-serif;
      background: transparent;
    }
    * { box-sizing: border-box; }
    html, body {
      width: 100%;
      height: 100%;
      margin: 0;
      overflow: hidden;
      background: transparent !important;
      user-select: none;
    }
    .stage {
      position: relative;
      width: 100vw;
      height: 100vh;
      padding: 4px;
      background: transparent !important;
    }
    .menu {
      width: 392px;
      height: calc(100vh - 8px);
      display: grid;
      grid-template-rows: 44px 1fr;
      overflow: hidden;
      border: 1px solid rgba(255, 255, 255, .52);
      border-radius: 18px;
      background: rgba(242, 243, 245, .78);
      box-shadow: inset 0 1px 0 rgba(255, 255, 255, .80);
      backdrop-filter: blur(20px) saturate(1.6);
      -webkit-backdrop-filter: blur(20px) saturate(1.6);
    }
    .top {
      display: grid;
      grid-template-columns: minmax(0, 1fr) 34px 54px 28px;
      gap: 6px;
      align-items: center;
      padding: 7px;
      border-bottom: 1px solid rgba(255, 255, 255, .46);
    }
    #q {
      width: 100%;
      height: 28px;
      border: 1px solid rgba(255, 255, 255, .78);
      border-radius: 9px;
      padding: 0 10px;
      outline: none;
      font-size: 13px;
      background: rgba(255, 255, 255, .70);
      box-shadow: inset 0 1px 0 rgba(255, 255, 255, .78), 0 1px 7px rgba(0, 0, 0, .035);
      color: rgba(16, 20, 24, .92);
    }
    #q:focus { border-color: rgba(0, 122, 255, .45); box-shadow: 0 0 0 3px rgba(0, 122, 255, .13); }
    button {
      height: 28px;
      border: 1px solid rgba(255, 255, 255, .76);
      border-radius: 9px;
      padding: 0 9px;
      background: rgba(255, 255, 255, .66);
      box-shadow: inset 0 1px 0 rgba(255, 255, 255, .74), 0 1px 6px rgba(0, 0, 0, .035);
      color: rgba(16, 20, 24, .90);
      font-size: 12px;
    }
    button:hover { background: rgba(255, 255, 255, .80); }
    #control {
      display: flex;
      align-items: center;
      justify-content: center;
      gap: 4px;
      width: 34px;
      padding: 0;
      font-size: 12px;
      line-height: 1;
      font-weight: 600;
    }
    #quit {
      width: 28px;
      padding: 0;
      font-size: 17px;
      line-height: 1;
      color: rgba(111, 31, 31, .88);
    }
    #quit:hover {
      background: rgba(255, 236, 236, .82);
      border-color: rgba(255, 176, 176, .72);
    }
    .settings-icon {
      position: relative;
      width: 13px;
      height: 13px;
      border: 1.5px solid currentColor;
      border-radius: 50%;
      opacity: .82;
    }
    .settings-icon::before {
      content: "";
      position: absolute;
      inset: 3px;
      border-radius: 50%;
      background: currentColor;
    }
    .list {
      min-height: 0;
      overflow-y: auto;
      padding: 5px;
    }
    .item {
      height: 44px;
      display: grid;
      grid-template-columns: 28px 1fr;
      grid-template-rows: 18px 15px;
      grid-template-areas: "icon title" "icon subtitle";
      column-gap: 9px;
      align-content: center;
      padding: 0 8px;
      border-radius: 6px;
      cursor: default;
    }
    .item.active {
      background: linear-gradient(145deg, rgba(38, 145, 255, .94), rgba(0, 112, 232, .88));
      box-shadow: inset 0 1px 0 rgba(255, 255, 255, .38), 0 6px 14px rgba(0, 114, 230, .22);
      color: white;
    }
    .kind-icon {
      grid-area: icon;
      align-self: center;
      width: 28px;
      height: 28px;
      display: grid;
      place-items: center;
      border-radius: 7px;
      background: rgba(180, 185, 195, .38);
      box-shadow: inset 0 1px 0 rgba(255, 255, 255, .60), 0 1px 2px rgba(0,0,0,.06);
      color: rgba(50, 55, 65, .75);
      font-size: 12px;
      line-height: 1;
      overflow: hidden;
    }
    .kind-icon img.thumb {
      width: 100%;
      height: 100%;
      object-fit: cover;
      border-radius: 7px;
      display: block;
    }
    .kind-icon svg {
      width: 14px;
      height: 14px;
      flex-shrink: 0;
    }
    .item.active .kind-icon {
      background: rgba(255, 255, 255, .16);
      color: white;
    }
    .title {
      grid-area: title;
      overflow: hidden;
      text-overflow: ellipsis;
      white-space: nowrap;
      font-size: 13px;
      line-height: 17px;
      color: rgba(10, 14, 17, .96);
    }
    .item.active .title { color: white; }
    .subtitle {
      grid-area: subtitle;
      overflow: hidden;
      text-overflow: ellipsis;
      white-space: nowrap;
      font-size: 11px;
      line-height: 15px;
      color: rgba(40, 47, 53, .78);
    }
    .item.active .subtitle { color: rgba(255, 255, 255, .78); }
      .bubble {
      position: absolute;
      left: 410px;
      top: 54px;
      width: 340px;
      max-height: 450px;
      display: none;
      padding: 11px 13px;
      overflow: auto;
      border: 1px solid rgba(255, 255, 255, .52);
      border-radius: 20px;
      background: rgba(242, 243, 245, .78);
      box-shadow:
        inset 0 1px 0 rgba(255, 255, 255, .80),
        0 8px 32px rgba(20, 29, 39, .22),
        0 2px 8px rgba(20, 29, 39, .12);
      backdrop-filter: blur(20px) saturate(1.6);
      -webkit-backdrop-filter: blur(20px) saturate(1.6);
      pointer-events: none;
      cursor: default;
      visibility: hidden;
      opacity: 0;
      transform: translateX(-4px) scale(.985);
      transition: opacity .09s ease, transform .09s ease;
    }
    .bubble.visible {
      display: block;
      visibility: visible;
      pointer-events: auto;
      opacity: 1;
      transform: translateX(0) scale(1);
    }
    .bubble pre {
      margin: 0;
      white-space: pre-wrap;
      word-break: break-word;
      overflow-wrap: anywhere;
      font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
      font-size: 12px;
      line-height: 1.48;
      color: rgba(9, 15, 19, .92);
      text-shadow: 0 1px 0 rgba(255, 255, 255, .56), 0 0 10px rgba(255, 255, 255, .10);
      user-select: text;
    }
    .bubble img {
      display: block;
      max-width: 100%;
      max-height: 350px;
      margin: 0 auto 10px;
      object-fit: contain;
      border-radius: 10px;
      background: rgba(255, 255, 255, .52);
      box-shadow: inset 0 1px 0 rgba(255, 255, 255, .66);
    }
    .empty {
      height: 100%;
      display: grid;
      place-items: center;
      color: #74746f;
      font-size: 13px;
    }
    .meta {
      margin-top: 8px;
      color: #64645f;
      font-size: 12px;
      text-align: center;
    }
    @media (prefers-color-scheme: dark) {
      .menu {
        border-color: rgba(255, 255, 255, .14);
        background: rgba(30, 32, 36, .72);
        backdrop-filter: blur(20px) saturate(1.8);
        -webkit-backdrop-filter: blur(20px) saturate(1.8);
      }
      .top { border-color: rgba(255, 255, 255, .08); }
      #q, button {
        border-color: rgba(255, 255, 255, .12);
        background: rgba(255, 255, 255, .10);
        color: #f0f0ec;
      }
      .bubble {
        border-color: rgba(255, 255, 255, .10);
        background: rgba(30, 32, 36, .72);
        backdrop-filter: blur(20px) saturate(1.8);
        -webkit-backdrop-filter: blur(20px) saturate(1.8);
      }
      .title { color: #f4f4f0; }
      .subtitle, .meta, .empty { color: #aaa9a2; }
      .bubble pre { color: #f0f0ec; }
      .bubble pre { text-shadow: 0 1px 0 rgba(0, 0, 0, .20); }
      .item.active { background: linear-gradient(#0a84ff, #006ed8); }
    }
  </style>
</head>
<body>
  <div class="stage">
    <div class="menu">
      <div class="top">
        <input id="q" autocomplete="off" spellcheck="false" placeholder="Search clipboard history">
        <button id="control" type="button" aria-label="Open Settings"><span class="settings-icon"></span></button>
        <button id="clear" type="button">Clear</button>
        <button id="quit" type="button" aria-label="Quit lanclip">×</button>
      </div>
      <div id="list" class="list"></div>
    </div>
    <div id="bubble" class="bubble"></div>
  </div>
  <script>
    const list = document.getElementById('list');
    const bubble = document.getElementById('bubble');
    const q = document.getElementById('q');
    const clear = document.getElementById('clear');
    const control = document.getElementById('control');
    const quit = document.getElementById('quit');
    let entries = [];
    let filtered = [];
    let active = 0;
    let hovering = false;
    let previewHash = null;
    let hideTimer = null;

    // SVG icons for list items
    const ICON_TEXT = `<svg viewBox="0 0 14 14" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round">
      <path d="M2.5 3.5h9M2.5 6.5h9M2.5 9.5h6"/>
    </svg>`;
    const ICON_IMAGE = `<svg viewBox="0 0 14 14" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
      <rect x="1.5" y="1.5" width="11" height="11" rx="2"/>
      <circle cx="4.5" cy="4.5" r="1"/>
      <path d="M1.5 9.5l3-3 2.5 2.5 2-2 3 3"/>
    </svg>`;
    const ICON_FILE = `<svg viewBox="0 0 14 14" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
      <path d="M3 1.5h5l2.5 2.5V12.5a.5.5 0 0 1-.5.5H3a.5.5 0 0 1-.5-.5v-11A.5.5 0 0 1 3 1.5z"/>
      <path d="M8 1.5V4h2.5"/>
    </svg>`;

    function makeKindIcon(entry) {
      const div = document.createElement('div');
      div.className = 'kind-icon';
      if (entry.kind === 'image' && entry._thumbSrc) {
        const img = document.createElement('img');
        img.className = 'thumb';
        img.src = entry._thumbSrc;
        div.appendChild(img);
      } else if (entry.kind === 'image') {
        div.innerHTML = ICON_IMAGE;
      } else if (entry.kind === 'files') {
        div.innerHTML = ICON_FILE;
      } else {
        div.innerHTML = ICON_TEXT;
      }
      return div;
    }

    function post(message) {
      if (window.ipc && window.ipc.postMessage) window.ipc.postMessage(JSON.stringify(message));
    }

    function setPreviewWindowVisible(visible) {
      post({ type: 'preview', visible });
    }

    function textOf(entry) {
      return [entry.title, entry.subtitle, entry.searchText || '', entry.previewLabel || ''].join(' ').toLowerCase();
    }

    function debounce(fn, delay) {
      let timer = null;
      return (...args) => {
        if (timer) clearTimeout(timer);
        timer = setTimeout(() => fn(...args), delay);
      };
    }

    function applyFilter() {
      const needle = q.value.trim().toLowerCase();
      filtered = needle ? entries.filter((entry) => textOf(entry).includes(needle)) : entries.slice();
      active = Math.min(active, Math.max(filtered.length - 1, 0));
      render();
    }

    function render() {
      list.textContent = '';
      if (!filtered.length) {
        const empty = document.createElement('div');
        empty.className = 'empty';
        empty.textContent = entries.length ? 'No matching clips' : 'No clipboard history yet';
        list.appendChild(empty);
        hidePreview();
        return;
      }
      filtered.forEach((entry, index) => {
        const row = document.createElement('div');
        row.className = 'item' + (index === active ? ' active' : '');
        const iconEl = makeKindIcon(entry);
        const titleEl = document.createElement('div');
        titleEl.className = 'title';
        titleEl.textContent = entry.title;
        const subtitleEl = document.createElement('div');
        subtitleEl.className = 'subtitle';
        subtitleEl.textContent = entry.subtitle;
        row.appendChild(iconEl);
        row.appendChild(titleEl);
        row.appendChild(subtitleEl);
        row.addEventListener('mouseenter', () => {
          clearPendingHide();
          hovering = true;
          active = index;
          renderActive();
          showPreview(entry, row);
        });
        row.addEventListener('mouseleave', () => {
          hovering = false;
          scheduleHidePreview();
        });
        row.addEventListener('click', () => post({ type: 'select', hash: entry.hash }));
        list.appendChild(row);
      });
      hidePreview();
    }

    function renderActive() {
      [...list.children].forEach((child, index) => child.classList.toggle('active', index === active));
    }

    function showPreview(entry, row) {
      setPreviewWindowVisible(true);
      bubble.textContent = '';
      previewHash = entry.hash;
      bubble.style.width = entry.kind === 'image' ? '340px' : 'max-content';
      bubble.style.maxWidth = '340px';
      const loading = document.createElement('div');
      loading.className = 'meta';
      loading.textContent = 'Loading preview...';
      bubble.appendChild(loading);
      bubble.classList.add('visible');
      requestAnimationFrame(() => placeBubble(row));
      post({ type: 'preview_request', hash: entry.hash });
    }

    function renderPreview(payload) {
      if (!payload) return;
      // Update thumbnail cache for image entries
      if (payload.kind === 'image' && payload.imageSrc) {
        const entry = entries.find(e => e.hash === payload.hash);
        if (entry && !entry._thumbSrc) {
          entry._thumbSrc = payload.imageSrc;
          // Update icon in rendered list if visible
          const idx = filtered.findIndex(e => e.hash === payload.hash);
          if (idx >= 0) {
            const rows = list.querySelectorAll('.item');
            if (rows[idx]) {
              const iconEl = rows[idx].querySelector('.kind-icon');
              if (iconEl && !iconEl.querySelector('img.thumb')) {
                const img = document.createElement('img');
                img.className = 'thumb';
                img.src = payload.imageSrc;
                iconEl.innerHTML = '';
                iconEl.appendChild(img);
              }
            }
          }
        }
      }
      if (payload.hash !== previewHash) return;
      bubble.textContent = '';
      bubble.style.width = payload.kind === 'image' ? '340px' : 'max-content';
      bubble.style.maxWidth = '340px';
      if (payload.kind === 'image') {
        if (payload.imageSrc) {
          const img = document.createElement('img');
          img.src = payload.imageSrc;
          bubble.appendChild(img);
        }
        const meta = document.createElement('div');
        meta.className = 'meta';
        meta.textContent = payload.label || 'Image preview unavailable';
        bubble.appendChild(meta);
      } else {
        const pre = document.createElement('pre');
        pre.textContent = payload.text || '';
        bubble.appendChild(pre);
        if (payload.label) {
          const meta = document.createElement('div');
          meta.className = 'meta';
          meta.textContent = payload.label;
          bubble.appendChild(meta);
        }
      }
    }

    function hidePreview() {
      bubble.classList.remove('visible');
      previewHash = null;
      bubble.textContent = '';
      bubble.style.removeProperty('width');
      bubble.style.removeProperty('max-width');
      setPreviewWindowVisible(false);
    }

    function scheduleHidePreview() {
      clearPendingHide();
      hideTimer = setTimeout(() => { if (!hovering) hidePreview(); }, 90);
    }

    function clearPendingHide() {
      if (hideTimer) {
        clearTimeout(hideTimer);
        hideTimer = null;
      }
    }

    function placeBubble(row) {
      const rowRect = row.getBoundingClientRect();
      const margin = 8;
      const maxTop = window.innerHeight - bubble.offsetHeight - margin;
      const top = Math.max(margin, Math.min(rowRect.top - 10, maxTop));
      bubble.style.top = `${top}px`;
    }

    // 本地即时过滤（已加载条目），同时把查询发给后端做 SQLite 全文检索补全历史。
    const debouncedFilter = debounce(() => {
      active = 0;
      hidePreview();
      applyFilter();
    }, 35);
    const debouncedServerSearch = debounce(() => {
      post({ type: 'search', query: q.value.trim() });
    }, 140);
    q.addEventListener('input', () => {
      debouncedFilter();
      debouncedServerSearch();
    });
    q.addEventListener('keydown', (event) => {
      if (event.key === 'Escape') {
        post({ type: 'hide' });
      } else if (event.key === 'ArrowDown') {
        event.preventDefault();
        active = Math.min(active + 1, Math.max(filtered.length - 1, 0));
        render();
      } else if (event.key === 'ArrowUp') {
        event.preventDefault();
        active = Math.max(active - 1, 0);
        render();
      } else if (event.key === 'Enter' && filtered[active]) {
        post({ type: 'select', hash: filtered[active].hash });
      }
    });
    clear.addEventListener('click', () => post({ type: 'clear' }));
    control.addEventListener('click', () => post({ type: 'open_control' }));
    quit.addEventListener('click', () => post({ type: 'quit' }));
    bubble.addEventListener('mouseenter', () => {
      clearPendingHide();
      hovering = true;
    });
    bubble.addEventListener('mouseleave', () => {
      hovering = false;
      scheduleHidePreview();
    });
    bubble.addEventListener('click', () => {
      if (previewHash) post({ type: 'select', hash: previewHash });
    });

    window.lanclipSetEntries = (nextEntries) => {
      entries = Array.isArray(nextEntries) ? nextEntries : [];
      // Reset thumbnails on new entry set
      entries.forEach(e => { e._thumbSrc = null; e._thumbRequested = false; });
      active = 0;
      applyFilter();
      // Pre-request thumbnails for image entries (first 12)
      entries.filter(e => e.kind === 'image').slice(0, 12).forEach(e => {
        e._thumbRequested = true;
        post({ type: 'preview_request', hash: e.hash });
      });
    };
    // Server-side search results: already filtered by backend (SQLite full-text),
    // so render them directly without re-applying the local needle filter.
    window.lanclipSetSearchResults = (nextEntries) => {
      const next = Array.isArray(nextEntries) ? nextEntries : [];
      next.forEach(e => { e._thumbSrc = null; e._thumbRequested = false; });
      entries = next;
      filtered = next.slice();
      active = 0;
      render();
      next.filter(e => e.kind === 'image').slice(0, 12).forEach(e => {
        e._thumbRequested = true;
        post({ type: 'preview_request', hash: e.hash });
      });
    };
    window.lanclipSetPreview = renderPreview;
    window.lanclipFocusSearch = () => {
      q.focus();
      q.select();
    };
  </script>
</body>
</html>"#.to_string()
}
