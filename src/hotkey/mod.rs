//! Global mic hotkey: push-to-talk, push-to-mute or toggle mute on one strip (RustBus
//! extension). The key is polled with `GetAsyncKeyState`, which sees keyboard keys and
//! mouse buttons while any other window (a game) has focus. No keyboard hook is
//! installed, so nothing is injected into other processes.

pub mod config;
pub mod ffi;

pub use config::*;
pub use ffi::*;
