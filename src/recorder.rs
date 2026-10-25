//! Tape deck: recorder (armed buses / inputs → WAV, optional multitrack) and player
//! (WAV → A1..B2). ANALYSIS §6 (recorder XML, file naming) and help TAPE_*.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;

use parking_lot::Mutex;

use crate::engine::mixer::Mixer;
use crate::engine::ring::{ring, Consumer, Producer, StreamResampler};
use crate::params::{MAX_CH, NUM_BUSES, NUM_STRIPS};
use crate::settings::{RecorderConfig, FILE_BWF, FILE_WAV};

pub const SEEK_SECONDS: f64 = 5.0;

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
    file: Mutex<Option<Arc<Vec<f32>>>>,
    file_gen: AtomicU32,
    rec_prod: Mutex<Option<Producer>>,
    rec_gen: AtomicU32,
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

/// Recording file name per the original pattern ("Record %04i-%02i-%02i at %02ih%02im%02is").
pub fn record_file_name(prefix: &str, now: chrono::NaiveDateTime, ext: &str) -> String {
    let stamp = now.format("%Y-%m-%d at %Hh%Mm%Ss");
    if prefix.trim().is_empty() {
        format!("Record {stamp}.{ext}")
    } else {
        format!("{} {stamp}.{ext}", prefix.trim())
    }
}

pub struct RecorderJob {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<Result<Vec<PathBuf>, String>>>,
}

impl RecorderJob {
    /// Start recording. Spawns the writer thread and hands a ring producer to the audio thread.
    pub fn start(shared: &Arc<TapeShared>, cfg: &RecorderConfig, engine_sr: u32) -> Result<RecorderJob, String> {
        if cfg.file_type != FILE_WAV && cfg.file_type != FILE_BWF {
            return Err("Only WAV/BWF recording is implemented (MP3/AIFF are not).".into());
        }
        let dir = PathBuf::from(if cfg.directory.trim().is_empty() { r"C:\VoicemeeterRecord" } else { cfg.directory.trim() });
        std::fs::create_dir_all(&dir).map_err(|e| format!("Cannot create {}: {e}", dir.display()))?;
        let ch = cfg.channels.clamp(1, MAX_CH as i32) as usize;
        let out_sr = if cfg.target_sr > 0 { cfg.target_sr as u32 } else { engine_sr };
        let bits = match cfg.bit_res {
            16 => 16,
            32 => 32,
            _ => 24,
        };
        let name = record_file_name(&cfg.prefix, chrono::Local::now().naive_local(), "wav");
        let base = dir.join(&name);
        let multitrack = cfg.multitrack;
        let timer_stop_frames = (cfg.timer_stop.max(0) as u64) * engine_sr as u64;

        let (prod, cons) = ring(engine_sr as usize * ch * 4);
        shared.rec_channels.store(ch as u32, Ordering::Relaxed);
        shared.set_arm(cfg);
        shared.rec_frames.store(0, Ordering::Relaxed);
        *shared.rec_prod.lock() = Some(prod);
        shared.rec_gen.fetch_add(1, Ordering::Release);
        shared.rec_paused.store(false, Ordering::Relaxed);
        shared.recording.store(true, Ordering::Relaxed);
        *shared.status.lock() = format!("Recording... {}", base.display());

        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let shared2 = shared.clone();
        let handle = std::thread::spawn(move || {
            let r = writer(cons, &base, ch, engine_sr, out_sr, bits, multitrack, timer_stop_frames, &shared2, &stop2);
            shared2.recording.store(false, Ordering::Relaxed);
            r
        });
        Ok(RecorderJob { stop, handle: Some(handle) })
    }

    pub fn is_finished(&self) -> bool {
        self.handle.as_ref().map(|h| h.is_finished()).unwrap_or(true)
    }

    pub fn finish(mut self) -> Result<Vec<PathBuf>, String> {
        self.stop.store(true, Ordering::Relaxed);
        match self.handle.take() {
            Some(h) => h.join().map_err(|_| "recorder thread panicked".to_string())?,
            None => Ok(vec![]),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn writer(
    mut cons: Consumer,
    base: &Path,
    ch: usize,
    in_sr: u32,
    out_sr: u32,
    bits: u16,
    multitrack: bool,
    timer_stop_frames: u64,
    shared: &TapeShared,
    stop: &AtomicBool,
) -> Result<Vec<PathBuf>, String> {
    let fmt = if bits == 32 { hound::SampleFormat::Float } else { hound::SampleFormat::Int };
    let paths: Vec<PathBuf> = if multitrack {
        (1..=ch)
            .map(|t| {
                let stem = base.file_stem().unwrap_or_default().to_string_lossy();
                base.with_file_name(format!("{stem}_Track{t}.wav"))
            })
            .collect()
    } else {
        vec![base.to_path_buf()]
    };
    let file_ch = if multitrack { 1 } else { ch as u16 };
    let spec = hound::WavSpec { channels: file_ch, sample_rate: out_sr, bits_per_sample: bits, sample_format: fmt };
    let mut writers = paths
        .iter()
        .map(|p| hound::WavWriter::create(p, spec).map_err(|e| format!("Failed to Create Audio File: {e}")))
        .collect::<Result<Vec<_>, _>>()?;
    let mut rs = StreamResampler::new(ch, in_sr as f64 / out_sr as f64);
    let mut buf = vec![0.0f32; 16384 - 16384 % ch];
    let mut out = Vec::with_capacity(32768);
    let scale = ((1i64 << (bits.min(31) - 1)) - 1) as f32;
    loop {
        let n = cons.pop(&mut buf);
        let n = n - n % ch;
        if n == 0 {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            if timer_stop_frames > 0 && shared.rec_frames.load(Ordering::Relaxed) >= timer_stop_frames {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
            continue;
        }
        out.clear();
        rs.process(&buf[..n], &mut out);
        for frame in out.chunks_exact(ch) {
            for (c, x) in frame.iter().enumerate() {
                let w = if multitrack { &mut writers[c] } else { &mut writers[0] };
                let r = if bits == 32 { w.write_sample(*x) } else { w.write_sample((x.clamp(-1.0, 1.0) * scale) as i32) };
                r.map_err(|e| format!("Error: write failure ({e})"))?;
            }
        }
    }
    for w in writers {
        w.finalize().map_err(|e| e.to_string())?;
    }
    Ok(paths)
}

/// Decode a WAV file to stereo f32 at `engine_sr` (runs off the audio thread).
pub fn load_audio_file(path: &Path, engine_sr: u32) -> Result<Vec<f32>, String> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    if !matches!(ext.as_str(), "wav" | "bwf") {
        return Err(format!("'.{ext}' playback is not implemented (WAV/BWF only)."));
    }
    let mut r = hound::WavReader::open(path).map_err(|e| format!("Error: Cannot open File ({e})"))?;
    let spec = r.spec();
    let ch = spec.channels.max(1) as usize;
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => r.samples::<f32>().map(|s| s.unwrap_or(0.0)).collect(),
        hound::SampleFormat::Int => {
            let s = 1.0 / (1i64 << (spec.bits_per_sample - 1)) as f32;
            r.samples::<i32>().map(|v| v.unwrap_or(0) as f32 * s).collect()
        }
    };
    let mut stereo = Vec::with_capacity(samples.len() / ch * 2 + 2);
    for f in samples.chunks_exact(ch) {
        stereo.push(f[0]);
        stereo.push(if ch > 1 { f[1] } else { f[0] });
    }
    let mut out = Vec::with_capacity(stereo.len());
    let mut rs = StreamResampler::new(2, spec.sample_rate as f64 / engine_sr as f64);
    rs.process(&stereo, &mut out);
    Ok(out)
}

pub fn set_loaded_file(shared: &TapeShared, name: &str, data: Vec<f32>, play: bool) {
    shared.playing.store(false, Ordering::Relaxed);
    shared.len.store((data.len() / 2) as u64, Ordering::Relaxed);
    shared.pos.store(0, Ordering::Relaxed);
    *shared.file.lock() = Some(Arc::new(data));
    shared.file_gen.fetch_add(1, Ordering::Release);
    *shared.file_name.lock() = name.to_string();
    if play {
        shared.playing.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::mixer::{Meters, Mixer};
    use crate::params::MixParams;

    #[test]
    fn file_name_pattern() {
        let t = chrono::NaiveDate::from_ymd_opt(2026, 10, 16).unwrap().and_hms_opt(9, 5, 7).unwrap();
        assert_eq!(record_file_name("", t, "wav"), "Record 2026-10-16 at 09h05m07s.wav");
        assert_eq!(record_file_name("Show", t, "wav"), "Show 2026-10-16 at 09h05m07s.wav");
    }

    #[test]
    fn record_then_play_back() {
        let dir = std::env::temp_dir().join(format!("rustbus-test-{}", std::process::id()));
        let shared = Arc::new(TapeShared::default());
        let cfg = RecorderConfig { directory: dir.to_string_lossy().into(), bit_res: 32, ..Default::default() };
        let job = RecorderJob::start(&shared, &cfg, 48000).unwrap();

        let mut core = TapeCore::new(shared.clone());
        let mut mixer = Mixer::new(48000.0);
        let meters = Meters::default();
        let p = MixParams::default();
        for _ in 0..20 {
            mixer.clear_inputs(480);
            mixer.inputs[0][0][..480].fill(0.25);
            mixer.process(&p, 480, &meters);
            core.capture(&mixer, 480);
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
        let files = job.finish().unwrap();
        assert_eq!(files.len(), 1);

        let data = load_audio_file(&files[0], 48000).unwrap();
        assert_eq!(data.len(), 20 * 480 * 2);
        assert!((data[data.len() - 2] - 0.25).abs() < 1e-4);

        set_loaded_file(&shared, "x", data, true);
        mixer.clear_inputs(480);
        core.playback(&mut mixer, 480);
        assert!((mixer.tape[0][479] - 0.25).abs() < 1e-4);
        let _ = std::fs::remove_dir_all(dir);
    }
}
