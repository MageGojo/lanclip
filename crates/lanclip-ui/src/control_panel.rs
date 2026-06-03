//! Settings and LAN pairing control panel backed by Tao + Wry.

use lanclip_app::{AppConfig, ClipboardHistory};
use lanclip_domain::DeviceId;
use serde::Serialize;
use tao::{
    dpi::LogicalSize,
    event_loop::{EventLoop, EventLoopProxy},
    window::{Window, WindowBuilder, WindowId},
};
use tracing::warn;
use wry::{Rect, WebView, WebViewBuilder};

use crate::{short_hash, UserEvent};

const CONTROL_W: f64 = 760.0;
const CONTROL_H: f64 = 560.0;

#[derive(Debug, Clone)]
pub enum ControlPanelAction {
    Refresh,
    UpdateSettings(ControlSettingsUpdate),
    PairRequest(String),
    PairConfirm(String),
    PairCancel(String),
}

#[derive(Debug, Clone, Default)]
pub struct ControlSettingsUpdate {
    pub device_name: Option<String>,
    pub clipboard_sync_enabled: Option<bool>,
    pub sync_text: Option<bool>,
    pub sync_images: Option<bool>,
    pub show_file_refs: Option<bool>,
}

pub struct ControlPanel {
    webview: WebView,
    window: Window,
    visible: bool,
}

impl ControlPanel {
    pub fn new(
        event_loop: &EventLoop<UserEvent>,
        proxy: EventLoopProxy<UserEvent>,
    ) -> anyhow::Result<Self> {
        let window = WindowBuilder::new()
            .with_title("lanclip Control")
            .with_inner_size(LogicalSize::new(CONTROL_W, CONTROL_H))
            .with_resizable(true)
            .with_visible(false)
            .with_decorations(true)
            .build(event_loop)?;

        let proxy_for_ipc = proxy.clone();
        let webview = WebViewBuilder::new()
            .with_html(control_html())
            .with_bounds(Rect {
                position: wry::dpi::LogicalPosition::new(0, 0).into(),
                size: wry::dpi::LogicalSize::new(CONTROL_W, CONTROL_H).into(),
            })
            .with_ipc_handler(move |request| {
                if let Some(action) = parse_action(request.body()) {
                    let _ = proxy_for_ipc.send_event(UserEvent::Control(action));
                }
            })
            .build(&window)?;

        Ok(Self {
            webview,
            window,
            visible: false,
        })
    }

    pub fn window_id(&self) -> WindowId {
        self.window.id()
    }

    pub fn show(&mut self, state: ControlState) {
        self.visible = true;
        self.window.set_visible(true);
        self.window.set_focus();
        self.set_state(state);
    }

    pub fn hide(&mut self) {
        self.visible = false;
        self.window.set_visible(false);
    }

    pub fn resize(&self, width: u32, height: u32) {
        if let Err(e) = self.webview.set_bounds(Rect {
            position: wry::dpi::LogicalPosition::new(0, 0).into(),
            size: wry::dpi::LogicalSize::new(width as f64, height as f64).into(),
        }) {
            warn!("control panel resize failed: {e}");
        }
    }

    pub fn set_state(&self, state: ControlState) {
        match serde_json::to_string(&state) {
            Ok(json) => {
                let script = format!(
                    "window.lanclipSetControlState && window.lanclipSetControlState({json});"
                );
                if let Err(e) = self.webview.evaluate_script(&script) {
                    warn!("control panel update failed: {e}");
                }
            }
            Err(e) => warn!("control state json failed: {e}"),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlState {
    pub device_id: String,
    pub short_device_id: String,
    pub device_name: String,
    pub port: u16,
    pub history_count: usize,
    pub connected_count: usize,
    pub clipboard_sync_enabled: bool,
    pub sync_text: bool,
    pub sync_images: bool,
    pub show_file_refs: bool,
    pub peers: Vec<ControlPeer>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlPeer {
    pub id: String,
    pub short_id: String,
    pub connected: bool,
    pub trusted: bool,
    pub code: String,
}

impl ControlState {
    pub fn from_parts(
        config: &AppConfig,
        self_id: &DeviceId,
        port: u16,
        history: &ClipboardHistory,
        connected_peers: Vec<DeviceId>,
    ) -> Self {
        let mut peers: Vec<ControlPeer> = connected_peers
            .into_iter()
            .map(|id| {
                let trusted = config.trusted_peers.iter().any(|trusted| trusted == &id);
                ControlPeer {
                    code: pair_code(self_id, &id),
                    short_id: short_hash(id.as_str()),
                    id: id.0,
                    connected: true,
                    trusted,
                }
            })
            .collect();
        for id in &config.trusted_peers {
            if peers.iter().any(|peer| peer.id == id.0) {
                continue;
            }
            peers.push(ControlPeer {
                id: id.0.clone(),
                short_id: short_hash(id.as_str()),
                connected: false,
                trusted: true,
                code: pair_code(self_id, id),
            });
        }
        peers.sort_by(|a, b| {
            b.connected
                .cmp(&a.connected)
                .then_with(|| a.short_id.cmp(&b.short_id))
        });

        Self {
            device_id: self_id.0.clone(),
            short_device_id: short_hash(self_id.as_str()),
            device_name: config.device_name.clone(),
            port,
            history_count: history.total_count(),
            connected_count: peers.iter().filter(|peer| peer.connected).count(),
            clipboard_sync_enabled: config.clipboard_sync_enabled,
            sync_text: config.sync_text,
            sync_images: config.sync_images,
            show_file_refs: config.show_file_refs,
            peers,
        }
    }
}

pub fn pair_code(a: &DeviceId, b: &DeviceId) -> String {
    let (left, right) = if a <= b {
        (a.as_str(), b.as_str())
    } else {
        (b.as_str(), a.as_str())
    };
    let digest = blake3::hash(format!("lanclip-pair:{left}:{right}").as_bytes());
    let n = u32::from_be_bytes([
        digest.as_bytes()[0],
        digest.as_bytes()[1],
        digest.as_bytes()[2],
        digest.as_bytes()[3],
    ]) % 1_000_000;
    format!("{n:06}")
}

fn parse_action(body: &str) -> Option<ControlPanelAction> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    match v.get("type")?.as_str()? {
        "refresh" => Some(ControlPanelAction::Refresh),
        "settings" => Some(ControlPanelAction::UpdateSettings(ControlSettingsUpdate {
            device_name: v
                .get("deviceName")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            clipboard_sync_enabled: v.get("clipboardSyncEnabled").and_then(|v| v.as_bool()),
            sync_text: v.get("syncText").and_then(|v| v.as_bool()),
            sync_images: v.get("syncImages").and_then(|v| v.as_bool()),
            show_file_refs: v.get("showFileRefs").and_then(|v| v.as_bool()),
        })),
        "pair_request" => v
            .get("peerId")
            .and_then(|v| v.as_str())
            .map(|id| ControlPanelAction::PairRequest(id.to_string())),
        "pair_confirm" => v
            .get("peerId")
            .and_then(|v| v.as_str())
            .map(|id| ControlPanelAction::PairConfirm(id.to_string())),
        "pair_cancel" => v
            .get("peerId")
            .and_then(|v| v.as_str())
            .map(|id| ControlPanelAction::PairCancel(id.to_string())),
        _ => None,
    }
}

fn control_html() -> String {
    r#"<!doctype html>
<html>
<head>
  <meta charset="utf-8">
  <style>
    :root { color-scheme: light dark; font-family: -apple-system, BlinkMacSystemFont, "SF Pro Text", sans-serif; }
    * { box-sizing: border-box; }
    body { margin: 0; min-height: 100vh; background: linear-gradient(135deg, #eef5f8, #f8f8f5); color: #151719; }
    .app { display: grid; grid-template-columns: 220px 1fr; min-height: 100vh; }
    aside { padding: 22px 18px; border-right: 1px solid rgba(0,0,0,.08); background: rgba(255,255,255,.42); backdrop-filter: blur(30px) saturate(1.4); }
    h1 { margin: 0 0 6px; font-size: 22px; letter-spacing: 0; }
    .muted { color: rgba(0,0,0,.54); font-size: 12px; }
    nav { margin-top: 24px; display: grid; gap: 8px; }
    nav button { text-align: left; border: 0; border-radius: 8px; padding: 9px 10px; background: transparent; font-size: 13px; color: inherit; }
    nav button.active { background: rgba(0,122,255,.14); color: #0068d8; }
    main { padding: 22px; overflow: auto; }
    section { display: none; }
    section.active { display: block; }
    .grid { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); gap: 12px; margin-bottom: 18px; }
    .tile, .panel, .peer { border: 1px solid rgba(255,255,255,.55); border-radius: 12px; background: rgba(255,255,255,.46); box-shadow: inset 0 1px rgba(255,255,255,.55), 0 12px 30px rgba(0,0,0,.06); backdrop-filter: blur(34px) saturate(1.7); }
    .tile { padding: 14px; }
    .label { font-size: 11px; color: rgba(0,0,0,.48); margin-bottom: 7px; }
    .value { font-size: 20px; font-weight: 650; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
    .panel { padding: 16px; margin-bottom: 14px; }
    h2 { margin: 0 0 12px; font-size: 15px; }
    .row { display: flex; align-items: center; justify-content: space-between; gap: 14px; padding: 10px 0; border-top: 1px solid rgba(0,0,0,.06); }
    .row:first-of-type { border-top: 0; }
    input[type=text] { width: 260px; height: 30px; border-radius: 8px; border: 1px solid rgba(0,0,0,.12); padding: 0 10px; background: rgba(255,255,255,.42); }
    input[type=checkbox] { width: 16px; height: 16px; }
    .peer { padding: 13px; display: grid; grid-template-columns: 1fr auto; align-items: center; gap: 10px; margin-bottom: 10px; }
    .code { font-family: ui-monospace, SFMono-Regular, Menlo, monospace; font-size: 18px; padding: 5px 8px; border-radius: 7px; background: rgba(255,255,255,.48); }
    .actions { display: flex; gap: 8px; }
    .cmd { border: 1px solid rgba(0,0,0,.10); border-radius: 8px; background: rgba(255,255,255,.44); padding: 7px 10px; font-size: 12px; color: inherit; }
    .primary { background: rgba(0,122,255,.90); color: white; border-color: rgba(0,122,255,.20); }
    @media (prefers-color-scheme: dark) {
      body { background: linear-gradient(135deg, #1d2022, #111315); color: #f4f4f0; }
      aside { background: rgba(18,20,22,.52); border-color: rgba(255,255,255,.08); }
      .muted, .label { color: rgba(255,255,255,.56); }
      .tile, .panel, .peer { border-color: rgba(255,255,255,.10); background: rgba(255,255,255,.08); box-shadow: inset 0 1px rgba(255,255,255,.12), 0 12px 30px rgba(0,0,0,.20); }
      .row { border-color: rgba(255,255,255,.08); }
      input[type=text], .cmd, .code { background: rgba(255,255,255,.08); border-color: rgba(255,255,255,.12); color: inherit; }
    }
  </style>
</head>
<body>
<div class="app">
  <aside>
    <h1>lanclip</h1>
    <div class="muted" id="self">Control panel</div>
    <nav>
      <button class="active" data-tab="status">Status</button>
      <button data-tab="devices">Devices</button>
      <button data-tab="settings">Settings</button>
      <button data-tab="history">History</button>
    </nav>
  </aside>
  <main>
    <section id="status" class="active">
      <div class="grid">
        <div class="tile"><div class="label">Port</div><div class="value" id="port">-</div></div>
        <div class="tile"><div class="label">Connected</div><div class="value" id="connected">-</div></div>
        <div class="tile"><div class="label">History</div><div class="value" id="history">-</div></div>
      </div>
      <div class="panel"><h2>Sync</h2><div id="syncStatus" class="muted"></div></div>
    </section>
    <section id="devices"><div class="panel"><h2>LAN Devices</h2><div id="peers"></div></div></section>
    <section id="settings">
      <div class="panel">
        <h2>Settings</h2>
        <div class="row"><span>Device name</span><input id="deviceName" type="text"></div>
        <label class="row"><span>Clipboard sync</span><input id="clipboardSyncEnabled" type="checkbox"></label>
        <label class="row"><span>Sync text</span><input id="syncText" type="checkbox"></label>
        <label class="row"><span>Sync images</span><input id="syncImages" type="checkbox"></label>
        <label class="row"><span>Show file/folder references</span><input id="showFileRefs" type="checkbox"></label>
        <button class="cmd primary" id="save">Save Settings</button>
      </div>
    </section>
    <section id="history"><div class="panel"><h2>History & Transfer</h2><div id="historyInfo" class="muted"></div></div></section>
  </main>
</div>
<script>
  let state = null;
  const post = (message) => window.ipc && window.ipc.postMessage(JSON.stringify(message));
  document.querySelectorAll('nav button').forEach((button) => button.addEventListener('click', () => {
    document.querySelectorAll('nav button').forEach((b) => b.classList.toggle('active', b === button));
    document.querySelectorAll('section').forEach((section) => section.classList.toggle('active', section.id === button.dataset.tab));
  }));
  function render(next) {
    state = next;
    document.getElementById('self').textContent = `${state.deviceName} · ${state.shortDeviceId}`;
    document.getElementById('port').textContent = state.port;
    document.getElementById('connected').textContent = state.connectedCount;
    document.getElementById('history').textContent = state.historyCount;
    document.getElementById('syncStatus').textContent = state.clipboardSyncEnabled ? 'Clipboard sync is enabled for trusted peers.' : 'Clipboard sync is paused.';
    document.getElementById('historyInfo').textContent = `${state.historyCount} clips saved locally. File and folder references are display-only in this version.`;
    ['deviceName','clipboardSyncEnabled','syncText','syncImages','showFileRefs'].forEach((id) => {
      const el = document.getElementById(id);
      if (el.type === 'checkbox') el.checked = !!state[id]; else el.value = state[id] || '';
    });
    const peers = document.getElementById('peers');
    peers.textContent = '';
    if (!state.peers.length) {
      const empty = document.createElement('div');
      empty.className = 'muted';
      empty.textContent = 'No LAN peers connected yet.';
      peers.appendChild(empty);
    }
    state.peers.forEach((peer) => {
      const row = document.createElement('div');
      row.className = 'peer';
      row.innerHTML = `<div><strong>${peer.shortId}</strong><div class="muted">${peer.connected ? 'connected' : 'offline'} · ${peer.trusted ? 'trusted' : 'not paired'} · code <span class="code">${peer.code}</span></div></div>`;
      const actions = document.createElement('div');
      actions.className = 'actions';
      const request = document.createElement('button');
      request.className = 'cmd';
      request.textContent = 'Show Code';
      request.onclick = () => post({ type: 'pair_request', peerId: peer.id });
      const confirm = document.createElement('button');
      confirm.className = 'cmd primary';
      confirm.textContent = peer.trusted ? 'Trusted' : 'Confirm';
      confirm.disabled = peer.trusted;
      confirm.onclick = () => post({ type: 'pair_confirm', peerId: peer.id });
      const cancel = document.createElement('button');
      cancel.className = 'cmd';
      cancel.textContent = peer.trusted ? 'Forget' : 'Cancel';
      cancel.onclick = () => post({ type: 'pair_cancel', peerId: peer.id });
      actions.append(request, confirm, cancel);
      row.appendChild(actions);
      peers.appendChild(row);
    });
  }
  document.getElementById('save').addEventListener('click', () => post({
    type: 'settings',
    deviceName: document.getElementById('deviceName').value,
    clipboardSyncEnabled: document.getElementById('clipboardSyncEnabled').checked,
    syncText: document.getElementById('syncText').checked,
    syncImages: document.getElementById('syncImages').checked,
    showFileRefs: document.getElementById('showFileRefs').checked
  }));
  window.lanclipSetControlState = render;
  post({ type: 'refresh' });
</script>
</body>
</html>"#.to_string()
}
