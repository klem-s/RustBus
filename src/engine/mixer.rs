//! Real-time mixing core. No I/O, no allocation in `process`.
//!
//! Signal flow (ANALYSIS §2–4):
//!   HW strip:   input → denoise → gate → comp → intellipan (colour | 3D pan | modulation) → mono
//!               → limiter (pre-fader) → fader/mute/solo → A1..B2
//!   Virt strip: input (8 ch) → 3-band EQ → M.C./karaoke → pan → mono → limiter
//!               → fader/mute/solo → A1..B2
//!   Bus:        Σ strips + tape → (composite) → mode → EQ → mono → fader/mute → limiter

use std::sync::atomic::{AtomicU32, Ordering};

use crate::dsp::biquad::{Biquad, Coefs};
use crate::dsp::denoise::Denoiser;
use crate::dsp::dynamics::{Compressor, Gate, Limiter};
use crate::dsp::modulation::{Chorus, ColorPanel};
use crate::dsp::{busmode, db_to_lin, pan_gains, DelayLine};
use crate::params::*;

pub const MAX_BLOCK: usize = 2048;

/// Peak meters shared with the GUI. Positive f32 bit patterns order like u32, so
/// `fetch_max` gives a lock-free peak-hold; the GUI takes it with `swap(0)`.
pub struct Meters {
    pub strips: [[AtomicU32; MAX_CH]; NUM_STRIPS],
    pub buses: [[AtomicU32; MAX_CH]; NUM_BUSES],
    pub strip_gate: [AtomicU32; NUM_STRIPS],
}

impl Default for Meters {
    fn default() -> Self {
        Meters {
            strips: std::array::from_fn(|_| std::array::from_fn(|_| AtomicU32::new(0))),
            buses: std::array::from_fn(|_| std::array::from_fn(|_| AtomicU32::new(0))),
            strip_gate: std::array::from_fn(|_| AtomicU32::new(1f32.to_bits())),
        }
    }
}

impl Meters {
    #[inline]
    fn put(slot: &AtomicU32, data: &[f32]) {
        let peak = data.iter().fold(0.0f32, |m, x| m.max(x.abs()));
        slot.fetch_max(peak.to_bits(), Ordering::Relaxed);
    }
    pub fn take(slot: &AtomicU32) -> f32 {
        f32::from_bits(slot.swap(0, Ordering::Relaxed))
    }
}

type Planar = [Vec<f32>; MAX_CH];

fn planar() -> Planar {
    std::array::from_fn(|_| vec![0.0; MAX_BLOCK])
}

struct StripDsp {
    gate: Gate,
    comp: Compressor,
    limiter: Limiter,
    color: ColorPanel,
    chorus: Chorus,
    eq: [[Biquad; 3]; MAX_CH],
    eq_last: [f32; 3],
    karaoke_bp: [Biquad; 2],
    karaoke_lp: [Biquad; 2],
    gain: f32,
    /// Hardware strips only.
    denoise: Option<Box<Denoiser>>,
    denoise_active: bool,
}

struct BusDsp {
    eq: [[Biquad; EQ_CELLS]; MAX_CH],
    eq_last: [BusEqChannel; MAX_CH],
    delay: Vec<DelayLine>,
    limiter: Limiter,
    gain: f32,
}

pub struct Mixer {
    sr: f32,
    strips: Vec<StripDsp>,
    busdsp: Vec<BusDsp>,
    /// Caller writes strip inputs here before `process`.
    pub inputs: [Planar; NUM_STRIPS],
    /// Stereo tape-deck playback, mixed into buses per `TapeParams`.
    pub tape: [Vec<f32>; 2],
    /// Bus outputs after `process`.
    pub buses: [Planar; NUM_BUSES],
    pre_fader: [Planar; NUM_STRIPS],
    post_fader: [Planar; NUM_STRIPS],
}

impl Mixer {
    pub fn new(sr: f32) -> Self {
        let strips = (0..NUM_STRIPS)
            .map(|s| StripDsp {
                denoise: (!is_virtual_strip(s)).then(Box::<Denoiser>::default),
                denoise_active: false,
                gate: Gate::default(),
                comp: Compressor::default(),
                limiter: Limiter::default(),
                color: ColorPanel::default(),
                chorus: Chorus::new(sr),
                eq: [[Biquad::default(); 3]; MAX_CH],
                eq_last: [0.0; 3],
                karaoke_bp: [Biquad::new(Coefs::band_pass(sr, 1200.0, 0.5)); 2],
                karaoke_lp: [Biquad::new(Coefs::low_pass(sr, 150.0, 0.707)); 2],
                gain: 1.0,
            })
            .collect();
        let busdsp = (0..NUM_BUSES)
            .map(|_| BusDsp {
                eq: [[Biquad::default(); EQ_CELLS]; MAX_CH],
                eq_last: [BusEqChannel::default(); MAX_CH],
                delay: (0..MAX_CH).map(|_| DelayLine::new((sr * 0.5) as usize)).collect(),
                limiter: Limiter::default(),
                gain: 1.0,
            })
            .collect();
        let mut m = Mixer {
            sr,
            strips,
            busdsp,
            inputs: std::array::from_fn(|_| planar()),
            tape: [vec![0.0; MAX_BLOCK], vec![0.0; MAX_BLOCK]],
            buses: std::array::from_fn(|_| planar()),
            pre_fader: std::array::from_fn(|_| planar()),
            post_fader: std::array::from_fn(|_| planar()),
        };
        // Force coefficient computation on first block.
        for b in m.busdsp.iter_mut() {
            for c in b.eq_last.iter_mut() {
                c.trim_db = f32::NAN;
            }
        }
        m
    }

    pub fn sample_rate(&self) -> f32 {
        self.sr
    }

    /// Post-fader signal of a strip channel from the last `process` call.
    pub fn strip_post(&self, strip: usize, ch: usize) -> &[f32] {
        &self.post_fader[strip][ch]
    }

    /// Clear inputs and tape for the next block.
    pub fn clear_inputs(&mut self, frames: usize) {
        for s in self.inputs.iter_mut() {
            for c in s.iter_mut() {
                c[..frames].fill(0.0);
            }
        }
        for t in self.tape.iter_mut() {
            t[..frames].fill(0.0);
        }
    }

    pub fn process(&mut self, p: &MixParams, frames: usize, meters: &Meters) {
        debug_assert!(frames <= MAX_BLOCK);
        let sr = self.sr;
        let any_solo = p.any_solo();

        for b in self.buses.iter_mut() {
            for c in b.iter_mut() {
                c[..frames].fill(0.0);
            }
        }

        for s in 0..NUM_STRIPS {
            let sp = &p.strips[s];
            let nch = strip_channels(s);
            let dsp = &mut self.strips[s];
            let input = &mut self.inputs[s];

            if is_virtual_strip(s) {
                process_virtual_strip(sr, sp, dsp, input, frames, s == NUM_HW_STRIPS + 1);
            } else {
                process_hw_strip(sr, sp, dsp, input, frames);
            }

            limit(&mut dsp.limiter, sr, sp.limit_db, input, nch, frames);

            for c in 0..nch {
                self.pre_fader[s][c][..frames].copy_from_slice(&input[c][..frames]);
            }

            let audible = !sp.mute && (!any_solo || sp.solo);
            let target = if audible { db_to_lin(clamp_gain(sp.gain_db)) } else { 0.0 };
            ramp_gain(&mut dsp.gain, target, input, nch, frames);

            for c in 0..nch {
                Meters::put(&meters.strips[s][c], &input[c][..frames]);
                self.post_fader[s][c][..frames].copy_from_slice(&input[c][..frames]);
            }

            for (b, routed) in sp.route.iter().enumerate() {
                if !routed {
                    continue;
                }
                let bus = &mut self.buses[b];
                if nch == 2 {
                    // HW strip: 3D panel y sends part of the signal to the rears.
                    let y = if sp.panel_type == PanelType::Position3D { sp.pan_y.clamp(-0.5, 0.5) } else { 0.0 };
                    let rear = (y + 0.5).clamp(0.0, 1.0);
                    let front = 1.0 - rear;
                    if y <= -0.499 {
                        add(&mut bus[0], &input[0], frames, 1.0);
                        add(&mut bus[1], &input[1], frames, 1.0);
                    } else {
                        let f = (front * 2.0).min(1.0);
                        let r = (rear * 2.0 - 1.0).max(0.0);
                        add(&mut bus[0], &input[0], frames, f);
                        add(&mut bus[1], &input[1], frames, f);
                        if r > 0.0 {
                            add(&mut bus[4], &input[0], frames, r);
                            add(&mut bus[5], &input[1], frames, r);
                        }
                    }
                } else {
                    for c in 0..MAX_CH {
                        add(&mut bus[c], &input[c], frames, 1.0);
                    }
                }
            }
        }

        // Tape deck playback.
        let tg = db_to_lin(p.tape.gain_db);
        for (b, routed) in p.tape.route.iter().enumerate() {
            if *routed {
                add(&mut self.buses[b][0], &self.tape[0], frames, tg);
                add(&mut self.buses[b][1], &self.tape[1], frames, tg);
            }
        }

        for b in 0..NUM_BUSES {
            let bp = &p.buses[b];
            if bp.mode == BusMode::Composite {
                for c in 0..MAX_CH {
                    let sel = p.composite.channels[c] as usize;
                    let dst = &mut self.buses[b][c];
                    dst[..frames].fill(0.0);
                    if let Some((s, sc)) = composite_source(sel) {
                        let src = if p.composite.post_fader { &self.post_fader[s][sc] } else { &self.pre_fader[s][sc] };
                        dst[..frames].copy_from_slice(&src[..frames]);
                    }
                }
            }
            process_bus(sr, bp, &mut self.busdsp[b], &mut self.buses[b], frames);
            for c in 0..MAX_CH {
                Meters::put(&meters.buses[b][c], &self.buses[b][c][..frames]);
            }
        }

        for s in 0..NUM_HW_STRIPS {
            meters.strip_gate[s].store(self.strips[s].gate.current_gain().to_bits(), Ordering::Relaxed);
        }
    }
}

/// Composite input index (1-based, 0 = none) → (strip, channel).
pub fn composite_source(sel: usize) -> Option<(usize, usize)> {
    if sel == 0 {
        return None;
    }
    let i = sel - 1;
    if i < 2 * NUM_HW_STRIPS {
        Some((i / 2, i % 2))
    } else {
        let v = i - 2 * NUM_HW_STRIPS;
        let s = NUM_HW_STRIPS + v / MAX_CH;
        (s < NUM_STRIPS).then_some((s, v % MAX_CH))
    }
}

pub fn composite_label(sel: usize) -> String {
    match composite_source(sel) {
        None => "- none -".into(),
        Some((s, c)) if s < NUM_HW_STRIPS => format!("IN#{} {}", s + 1, if c == 0 { "Left" } else { "Right" }),
        Some((s, c)) => {
            const N: [&str; 8] = ["Left", "Right", "Center", "LFE", "SL", "SR", "BL", "BR"];
            format!("IN#{} {}", s + 1, N[c])
        }
    }
}

#[inline]
fn add(dst: &mut [f32], src: &[f32], frames: usize, g: f32) {
    for (d, s) in dst[..frames].iter_mut().zip(&src[..frames]) {
        *d += *s * g;
    }
}

fn ramp_gain(state: &mut f32, target: f32, ch: &mut Planar, nch: usize, frames: usize) {
    let start = *state;
    let step = (target - start) / frames.max(1) as f32;
    for c in ch.iter_mut().take(nch) {
        let mut g = start;
        for x in c[..frames].iter_mut() {
            g += step;
            *x *= g;
        }
    }
    *state = target;
}

fn process_hw_strip(sr: f32, sp: &StripParams, dsp: &mut StripDsp, input: &mut Planar, frames: usize) {
    let (l, rest) = input.split_at_mut(1);
    let (l, r) = (&mut l[0], &mut rest[0]);
    // Noise removal first, so gate and compressor see the cleaned voice.
    match (&mut dsp.denoise, sp.denoise_on) {
        (Some(d), true) => {
            if !dsp.denoise_active {
                d.reset();
                dsp.denoise_active = true;
            }
            d.process(l, r, frames, sp.denoise);
        }
        _ => dsp.denoise_active = false,
    }
    if sp.gate_on {
        dsp.gate.process(sr, &sp.gate, &mut [&mut l[..], &mut r[..]], frames);
    }
    if sp.comp_on {
        dsp.comp.process(sr, &sp.comp, &mut [&mut l[..], &mut r[..]], frames);
    }
    match sp.panel_type {
        PanelType::Color => {
            dsp.color.update(sr, sp.color_x, sp.color_y);
            dsp.color.process(l, r, frames);
        }
        PanelType::Modulation => {
            dsp.chorus.process(sr, sp.fx_x, sp.fx_y, l, r, frames);
        }
        PanelType::Position3D => {
            let (gl, gr) = pan_gains(sp.pan_x);
            if gl != 1.0 || gr != 1.0 {
                for i in 0..frames {
                    l[i] *= gl;
                    r[i] *= gr;
                }
            }
        }
    }
    if sp.mono {
        for i in 0..frames {
            let m = 0.5 * (l[i] + r[i]);
            l[i] = m;
            r[i] = m;
        }
    }
}

fn process_virtual_strip(sr: f32, sp: &StripParams, dsp: &mut StripDsp, input: &mut Planar, frames: usize, has_karaoke: bool) {
    // 3-band EQ (bass / medium / treble), recomputed on change.
    if dsp.eq_last != sp.eq_gain {
        dsp.eq_last = sp.eq_gain;
        let g = sp.eq_gain.map(|v| v.clamp(-12.0, 12.0));
        let cs = [
            if g[0] == 0.0 { Coefs::IDENTITY } else { Coefs::low_shelf(sr, 120.0, 0.7, g[0]) },
            if g[1] == 0.0 { Coefs::IDENTITY } else { Coefs::peak(sr, 1000.0, 0.7, g[1]) },
            if g[2] == 0.0 { Coefs::IDENTITY } else { Coefs::high_shelf(sr, 7000.0, 0.7, g[2]) },
        ];
        for ch in dsp.eq.iter_mut() {
            for (b, c) in ch.iter_mut().zip(cs) {
                b.c = c;
            }
        }
    }
    if sp.eq_gain != [0.0; 3] {
        for (c, ch) in input.iter_mut().enumerate() {
            for x in ch[..frames].iter_mut() {
                let mut v = *x;
                for b in dsp.eq[c].iter_mut() {
                    v = b.process(v);
                }
                *x = v;
            }
        }
    }

    if sp.mute_center && !has_karaoke {
        input[2][..frames].fill(0.0);
    }
    if has_karaoke && sp.karaoke > 0 {
        karaoke(sp.karaoke, dsp, input, frames);
    }

    // Pan: x = L/R balance on every pair, y = front/rear balance.
    let (gl, gr) = pan_gains(sp.pan_x);
    let y = sp.pan_y.clamp(-0.5, 0.5);
    let front = (1.0 - 2.0 * y).min(1.0);
    let rear = (1.0 + 2.0 * y).min(1.0);
    if gl != 1.0 || gr != 1.0 || y != 0.0 {
        for (c, ch) in input.iter_mut().enumerate() {
            let side = match c {
                2 | 3 => 1.0,
                _ if c % 2 == 0 => gl,
                _ => gr,
            };
            let depth = if c >= 4 { rear } else { front };
            let g = side * depth;
            for x in ch[..frames].iter_mut() {
                *x *= g;
            }
        }
    }

    if sp.mono {
        let (l, rest) = input.split_at_mut(1);
        for i in 0..frames {
            let m = 0.5 * (l[0][i] + rest[0][i]);
            l[0][i] = m;
            rest[0][i] = m;
        }
    }
}

/// Karaoke voice removal; the four algorithms are our own variants [U original].
fn karaoke(mode: u8, dsp: &mut StripDsp, input: &mut Planar, frames: usize) {
    let (l, rest) = input.split_at_mut(1);
    let (l, r) = (&mut l[0], &mut rest[0]);
    for i in 0..frames {
        let (a, b) = (l[i], r[i]);
        let mid = 0.5 * (a + b);
        let (na, nb) = match mode {
            1 => (a - b, b - a),
            2 => {
                let v = dsp.karaoke_bp[0].process(mid);
                (a - v, b - v)
            }
            3 => {
                let bass = dsp.karaoke_lp[0].process(mid);
                (a - b + bass, b - a + bass)
            }
            _ => {
                // Stronger band-limited removal.
                let v = dsp.karaoke_bp[1].process(mid) * 1.4;
                (a - v, b - v)
            }
        };
        l[i] = na;
        r[i] = nb;
    }
    input[2][..frames].fill(0.0);
}

fn process_bus(sr: f32, bp: &BusParams, dsp: &mut BusDsp, ch: &mut Planar, frames: usize) {
    busmode::apply(bp.mode, ch, frames);

    if bp.eq_on {
        for c in 0..MAX_CH {
            if dsp.eq_last[c] != bp.eq[c] {
                dsp.eq_last[c] = bp.eq[c];
                for k in 0..EQ_CELLS {
                    dsp.eq[c][k].c = Coefs::from_cell(sr, &bp.eq[c].cells[k]);
                }
            }
            let trim = db_to_lin(bp.eq[c].trim_db);
            let delay = bp.eq[c].delay_ms * 0.001 * sr;
            let any_cell = bp.eq[c].cells.iter().any(|x| x.on);
            for x in ch[c][..frames].iter_mut() {
                let mut v = *x;
                if any_cell {
                    for b in dsp.eq[c].iter_mut() {
                        v = b.process(v);
                    }
                }
                v *= trim;
                if delay > 0.0 {
                    dsp.delay[c].push(v);
                    v = dsp.delay[c].read(delay);
                }
                *x = v;
            }
        }
    }

    match bp.mono {
        1 => {
            let (l, rest) = ch.split_at_mut(1);
            for i in 0..frames {
                let m = 0.5 * (l[0][i] + rest[0][i]);
                l[0][i] = m;
                rest[0][i] = m;
            }
        }
        2 => {
            let (l, rest) = ch.split_at_mut(1);
            l[0][..frames].swap_with_slice(&mut rest[0][..frames]);
        }
        _ => {}
    }

    let target = if bp.mute { 0.0 } else { db_to_lin(clamp_gain(bp.gain_db)) };
    ramp_gain(&mut dsp.gain, target, ch, MAX_CH, frames);
    limit(&mut dsp.limiter, sr, bp.limit_db, ch, MAX_CH, frames);
}

/// Linked limiter over the first `nch` channels (2 or 8) without allocating.
fn limit(l: &mut Limiter, sr: f32, db: f32, ch: &mut Planar, nch: usize, frames: usize) {
    let [c0, c1, c2, c3, c4, c5, c6, c7] = ch;
    if nch == 2 {
        l.process(sr, db, &mut [&mut c0[..], &mut c1[..]], frames);
    } else {
        l.process(
            sr,
            db,
            &mut [&mut c0[..], &mut c1[..], &mut c2[..], &mut c3[..], &mut c4[..], &mut c5[..], &mut c6[..], &mut c7[..]],
            frames,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(p: &MixParams, fill: impl Fn(&mut Mixer)) -> Mixer {
        let mut m = Mixer::new(48000.0);
        let meters = Meters::default();
        // Two blocks so gain ramps settle.
        for _ in 0..2 {
            m.clear_inputs(256);
            fill(&mut m);
            m.process(p, 256, &meters);
        }
        m
    }

    #[test]
    fn default_routes_strip_to_a1_and_b1() {
        let p = MixParams::default();
        let m = run(&p, |m| m.inputs[0][0][..256].fill(0.5));
        assert!((m.buses[0][0][255] - 0.5).abs() < 1e-4);
        assert!((m.buses[3][0][255] - 0.5).abs() < 1e-4);
        assert_eq!(m.buses[1][0][255], 0.0);
    }

    #[test]
    fn mute_and_solo() {
        let mut p = MixParams::default();
        p.strips[0].mute = true;
        let m = run(&p, |m| m.inputs[0][0][..256].fill(0.5));
        assert_eq!(m.buses[0][0][255], 0.0);

        let mut p = MixParams::default();
        p.strips[1].solo = true;
        let m = run(&p, |m| {
            m.inputs[0][0][..256].fill(0.5);
            m.inputs[1][0][..256].fill(0.25);
        });
        assert!((m.buses[0][0][255] - 0.25).abs() < 1e-4);
    }

    #[test]
    fn fader_applies_gain() {
        let mut p = MixParams::default();
        p.strips[0].gain_db = -6.0;
        p.buses[0].gain_db = -6.0;
        let m = run(&p, |m| m.inputs[0][0][..256].fill(1.0));
        assert!((m.buses[0][0][255] - db_to_lin(-12.0)).abs() < 1e-3);
    }

    #[test]
    fn virtual_strip_keeps_8_channels() {
        let p = MixParams::default();
        let m = run(&p, |m| m.inputs[3][6][..256].fill(0.3));
        assert!((m.buses[0][6][255] - 0.3).abs() < 1e-4);
    }

    #[test]
    fn mute_center_on_virtual_1() {
        let mut p = MixParams::default();
        p.strips[3].mute_center = true;
        let m = run(&p, |m| m.inputs[3][2][..256].fill(0.3));
        assert_eq!(m.buses[0][2][255], 0.0);
    }

    #[test]
    fn composite_picks_input_channel() {
        let mut p = MixParams::default();
        p.buses[1].mode = BusMode::Composite;
        p.composite.channels[0] = 2; // IN#1 Right -> bus ch1
        let m = run(&p, |m| m.inputs[0][1][..256].fill(0.4));
        assert!((m.buses[1][0][255] - 0.4).abs() < 1e-4);
    }

    #[test]
    fn tape_routes_to_selected_bus() {
        let mut p = MixParams::default();
        p.tape.route = [false, false, false, false, true, false];
        let m = run(&p, |m| m.tape[0][..256].fill(0.2));
        assert!((m.buses[4][0][255] - 0.2).abs() < 1e-4);
        assert_eq!(m.buses[0][0][255], 0.0);
    }

    #[test]
    fn bus_mono_swap() {
        let mut p = MixParams::default();
        p.buses[0].mono = 2;
        let m = run(&p, |m| m.inputs[0][0][..256].fill(0.6));
        assert_eq!(m.buses[0][0][255], 0.0);
        assert!((m.buses[0][1][255] - 0.6).abs() < 1e-4);
    }

    #[test]
    fn composite_source_mapping() {
        assert_eq!(composite_source(0), None);
        assert_eq!(composite_source(1), Some((0, 0)));
        assert_eq!(composite_source(6), Some((2, 1)));
        assert_eq!(composite_source(7), Some((3, 0)));
        assert_eq!(composite_source(22), Some((4, 7)));
        assert_eq!(composite_source(30), Some((5, 7)));
        assert_eq!(composite_source(31), None);
    }
}

#[cfg(test)]
mod virtual3_tests {
    use super::*;

    #[test]
    fn third_virtual_strip_routes_8_channels() {
        let mut p = MixParams::default();
        p.strips[5].route = [false, false, false, false, false, true];
        let mut m = Mixer::new(48000.0);
        let meters = Meters::default();
        for _ in 0..2 {
            m.clear_inputs(256);
            m.inputs[5][7][..256].fill(0.3);
            m.process(&p, 256, &meters);
        }
        assert!((m.buses[5][7][255] - 0.3).abs() < 1e-4);
        assert_eq!(m.buses[0][7][255], 0.0);
    }
}

#[cfg(test)]
mod golden_tests {
    use super::*;
    use crate::engine::golden::{level, params, Fnv, Lcg};

    // Characterization golden: must not change during refactor Phases 2-5.
    const GOLDEN_MIXER_HASH: u64 = 0x1ba5_067f_9cb1_c152;

    /// 64 blocks × 256 frames @48 kHz of seeded input through every strip and the tape;
    /// hashes all bus channels of every block.
    #[test]
    fn golden_mixer_output() {
        let p = params();
        let mut m = Mixer::new(48000.0);
        let meters = Meters::default();
        let mut rng = Lcg::new(0x5eed_0001);
        let mut h = Fnv::new();
        let mut peak = [0.0f32; NUM_BUSES];
        const FRAMES: usize = 256;
        for b in 0..64 {
            let a = level(b);
            m.clear_inputs(FRAMES);
            for s in 0..NUM_STRIPS {
                for c in 0..strip_channels(s) {
                    for x in m.inputs[s][c][..FRAMES].iter_mut() {
                        *x = a * rng.next();
                    }
                }
            }
            for t in m.tape.iter_mut() {
                for x in t[..FRAMES].iter_mut() {
                    *x = a * rng.next();
                }
            }
            m.process(&p, FRAMES, &meters);
            for (bus, pk) in m.buses.iter().zip(peak.iter_mut()) {
                for c in bus.iter() {
                    h.samples(&c[..FRAMES]);
                    *pk = c[..FRAMES].iter().fold(*pk, |a, x| a.max(x.abs()));
                }
            }
        }
        assert!(peak.iter().all(|&x| x > 1e-3), "every bus carries signal: {peak:?}");
        assert_eq!(h.finish(), GOLDEN_MIXER_HASH, "got {:#018x}", h.finish());
    }
}
