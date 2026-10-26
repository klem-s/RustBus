//! Contextual help: the caption shown in the status line for the hovered control.

use eframe::egui;

use super::App;

impl App {
    pub(super) fn hover_help(&mut self, r: &egui::Response, id: &str) {
        if r.hovered() {
            self.help = help_text(id).to_string();
        }
    }
}

/// Contextual help captions (our own wording; ids follow the original's help keys).
pub(super) fn help_text(id: &str) -> &'static str {
    match id {
        "BUTNAME" => "Product name. Open the menu for About / settings.",
        "MENU" => "MAIN MENU: engine restart, load/save settings, options and dialogs.",
        "BUTVBAN" => "VBAN: open the network audio / remote-control configuration (Ctrl+1).",
        "LOCKICON" => "GUI LOCKED: audio controls are locked (tape deck still works). Unlock in menu.",
        "STRIP0" | "STRIP1" | "STRIP2" => "Hardware input strip: click the device line to select an input device, right-click the name for strip menu.",
        "STRIP3" | "STRIP4" | "STRIP5" => "Virtual input strip (8 ch): select the capture endpoint that applications play into (e.g. a virtual cable).",
        "BUSA1" => "A1: main physical output and master clock of the audio engine.",
        "BUSA2" | "BUSA3" => "Physical output bus (up to 8 channels).",
        "BUSB1" | "BUSB2" | "BUSB3" => "Virtual output bus: feeds a device that other applications record from.",
        "STRIP_PANELC" => "INTELLIPAN colour panel: drag to tilt dark/bright (x) and warmth/presence (y). Double-click resets.",
        "STRIP_PANELP" => "INTELLIPAN 3D panel: left/right position (x) and front/rear (y). Double-click resets.",
        "STRIP_PANELM" => "INTELLIPAN modulation panel: chorus rate (x) and amount (y). Double-click resets.",
        "STRIP_COMP" => "COMPRESSOR with auto make-up gain (0 = off). Double-click resets.",
        "STRIP_GATE" => "GATE: removes low-level noise below the threshold (0 = off).",
        "STRIP_EQLO" => "3-band EQ bass (±12 dB). Double-click resets.",
        "STRIP_EQMED" => "3-band EQ medium (±12 dB). Double-click resets.",
        "STRIP_EQHI" => "3-band EQ treble (±12 dB). Double-click resets.",
        "STRIP_PANPOT" => "Multichannel pan: left/right (x) and front/rear (y). Double-click resets.",
        "STRIP_A1" | "STRIP_A2" | "STRIP_A3" => "Route this strip to a physical output bus.",
        "STRIP_B1" | "STRIP_B2" => "Route this strip to a virtual output bus.",
        "STRIP_MONO" => "MONO: L = R = (L+R)/2.",
        "STRIP_SOLO" => "SOLO: only soloed strips are heard.",
        "STRIP_MUTE" => "MUTE this strip.",
        "STRIP_DENOISE" => "DENOISE: AI noise removal (RNNoise) keeps your voice and removes keyboard, fan and room noise. Adds 10 ms. Right-click for amount.",
        "STRIP_MC" =>"M.C.: mute the centre channel (channel 3, usually dialogue).",
        "STRIP_KARAOKE" => "KARAOKE: click to cycle voice-removal algorithms 1-4, right-click for off.",
        "STRIP_FADER" => "Strip gain fader. Double-click = 0 dB, wheel = ±0.5 dB, Shift = fine, Ctrl+drag sets the limiter.",
        "BUS_FADER" => "Bus gain fader. Double-click = 0 dB, Ctrl+drag sets the limiter (after fader).",
        "BUS_MIXNORMAL" => "Bus mode: normal, mix-down, repeat, composite, TV-mix, up-mix, channel extract.",
        "BUS_MONO" => "MONO bus; click again to swap L and R.",
        "BUS_EQ" => "Bus parametric EQ on/off. Right-click to edit the 6-band EQ.",
        "BUS_MUTE" => "MUTE this bus.",
        "TAPE" => "Tape deck: click to load a WAV file, right-click for recorder options.",
        "TAPE_REW" => "Rewind 5 s.",
        "TAPE_FF" => "Fast forward 5 s.",
        "TAPE_PLAY" => "Play the loaded file.",
        "TAPE_STOP" => "Stop playback / recording. Click again to go to zero.",
        "TAPE_REC" => "Record armed buses/inputs to a new file. Click again to pause.",
        "TAPE_A1" | "TAPE_A2" | "TAPE_A3" | "TAPE_B1" | "TAPE_B2" => "Route tape playback to this bus.",
        _ => "",
    }
}
