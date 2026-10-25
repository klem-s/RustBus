//! DSP building blocks. These are standard textbook algorithms chosen to reproduce
//! the documented *behaviour* of each control; the original proprietary curves were
//! not decompiled (ANALYSIS §3 [U]).

pub mod biquad;
pub mod busmode;
pub mod denoise;
pub mod dynamics;
pub mod modulation;

#[inline]
pub fn db_to_lin(db: f32) -> f32 {
    if db <= -200.0 {
        0.0
    } else {
        10f32.powf(db / 20.0)
    }
}

#[inline]
pub fn lin_to_db(lin: f32) -> f32 {
    if lin <= 1e-10 {
        -200.0
    } else {
        20.0 * lin.log10()
    }
}

/// One-pole smoother for click-free gain changes.
#[derive(Clone, Copy, Debug)]
pub struct Smoother {
    pub current: f32,
    coef: f32,
}

impl Smoother {
    pub fn new(sr: f32, time_ms: f32, initial: f32) -> Self {
        let coef = (-1.0 / (time_ms * 0.001 * sr)).exp();
        Smoother { current: initial, coef }
    }
    #[inline]
    pub fn next(&mut self, target: f32) -> f32 {
        self.current = target + (self.current - target) * self.coef;
        self.current
    }
}

/// Equal-power stereo pan gains for x in -0.5..0.5.
pub fn pan_gains(x: f32) -> (f32, f32) {
    let p = (x.clamp(-0.5, 0.5) + 0.5) * std::f32::consts::FRAC_PI_2;
    // Normalised so centre = unity on both sides (like a mixer balance law).
    let s = std::f32::consts::SQRT_2;
    ((p.cos() * s).min(1.0), (p.sin() * s).min(1.0))
}

/// Simple fractional delay line used by modulation FX and bus delay.
#[derive(Clone, Debug)]
pub struct DelayLine {
    buf: Vec<f32>,
    pos: usize,
}

impl DelayLine {
    pub fn new(max_samples: usize) -> Self {
        DelayLine { buf: vec![0.0; max_samples.max(4)], pos: 0 }
    }
    #[inline]
    pub fn push(&mut self, x: f32) {
        self.buf[self.pos] = x;
        self.pos = (self.pos + 1) % self.buf.len();
    }
    /// Read `d` samples behind the most recently pushed sample (linear interpolation).
    #[inline]
    pub fn read(&self, d: f32) -> f32 {
        let len = self.buf.len();
        let d = d.clamp(0.0, (len - 2) as f32);
        let i = d.floor() as usize;
        let f = d - i as f32;
        let a = self.buf[(self.pos + len - 1 - i) % len];
        let b = self.buf[(self.pos + len - 2 - i) % len];
        a + (b - a) * f
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn db_roundtrip() {
        for db in [-60.0f32, -6.0, 0.0, 6.0, 12.0] {
            assert!((lin_to_db(db_to_lin(db)) - db).abs() < 1e-3);
        }
        assert_eq!(db_to_lin(-300.0), 0.0);
    }

    #[test]
    fn pan_centre_is_unity() {
        let (l, r) = pan_gains(0.0);
        assert!((l - 1.0).abs() < 1e-4 && (r - 1.0).abs() < 1e-4);
        let (l, r) = pan_gains(-0.5);
        assert!((l - 1.0).abs() < 1e-4 && r.abs() < 1e-4);
    }

    #[test]
    fn delay_line_reads_back() {
        let mut d = DelayLine::new(16);
        for i in 0..10 {
            d.push(i as f32);
        }
        assert_eq!(d.read(0.0), 9.0);
        assert_eq!(d.read(3.0), 6.0);
        assert!((d.read(1.5) - 7.5).abs() < 1e-6);
    }
}
