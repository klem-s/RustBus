//! Tape deck core: state shared with the GUI and writer thread, and the audio-thread
//! side that feeds playback into the mixer and captures armed sources for the recorder.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;

use super::mixer::Mixer;
use crate::ring::Producer;
use crate::params::{MAX_CH, NUM_BUSES, NUM_STRIPS};
use crate::settings::RecorderConfig;

/// State shared between GUI, audio thread and writer thread.
pub struct TapeShared {
    pub playing: AtomicBool,
    pub recording: AtomicBool,
    pub rec_paused: AtomicBool,
    pub pos: AtomicU64,
    pub len: AtomicU64,
    pub loop_play: AtomicBool,
    pub rec_frames: AtomicU64,
    pub engine_sr: AtomicU32,
    /// Stereo interleaved, engine sample rate.
    pub(crate) file: Mutex<Option<Arc<Vec<f32>>>>,
    pub(crate) file_gen: AtomicU32,
    pub(crate) rec_prod: Mutex<Option<Producer>>,
    pub(crate) rec_gen: AtomicU32,
    pub rec_channels: AtomicU32,
    /// Bitmask: bits 0..4 armed strips, bits 8..12 armed buses.
    pub arm: AtomicU32,
    pub file_name: Mutex<String>,
    pub status: Mutex<String>,
}

impl Default for TapeShared {
    fn default() -> Self {
        TapeShared {
            playing: AtomicBool::new(false),
            recording: AtomicBool::new(false),
            rec_paused: AtomicBool::new(false),
            pos: AtomicU64::new(0),
            len: AtomicU64::new(0),
            loop_play: AtomicBool::new(false),
            rec_frames: AtomicU64::new(0),
            engine_sr: AtomicU32::new(48000),
            file: Mutex::new(None),
            file_gen: AtomicU32::new(0),
            rec_prod: Mutex::new(None),
            rec_gen: AtomicU32::new(0),
            rec_channels: AtomicU32::new(2),
            arm: AtomicU32::new(1 << 8),
            file_name: Mutex::new(String::new()),
            status: Mutex::new(String::new()),
        }
    }
}

impl TapeShared {
    pub fn set_arm(&self, cfg: &RecorderConfig) {
        let mut m = 0u32;
        for (i, a) in cfg.arm_inputs.iter().enumerate() {
            m |= (*a as u32) << i;
        }
        for (i, a) in cfg.arm_buses.iter().enumerate() {
            m |= (*a as u32) << (8 + i);
        }
        if m == 0 {
            m = 1 << (8 + cfg.rec_bus.clamp(0, NUM_BUSES as i32 - 1));
        }
        self.arm.store(m, Ordering::Relaxed);
    }

    pub fn seconds(&self, frames: u64) -> f64 {
        frames as f64 / self.engine_sr.load(Ordering::Relaxed).max(1) as f64
    }

    pub fn has_file(&self) -> bool {
        self.len.load(Ordering::Relaxed) > 0
    }

    pub fn eject(&self) {
        self.playing.store(false, Ordering::Relaxed);
        *self.file.lock() = None;
        self.file_gen.fetch_add(1, Ordering::Release);
        self.len.store(0, Ordering::Relaxed);
        self.pos.store(0, Ordering::Relaxed);
        self.file_name.lock().clear();
    }

    pub fn seek_by(&self, seconds: f64) {
        let sr = self.engine_sr.load(Ordering::Relaxed) as f64;
        let len = self.len.load(Ordering::Relaxed) as i64;
        let pos = self.pos.load(Ordering::Relaxed) as i64 + (seconds * sr) as i64;
        self.pos.store(pos.clamp(0, len.max(0)) as u64, Ordering::Relaxed);
    }
}

/// Audio-thread side of the tape deck.
pub struct TapeCore {
    shared: Arc<TapeShared>,
    file: Option<Arc<Vec<f32>>>,
    file_gen: u32,
    rec: Option<Producer>,
    rec_gen: u32,
    scratch: Vec<f32>,
}

impl TapeCore {
    pub fn new(shared: Arc<TapeShared>) -> Self {
        TapeCore { shared, file: None, file_gen: u32::MAX, rec: None, rec_gen: u32::MAX, scratch: vec![0.0; 2048 * MAX_CH] }
    }

    /// Fill the mixer's tape input with playback for this block.
    pub fn playback(&mut self, mixer: &mut Mixer, frames: usize) {
        let g = self.shared.file_gen.load(Ordering::Acquire);
        if g != self.file_gen {
            if let Some(f) = self.shared.file.try_lock() {
                self.file = f.clone();
                self.file_gen = g;
            }
        }
        if !self.shared.playing.load(Ordering::Relaxed) {
            return;
        }
        let Some(data) = &self.file else { return };
        let len = data.len() / 2;
        let mut pos = self.shared.pos.load(Ordering::Relaxed) as usize;
        for i in 0..frames {
            if pos >= len {
                if self.shared.loop_play.load(Ordering::Relaxed) && len > 0 {
                    pos = 0;
                } else {
                    self.shared.playing.store(false, Ordering::Relaxed);
                    break;
                }
            }
            mixer.tape[0][i] = data[pos * 2];
            mixer.tape[1][i] = data[pos * 2 + 1];
            pos += 1;
        }
        self.shared.pos.store(pos.min(len) as u64, Ordering::Relaxed);
    }

    /// After mixing: push the armed sources to the recorder ring.
    pub fn capture(&mut self, mixer: &Mixer, frames: usize) {
        let g = self.shared.rec_gen.load(Ordering::Acquire);
        if g != self.rec_gen {
            if let Some(mut p) = self.shared.rec_prod.try_lock() {
                self.rec = p.take();
                self.rec_gen = g;
            }
        }
        let Some(prod) = self.rec.as_mut() else { return };
        if !self.shared.recording.load(Ordering::Relaxed) || self.shared.rec_paused.load(Ordering::Relaxed) {
            return;
        }
        let ch = (self.shared.rec_channels.load(Ordering::Relaxed) as usize).clamp(1, MAX_CH);
        let arm = self.shared.arm.load(Ordering::Relaxed);
        let out = &mut self.scratch[..frames * ch];
        out.fill(0.0);
        for b in 0..NUM_BUSES {
            if arm & (1 << (8 + b)) != 0 {
                for c in 0..ch {
                    let src = &mixer.buses[b][c];
                    for i in 0..frames {
                        out[i * ch + c] += src[i];
                    }
                }
            }
        }
        for s in 0..NUM_STRIPS {
            if arm & (1 << s) != 0 {
                let nch = crate::params::strip_channels(s);
                for c in 0..ch.min(nch) {
                    let src = mixer.strip_post(s, c);
                    for i in 0..frames {
                        out[i * ch + c] += src[i];
                    }
                }
            }
        }
        prod.push(out);
        self.shared.rec_frames.fetch_add(frames as u64, Ordering::Relaxed);
    }
}
