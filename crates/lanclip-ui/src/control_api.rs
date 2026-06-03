#![allow(dead_code)]

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::hotkey_config;
use lanclip_app::{AppConfig, ClipboardHistory, ConnectionManager, HistoryEntry, Msg};
use lanclip_domain::{ClipboardPayload, DeviceId};
use serde::{Deserialize, Serialize};
use tokio::runtime::Handle;
use tokio::sync::RwLock;
use tracing::warn;

#[derive(Debug, Clone)]
pub struct ControlEndpoint {
    pub base_url: String,
    pub token: String,
}

#[derive(Debug, Clone, Copy)]
pub enum ControlRuntimeEvent {
    MenuHotkeyChanged,
}

pub type RuntimeNotify = Arc<dyn Fn(ControlRuntimeEvent) + Send + Sync + 'static>;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ControlStateDto {
    pub device_id: String,
    pub short_device_id: String,
    pub device_name: String,
    pub language: String,
    pub port: u16,
    pub history_count: usize,
    pub connected_count: usize,
    pub clipboard_sync_enabled: bool,
    pub sync_text: bool,
    pub sync_images: bool,
    pub show_file_refs: bool,
    pub launch_at_login: bool,
    pub menu_hotkey: String,
    pub peers: Vec<ControlPeerDto>,
    pub history: Vec<HistoryItemDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ControlPeerDto {
    pub id: String,
    pub short_id: String,
    pub connected: bool,
    pub trusted: bool,
    pub code: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct HistoryItemDto {
    pub hash: String,
    pub kind: String,
    pub title: String,
    pub detail: String,
    pub source: String,
    pub time: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SettingsPatchDto {
    pub device_name: Option<String>,
    pub clipboard_sync_enabled: Option<bool>,
    pub sync_text: Option<bool>,
    pub sync_images: Option<bool>,
    pub show_file_refs: Option<bool>,
    pub launch_at_login: Option<bool>,
    pub menu_hotkey: Option<String>,
    pub language: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PeerPatchDto {
    pub peer_id: String,
}

pub fn start_control_server(
    self_id: DeviceId,
    lan_port: u16,
    config: Arc<RwLock<AppConfig>>,
    history: Arc<ClipboardHistory>,
    conn_mgr: Arc<ConnectionManager>,
    rt: Handle,
    runtime_notify: Option<RuntimeNotify>,
) -> anyhow::Result<ControlEndpoint> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let addr = listener.local_addr()?;
    let token = make_token(&self_id);
    let server_token = token.clone();

    thread::Builder::new()
        .name("lanclip-control-api".to_string())
        .spawn(move || {
            for stream in listener.incoming() {
                match stream {
                    Ok(stream) => handle_stream(
                        stream,
                        &server_token,
                        &self_id,
                        lan_port,
                        &config,
                        &history,
                        &conn_mgr,
                        &rt,
                        runtime_notify.as_ref(),
                    ),
                    Err(e) => warn!("control api accept failed: {e}"),
                }
            }
        })?;

    Ok(ControlEndpoint {
        base_url: format!("http://{addr}"),
        token,
    })
}

fn make_token(self_id: &DeviceId) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    blake3::hash(format!("{}:{now}:{}", self_id.as_str(), uuid::Uuid::new_v4()).as_bytes())
        .to_hex()
        .to_string()
}

fn handle_stream(
    mut stream: TcpStream,
    token: &str,
    self_id: &DeviceId,
    lan_port: u16,
    config: &Arc<RwLock<AppConfig>>,
    history: &Arc<ClipboardHistory>,
    conn_mgr: &Arc<ConnectionManager>,
    rt: &Handle,
    runtime_notify: Option<&RuntimeNotify>,
) {
    let mut buf = vec![0u8; 1024 * 128];
    let Ok(n) = stream.read(&mut buf) else {
        return;
    };
    let req = String::from_utf8_lossy(&buf[..n]);
    let Some((head, body)) = req.split_once("\r\n\r\n") else {
        write_response(&mut stream, 400, "bad request");
        return;
    };
    let mut lines = head.lines();
    let Some(first) = lines.next() else {
        write_response(&mut stream, 400, "bad request");
        return;
    };
    let parts: Vec<&str> = first.split_whitespace().collect();
    if parts.len() < 2 {
        write_response(&mut stream, 400, "bad request");
        return;
    }
    let authorized = head
        .lines()
        .any(|line| line.eq_ignore_ascii_case(&format!("x-lanclip-token: {token}")));
    if !authorized {
        write_response(&mut stream, 401, "unauthorized");
        return;
    }

    match (parts[0], parts[1]) {
        ("GET", "/state") | ("GET", "/history") => {
            let state = build_state(config, self_id, lan_port, history, conn_mgr, rt);
            write_json(&mut stream, 200, &state);
        }
        ("POST", "/settings") => match serde_json::from_str::<SettingsPatchDto>(body) {
            Ok(patch) => {
                if let Some(v) = patch.launch_at_login {
                    if let Err(e) = set_launch_at_login(v) {
                        write_response(&mut stream, 500, &format!("launch at login failed: {e}"));
                        return;
                    }
                }
                let hotkey_changed = rt.block_on(async {
                    let mut hotkey_changed = false;
                    let mut cfg = config.write().await;
                    if let Some(name) = patch.device_name.map(|s| s.trim().to_string()) {
                        if !name.is_empty() {
                            cfg.device_name = name;
                        }
                    }
                    if let Some(v) = patch.clipboard_sync_enabled {
                        cfg.clipboard_sync_enabled = v;
                    }
                    if let Some(v) = patch.sync_text {
                        cfg.sync_text = v;
                    }
                    if let Some(v) = patch.sync_images {
                        cfg.sync_images = v;
                    }
                    if let Some(v) = patch.show_file_refs {
                        cfg.show_file_refs = v;
                    }
                    if let Some(v) = patch.launch_at_login {
                        cfg.launch_at_login = v;
                    }
                    if let Some(raw) = patch.menu_hotkey {
                        if let Some(normalized) = hotkey_config::normalize_menu_hotkey(&raw) {
                            if cfg.menu_hotkey != normalized {
                                cfg.menu_hotkey = normalized;
                                hotkey_changed = true;
                            }
                        } else {
                            warn!("ignore invalid menu hotkey: {raw}");
                        }
                    }
                    if let Some(lang) = patch.language {
                        cfg.language = if lang == "en" { "en" } else { "zh" }.to_string();
                    }
                    if let Err(e) = cfg.save() {
                        warn!("save config failed: {e}");
                    }
                    hotkey_changed
                });
                if hotkey_changed {
                    if let Some(notify) = runtime_notify {
                        notify(ControlRuntimeEvent::MenuHotkeyChanged);
                    }
                }
                let state = build_state(config, self_id, lan_port, history, conn_mgr, rt);
                write_json(&mut stream, 200, &state);
            }
            Err(_) => write_response(&mut stream, 400, "bad json"),
        },
        ("POST", "/pair/confirm") | ("POST", "/pair/cancel") => {
            match serde_json::from_str::<PeerPatchDto>(body) {
                Ok(peer) => {
                    let peer_id = DeviceId(peer.peer_id);
                    if parts[1].ends_with("confirm") {
                        confirm_peer(config, conn_mgr, rt, self_id, peer_id);
                    } else {
                        cancel_peer(config, conn_mgr, rt, self_id, peer_id);
                    }
                    let state = build_state(config, self_id, lan_port, history, conn_mgr, rt);
                    write_json(&mut stream, 200, &state);
                }
                Err(_) => write_response(&mut stream, 400, "bad json"),
            }
        }
        _ => write_response(&mut stream, 404, "not found"),
    }
}

fn confirm_peer(
    config: &Arc<RwLock<AppConfig>>,
    conn_mgr: &Arc<ConnectionManager>,
    rt: &Handle,
    self_id: &DeviceId,
    peer_id: DeviceId,
) {
    let code = pair_code(self_id, &peer_id);
    rt.block_on(async {
        let mut cfg = config.write().await;
        if !cfg.trusted_peers.iter().any(|id| id == &peer_id) {
            cfg.trusted_peers.push(peer_id.clone());
        }
        if let Err(e) = cfg.save() {
            warn!("save trusted peer failed: {e}");
        }
    });
    let msg = Msg::PairConfirm {
        origin: self_id.0.clone(),
        code,
    };
    let mgr = conn_mgr.clone();
    rt.spawn(async move {
        if let Err(e) = mgr.send_control(&peer_id, &msg).await {
            warn!("pair confirm failed: {e}");
        }
    });
}

fn cancel_peer(
    config: &Arc<RwLock<AppConfig>>,
    conn_mgr: &Arc<ConnectionManager>,
    rt: &Handle,
    self_id: &DeviceId,
    peer_id: DeviceId,
) {
    rt.block_on(async {
        let mut cfg = config.write().await;
        cfg.trusted_peers.retain(|id| id != &peer_id);
        if let Err(e) = cfg.save() {
            warn!("save trusted peer removal failed: {e}");
        }
    });
    let msg = Msg::PairCancel {
        origin: self_id.0.clone(),
    };
    let mgr = conn_mgr.clone();
    rt.spawn(async move {
        if let Err(e) = mgr.send_control(&peer_id, &msg).await {
            warn!("pair cancel failed: {e}");
        }
    });
}

fn build_state(
    config: &Arc<RwLock<AppConfig>>,
    self_id: &DeviceId,
    lan_port: u16,
    history: &Arc<ClipboardHistory>,
    conn_mgr: &Arc<ConnectionManager>,
    rt: &Handle,
) -> ControlStateDto {
    let cfg = rt.block_on(async { config.read().await.clone() });
    let connected = rt.block_on(async { conn_mgr.connected_peers().await });
    let mut peers: Vec<ControlPeerDto> = connected
        .into_iter()
        .map(|id| ControlPeerDto {
            trusted: cfg.trusted_peers.iter().any(|trusted| trusted == &id),
            code: pair_code(self_id, &id),
            short_id: short_hash(id.as_str()),
            id: id.0,
            connected: true,
        })
        .collect();
    for id in &cfg.trusted_peers {
        if !peers.iter().any(|peer| peer.id == id.0) {
            peers.push(ControlPeerDto {
                id: id.0.clone(),
                short_id: short_hash(id.as_str()),
                connected: false,
                trusted: true,
                code: pair_code(self_id, id),
            });
        }
    }
    peers.sort_by(|a, b| {
        b.connected
            .cmp(&a.connected)
            .then_with(|| a.short_id.cmp(&b.short_id))
    });
    let history_items: Vec<HistoryItemDto> = history
        .snapshot()
        .into_iter()
        .take(30)
        .map(history_item)
        .collect();

    ControlStateDto {
        device_id: self_id.0.clone(),
        short_device_id: short_hash(self_id.as_str()),
        device_name: cfg.device_name,
        language: if cfg.language == "en" { "en" } else { "zh" }.to_string(),
        port: lan_port,
        history_count: history.total_count(),
        connected_count: peers.iter().filter(|peer| peer.connected).count(),
        clipboard_sync_enabled: cfg.clipboard_sync_enabled,
        sync_text: cfg.sync_text,
        sync_images: cfg.sync_images,
        show_file_refs: cfg.show_file_refs,
        launch_at_login: cfg.launch_at_login,
        menu_hotkey: cfg.menu_hotkey,
        peers,
        history: history_items,
    }
}

#[cfg(target_os = "macos")]
fn set_launch_at_login(enabled: bool) -> anyhow::Result<()> {
    use std::fs;
    use std::path::Path;
    use std::process::Command;

    const LABEL: &str = "dev.self.lanclip";

    let home = std::env::var_os("HOME").ok_or_else(|| anyhow::anyhow!("HOME is not set"))?;
    let agent_dir = Path::new(&home).join("Library").join("LaunchAgents");
    let plist_path = agent_dir.join(format!("{LABEL}.plist"));

    if enabled {
        fs::create_dir_all(&agent_dir)?;
        let exe = std::env::current_exe()?;
        let exe = xml_escape(&exe.to_string_lossy());
        let plist = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{LABEL}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{exe}</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <false/>
</dict>
</plist>
"#
        );
        fs::write(plist_path, plist)?;
    } else {
        let _ = Command::new("launchctl")
            .arg("bootout")
            .arg(format!("gui/{}", current_uid()))
            .arg(&plist_path)
            .status();
        if plist_path.exists() {
            fs::remove_file(plist_path)?;
        }
    }

    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn set_launch_at_login(enabled: bool) -> anyhow::Result<()> {
    let _ = enabled;
    anyhow::bail!("launch at login is only implemented on macOS")
}

#[cfg(target_os = "macos")]
fn current_uid() -> String {
    std::process::Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "501".to_string())
}

#[cfg(target_os = "macos")]
fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn history_item(entry: HistoryEntry) -> HistoryItemDto {
    let (kind, title, detail) = match &entry.payload {
        ClipboardPayload::Text { plain, .. } => {
            let one_line = plain.replace(['\n', '\r', '\t'], " ");
            let chars = plain.chars().count();
            let lines = plain.lines().count().max(1);
            (
                "text".to_string(),
                truncate(&one_line, 48),
                format!("{chars} chars / {lines} lines"),
            )
        }
        ClipboardPayload::ImagePng {
            width,
            height,
            data,
        } => (
            "image".to_string(),
            format!("Image {width}x{height}"),
            format!("{:.0} KB PNG", data.len() as f64 / 1024.0),
        ),
        ClipboardPayload::FileRefs { entries } => {
            let first = entries
                .first()
                .map(|entry| entry.name.as_str())
                .unwrap_or("File");
            let folders = entries.iter().filter(|entry| entry.is_dir).count();
            let detail = if entries.len() == 1 && folders == 1 {
                entries[0]
                    .child_count
                    .map(|n| format!("folder / {n} items"))
                    .unwrap_or_else(|| "folder".to_string())
            } else {
                format!("{} item(s)", entries.len())
            };
            ("file".to_string(), truncate(first, 48), detail)
        }
    };
    HistoryItemDto {
        hash: entry.hash.0,
        kind,
        title,
        detail,
        source: entry
            .from_peer
            .map(|id| format!("from {}", short_hash(id.as_str())))
            .unwrap_or_else(|| "local".to_string()),
        time: relative_time(entry.timestamp_secs),
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

pub fn short_hash(s: &str) -> String {
    s.chars().take(8).collect()
}

fn truncate(s: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i >= max_chars {
            out.push('…');
            break;
        }
        out.push(c);
    }
    out
}

fn relative_time(ts: u64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(ts);
    let age = now.saturating_sub(ts);
    match age {
        0..=59 => "just now".to_string(),
        60..=3599 => format!("{}m ago", age / 60),
        3600..=86_399 => format!("{}h ago", age / 3600),
        _ => format!("{}d ago", age / 86_400),
    }
}

fn write_json<T: Serialize>(stream: &mut TcpStream, code: u16, value: &T) {
    match serde_json::to_string(value) {
        Ok(body) => write_response_with_type(stream, code, "application/json", &body),
        Err(_) => write_response(stream, 500, "json error"),
    }
}

fn write_response(stream: &mut TcpStream, code: u16, body: &str) {
    write_response_with_type(stream, code, "text/plain; charset=utf-8", body);
}

fn write_response_with_type(stream: &mut TcpStream, code: u16, content_type: &str, body: &str) {
    let reason = match code {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        _ => "Internal Server Error",
    };
    let response = format!(
        "HTTP/1.1 {code} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.as_bytes().len()
    );
    let _ = stream.write_all(response.as_bytes());
}

pub mod client {
    use super::{ControlStateDto, PeerPatchDto, SettingsPatchDto};
    use std::io::{Read, Write};
    use std::net::TcpStream;

    pub fn get_state(base: &str, token: &str) -> anyhow::Result<ControlStateDto> {
        request_json(base, token, "GET", "/state", None)
    }

    pub fn update_settings(
        base: &str,
        token: &str,
        patch: &SettingsPatchDto,
    ) -> anyhow::Result<ControlStateDto> {
        let body = serde_json::to_string(patch)?;
        request_json(base, token, "POST", "/settings", Some(&body))
    }

    pub fn confirm_peer(base: &str, token: &str, peer_id: &str) -> anyhow::Result<ControlStateDto> {
        let body = serde_json::to_string(&PeerPatchDto {
            peer_id: peer_id.to_string(),
        })?;
        request_json(base, token, "POST", "/pair/confirm", Some(&body))
    }

    pub fn cancel_peer(base: &str, token: &str, peer_id: &str) -> anyhow::Result<ControlStateDto> {
        let body = serde_json::to_string(&PeerPatchDto {
            peer_id: peer_id.to_string(),
        })?;
        request_json(base, token, "POST", "/pair/cancel", Some(&body))
    }

    fn request_json(
        base: &str,
        token: &str,
        method: &str,
        path: &str,
        body: Option<&str>,
    ) -> anyhow::Result<ControlStateDto> {
        let addr = base
            .strip_prefix("http://")
            .unwrap_or(base)
            .trim_end_matches('/');
        let mut stream = TcpStream::connect(addr)?;
        let body = body.unwrap_or("");
        let request = format!(
            "{method} {path} HTTP/1.1\r\nHost: {addr}\r\nX-Lanclip-Token: {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.as_bytes().len()
        );
        stream.write_all(request.as_bytes())?;
        let mut response = String::new();
        stream.read_to_string(&mut response)?;
        let Some((head, body)) = response.split_once("\r\n\r\n") else {
            anyhow::bail!("bad response");
        };
        if !head.starts_with("HTTP/1.1 200") {
            anyhow::bail!("control api error: {head}");
        }
        Ok(serde_json::from_str(body)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_defaults_are_serializable() {
        let patch = SettingsPatchDto {
            language: Some("zh".into()),
            launch_at_login: Some(true),
            menu_hotkey: Some("command+KeyV".into()),
            ..Default::default()
        };
        let json = serde_json::to_string(&patch).unwrap();
        assert!(json.contains("language"));
        assert!(json.contains("launchAtLogin"));
        assert!(json.contains("menuHotkey"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn launch_agent_xml_escape_handles_paths() {
        assert_eq!(
            xml_escape("/tmp/A&B <demo> \"lanclip\""),
            "/tmp/A&amp;B &lt;demo&gt; &quot;lanclip&quot;"
        );
    }
}
