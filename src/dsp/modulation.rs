//! Intellipan helpers: colour panel (tonal tilt) and modulation panel (chorus).
//! The original algorithms are unknown [U]; these reproduce the documented intent:
//! "equalize your input sound quickly" and "add chorus / phaser / modulation FX".

use super::biquad::{Biquad, Coefs};
use super::DelayLine;

/// Colour panel: x tilts brightness (-0.5 dark .. +0.5 bright), y adds presence/warmth.
#[derive(Clone, Copy, Debug, Default)]
pub struct ColorPanel {
    low: [Biquad; 2],
    high: [Biquad; 2],
    mid: [Biquad; 2],
    last: (f32, f32),
}

impl ColorPanel {
    pub fn update(&mut self, sr: f32, x: f32, y: f32) {
        if self.last == (x, y) {
            return;
        }
        self.last = (x, y);
        let tilt = x.clamp(-0.5, 0.5) * 18.0;
        let mid = y.clamp(-0.5, 0.5) * 12.0;
        let lo = if tilt.abs() < 0.01 { Coefs::IDENTITY } else { Coefs::low_shelf(sr, 250.0, 0.7, -tilt) };
        let hi = if tilt.abs() < 0.01 { Coefs::IDENTITY } else { Coefs::high_shelf(sr, 3000.0, 0.7, tilt) };
        let md = if mid.abs() < 0.01 { Coefs::IDENTITY } else { Coefs::peak(sr, 1500.0, 0.8, mid) };
        for c in 0..2 {
            self.low[c].c = lo;
            self.high[c].c = hi;
            self.mid[c].c = md;
        }
    }

    pub fn process(&mut self, l: &mut [f32], r: &mut [f32], frames: usize) {
        if self.last == (0.0, 0.0) {
            return;
        }
        for (c, buf) in [l, r].into_iter().enumerate() {
            for x in buf[..frames].iter_mut() {
                *x = self.mid[c].process(self.high[c].process(self.low[c].process(*x)));
            }
        }
    }
}

/// Modulation panel: x = rate/depth character, y = wet amount (stereo chorus).
#[derive(Clone, Debug)]
pub struct Chorus {
    lines: [DelayLine; 2],
    phase: f32,
}

impl Chorus {
    pub fn new(sr: f32) -> Self {
        let n = (sr * 0.05) as usize;
        Chorus { lines: [DelayLine::new(n), DelayLine::new(n)], phase: 0.0 }
    }

    pub fn process(&mut self, sr: f32, x: f32, y: f32, l: &mut [f32], r: &mut [f32], frames: usize) {
        let wet = y.clamp(0.0, 1.0);
        if wet <= 0.001 {
            return;
        }
        let rate = 0.2 + (x.clamp(-0.5, 0.5) + 0.5) * 4.0; // 0.2..4.2 Hz
        let depth = 0.002 * sr; // 2 ms
        let base = 0.010 * sr; // 10 ms
        let inc = rate / sr * std::f32::consts::TAU;
        for i in 0..frames {
            self.phase = (self.phase + inc) % std::f32::consts::TAU;
            let dl = base + depth * self.phase.sin();
            let dr = base + depth * (self.phase + std::f32::consts::FRAC_PI_2).sin();
            self.lines[0].push(l[i]);
            self.lines[1].push(r[i]);
            l[i] = l[i] * (1.0 - wet * 0.5) + self.lines[0].read(dl) * wet * 0.5;
            r[i] = r[i] * (1.0 - wet * 0.5) + self.lines[1].read(dr) * wet * 0.5;
        }
    }
}
