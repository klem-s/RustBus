//! Hotkey configuration and the pure mute state machine (`step`).

use crate::params::NUM_STRIPS;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum HotkeyMode {
    #[default]
    Off = 0,
    /// Muted unless the key is held.
    PushToTalk = 1,
    /// Muted while the key is held.
    PushToMute = 2,
    /// Each press flips mute.
    Toggle = 3,
}

impl HotkeyMode {
    pub const ALL: [HotkeyMode; 4] = [HotkeyMode::Off, HotkeyMode::PushToTalk, HotkeyMode::PushToMute, HotkeyMode::Toggle];

    pub fn from_index(i: i64) -> HotkeyMode {
        Self::ALL.get(i.max(0) as usize).copied().unwrap_or_default()
    }

    pub fn label(self) -> &'static str {
        match self {
            HotkeyMode::Off => "Off",
            HotkeyMode::PushToTalk => "Push to talk",
            HotkeyMode::PushToMute => "Push to mute",
            HotkeyMode::Toggle => "Toggle mute",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HotkeyConfig {
    pub mode: HotkeyMode,
    /// Windows virtual-key code; 0 = no key chosen.
    pub vk: u16,
    pub strip: usize,
}

impl Default for HotkeyConfig {
    fn default() -> Self {
        HotkeyConfig { mode: HotkeyMode::Off, vk: 0, strip: 0 }
    }
}

impl HotkeyConfig {
    pub fn active(&self) -> bool {
        self.mode != HotkeyMode::Off && self.vk != 0 && self.strip < NUM_STRIPS
    }
}

/// What the poller does with the strip's mute for the current key state.
/// `held_mute` remembers the mute state from before a push-to-mute press.
pub(super) fn step(mode: HotkeyMode, down: bool, was_down: bool, mute: bool, held_mute: &mut bool) -> Option<bool> {
    match mode {
        HotkeyMode::Off => None,
        // Level-based: the key owns the mute, so a stray GUI write is corrected next tick.
        HotkeyMode::PushToTalk => (mute != !down).then_some(!down),
        HotkeyMode::PushToMute => match (down, was_down) {
            (true, false) => {
                *held_mute = mute;
                Some(true)
            }
            (true, true) => (!mute).then_some(true),
            (false, true) => Some(*held_mute),
            (false, false) => None,
        },
        HotkeyMode::Toggle => (down && !was_down).then_some(!mute),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_to_talk_follows_key() {
        let mut h = false;
        assert_eq!(step(HotkeyMode::PushToTalk, false, false, false, &mut h), Some(true));
        assert_eq!(step(HotkeyMode::PushToTalk, true, false, true, &mut h), Some(false));
        assert_eq!(step(HotkeyMode::PushToTalk, true, true, false, &mut h), None);
    }

    #[test]
    fn push_to_mute_restores_previous_state() {
        let mut h = false;
        // Unmuted, press: mute; release: back to unmuted.
        assert_eq!(step(HotkeyMode::PushToMute, true, false, false, &mut h), Some(true));
        assert_eq!(step(HotkeyMode::PushToMute, false, true, true, &mut h), Some(false));
        // Already muted by hand: release keeps it muted.
        assert_eq!(step(HotkeyMode::PushToMute, true, false, true, &mut h), Some(true));
        assert_eq!(step(HotkeyMode::PushToMute, false, true, true, &mut h), Some(true));
        // Idle: manual mute is left alone.
        assert_eq!(step(HotkeyMode::PushToMute, false, false, true, &mut h), None);
    }

    #[test]
    fn toggle_on_press_edge_only() {
        let mut h = false;
        assert_eq!(step(HotkeyMode::Toggle, true, false, false, &mut h), Some(true));
        assert_eq!(step(HotkeyMode::Toggle, true, true, true, &mut h), None);
        assert_eq!(step(HotkeyMode::Toggle, false, true, true, &mut h), None);
    }
}
