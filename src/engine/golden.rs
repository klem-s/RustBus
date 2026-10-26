//! Test-only helpers for the characterization goldens (`golden_*` tests).
//! Characterization golden: must not change during refactor Phases 2-5.
//!
//! Everything here is deterministic and toolchain-independent: a fixed LCG for input,
//! a hand-written FNV-1a 64 (std's `DefaultHasher` is not stable across Rust versions),
//! and outputs quantized to 2^-20 before hashing.

use crate::params::*;

/// FNV-1a 64-bit.
pub struct Fnv(u64);

impl Fnv {
    pub fn new() -> Self {
        Fnv(0xcbf2_9ce4_8422_2325)
    }
    pub fn bytes(&mut self, data: &[u8]) {
        for b in data {
            self.0 ^= *b as u64;
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    /// Hash samples quantized to 2^-20 (saturating), little-endian i32.
    pub fn samples(&mut self, data: &[f32]) {
        for x in data {
            let q = (*x * (1u32 << 20) as f32).round() as i32;
            self.bytes(&q.to_le_bytes());
        }
    }
    pub fn finish(&self) -> u64 {
        self.0
    }
}

/// Numerical Recipes LCG; `next` returns a value in [-1, 1).
pub struct Lcg(u32);

impl Lcg {
    pub fn new(seed: u32) -> Self {
        Lcg(seed)
    }
    pub fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (self.0 >> 8) as f32 / (1u32 << 23) as f32 - 1.0
    }
}

/// Input level for block `b`: a slow staircase so gate, compressor and limiters move.
pub fn level(b: usize) -> f32 {
    0.05 + 0.15 * (b % 8) as f32
}

/// Non-default parameters exercising routes, EQs, dynamics, panels, bus modes and limiters.
pub fn params() -> MixParams {
    let mut p = MixParams::default();
    {
        let s = &mut p.strips[0];
        s.gate_on = true;
        s.gate.set_knob(5.0);
        s.gate.hold_ms = 20.0;
        s.gate.release_ms = 80.0;
        s.comp_on = true;
        s.comp.set_knob(4.0);
        s.comp.threshold_db = -18.0;
        s.panel_type = PanelType::Color;
        s.color_x = 0.3;
        s.color_y = -0.2;
        s.gain_db = -3.0;
        s.limit_db = -6.0;
        s.route = [true, true, false, true, false, false];
    }
    {
        let s = &mut p.strips[1];
        s.panel_type = PanelType::Modulation;
        s.fx_x = 0.2;
        s.fx_y = 0.6;
        s.mono = true;
        s.comp_on = false;
        s.denoise_on = true;
        s.denoise = 0.5;
        s.route = [true, false, true, false, false, false];
    }
    {
        let s = &mut p.strips[2];
        s.pan_x = 0.25;
        s.pan_y = 0.2;
        s.gain_db = 4.0;
        s.limit_db = -10.0;
        s.route = [true, false, false, false, true, false];
    }
    {
        let s = &mut p.strips[3];
        s.eq_gain = [4.0, -3.0, 2.0];
        s.mute_center = true;
        s.pan_x = -0.2;
        s.pan_y = 0.1;
        s.route = [true, true, false, true, false, false];
    }
    {
        let s = &mut p.strips[4];
        s.karaoke = 2;
        s.gain_db = -2.0;
        s.route = [false, false, true, false, true, false];
    }
    {
        let s = &mut p.strips[5];
        s.eq_gain = [0.0, 3.0, 0.0];
        s.mono = true;
        s.route = [true, false, false, false, false, true];
    }
    {
        let b = &mut p.buses[0];
        b.eq_on = true;
        for c in 0..MAX_CH {
            b.eq[c].cells[1] = EqCell { on: true, kind: EqType::LowCut, gain_db: 0.0, freq: 80.0, q: 0.7 };
            b.eq[c].cells[3] = EqCell { on: true, kind: EqType::Peak, gain_db: 6.0, freq: 1000.0, q: 1.2 };
            b.eq[c].cells[5] = EqCell { on: true, kind: EqType::HighShelf, gain_db: -4.0, freq: 8000.0, q: 0.7 };
            b.eq[c].trim_db = -2.0;
        }
        b.eq[1].delay_ms = 3.0;
        b.limit_db = -6.0;
    }
    p.buses[1].mode = BusMode::UpMix41;
    p.buses[1].mono = 1;
    p.buses[2].mode = BusMode::Composite;
    p.composite.channels = [1, 2, 7, 8, 0, 22, 30, 4];
    p.composite.post_fader = true;
    p.buses[3].mode = BusMode::MixDownA;
    p.buses[3].mono = 2;
    p.buses[4].mode = BusMode::TvMix;
    p.buses[4].gain_db = -4.0;
    p.buses[5].mode = BusMode::RearOnly;
    p.buses[5].gain_db = 6.0;
    p.buses[5].limit_db = -12.0;
    p.tape.route = [true, false, false, true, false, false];
    p.tape.gain_db = -6.0;
    p
}
