//! Mixer parameter model.
//!
//! Layout mirrors the Banana topology recovered in docs/ANALYSIS.md §2:
//! 3 hardware strips (stereo), 3 virtual strips (8 ch; the 3rd is beyond Banana), buses A1 A2 A3 B1 B2 B3 (B3 added beyond Banana).
//! `MixParams` is `Copy` so the audio thread can snapshot it without allocating.

pub const NUM_HW_STRIPS: usize = 3;
pub const NUM_STRIPS: usize = 6;
pub const NUM_BUSES: usize = 6;
pub const MAX_CH: usize = 8;
pub const EQ_CELLS: usize = 6;

pub const GAIN_MIN_DB: f32 = -60.0;
pub const GAIN_MAX_DB: f32 = 12.0;
pub const LIMIT_MIN_DB: f32 = -40.0;
pub const LIMIT_MAX_DB: f32 = 12.0;

pub const BUS_NAMES: [&str; NUM_BUSES] = ["A1", "A2", "A3", "B1", "B2", "B3"];

pub fn is_virtual_strip(i: usize) -> bool {
    i >= NUM_HW_STRIPS
}

pub fn is_virtual_bus(i: usize) -> bool {
    i >= 3
}

pub fn strip_channels(i: usize) -> usize {
    if is_virtual_strip(i) {
        MAX_CH
    } else {
        2
    }
}

/// Bus mode, numbered as in the settings file `BusMode` attribute (ANALYSIS §6.1 [I]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum BusMode {
    #[default]
    Normal = 0,
    MixDownA = 1,
    MixDownB = 2,
    Repeat = 3,
    Composite = 4,
    TvMix = 5,
    UpMix21 = 6,
    UpMix41 = 7,
    UpMix61 = 8,
    LeftOnly = 9,
    RightOnly = 10,
    CenterOnly = 11,
    LfeOnly = 12,
    RearOnly = 13,
}

impl BusMode {
    pub const ALL: [BusMode; 14] = [
        BusMode::Normal,
        BusMode::MixDownA,
        BusMode::MixDownB,
        BusMode::Repeat,
        BusMode::Composite,
        BusMode::TvMix,
        BusMode::UpMix21,
        BusMode::UpMix41,
        BusMode::UpMix61,
        BusMode::LeftOnly,
        BusMode::RightOnly,
        BusMode::CenterOnly,
        BusMode::LfeOnly,
        BusMode::RearOnly,
    ];

    pub fn from_index(i: i64) -> BusMode {
        Self::ALL.get(i.max(0) as usize).copied().unwrap_or_default()
    }

    pub fn label(self) -> &'static str {
        match self {
            BusMode::Normal => "NORMAL",
            BusMode::MixDownA => "MIX-DOWN A",
            BusMode::MixDownB => "MIX-DOWN B",
            BusMode::Repeat => "STEREO REPEAT",
            BusMode::Composite => "COMPOSITE",
            BusMode::TvMix => "TV MIX",
            BusMode::UpMix21 => "UP-MIX 2.1",
            BusMode::UpMix41 => "UP-MIX 4.1",
            BusMode::UpMix61 => "UP-MIX 6.1",
            BusMode::LeftOnly => "LEFT ONLY",
            BusMode::RightOnly => "RIGHT ONLY",
            BusMode::CenterOnly => "CENTER ONLY",
            BusMode::LfeOnly => "LFE ONLY",
            BusMode::RearOnly => "REAR ONLY",
        }
    }

    /// Name used by the script language: `Bus[i].mode.<name> = 1`.
    pub fn script_name(self) -> &'static str {
        match self {
            BusMode::Normal => "normal",
            BusMode::MixDownA => "amix",
            BusMode::MixDownB => "bmix",
            BusMode::Repeat => "repeat",
            BusMode::Composite => "composite",
            BusMode::TvMix => "tvmix",
            BusMode::UpMix21 => "upmix21",
            BusMode::UpMix41 => "upmix41",
            BusMode::UpMix61 => "upmix61",
            BusMode::LeftOnly => "leftonly",
            BusMode::RightOnly => "rightonly",
            BusMode::CenterOnly => "centeronly",
            BusMode::LfeOnly => "lfeonly",
            BusMode::RearOnly => "rearonly",
        }
    }
}

/// Detailed compressor settings (`<StripComp …>`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CompParams {
    pub gain_in: f32,
    pub attack_ms: f32,
    pub release_ms: f32,
    pub knee: f32,
    pub ratio: f32,
    pub threshold_db: f32,
    pub auto_makeup: bool,
    pub gain_out: f32,
}

impl Default for CompParams {
    fn default() -> Self {
        CompParams {
            gain_in: 0.0,
            attack_ms: 10.0,
            release_ms: 50.0,
            knee: 0.5,
            ratio: 1.0,
            threshold_db: -20.0,
            auto_makeup: true,
            gain_out: 0.0,
        }
    }
}

impl CompParams {
    /// The single "COMP" knob (0..10) drives ratio; a design choice documented in FEATURES.md.
    pub fn knob(&self) -> f32 {
        ((self.ratio - 1.0) / 0.5).clamp(0.0, 10.0)
    }
    pub fn set_knob(&mut self, v: f32) {
        self.ratio = 1.0 + v.clamp(0.0, 10.0) * 0.5;
    }
}

/// Detailed gate settings (`<StripGate …>`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GateParams {
    pub threshold_db: f32,
    pub damping_db: f32,
    pub bp_sidechain_hz: f32,
    pub attack_ms: f32,
    pub hold_ms: f32,
    pub release_ms: f32,
}

impl Default for GateParams {
    fn default() -> Self {
        GateParams {
            threshold_db: -60.0,
            damping_db: -60.0,
            bp_sidechain_hz: 100.0,
            attack_ms: 0.0,
            hold_ms: 500.0,
            release_ms: 1000.0,
        }
    }
}

impl GateParams {
    /// GATE knob 0..10: 0 = threshold -60 dB (off), 10 = -0 dB.
    pub fn knob(&self) -> f32 {
        ((self.threshold_db + 60.0) / 6.0).clamp(0.0, 10.0)
    }
    pub fn set_knob(&mut self, v: f32) {
        self.threshold_db = -60.0 + v.clamp(0.0, 10.0) * 6.0;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum PanelType {
    Color = 0,
    #[default]
    Position3D = 1,
    Modulation = 2,
}

impl PanelType {
    pub fn from_index(i: i64) -> PanelType {
        match i {
            0 => PanelType::Color,
            2 => PanelType::Modulation,
            _ => PanelType::Position3D,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StripParams {
    pub gain_db: f32,
    pub limit_db: f32,
    pub mute: bool,
    pub solo: bool,
    pub mono: bool,
    /// Mute centre channel (virtual strip "M.C.").
    pub mute_center: bool,
    pub route: [bool; NUM_BUSES],
    /// 0 = off, 1..4 = karaoke algorithm.
    pub karaoke: u8,
    pub strip_mode: u8,
    pub comp_on: bool,
    pub gate_on: bool,
    pub comp: CompParams,
    pub gate: GateParams,
    pub panel_type: PanelType,
    /// Pan / 3D position, x in -0.5..0.5 (L..R), y in -0.5..0.5 (front..rear).
    pub pan_x: f32,
    pub pan_y: f32,
    /// Intellipan colour panel (quick tonal EQ), x/y in -0.5..0.5.
    pub color_x: f32,
    pub color_y: f32,
    /// Intellipan modulation panel (chorus-type FX), x in -0.5..0.5, y in 0..1.
    pub fx_x: f32,
    pub fx_y: f32,
    /// 3-band EQ for virtual strips, dB (-12..+12): bass, medium, treble.
    pub eq_gain: [f32; 3],
    /// RNNoise voice noise removal on hardware strips (RustBus extension).
    pub denoise_on: bool,
    /// Denoise amount 0..1 (dry/denoised blend).
    pub denoise: f32,
}

impl Default for StripParams {
    fn default() -> Self {
        StripParams {
            gain_db: 0.0,
            limit_db: LIMIT_MAX_DB,
            mute: false,
            solo: false,
            mono: false,
            mute_center: false,
            route: [true, false, false, true, false, false],
            karaoke: 0,
            strip_mode: 0,
            comp_on: true,
            gate_on: true,
            comp: CompParams::default(),
            gate: GateParams::default(),
            panel_type: PanelType::Position3D,
            pan_x: 0.0,
            pan_y: 0.0,
            color_x: 0.0,
            color_y: 0.0,
            fx_x: 0.0,
            fx_y: 0.0,
            eq_gain: [0.0; 3],
            denoise_on: false,
            denoise: 1.0,
        }
    }
}

/// EQ cell type as stored in `EQtype` (parametric EQ "EQPro-G6").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum EqType {
    #[default]
    Peak = 0,
    LowShelf = 1,
    HighShelf = 2,
    LowCut = 3,
    HighCut = 4,
}

impl EqType {
    pub const ALL: [EqType; 5] =
        [EqType::Peak, EqType::LowShelf, EqType::HighShelf, EqType::LowCut, EqType::HighCut];
    pub fn from_index(i: i64) -> EqType {
        Self::ALL.get(i.max(0) as usize).copied().unwrap_or_default()
    }
    pub fn label(self) -> &'static str {
        match self {
            EqType::Peak => "Peak",
            EqType::LowShelf => "Low Shelf",
            EqType::HighShelf => "High Shelf",
            EqType::LowCut => "Low Cut",
            EqType::HighCut => "High Cut",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EqCell {
    pub on: bool,
    pub kind: EqType,
    pub gain_db: f32,
    pub freq: f32,
    pub q: f32,
}

pub const DEFAULT_EQ_FREQS: [f32; EQ_CELLS] = [60.0, 200.0, 600.0, 2000.0, 6000.0, 12000.0];

impl EqCell {
    pub fn default_for(cell: usize) -> Self {
        EqCell {
            on: false,
            kind: match cell {
                0 => EqType::LowShelf,
                5 => EqType::HighShelf,
                _ => EqType::Peak,
            },
            gain_db: 0.0,
            freq: DEFAULT_EQ_FREQS[cell.min(EQ_CELLS - 1)],
            q: 1.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BusEqChannel {
    pub cells: [EqCell; EQ_CELLS],
    pub trim_db: f32,
    pub delay_ms: f32,
}

impl Default for BusEqChannel {
    fn default() -> Self {
        BusEqChannel {
            cells: std::array::from_fn(EqCell::default_for),
            trim_db: 0.0,
            delay_ms: 0.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BusParams {
    pub gain_db: f32,
    pub limit_db: f32,
    pub mute: bool,
    /// 0 = stereo, 1 = mono, 2 = mono + swapped (help: "click again to exchange L and R").
    pub mono: u8,
    pub mode: BusMode,
    pub eq_on: bool,
    pub sel: bool,
    pub monitor: bool,
    pub eq: [BusEqChannel; MAX_CH],
}

impl Default for BusParams {
    fn default() -> Self {
        BusParams {
            gain_db: 0.0,
            limit_db: LIMIT_MAX_DB,
            mute: false,
            mono: 0,
            mode: BusMode::Normal,
            eq_on: false,
            sel: false,
            monitor: false,
            eq: [BusEqChannel::default(); MAX_CH],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TapeParams {
    pub route: [bool; NUM_BUSES],
    pub gain_db: f32,
}

impl Default for TapeParams {
    fn default() -> Self {
        TapeParams { route: [true, false, false, false, false, false], gain_db: 0.0 }
    }
}

/// Composite patch: for each of 8 bus channels, which input channel (0 = none, 1.. = global
/// input channel index: strip1 L,R, strip2 L,R, strip3 L,R, virtual1 1..8, virtual2 1..8).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CompositePatch {
    pub channels: [u8; MAX_CH],
    pub post_fader: bool,
}

impl Default for CompositePatch {
    fn default() -> Self {
        CompositePatch { channels: [0; MAX_CH], post_fader: false }
    }
}

pub const COMPOSITE_INPUTS: usize = 2 * NUM_HW_STRIPS + MAX_CH * (NUM_STRIPS - NUM_HW_STRIPS);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MixParams {
    pub strips: [StripParams; NUM_STRIPS],
    pub buses: [BusParams; NUM_BUSES],
    pub tape: TapeParams,
    pub composite: CompositePatch,
}

impl Default for MixParams {
    fn default() -> Self {
        let mut strips = [StripParams::default(); NUM_STRIPS];
        // Virtual strips default to A1 + B1 like hardware ones; karaoke/MC off.
        for (i, s) in strips.iter_mut().enumerate() {
            if is_virtual_strip(i) {
                s.comp_on = false;
                s.gate_on = false;
            }
        }
        MixParams {
            strips,
            buses: [BusParams::default(); NUM_BUSES],
            tape: TapeParams::default(),
            composite: CompositePatch::default(),
        }
    }
}

impl MixParams {
    pub fn any_solo(&self) -> bool {
        self.strips.iter().any(|s| s.solo)
    }
}

/// Three-way merge used by the GUI write-back: every leaf field where `edited` differs from
/// `before` is copied onto `shared`; all other fields keep the shared value, so concurrent
/// writes from MIDI, the hotkey thread or the tray made during the frame survive.
pub fn merge(before: &MixParams, edited: &MixParams, shared: &mut MixParams) {
    before.merge_into(edited, shared);
}

trait Merge {
    fn merge_into(&self, edited: &Self, shared: &mut Self);
}

macro_rules! merge_leaf {
    ($($t:ty),*) => {$(
        impl Merge for $t {
            fn merge_into(&self, edited: &Self, shared: &mut Self) {
                if self != edited {
                    *shared = *edited;
                }
            }
        }
    )*};
}
merge_leaf!(f32, bool, u8, BusMode, PanelType, EqType);

impl<T: Merge, const N: usize> Merge for [T; N] {
    fn merge_into(&self, edited: &Self, shared: &mut Self) {
        for ((b, e), s) in self.iter().zip(edited).zip(shared.iter_mut()) {
            b.merge_into(e, s);
        }
    }
}

/// The destructuring pattern has no `..`, so adding a field without listing it here fails to compile.
macro_rules! merge_struct {
    ($t:ident { $($f:ident),* $(,)? }) => {
        impl Merge for $t {
            fn merge_into(&self, edited: &Self, shared: &mut Self) {
                let $t { $($f: _),* } = self;
                $( self.$f.merge_into(&edited.$f, &mut shared.$f); )*
            }
        }
    };
}
merge_struct!(CompParams { gain_in, attack_ms, release_ms, knee, ratio, threshold_db, auto_makeup, gain_out });
merge_struct!(GateParams { threshold_db, damping_db, bp_sidechain_hz, attack_ms, hold_ms, release_ms });
merge_struct!(StripParams {
    gain_db, limit_db, mute, solo, mono, mute_center, route, karaoke, strip_mode, comp_on, gate_on, comp, gate,
    panel_type, pan_x, pan_y, color_x, color_y, fx_x, fx_y, eq_gain, denoise_on, denoise,
});
merge_struct!(EqCell { on, kind, gain_db, freq, q });
merge_struct!(BusEqChannel { cells, trim_db, delay_ms });
merge_struct!(BusParams { gain_db, limit_db, mute, mono, mode, eq_on, sel, monitor, eq });
merge_struct!(TapeParams { route, gain_db });
merge_struct!(CompositePatch { channels, post_fader });
merge_struct!(MixParams { strips, buses, tape, composite });

/// Non-realtime configuration: labels and selected devices.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Labels {
    pub strips: [String; NUM_STRIPS],
    pub buses: [String; NUM_BUSES],
}

impl Labels {
    pub fn strip_display(&self, i: usize) -> String {
        if !self.strips[i].is_empty() {
            return self.strips[i].clone();
        }
        if is_virtual_strip(i) {
            ["Virtual Input 1", "Virtual Input 2", "Virtual Input 3"][i - NUM_HW_STRIPS].to_string()
        } else {
            format!("Hardware Input {}", i + 1)
        }
    }
    pub fn bus_display(&self, i: usize) -> String {
        if !self.buses[i].is_empty() {
            self.buses[i].clone()
        } else {
            BUS_NAMES[i].to_string()
        }
    }
}

pub fn clamp_gain(db: f32) -> f32 {
    db.clamp(GAIN_MIN_DB, GAIN_MAX_DB)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_keeps_concurrent_mute_and_local_gain() {
        let before = MixParams::default();
        let mut edited = before;
        edited.strips[0].gain_db = -6.0;
        let mut shared = before;
        shared.strips[0].mute = true; // e.g. push-to-talk hotkey during the frame
        merge(&before, &edited, &mut shared);
        assert_eq!(shared.strips[0].gain_db, -6.0);
        assert!(shared.strips[0].mute);
    }

    #[test]
    fn merge_keeps_preset_recall_in_same_frame_as_widget_edit() {
        let before = MixParams::default();
        let mut edited = before;
        edited.buses[1].gain_db = -3.0;
        let mut preset = MixParams::default();
        preset.strips[2].gain_db = 4.0;
        preset.strips[2].route = [false, true, false, false, false, true];
        preset.buses[1].mode = BusMode::TvMix;
        preset.buses[4].eq[3].cells[2].gain_db = 5.0;
        let mut shared = preset; // recall wrote the shared params directly
        merge(&before, &edited, &mut shared);
        let mut expected = preset;
        expected.buses[1].gain_db = -3.0;
        assert_eq!(shared, expected);
    }

    #[test]
    fn merge_is_per_array_element_and_nested_field() {
        let before = MixParams::default();
        let mut edited = before;
        edited.strips[1].route[4] = true;
        edited.buses[0].eq[7].cells[5].freq = 9000.0;
        edited.strips[3].comp.ratio = 3.0;
        let mut shared = before;
        shared.strips[1].route[0] = false;
        shared.buses[0].eq[7].cells[5].q = 2.0;
        shared.strips[3].comp.threshold_db = -10.0;
        merge(&before, &edited, &mut shared);
        assert!(shared.strips[1].route[4] && !shared.strips[1].route[0]);
        assert_eq!(shared.buses[0].eq[7].cells[5].freq, 9000.0);
        assert_eq!(shared.buses[0].eq[7].cells[5].q, 2.0);
        assert_eq!(shared.strips[3].comp.ratio, 3.0);
        assert_eq!(shared.strips[3].comp.threshold_db, -10.0);
    }

    #[test]
    fn merge_without_edits_leaves_shared_unchanged() {
        let before = MixParams::default();
        let mut shared = before;
        shared.tape.gain_db = -12.0;
        shared.composite.channels[2] = 7;
        let expected = shared;
        merge(&before, &before, &mut shared);
        assert_eq!(shared, expected);
    }
}
