//! Voice noise removal with RNNoise (pure-Rust port `nnnoiseless`), a RustBus extension.
//!
//! RNNoise works on fixed 480-sample frames (10 ms at 48 kHz) of 16-bit-scaled audio.
//! Samples go through a one-frame FIFO, so the strip gets a constant 480-sample delay
//! and `process` can take any block size without allocating.

use nnnoiseless::DenoiseState;

pub const FRAME: usize = DenoiseState::FRAME_SIZE;
const SCALE: f32 = 32768.0;

pub struct Denoiser {
    states: [Box<DenoiseState<'static>>; 2],
    input: [[f32; FRAME]; 2],
    /// Last processed frame and its dry copy, played back while the next one fills.
    wet: [[f32; FRAME]; 2],
    dry: [[f32; FRAME]; 2],
    pos: usize,
    /// Voice probability of the last frame (0..1).
    pub vad: f32,
}

impl Default for Denoiser {
    fn default() -> Self {
        Denoiser {
            states: [DenoiseState::new(), DenoiseState::new()],
            input: [[0.0; FRAME]; 2],
            wet: [[0.0; FRAME]; 2],
            dry: [[0.0; FRAME]; 2],
            pos: 0,
            vad: 0.0,
        }
    }
}

impl Denoiser {
    /// Clear the FIFO when the effect is switched back on, so no stale audio plays.
    /// The model state is kept (resetting it would allocate); it re-adapts in a few frames.
    pub fn reset(&mut self) {
        self.input = [[0.0; FRAME]; 2];
        self.wet = [[0.0; FRAME]; 2];
        self.dry = [[0.0; FRAME]; 2];
        self.pos = 0;
        self.vad = 0.0;
    }

    /// Denoise a stereo pair in place. `amount` 0..1 blends dry (delayed) and denoised.
    pub fn process(&mut self, l: &mut [f32], r: &mut [f32], frames: usize, amount: f32) {
        let a = amount.clamp(0.0, 1.0);
        for i in 0..frames {
            let p = self.pos;
            let (xl, xr) = (l[i], r[i]);
            l[i] = self.dry[0][p] + (self.wet[0][p] - self.dry[0][p]) * a;
            r[i] = self.dry[1][p] + (self.wet[1][p] - self.dry[1][p]) * a;
            self.input[0][p] = xl * SCALE;
            self.input[1][p] = xr * SCALE;
            self.pos += 1;
            if self.pos == FRAME {
                self.pos = 0;
                self.run_frame();
            }
        }
    }

    fn run_frame(&mut self) {
        let mut out = [0.0f32; FRAME];
        self.vad = self.states[0].process_frame(&mut out, &self.input[0]);
        for (w, o) in self.wet[0].iter_mut().zip(out) {
            *w = o / SCALE;
        }
        // A mono mic duplicated to both sides only needs one pass.
        if self.input[1] == self.input[0] {
            self.wet[1] = self.wet[0];
        } else {
            let v = self.states[1].process_frame(&mut out, &self.input[1]);
            self.vad = self.vad.max(v);
            for (w, o) in self.wet[1].iter_mut().zip(out) {
                *w = o / SCALE;
            }
        }
        for c in 0..2 {
            for (d, x) in self.dry[c].iter_mut().zip(self.input[c]) {
                *d = x / SCALE;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic white-ish noise.
    fn noise(seed: &mut u32) -> f32 {
        *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        ((*seed >> 9) as f32 / (1u32 << 23) as f32) * 2.0 - 1.0
    }

    #[test]
    fn delay_is_one_frame_when_dry() {
        let mut d = Denoiser::default();
        let n = FRAME * 3;
        let mut l: Vec<f32> = (0..n).map(|i| (i as f32 * 0.01).sin() * 0.5).collect();
        let orig = l.clone();
        let mut r = l.clone();
        // Odd block sizes must not matter.
        let mut done = 0;
        for chunk in [100, 333, 47, 960] {
            let end = (done + chunk).min(n);
            let (a, b) = (&mut l[done..end], &mut r[done..end]);
            d.process(a, b, end - done, 0.0);
            done = end;
        }
        for i in FRAME..n {
            assert!((l[i] - orig[i - FRAME]).abs() < 1e-6);
        }
    }

    #[test]
    fn removes_steady_noise() {
        // Roughly pink (low-passed white) noise, like a fan or room hum. Measured: 33-54 dB
        // less from -53 to -17 dBFS. Harsh white noise is mostly left alone by RNNoise.
        let rms = |x: &[f32]| (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt();
        for level in [0.01f32, 0.05] {
            let mut d = Denoiser::default();
            let mut seed = 7;
            let n = FRAME * 300;
            let mut lp = 0.0f32;
            let mut l: Vec<f32> = (0..n)
                .map(|_| {
                    lp = 0.97 * lp + 0.03 * noise(&mut seed);
                    lp * level * 10.0
                })
                .collect();
            let mut r = l.clone();
            let before = rms(&l[n - FRAME * 100..]);
            d.process(&mut l, &mut r, n, 1.0);
            let after = rms(&l[n - FRAME * 100..]);
            let less_db = 20.0 * (before / after.max(1e-9)).log10();
            assert!(less_db > 20.0, "level {level}: only {less_db:.1} dB less");
        }
    }
}
