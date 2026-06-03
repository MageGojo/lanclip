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
}
