//! Compressor, gate and brick-wall limiter (stereo-linked / multichannel-linked).

use super::{db_to_lin, lin_to_db};
use crate::params::{CompParams, GateParams};

fn coef(sr: f32, ms: f32) -> f32 {
    if ms <= 0.0 {
        0.0
    } else {
        (-1.0 / (ms * 0.001 * sr)).exp()
    }
}

/// Feed-forward RMS-ish compressor with soft knee and auto make-up.
#[derive(Clone, Copy, Debug)]
pub struct Compressor {
    env_db: f32,
}

impl Default for Compressor {
    fn default() -> Self {
        Compressor { env_db: -120.0 }
    }
}

impl Compressor {
    /// Static gain computer: returns gain reduction in dB (<= 0) for a level in dB.
    pub fn curve(p: &CompParams, level_db: f32) -> f32 {
        let ratio = p.ratio.max(1.0);
        if ratio <= 1.0001 {
            return 0.0;
        }
        let knee_db = (p.knee.clamp(0.0, 1.0)) * 12.0;
        let over = level_db - p.threshold_db;
        let slope = 1.0 / ratio - 1.0;
        if knee_db > 0.0 && over.abs() <= knee_db / 2.0 {
            slope * (over + knee_db / 2.0).powi(2) / (2.0 * knee_db)
        } else if over > 0.0 {
            slope * over
        } else {
            0.0
        }
    }

    pub fn makeup_db(p: &CompParams) -> f32 {
        if p.auto_makeup {
            // Compensate half the reduction a full-scale signal would get.
            -Self::curve(p, 0.0) * 0.5
        } else {
            0.0
        }
    }

    /// Process `frames` frames of planar channels in place.
    pub fn process(&mut self, sr: f32, p: &CompParams, chans: &mut [&mut [f32]], frames: usize) {
        if p.ratio <= 1.0001 && p.gain_in == 0.0 && p.gain_out == 0.0 {
            return;
        }
        let att = coef(sr, p.attack_ms.max(0.1));
        let rel = coef(sr, p.release_ms.max(1.0));
        let gin = db_to_lin(p.gain_in);
        let makeup = db_to_lin(Self::makeup_db(p) + p.gain_out);
        for i in 0..frames {
            let mut peak = 0.0f32;
            for c in chans.iter_mut() {
                c[i] *= gin;
                peak = peak.max(c[i].abs());
            }
            let lvl = lin_to_db(peak);
            let k = if lvl > self.env_db { att } else { rel };
            self.env_db = lvl + (self.env_db - lvl) * k;
            let g = db_to_lin(Self::curve(p, self.env_db)) * makeup;
            for c in chans.iter_mut() {
                c[i] *= g;
            }
        }
    }
}

/// Noise gate with hold and damping (floor attenuation).
#[derive(Clone, Copy, Debug)]
pub struct Gate {
    env: f32,
    gain: f32,
    hold_left: f32,
}

impl Default for Gate {
    fn default() -> Self {
        Gate { env: 0.0, gain: 1.0, hold_left: 0.0 }
    }
}

impl Gate {
    pub fn process(&mut self, sr: f32, p: &GateParams, chans: &mut [&mut [f32]], frames: usize) {
        if p.threshold_db <= -59.9 {
            self.gain = 1.0;
            return;
        }
        let thr = db_to_lin(p.threshold_db);
        let floor = db_to_lin(p.damping_db.min(0.0));
        let env_rel = coef(sr, 10.0);
        let att = coef(sr, p.attack_ms.max(0.05));
        let rel = coef(sr, p.release_ms.max(1.0));
        let hold = p.hold_ms * 0.001 * sr;
        for i in 0..frames {
            let mut peak = 0.0f32;
            for c in chans.iter() {
                peak = peak.max(c[i].abs());
            }
            self.env = if peak > self.env { peak } else { peak + (self.env - peak) * env_rel };
            let target = if self.env >= thr {
                self.hold_left = hold;
                1.0
            } else if self.hold_left > 0.0 {
                self.hold_left -= 1.0;
                1.0
            } else {
                floor
            };
            let k = if target > self.gain { att } else { rel };
            self.gain = target + (self.gain - target) * k;
            for c in chans.iter_mut() {
                c[i] *= self.gain;
            }
        }
    }

    pub fn current_gain(&self) -> f32 {
        self.gain
    }
}

/// Brick-wall limiter: instant attack, smooth release, linked across channels.
#[derive(Clone, Copy, Debug)]
pub struct Limiter {
    gain: f32,
}

impl Default for Limiter {
    fn default() -> Self {
        Limiter { gain: 1.0 }
    }
}

impl Limiter {
    pub fn process(&mut self, sr: f32, limit_db: f32, chans: &mut [&mut [f32]], frames: usize) {
        if limit_db >= crate::params::LIMIT_MAX_DB {
            self.gain = 1.0;
            return;
        }
        let ceil = db_to_lin(limit_db);
        let rel = coef(sr, 80.0);
        for i in 0..frames {
            let mut peak = 0.0f32;
            for c in chans.iter() {
                peak = peak.max(c[i].abs());
            }
            let need = if peak * self.gain > ceil { ceil / peak } else { 1.0 };
            self.gain = if need < self.gain { need } else { need + (self.gain - need) * rel };
            for c in chans.iter_mut() {
                c[i] = (c[i] * self.gain).clamp(-ceil, ceil);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comp_curve_hard_knee() {
        let p = CompParams { ratio: 4.0, threshold_db: -20.0, knee: 0.0, ..Default::default() };
        assert_eq!(Compressor::curve(&p, -30.0), 0.0);
        // 12 dB over threshold at 4:1 -> 9 dB reduction.
        assert!((Compressor::curve(&p, -8.0) + 9.0).abs() < 1e-4);
    }

    #[test]
    fn limiter_caps_level() {
        let mut l = Limiter::default();
        let mut a = vec![1.0f32; 256];
        let mut b = vec![-1.0f32; 256];
        l.process(48000.0, -6.0, &mut [&mut a[..], &mut b[..]], 256);
        let ceil = db_to_lin(-6.0);
        assert!(a.iter().chain(b.iter()).all(|x| x.abs() <= ceil + 1e-6));
    }

    #[test]
    fn gate_closes_on_silence() {
        let mut g = Gate::default();
        let p = GateParams { threshold_db: -30.0, hold_ms: 0.0, release_ms: 5.0, ..Default::default() };
        let mut x = vec![0.001f32; 48000];
        g.process(48000.0, &p, &mut [&mut x[..]], 48000);
        assert!(g.current_gain() < 0.01);
    }
}
