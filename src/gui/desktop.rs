//! Windows desktop integration (RustBus extension): tray icon, close-to-tray,
//! start with Windows and a single-instance guard.
//!
//! The window is shown and hidden with Win32 `ShowWindow` directly: egui stops running
//! frames while the window is hidden, so tray clicks must not depend on a frame running.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::Arc;

use eframe::egui;
use parking_lot::Mutex;
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

use rustbus::params::MixParams;

#[link(name = "user32")]
extern "system" {
    fn ShowWindow(hwnd: isize, cmd: i32) -> i32;
    fn SetForegroundWindow(hwnd: isize) -> i32;
    fn IsIconic(hwnd: isize) -> i32;
    fn EnumWindows(cb: unsafe extern "system" fn(isize, isize) -> i32, lparam: isize) -> i32;
    fn GetWindowTextW(hwnd: isize, buf: *mut u16, len: i32) -> i32;
    fn GetWindowThreadProcessId(hwnd: isize, pid: *mut u32) -> u32;
}

#[link(name = "kernel32")]
extern "system" {
    fn CreateMutexW(attrs: *mut c_void, owner: i32, name: *const u16) -> isize;
    fn GetLastError() -> u32;
    fn GetCurrentProcessId() -> u32;
    fn OpenProcess(access: u32, inherit: i32, pid: u32) -> isize;
    fn QueryFullProcessImageNameW(process: isize, flags: u32, buf: *mut u16, len: *mut u32) -> i32;
    fn CloseHandle(h: isize) -> i32;
}

#[link(name = "advapi32")]
extern "system" {
    fn RegSetKeyValueW(key: isize, sub: *const u16, name: *const u16, ty: u32, data: *const c_void, len: u32) -> i32;
    fn RegDeleteKeyValueW(key: isize, sub: *const u16, name: *const u16) -> i32;
    fn RegGetValueW(key: isize, sub: *const u16, name: *const u16, flags: u32, ty: *mut u32, data: *mut c_void, len: *mut u32) -> i32;
}

const SW_HIDE: i32 = 0;
const SW_SHOW: i32 = 5;
const SW_RESTORE: i32 = 9;
const ERROR_ALREADY_EXISTS: u32 = 183;
const HKEY_CURRENT_USER: isize = 0x8000_0001u32 as i32 as isize;
const REG_SZ: u32 = 1;
const RRF_RT_REG_SZ: u32 = 0x2;
const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const RUN_VALUE: &str = "RustBus";
/// Command-line flag used by the startup entry: start hidden in the tray.
pub const TRAY_ARG: &str = "--tray";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

// ---------------------------------------------------------------- main window

static WINDOW: AtomicIsize = AtomicIsize::new(0);
/// Set by the tray "Exit" item; the next frame then closes for real.
pub static QUIT: AtomicBool = AtomicBool::new(false);

pub fn set_window(hwnd: isize) {
    WINDOW.store(hwnd, Ordering::Relaxed);
}

pub fn show_window() {
    let h = WINDOW.load(Ordering::Relaxed);
    if h != 0 {
        unsafe {
            ShowWindow(h, SW_SHOW);
            if IsIconic(h) != 0 {
                ShowWindow(h, SW_RESTORE);
            }
            SetForegroundWindow(h);
        }
    }
}

pub fn hide_window() {
    let h = WINDOW.load(Ordering::Relaxed);
    if h != 0 {
        unsafe { ShowWindow(h, SW_HIDE) };
    }
}

/// Win32 handle of the eframe window.
pub fn hwnd_of(w: &impl raw_window_handle::HasWindowHandle) -> Option<isize> {
    match w.window_handle().ok()?.as_raw() {
        raw_window_handle::RawWindowHandle::Win32(h) => Some(h.hwnd.get()),
        _ => None,
    }
}

// ---------------------------------------------------------------- single instance

/// True when another RustBus already runs; that instance's window is brought forward.
pub fn activate_existing_instance() -> bool {
    unsafe {
        // The handle is kept for the process lifetime on purpose.
        CreateMutexW(std::ptr::null_mut(), 0, wide("Local\\RustBus.SingleInstance").as_ptr());
        if GetLastError() != ERROR_ALREADY_EXISTS {
            return false;
        }
        EnumWindows(find_other, 0);
    }
    true
}

unsafe extern "system" fn find_other(hwnd: isize, _: isize) -> i32 {
    let mut title = [0u16; 32];
    let n = GetWindowTextW(hwnd, title.as_mut_ptr(), title.len() as i32);
    if String::from_utf16_lossy(&title[..n.max(0) as usize]) != "RustBus" {
        return 1;
    }
    let mut pid = 0u32;
    GetWindowThreadProcessId(hwnd, &mut pid);
    if pid == GetCurrentProcessId() || !process_is_rustbus(pid) {
        return 1;
    }
    ShowWindow(hwnd, SW_SHOW);
    if IsIconic(hwnd) != 0 {
        ShowWindow(hwnd, SW_RESTORE);
    }
    SetForegroundWindow(hwnd);
    0
}

unsafe fn process_is_rustbus(pid: u32) -> bool {
    let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
    if h == 0 {
        return false;
    }
    let mut buf = [0u16; 1024];
    let mut len = buf.len() as u32;
    let ok = QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut len) != 0;
    CloseHandle(h);
    ok && String::from_utf16_lossy(&buf[..len as usize]).to_lowercase().ends_with("rustbus.exe")
}

// ---------------------------------------------------------------- start with Windows

pub fn autostart_enabled() -> bool {
    let mut buf = [0u16; 1024];
    let mut len = (buf.len() * 2) as u32;
    unsafe {
        RegGetValueW(HKEY_CURRENT_USER, wide(RUN_KEY).as_ptr(), wide(RUN_VALUE).as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), buf.as_mut_ptr().cast(), &mut len) == 0
    }
}

pub fn set_autostart(on: bool) -> Result<(), String> {
    let r = unsafe {
        if on {
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            let cmd = wide(&format!("\"{}\" {TRAY_ARG}", exe.display()));
            RegSetKeyValueW(HKEY_CURRENT_USER, wide(RUN_KEY).as_ptr(), wide(RUN_VALUE).as_ptr(), REG_SZ, cmd.as_ptr().cast(), (cmd.len() * 2) as u32)
        } else {
            let r = RegDeleteKeyValueW(HKEY_CURRENT_USER, wide(RUN_KEY).as_ptr(), wide(RUN_VALUE).as_ptr());
            // Already absent is fine.
            if r == 2 {
                0
            } else {
                r
            }
        }
    };
    if r == 0 {
        Ok(())
    } else {
        Err(format!("Could not change the Windows startup entry (error {r})"))
    }
}

/// Point an existing startup entry at this exe (e.g. after the build folder moved).
pub fn refresh_autostart() {
    if autostart_enabled() {
        let _ = set_autostart(true);
    }
}

// ---------------------------------------------------------------- tray icon

pub struct Tray {
    _icon: TrayIcon,
    mic: CheckMenuItem,
}

impl Tray {
    /// `mic_strip`: the strip the "Mute mic" item toggles.
    pub fn new(ctx: egui::Context, params: Arc<Mutex<MixParams>>, mic_strip: Arc<AtomicIsize>) -> Option<Tray> {
        let show = MenuItem::new("Show RustBus", true, None);
        let mic = CheckMenuItem::new("Mute mic", true, false, None);
        let exit = MenuItem::new("Exit", true, None);
        let menu = Menu::new();
        menu.append(&show).ok()?;
        menu.append(&mic).ok()?;
        menu.append(&PredefinedMenuItem::separator()).ok()?;
        menu.append(&exit).ok()?;
        let icon = TrayIconBuilder::new()
            .with_icon(icon())
            .with_tooltip("RustBus")
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .build()
            .ok()?;

        let (show_id, mic_id, exit_id) = (show.id().clone(), mic.id().clone(), exit.id().clone());
        let c = ctx.clone();
        MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
            if e.id == show_id {
                show_window();
            } else if e.id == mic_id {
                let s = mic_strip.load(Ordering::Relaxed).max(0) as usize;
                let mut p = params.lock();
                if let Some(st) = p.strips.get_mut(s) {
                    st.mute = !st.mute;
                }
            } else if e.id == exit_id {
                QUIT.store(true, Ordering::Relaxed);
                // Frames only run while visible; show so the close can go through.
                show_window();
            }
            c.request_repaint();
        }));
        TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| match e {
            TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } | TrayIconEvent::DoubleClick { .. } => {
                show_window();
                ctx.request_repaint();
            }
            _ => {}
        }));
        Some(Tray { _icon: icon, mic })
    }

    /// Keep the "Mute mic" tick in sync with the strip.
    pub fn set_mic_muted(&self, muted: bool) {
        if self.mic.is_checked() != muted {
            self.mic.set_checked(muted);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writes and removes the real HKCU Run entry:
    /// `cargo test --bin rustbus autostart_roundtrip -- --ignored`
    #[test]
    #[ignore]
    fn autostart_roundtrip() {
        let before = autostart_enabled();
        set_autostart(true).unwrap();
        assert!(autostart_enabled());
        set_autostart(false).unwrap();
        assert!(!autostart_enabled());
        set_autostart(false).unwrap();
        if before {
            set_autostart(true).unwrap();
        }
    }
}

/// 32×32 tray icon: the teal RustBus tile with three level bars.
fn icon() -> Icon {
    const N: usize = 32;
    let mut px = vec![0u8; N * N * 4];
    let (teal, dark) = ([0x5E, 0xEA, 0xD4], [0x06, 0x20, 0x1C]);
    let bars = [(9.0, 13.0, 12.0), (14.5, 18.5, 7.0), (20.0, 24.0, 15.0)];
    for y in 0..N {
        for x in 0..N {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            // Rounded square with radius 7.
            let dx = (fx - 16.0).abs() - 9.0;
            let dy = (fy - 16.0).abs() - 9.0;
            let d = (dx.max(0.0).powi(2) + dy.max(0.0).powi(2)).sqrt() - 7.0 + dx.max(dy).min(0.0);
            let a = (0.5 - d).clamp(0.0, 1.0);
            if a <= 0.0 {
                continue;
            }
            let on_bar = bars.iter().any(|&(x0, x1, top)| fx >= x0 && fx <= x1 && fy >= top && fy <= 24.0);
            let c = if on_bar { dark } else { teal };
            let i = (y * N + x) * 4;
            px[i..i + 3].copy_from_slice(&c);
            px[i + 3] = (a * 255.0) as u8;
        }
    }
    Icon::from_rgba(px, N as u32, N as u32).expect("valid icon size")
}
