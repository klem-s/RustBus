//! Tape deck: recorder (armed buses / inputs → WAV, optional multitrack) and player
//! (WAV → A1..B2). ANALYSIS §6 (recorder XML, file naming) and help TAPE_*.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;

use crate::ring::{ring, Consumer, StreamResampler};
use crate::params::MAX_CH;
use crate::settings::{RecorderConfig, FILE_BWF, FILE_WAV};

pub use crate::engine::tape::{TapeCore, TapeShared};

pub const SEEK_SECONDS: f64 = 5.0;

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
