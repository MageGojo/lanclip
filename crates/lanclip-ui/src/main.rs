//! lanclip desktop entry: menu-bar icon, native fallback menu, and searchable panel.

mod control_api;
mod tray;
mod web_panel;

use std::process::Command;
use std::sync::Mutex;

use control_api::{start_control_server, ControlEndpoint};
use global_hotkey::{
    hotkey::{Code, HotKey, Modifiers},
    GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState,
};
use lanclip_app::{logging, Application, ClipboardHistory, HistoryEntry};
use lanclip_clipboard::ClipboardService;
use lanclip_domain::{ClipboardPayload, ContentHash};
use tao::event::{Event, StartCause, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tracing::{error, info, warn};
use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use web_panel::{preview_from_payload, PanelAction, PanelEntry, WebPanel};

#[derive(Debug, Clone)]
enum UserEvent {
    HistoryUpdated,
    Tray(TrayIconEvent),
    HotKey(GlobalHotKeyEvent),
    Panel(PanelAction),
}

fn main() -> anyhow::Result<()> {
    logging::init();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let app = rt.block_on(Application::start())?;
    info!("lanclip up: id={}, port={}", app.self_id, app.listener_port);

    let history = app.clipboard_history.clone();
    let clipboard = app.clipboard.clone();
    let self_id = app.self_id.clone();
    let port = app.listener_port;
    let control_endpoint = start_control_server(
        self_id.clone(),
        port,
        app.config.clone(),
        history.clone(),
        app.conn_mgr.clone(),
        rt.handle().clone(),
    )?;
    let _app = Mutex::new(Some(app));
    let rt_h = rt.handle().clone();
    let _rt = rt;

    let mut el = EventLoopBuilder::<UserEvent>::with_user_event().build();
    #[cfg(target_os = "macos")]
    {
        use tao::platform::macos::{ActivationPolicy, EventLoopExtMacOS};
        el.set_activation_policy(ActivationPolicy::Accessory);
        el.set_activate_ignoring_other_apps(false);
    }
    let px = el.create_proxy();

    {
        let px = px.clone();
        TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
            let _ = px.send_event(UserEvent::Tray(e));
        }));
    }

    let clipboard_hotkey = clipboard_hotkey();
    let hotkey_manager = register_clipboard_hotkey(clipboard_hotkey);
    {
        let px = px.clone();
        GlobalHotKeyEvent::set_event_handler(Some(move |e: GlobalHotKeyEvent| {
            let _ = px.send_event(UserEvent::HotKey(e));
        }));
    }

    {
        let px = px.clone();
        let h = history.clone();
        rt_h.spawn(async move {
            let mut rx = h.subscribe();
            let _ = px.send_event(UserEvent::HistoryUpdated);
            while rx.changed().await.is_ok() {
                if px.send_event(UserEvent::HistoryUpdated).is_err() {
                    break;
                }
            }
        });
    }

    let mut _tray_icon: Option<TrayIcon> = None;
    let mut panel = WebPanel::new(&el, px.clone())?;
    let mut control_child: Option<std::process::Child> = None;

    el.run(move |event, _, cf| {
        *cf = ControlFlow::Wait;
        let _keep_hotkey_manager_alive = &hotkey_manager;

        match event {
            Event::NewEvents(StartCause::Init) => {
                match build_tray(port, &history) {
                    Ok(t) => _tray_icon = Some(t),
                    Err(e) => error!("tray: {e}"),
                }
                #[cfg(target_os = "macos")]
                {
                    objc2_core_foundation::CFRunLoop::main().map(|r| r.wake_up());
                }
            }

            Event::UserEvent(UserEvent::HistoryUpdated) => {
                refresh_tray(&_tray_icon, port, &history);
                if panel.is_visible() {
                    panel.set_entries(build_panel_entries(&history));
                }
            }

            Event::UserEvent(UserEvent::Tray(TrayIconEvent::Click {
                rect,
                button,
                button_state: MouseButtonState::Up,
                ..
            })) if matches!(button, MouseButton::Left | MouseButton::Right) => {
                let anchor = Some((
                    rect.position.x,
                    rect.position.y,
                    rect.size.width as f64,
                    rect.size.height as f64,
                ));
                toggle_panel(&mut panel, &history, anchor);
            }

            Event::UserEvent(UserEvent::HotKey(event))
                if event.id == clipboard_hotkey.id() && event.state == HotKeyState::Released =>
            {
                toggle_panel(&mut panel, &history, None);
            }

            Event::UserEvent(UserEvent::Panel(action)) => match action {
                PanelAction::Select(hash) => {
                    panel.hide();
                    if copy_history_entry(&hash, &history, &clipboard, &rt_h) {
                        refresh_tray(&_tray_icon, port, &history);
                    }
                }
                PanelAction::Clear => {
                    history.clear();
                    panel.set_entries(Vec::new());
                    refresh_tray(&_tray_icon, port, &history);
                }
                PanelAction::Hide => panel.hide(),
                PanelAction::PreviewVisible(visible) => panel.set_preview_expanded(visible),
                PanelAction::PreviewRequest(hash) => panel.send_preview(&hash),
                PanelAction::OpenControl => {
                    panel.hide();
                    launch_control_panel(&control_endpoint, &mut control_child)
                }
            },

            Event::WindowEvent {
                window_id, event, ..
            } if window_id == panel.window_id() => match event {
                WindowEvent::CloseRequested | WindowEvent::Focused(false) => panel.hide(),
                _ => {}
            },

            _ => {}
        }
    });
}

fn clipboard_hotkey() -> HotKey {
    #[cfg(target_os = "macos")]
    let mods = Modifiers::SUPER | Modifiers::SHIFT;
    #[cfg(not(target_os = "macos"))]
    let mods = Modifiers::CONTROL | Modifiers::SHIFT;
    HotKey::new(Some(mods), Code::KeyV)
}

fn register_clipboard_hotkey(hotkey: HotKey) -> Option<GlobalHotKeyManager> {
    let manager = match GlobalHotKeyManager::new() {
        Ok(manager) => manager,
        Err(e) => {
            warn!("global hotkey manager unavailable: {e}");
            return None;
        }
    };
    match manager.register(hotkey) {
        Ok(()) => {
            info!("global clipboard hotkey registered: {hotkey}");
            Some(manager)
        }
        Err(e) => {
            warn!("global clipboard hotkey unavailable ({hotkey}): {e}");
            None
        }
    }
}

fn toggle_panel(
    panel: &mut WebPanel,
    history: &ClipboardHistory,
    anchor: Option<(f64, f64, f64, f64)>,
) {
    if panel.is_visible() {
        panel.hide();
    } else {
        panel.show(build_panel_entries(history), anchor);
    }
}

fn build_panel_entries(history: &ClipboardHistory) -> Vec<PanelEntry> {
    history
        .snapshot()
        .into_iter()
        .take(80)
        .map(panel_entry_for_web_panel)
        .collect()
}

fn panel_entry_for_web_panel(e: HistoryEntry) -> PanelEntry {
    let (title, detail) = history_entry_title_detail(&e);
    let source = e
        .from_peer
        .as_ref()
        .map(|id| format!("from {}", short_hash(id.as_str())))
        .unwrap_or_else(|| "local".to_string());
    let subtitle = format!(
        "{detail}  ·  {source}  ·  {}",
        relative_time(e.timestamp_secs)
    );

    PanelEntry {
        hash: e.hash.0,
        title,
        subtitle,
        preview: preview_from_payload(&e.payload),
    }
}

fn history_entry_title_detail(e: &HistoryEntry) -> (String, String) {
    let (title, detail) = match &e.payload {
        ClipboardPayload::Text { plain, .. } => {
            let one_line = plain.replace(['\n', '\r', '\t'], " ");
            let title = if one_line.trim().is_empty() {
                "[empty text]".to_string()
            } else {
                truncate_chars(&one_line, 38)
            };
            let chars = plain.chars().count();
            let lines = plain.lines().count().max(1);
            let detail = if lines > 1 {
                format!("{chars} chars / {lines} lines")
            } else {
                format!("{chars} chars")
            };
            (title, detail)
        }
        ClipboardPayload::ImagePng {
            width,
            height,
            data,
            ..
        } => {
            let kb = data.len() as f64 / 1024.0;
            (
                format!("Image {}x{}", width, height),
                format!("{kb:.0} KB PNG"),
            )
        }
        ClipboardPayload::FileRefs { entries } => {
            let first = entries
                .first()
                .map(|entry| entry.name.as_str())
                .unwrap_or("File");
            let title = if entries.len() == 1 {
                let kind = if entries[0].is_dir { "Folder" } else { "File" };
                format!("{kind} {first}")
            } else {
                format!("{} files", entries.len())
            };
            let total_size: u64 = entries.iter().filter_map(|entry| entry.size).sum();
            let folders = entries.iter().filter(|entry| entry.is_dir).count();
            let detail = if total_size > 0 {
                format!(
                    "{} · {}",
                    format_file_size(total_size),
                    if folders > 0 {
                        "files/folders"
                    } else {
                        "files"
                    }
                )
            } else if entries.len() == 1 && entries[0].is_dir {
                entries[0]
                    .child_count
                    .map(|n| format!("folder · {n} items"))
                    .unwrap_or_else(|| "folder".to_string())
            } else {
                "file reference".to_string()
            };
            (truncate_chars(&title, 38), detail)
        }
    };
    (title, detail)
}

fn format_file_size(bytes: u64) -> String {
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

fn truncate_chars(s: &str, max_chars: usize) -> String {
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

fn short_hash(s: &str) -> String {
    s.chars().take(8).collect()
}

fn relative_time(ts: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
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

fn launch_control_panel(endpoint: &ControlEndpoint, child: &mut Option<std::process::Child>) {
    if let Some(existing) = child {
        match existing.try_wait() {
            Ok(None) => {
                focus_process(existing.id());
                return;
            }
            Ok(Some(_)) | Err(_) => *child = None,
        }
    }

    let exe = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|dir| dir.join("lanclip-control")));
    let spawned = if let Some(exe) = exe.filter(|path| path.exists()) {
        Command::new(exe)
            .arg("--control")
            .arg(&endpoint.base_url)
            .arg("--token")
            .arg(&endpoint.token)
            .spawn()
    } else {
        Command::new("cargo")
            .arg("run")
            .arg("-p")
            .arg("lanclip-ui")
            .arg("--bin")
            .arg("lanclip-control")
            .arg("--")
            .arg("--control")
            .arg(&endpoint.base_url)
            .arg("--token")
            .arg(&endpoint.token)
            .spawn()
    };

    match spawned {
        Ok(new_child) => {
            focus_process(new_child.id());
            *child = Some(new_child);
        }
        Err(e) => warn!("launch control panel failed: {e}"),
    }
}

fn focus_process(pid: u32) {
    #[cfg(target_os = "macos")]
    {
        let script = format!(
            r#"tell application "System Events"
  repeat 20 times
    set matches to every process whose unix id is {pid}
    if length of matches is greater than 0 then
      set frontmost of item 1 of matches to true
      return
    end if
    delay 0.1
  end repeat
end tell"#
        );
        if let Err(e) = Command::new("osascript").arg("-e").arg(script).spawn() {
            warn!("focus control panel failed: {e}");
        }
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = pid;
    }
}

fn refresh_tray(tray_icon: &Option<TrayIcon>, port: u16, history: &ClipboardHistory) {
    if let Some(tray_icon) = tray_icon {
        let _ = tray_icon.set_tooltip(Some(format!(
            "lanclip :{port} · {} items",
            history.total_count()
        )));
    }
}

fn copy_history_entry(
    hash: &str,
    history: &ClipboardHistory,
    clipboard: &std::sync::Arc<ClipboardService>,
    rt_h: &tokio::runtime::Handle,
) -> bool {
    let Some(entry) = history.find_by_hash(&ContentHash(hash.to_string())) else {
        return false;
    };

    let payload = match &entry.payload {
        ClipboardPayload::FileRefs { .. } => ClipboardPayload::plain_text(
            entry
                .payload
                .as_path_text()
                .unwrap_or_else(|| entry.summary(120)),
        ),
        _ => entry.payload.clone(),
    };
    let cb = clipboard.clone();
    let history_payload = payload.clone();
    rt_h.spawn(async move {
        if let Err(e) = cb.apply_remote(payload).await {
            warn!("copy: {e}");
        }
    });
    history.push(HistoryEntry::new_local(history_payload));
    true
}

fn build_tray(port: u16, _history: &ClipboardHistory) -> anyhow::Result<TrayIcon> {
    let icon = tray::load_icon()?;
    let mut b = TrayIconBuilder::new()
        .with_tooltip(format!("lanclip — port {port}"))
        .with_icon(icon)
        .with_menu_on_left_click(false);
    #[cfg(target_os = "macos")]
    {
        b = b.with_icon_as_template(true);
    }
    Ok(b.build()?)
}
