//! RustBus — a virtual audio mixer written in Rust; a clean-room reimplementation of
//! Voicemeeter-Banana-style behaviour.
//! See docs/ANALYSIS.md for the reverse-engineering findings and FEATURES.md for status.

pub mod dsp;
pub mod engine;
pub mod hotkey;
pub mod midi;
pub mod options;
pub mod params;
pub mod presets;
pub mod recorder;
pub mod script;
pub mod session;
pub mod settings;
pub mod vban;
pub mod xml;
