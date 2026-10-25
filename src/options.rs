//! Application options. The original keeps these in the registry under
//! `HKCU\…\VB-Audio\VoiceMeeter` (ANALYSIS §6); the rebuild uses an INI-style file with
//! the same key names in its own config directory.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::hotkey::{HotkeyConfig, HotkeyMode};
use crate::params::NUM_STRIPS;
use crate::settings::config_dir;

#[derive(Clone, Debug, PartialEq)]
pub struct AppOptions {
    pub sys_tray: bool,
    pub show_on_top: bool,
    pub lock_gui: bool,
    pub auto_restart: bool,
    pub auto_restart_all: bool,
    pub limit_remote_gain: bool,
    pub show_caption_help: bool,
    pub show_preset_scene: bool,
    pub use_startup_file: bool,
    pub startup_file: String,
    pub last_preset: i32,
    pub win_pos: Option<(f32, f32)>,
    pub zoom: f32,
    /// Global mic hotkey (RustBus extension).
    pub hotkey: HotkeyConfig,
}

impl Default for AppOptions {
    fn default() -> Self {
        AppOptions {
            sys_tray: false,
            show_on_top: false,
            lock_gui: false,
            auto_restart: true,
            auto_restart_all: false,
            limit_remote_gain: false,
            show_caption_help: true,
            show_preset_scene: false,
            use_startup_file: false,
            startup_file: String::new(),
            last_preset: -1,
            win_pos: None,
            zoom: 1.0,
            hotkey: HotkeyConfig::default(),
        }
    }
}

pub fn options_path() -> PathBuf {
    config_dir().join("options.ini")
}

impl AppOptions {
    pub fn to_ini(&self) -> String {
        let b = |v: bool| if v { "1" } else { "0" };
        let mut s = String::from("[VB-Audio\\VoiceMeeter]\n");
        let mut kv = |k: &str, v: String| {
            s.push_str(k);
            s.push('=');
            s.push_str(&v);
            s.push('\n');
        };
        kv("SysTray", b(self.sys_tray).into());
        kv("ShowOnTop", b(self.show_on_top).into());
        kv("LockGUI", b(self.lock_gui).into());
        kv("AutoRestart", b(self.auto_restart).into());
        kv("AutoRestartAll", b(self.auto_restart_all).into());
        kv("LimitRemoteGain", b(self.limit_remote_gain).into());
        kv("VMB_ShowCaptionHelp", b(self.show_caption_help).into());
        kv("VMB_ShowPresetScene", b(self.show_preset_scene).into());
        kv("VMB_UseStartupFile", b(self.use_startup_file).into());
        kv("VMB_StartupFile", self.startup_file.clone());
        kv("VMB_LastPresetInUse", self.last_preset.to_string());
        if let Some((x, y)) = self.win_pos {
            kv("WinPos_X", (x as i32).to_string());
            kv("WinPos_Y", (y as i32).to_string());
        }
        kv("Zoom", format!("{:.2}", self.zoom));
        kv("RB_HotkeyMode", (self.hotkey.mode as i32).to_string());
        kv("RB_HotkeyKey", self.hotkey.vk.to_string());
        kv("RB_HotkeyStrip", self.hotkey.strip.to_string());
        s
    }

    pub fn from_ini(text: &str) -> AppOptions {
        let map: BTreeMap<&str, &str> = text
            .lines()
            .filter_map(|l| l.split_once('='))
            .map(|(k, v)| (k.trim(), v.trim()))
            .collect();
        let d = AppOptions::default();
        let bo = |k: &str, def: bool| map.get(k).map(|v| *v == "1").unwrap_or(def);
        let pos = match (map.get("WinPos_X").and_then(|v| v.parse().ok()), map.get("WinPos_Y").and_then(|v| v.parse().ok())) {
            (Some(x), Some(y)) => Some((x, y)),
            _ => None,
        };
        AppOptions {
            sys_tray: bo("SysTray", d.sys_tray),
            show_on_top: bo("ShowOnTop", d.show_on_top),
            lock_gui: bo("LockGUI", d.lock_gui),
            auto_restart: bo("AutoRestart", d.auto_restart),
            auto_restart_all: bo("AutoRestartAll", d.auto_restart_all),
            limit_remote_gain: bo("LimitRemoteGain", d.limit_remote_gain),
            show_caption_help: bo("VMB_ShowCaptionHelp", d.show_caption_help),
            show_preset_scene: bo("VMB_ShowPresetScene", d.show_preset_scene),
            use_startup_file: bo("VMB_UseStartupFile", d.use_startup_file),
            startup_file: map.get("VMB_StartupFile").map(|s| s.to_string()).unwrap_or_default(),
            last_preset: map.get("VMB_LastPresetInUse").and_then(|v| v.parse().ok()).unwrap_or(-1),
            win_pos: pos,
            zoom: map.get("Zoom").and_then(|v| v.parse().ok()).unwrap_or(1.0f32).clamp(0.5, 1.5),
            hotkey: HotkeyConfig {
                mode: HotkeyMode::from_index(map.get("RB_HotkeyMode").and_then(|v| v.parse().ok()).unwrap_or(0)),
                vk: map.get("RB_HotkeyKey").and_then(|v| v.parse().ok()).unwrap_or(0),
                strip: map.get("RB_HotkeyStrip").and_then(|v| v.parse::<usize>().ok()).unwrap_or(0).min(NUM_STRIPS - 1),
            },
        }
    }

    pub fn load() -> AppOptions {
        std::fs::read_to_string(options_path()).map(|t| AppOptions::from_ini(&t)).unwrap_or_default()
    }

    pub fn save(&self) {
        let _ = std::fs::create_dir_all(config_dir());
        let _ = std::fs::write(options_path(), self.to_ini());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ini_roundtrip() {
        let o = AppOptions {
            lock_gui: true,
            startup_file: r"C:\x\y.xml".into(),
            win_pos: Some((10.0, 20.0)),
            zoom: 0.75,
            last_preset: 3,
            hotkey: HotkeyConfig { mode: HotkeyMode::PushToTalk, vk: 0x05, strip: 0 },
            ..Default::default()
        };
        assert_eq!(AppOptions::from_ini(&o.to_ini()), o);
    }
}
