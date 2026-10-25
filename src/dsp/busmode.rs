//! Bus channel-routing modes. Formulas follow the shipped help (ANALYSIS §4).
//! Channel order (WAVEFORMATEXTENSIBLE 7.1): 0 FL, 1 FR, 2 FC, 3 SW/LFE, 4 RL, 5 RR, 6 SL, 7 SR.

use crate::params::{BusMode, MAX_CH};

const FL: usize = 0;
const FR: usize = 1;
const FC: usize = 2;
const SW: usize = 3;
const RL: usize = 4;
const RR: usize = 5;
const SL: usize = 6;
const SR: usize = 7;

/// Apply `mode` in place to planar 8-channel bus data. `Composite` is handled by the
/// mixer (it needs access to the input channels) and is a no-op here.
pub fn apply(mode: BusMode, ch: &mut [Vec<f32>; MAX_CH], frames: usize) {
    match mode {
        BusMode::Normal | BusMode::Composite => {}
        BusMode::MixDownA | BusMode::MixDownB => {
            let b = mode == BusMode::MixDownB;
            for i in 0..frames {
                let common = 0.7 * ch[FC][i] + ch[SW][i];
                let (l, r) = if b {
                    (
                        ch[FL][i] + common + ch[RL][i] + ch[SL][i],
                        ch[FR][i] + common + ch[RR][i] + ch[SR][i],
                    )
                } else {
                    (
                        ch[FL][i] + common + ch[RL][i] - ch[SL][i],
                        ch[FR][i] + common - ch[RR][i] + ch[SR][i],
                    )
                };
                ch[FL][i] = l;
                ch[FR][i] = r;
            }
            zero_from(ch, 2, frames);
        }
        BusMode::Repeat => {
            for i in 0..frames {
                let (l, r) = (ch[FL][i], ch[FR][i]);
                for k in 1..MAX_CH / 2 {
                    ch[2 * k][i] = l;
                    ch[2 * k + 1][i] = r;
                }
            }
        }
        BusMode::TvMix => {
            for i in 0..frames {
                let (l, r) = (ch[FL][i], ch[FR][i]);
                ch[FC][i] = 0.2 * (l + r);
                ch[SW][i] = 0.5 * (l + r);
                ch[RL][i] = 0.7 * (l - r);
                ch[SL][i] = 0.7 * (l - r);
                ch[RR][i] = 0.7 * (r - l);
                ch[SR][i] = 0.7 * (r - l);
            }
        }
        BusMode::UpMix21 | BusMode::UpMix41 | BusMode::UpMix61 => {
            for i in 0..frames {
                let (l, r) = (ch[FL][i], ch[FR][i]);
                ch[FC][i] = 0.0;
                ch[SW][i] = 0.5 * (l + r);
                let (rl, rr) = if mode == BusMode::UpMix21 { (0.0, 0.0) } else { (l, r) };
                ch[RL][i] = rl;
                ch[RR][i] = rr;
                let (sl, sr) = if mode == BusMode::UpMix61 { (l, r) } else { (0.0, 0.0) };
                ch[SL][i] = sl;
                ch[SR][i] = sr;
            }
        }
        BusMode::LeftOnly => dup(ch, FL, FL, frames),
        BusMode::RightOnly => dup(ch, FR, FR, frames),
        BusMode::CenterOnly => dup(ch, FC, FC, frames),
        BusMode::LfeOnly => dup(ch, SW, SW, frames),
        BusMode::RearOnly => dup(ch, RL, RR, frames),
    }
}

fn dup(ch: &mut [Vec<f32>; MAX_CH], left_src: usize, right_src: usize, frames: usize) {
    for i in 0..frames {
        let (l, r) = (ch[left_src][i], ch[right_src][i]);
        ch[FL][i] = l;
        ch[FR][i] = r;
    }
    zero_from(ch, 2, frames);
}

fn zero_from(ch: &mut [Vec<f32>; MAX_CH], first: usize, frames: usize) {
    for c in ch.iter_mut().skip(first) {
        c[..frames].fill(0.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bus(vals: [f32; MAX_CH]) -> [Vec<f32>; MAX_CH] {
        std::array::from_fn(|c| vec![vals[c]; 4])
    }

    #[test]
    fn tvmix_matches_help_formula() {
        let mut b = bus([1.0, 0.5, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        apply(BusMode::TvMix, &mut b, 4);
        assert!((b[FC][0] - 0.3).abs() < 1e-6);
        assert!((b[SW][0] - 0.75).abs() < 1e-6);
        assert!((b[RL][0] - 0.35).abs() < 1e-6);
        assert!((b[SR][0] + 0.35).abs() < 1e-6);
    }

    #[test]
    fn rear_only_extracts_channels_5_6() {
        let mut b = bus([1.0, 1.0, 1.0, 1.0, 0.25, 0.75, 1.0, 1.0]);
        apply(BusMode::RearOnly, &mut b, 4);
        assert_eq!((b[0][0], b[1][0]), (0.25, 0.75));
        assert!(b[2..].iter().all(|c| c[0] == 0.0));
    }

    #[test]
    fn repeat_copies_stereo() {
        let mut b = bus([0.1, 0.2, 9.0, 9.0, 9.0, 9.0, 9.0, 9.0]);
        apply(BusMode::Repeat, &mut b, 4);
        assert_eq!(b[6][0], 0.1);
        assert_eq!(b[7][0], 0.2);
    }

    #[test]
    fn mixdown_b_sums_surrounds() {
        let mut b = bus([1.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
        apply(BusMode::MixDownB, &mut b, 4);
        assert!((b[0][0] - 2.7).abs() < 1e-6);
        assert!((b[1][0] - 0.7).abs() < 1e-6);
    }
}
