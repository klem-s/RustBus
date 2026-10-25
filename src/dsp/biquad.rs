//! RBJ "Audio EQ Cookbook" biquads.

use crate::params::{EqCell, EqType};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Coefs {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
}

impl Coefs {
    pub const IDENTITY: Coefs = Coefs { b0: 1.0, b1: 0.0, b2: 0.0, a1: 0.0, a2: 0.0 };

    fn norm(b0: f64, b1: f64, b2: f64, a0: f64, a1: f64, a2: f64) -> Coefs {
        Coefs {
            b0: (b0 / a0) as f32,
            b1: (b1 / a0) as f32,
            b2: (b2 / a0) as f32,
            a1: (a1 / a0) as f32,
            a2: (a2 / a0) as f32,
        }
    }

    pub fn peak(sr: f32, f: f32, q: f32, gain_db: f32) -> Coefs {
        let (w, alpha) = w_alpha(sr, f, q);
        let a = 10f64.powf(gain_db as f64 / 40.0);
        let c = w.cos();
        Self::norm(1.0 + alpha * a, -2.0 * c, 1.0 - alpha * a, 1.0 + alpha / a, -2.0 * c, 1.0 - alpha / a)
    }

    pub fn low_shelf(sr: f32, f: f32, q: f32, gain_db: f32) -> Coefs {
        let (w, alpha) = w_alpha(sr, f, q);
        let a = 10f64.powf(gain_db as f64 / 40.0);
        let c = w.cos();
        let sa = 2.0 * a.sqrt() * alpha;
        Self::norm(
            a * ((a + 1.0) - (a - 1.0) * c + sa),
            2.0 * a * ((a - 1.0) - (a + 1.0) * c),
            a * ((a + 1.0) - (a - 1.0) * c - sa),
            (a + 1.0) + (a - 1.0) * c + sa,
            -2.0 * ((a - 1.0) + (a + 1.0) * c),
            (a + 1.0) + (a - 1.0) * c - sa,
        )
    }

    pub fn high_shelf(sr: f32, f: f32, q: f32, gain_db: f32) -> Coefs {
        let (w, alpha) = w_alpha(sr, f, q);
        let a = 10f64.powf(gain_db as f64 / 40.0);
        let c = w.cos();
        let sa = 2.0 * a.sqrt() * alpha;
        Self::norm(
            a * ((a + 1.0) + (a - 1.0) * c + sa),
            -2.0 * a * ((a - 1.0) + (a + 1.0) * c),
            a * ((a + 1.0) + (a - 1.0) * c - sa),
            (a + 1.0) - (a - 1.0) * c + sa,
            2.0 * ((a - 1.0) - (a + 1.0) * c),
            (a + 1.0) - (a - 1.0) * c - sa,
        )
    }

    pub fn high_pass(sr: f32, f: f32, q: f32) -> Coefs {
        let (w, alpha) = w_alpha(sr, f, q);
        let c = w.cos();
        Self::norm((1.0 + c) / 2.0, -(1.0 + c), (1.0 + c) / 2.0, 1.0 + alpha, -2.0 * c, 1.0 - alpha)
    }

    pub fn low_pass(sr: f32, f: f32, q: f32) -> Coefs {
        let (w, alpha) = w_alpha(sr, f, q);
        let c = w.cos();
        Self::norm((1.0 - c) / 2.0, 1.0 - c, (1.0 - c) / 2.0, 1.0 + alpha, -2.0 * c, 1.0 - alpha)
    }

    pub fn band_pass(sr: f32, f: f32, q: f32) -> Coefs {
        let (w, alpha) = w_alpha(sr, f, q);
        let c = w.cos();
        Self::norm(alpha, 0.0, -alpha, 1.0 + alpha, -2.0 * c, 1.0 - alpha)
    }

    /// Magnitude response in dB at frequency `f` (for EQ curve display).
    pub fn magnitude_db(&self, sr: f32, f: f32) -> f32 {
        let w = 2.0 * std::f64::consts::PI * f as f64 / sr as f64;
        let (c1, s1, c2, s2) = (w.cos(), -w.sin(), (2.0 * w).cos(), -(2.0 * w).sin());
        let (b0, b1, b2, a1, a2) = (self.b0 as f64, self.b1 as f64, self.b2 as f64, self.a1 as f64, self.a2 as f64);
        let nr = b0 + b1 * c1 + b2 * c2;
        let ni = b1 * s1 + b2 * s2;
        let dr = 1.0 + a1 * c1 + a2 * c2;
        let di = a1 * s1 + a2 * s2;
        let mag = ((nr * nr + ni * ni) / (dr * dr + di * di).max(1e-30)).sqrt();
        (20.0 * mag.max(1e-12).log10()) as f32
    }

    pub fn from_cell(sr: f32, cell: &EqCell) -> Coefs {
        if !cell.on {
            return Coefs::IDENTITY;
        }
        let q = cell.q.max(0.1);
        match cell.kind {
            EqType::Peak => Self::peak(sr, cell.freq, q, cell.gain_db),
            EqType::LowShelf => Self::low_shelf(sr, cell.freq, q.min(1.5), cell.gain_db),
            EqType::HighShelf => Self::high_shelf(sr, cell.freq, q.min(1.5), cell.gain_db),
            EqType::LowCut => Self::high_pass(sr, cell.freq, q),
            EqType::HighCut => Self::low_pass(sr, cell.freq, q),
        }
    }
}

fn w_alpha(sr: f32, f: f32, q: f32) -> (f64, f64) {
    let f = (f as f64).clamp(10.0, sr as f64 * 0.49);
    let w = 2.0 * std::f64::consts::PI * f / sr as f64;
    (w, w.sin() / (2.0 * q.max(0.05) as f64))
}

/// Transposed direct form II biquad.
#[derive(Clone, Copy, Debug)]
pub struct Biquad {
    pub c: Coefs,
    z1: f32,
    z2: f32,
}

impl Default for Biquad {
    fn default() -> Self {
        Biquad { c: Coefs::IDENTITY, z1: 0.0, z2: 0.0 }
    }
}

impl Biquad {
    pub fn new(c: Coefs) -> Self {
        Biquad { c, z1: 0.0, z2: 0.0 }
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let c = &self.c;
        let y = c.b0 * x + self.z1;
        self.z1 = c.b1 * x - c.a1 * y + self.z2;
        self.z2 = c.b2 * x - c.a2 * y;
        // Flush denormals.
        if self.z1.abs() < 1e-20 {
            self.z1 = 0.0;
        }
        if self.z2.abs() < 1e-20 {
            self.z2 = 0.0;
        }
        y
    }

    pub fn is_identity(&self) -> bool {
        self.c == Coefs::IDENTITY
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gain_at(c: Coefs, sr: f32, f: f32) -> f32 {
        let mut b = Biquad { c, ..Default::default() };
        let n = (sr * 0.5) as usize;
        let mut peak = 0.0f32;
        for i in 0..n {
            let x = (2.0 * std::f32::consts::PI * f * i as f32 / sr).sin();
            let y = b.process(x);
            if i > n / 2 {
                peak = peak.max(y.abs());
            }
        }
        20.0 * peak.log10()
    }

    #[test]
    fn peak_boosts_at_centre() {
        let g = gain_at(Coefs::peak(48000.0, 1000.0, 1.0, 6.0), 48000.0, 1000.0);
        assert!((g - 6.0).abs() < 0.3, "{g}");
    }

    #[test]
    fn low_shelf_boosts_lows_not_highs() {
        let c = Coefs::low_shelf(48000.0, 200.0, 0.7, 9.0);
        assert!((gain_at(c, 48000.0, 40.0) - 9.0).abs() < 0.6);
        // 6 kHz = 8 samples/cycle, so the sampled peak hits the true crest.
        assert!(gain_at(c, 48000.0, 6000.0).abs() < 0.3);
    }

    #[test]
    fn high_pass_cuts() {
        let c = Coefs::high_pass(48000.0, 1000.0, 0.707);
        assert!(gain_at(c, 48000.0, 100.0) < -30.0);
    }
}
