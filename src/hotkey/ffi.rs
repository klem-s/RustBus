//! Win32 key polling (`GetAsyncKeyState`, key names) and the hotkey poller thread.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use parking_lot::Mutex;

use super::config::{step, HotkeyConfig, HotkeyMode};
use crate::params::MixParams;

#[cfg(windows)]
mod user32 {
    #[link(name = "user32")]
    extern "system" {
        pub fn GetAsyncKeyState(vk: i32) -> i16;
        pub fn MapVirtualKeyW(code: u32, map: u32) -> u32;
        pub fn GetKeyNameTextW(lparam: i32, buf: *mut u16, len: i32) -> i32;
    }
}

/// Is the key (or mouse button) down right now, whichever window has focus.
pub fn key_down(vk: u16) -> bool {
    #[cfg(windows)]
    unsafe {
        (user32::GetAsyncKeyState(vk as i32) as u16 & 0x8000) != 0
    }
    #[cfg(not(windows))]
    {
        let _ = vk;
        false
    }
}

/// Keys that are down now, for "press a key" capture. Left/right click are skipped so
/// clicking the capture button itself does not bind it.
pub fn keys_down() -> Vec<u16> {
    (3u16..=254).filter(|&vk| !matches!(vk, 0x10..=0x12) && key_down(vk)).collect()
}

/// Human-readable key name: "F13", "Mouse 4", "Caps Lock"...
pub fn key_name(vk: u16) -> String {
    match vk {
        0 => return "none".into(),
        0x01 => return "Left click".into(),
        0x02 => return "Right click".into(),
        0x04 => return "Middle click".into(),
        0x05 => return "Mouse 4".into(),
        0x06 => return "Mouse 5".into(),
        0x70..=0x87 => return format!("F{}", vk - 0x6F),
        0xA0 => return "Left Shift".into(),
        0xA1 => return "Right Shift".into(),
        0xA2 => return "Left Ctrl".into(),
        0xA3 => return "Right Ctrl".into(),
        0xA4 => return "Left Alt".into(),
        0xA5 => return "Right Alt".into(),
        _ => {}
    }
    #[cfg(windows)]
    unsafe {
        let scan = user32::MapVirtualKeyW(vk as u32, 0);
        // Navigation keys share scan codes with the numpad; the extended bit picks them.
        let extended = matches!(vk, 0x21..=0x28 | 0x2D | 0x2E | 0x5B | 0x5C | 0x6F | 0x90);
        let lparam = ((scan as i32) << 16) | if extended { 1 << 24 } else { 0 };
        let mut buf = [0u16; 64];
        let n = user32::GetKeyNameTextW(lparam, buf.as_mut_ptr(), buf.len() as i32);
        if n > 0 {
            return String::from_utf16_lossy(&buf[..n as usize]);
        }
    }
    format!("Key 0x{vk:02X}")
}

pub struct HotkeyRuntime {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl HotkeyRuntime {
    pub fn start(cfg: HotkeyConfig, params: Arc<Mutex<MixParams>>, wake: Arc<dyn Fn() + Send + Sync>) -> Option<HotkeyRuntime> {
        if !cfg.active() {
            return None;
        }
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let thread = std::thread::Builder::new()
            .name("rustbus-hotkey".into())
            .spawn(move || {
                let mut was_down = false;
                let mut held_mute = false;
                while !stop2.load(Ordering::Relaxed) {
                    let down = key_down(cfg.vk);
                    let mute = params.lock().strips[cfg.strip].mute;
                    if let Some(m) = step(cfg.mode, down, was_down, mute, &mut held_mute) {
                        params.lock().strips[cfg.strip].mute = m;
                        wake();
                    }
                    was_down = down;
                    std::thread::sleep(Duration::from_millis(5));
                }
                // Leaving push-to-talk must not leave the mic stuck muted.
                if cfg.mode == HotkeyMode::PushToTalk {
                    params.lock().strips[cfg.strip].mute = false;
                    wake();
                }
            })
            .ok()?;
        Some(HotkeyRuntime { stop, thread: Some(thread) })
    }
}

impl Drop for HotkeyRuntime {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(key_name(0x05), "Mouse 4");
        assert_eq!(key_name(0x7C), "F13");
    }
}
