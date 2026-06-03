#![allow(dead_code)]

use global_hotkey::hotkey::{Code, HotKey, Modifiers};

pub fn default_menu_hotkey() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "command+KeyV"
    }
    #[cfg(not(target_os = "macos"))]
    {
        "control+shift+KeyV"
    }
}

pub fn parse_menu_hotkey(spec: &str) -> HotKey {
    normalize_menu_hotkey(spec)
        .and_then(|spec| spec.parse::<HotKey>().ok())
        .or_else(|| default_menu_hotkey().parse::<HotKey>().ok())
        .expect("default menu hotkey must parse")
}

pub fn normalize_menu_hotkey(spec: &str) -> Option<String> {
    let normalized = normalize_aliases(spec);
    if normalized.trim().is_empty() {
        return None;
    }
    normalized.parse::<HotKey>().ok().map(HotKey::into_string)
}

pub fn normalize_recorded_menu_hotkey(
    platform: bool,
    control: bool,
    alt: bool,
    shift: bool,
    key: &str,
) -> Option<String> {
    let key = recorded_key_token(key)?;
    if !platform && !control && !alt && !shift {
        return None;
    }

    let mut parts = Vec::new();
    if shift {
        parts.push("shift".to_string());
    }
    if control {
        parts.push("control".to_string());
    }
    if alt {
        parts.push("alt".to_string());
    }
    if platform {
        parts.push("command".to_string());
    }
    parts.push(key);
    normalize_menu_hotkey(&parts.join("+"))
}

pub fn display_menu_hotkey(spec: &str) -> String {
    let hotkey = parse_menu_hotkey(spec);
    let mut parts = Vec::new();
    if hotkey.mods.contains(Modifiers::SUPER) {
        parts.push(if cfg!(target_os = "macos") {
            "Command".to_string()
        } else {
            "Super".to_string()
        });
    }
    if hotkey.mods.contains(Modifiers::CONTROL) {
        parts.push("Ctrl".to_string());
    }
    if hotkey.mods.contains(Modifiers::ALT) {
        parts.push(if cfg!(target_os = "macos") {
            "Option".to_string()
        } else {
            "Alt".to_string()
        });
    }
    if hotkey.mods.contains(Modifiers::SHIFT) {
        parts.push("Shift".to_string());
    }
    parts.push(key_label(hotkey.key));
    parts.join("+")
}

fn normalize_aliases(spec: &str) -> String {
    spec.trim()
        .replace('＋', "+")
        .replace('⌘', "command")
        .replace('⇧', "shift")
        .replace('⌥', "alt")
        .replace('⌃', "control")
}

fn recorded_key_token(key: &str) -> Option<String> {
    let key = key.trim();
    if key.is_empty()
        || matches!(
            key.to_ascii_lowercase().as_str(),
            "shift" | "control" | "ctrl" | "alt" | "option" | "cmd" | "command" | "platform"
        )
    {
        return None;
    }

    if key.len() == 1 {
        let ch = key.chars().next()?;
        if ch.is_ascii_alphabetic() {
            return Some(format!("Key{}", ch.to_ascii_uppercase()));
        }
        if ch.is_ascii_digit() {
            return Some(format!("Digit{ch}"));
        }
        return Some(key.to_string());
    }

    let token = match key.to_ascii_lowercase().as_str() {
        "esc" | "escape" => "Escape",
        "space" => "Space",
        "tab" => "Tab",
        "enter" | "return" => "Enter",
        "backspace" => "Backspace",
        "delete" => "Delete",
        "left" | "arrowleft" => "ArrowLeft",
        "right" | "arrowright" => "ArrowRight",
        "up" | "arrowup" => "ArrowUp",
        "down" | "arrowdown" => "ArrowDown",
        "pageup" => "PageUp",
        "pagedown" => "PageDown",
        "home" => "Home",
        "end" => "End",
        "insert" => "Insert",
        "minus" => "Minus",
        "equal" => "Equal",
        "comma" => "Comma",
        "period" => "Period",
        "slash" => "Slash",
        "backslash" => "Backslash",
        "quote" => "Quote",
        "semicolon" => "Semicolon",
        "backquote" => "Backquote",
        key if key.starts_with('f') && key[1..].parse::<u8>().is_ok() => {
            return Some(key.to_string())
        }
        _ => return None,
    };
    Some(token.to_string())
}

fn key_label(key: Code) -> String {
    let raw = key.to_string();
    if let Some(letter) = raw.strip_prefix("Key") {
        if letter.len() == 1 {
            return letter.to_string();
        }
    }
    if let Some(digit) = raw.strip_prefix("Digit") {
        if digit.len() == 1 {
            return digit.to_string();
        }
    }
    raw
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_human_hotkeys() {
        assert_eq!(normalize_menu_hotkey("cmd + v").unwrap(), "super+KeyV");
        assert_eq!(
            normalize_menu_hotkey("ctrl+shift+v").unwrap(),
            "shift+control+KeyV"
        );
    }

    #[test]
    fn rejects_empty_hotkey() {
        assert!(normalize_menu_hotkey(" ").is_none());
    }

    #[test]
    fn records_gpui_style_keystrokes() {
        assert_eq!(
            normalize_recorded_menu_hotkey(true, false, false, false, "v").unwrap(),
            "super+KeyV"
        );
        assert_eq!(
            normalize_recorded_menu_hotkey(false, true, false, true, "v").unwrap(),
            "shift+control+KeyV"
        );
        assert!(normalize_recorded_menu_hotkey(false, false, false, false, "v").is_none());
    }
}
