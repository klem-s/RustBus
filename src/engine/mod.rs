//! Audio engine: device I/O (WASAPI via cpal, ASIO on A1), clocking, VBAN and tape glue.
//!
//! Clocking mirrors the original (ANALYSIS §2): output A1 is the master clock and sets
//! the engine sample rate; every other audio point is resampled against it. With no A1
//! device the engine runs from an internal clock ("SYNC: Internal Master CLOCK").

pub mod asio;
pub mod mixer;
pub mod ring;
#[cfg(test)]
mod golden;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SizedSample};
use parking_lot::Mutex;

use crate::params::*;
use crate::recorder::{TapeCore, TapeShared};
use crate::settings::{Settings, DEV_ASIO};
use crate::vban::{InSink, OutSource, VbanRuntime, VbanStats};
use mixer::{Meters, Mixer, MAX_BLOCK};
use ring::{ring, AdaptiveReader, Producer};

/// Sample rates accepted for A1 (decompiled `FUN_00537d90`).
pub const A1_RATES: [u32; 7] = [32000, 44100, 48000, 88200, 96000, 176400, 192000];
pub const INTERNAL_SR: u32 = 48000;

#[derive(Clone, Debug, Default)]
pub struct EngineStatus {
    pub running: bool,
    pub sample_rate: u32,
    pub master: String,
    pub errors: Vec<String>,
    /// Per audio point: inputs 0..4, outputs 5..9 → Some(device description) when open.
    pub points: [Option<String>; NUM_STRIPS + NUM_BUSES],
    pub underruns: u32,
    /// The ASIO driver running A1, with its channel names.
    pub asio: Option<asio::AsioInfo>,
}

pub struct Shared {
    pub params: Arc<Mutex<MixParams>>,
    pub meters: Meters,
    pub tape: Arc<TapeShared>,
    pub status: Mutex<EngineStatus>,
    pub device_error: AtomicBool,
    /// Set when a driver asks to be reopened (ASIO reset request); the GUI restarts the engine.
    pub restart_request: AtomicBool,
    pub vban_stats: Arc<VbanStats>,
}

impl Shared {
    pub fn new(params: MixParams) -> Arc<Shared> {
        Arc::new(Shared {
            params: Arc::new(Mutex::new(params)),
            meters: Meters::default(),
            tape: Arc::new(TapeShared::default()),
            status: Mutex::new(EngineStatus::default()),
            device_error: AtomicBool::new(false),
            restart_request: AtomicBool::new(false),
            vban_stats: Arc::new(VbanStats::default()),
        })
    }
}

pub fn list_devices() -> (Vec<String>, Vec<String>) {
    let host = cpal::default_host();
    let names = |it: Option<Vec<String>>| {
        let mut v = it.unwrap_or_default();
        // Natural-ish order: "Line 2" before "Line 10", case-insensitive.
        v.sort_by_key(|n| sort_key(n));
        v
    };
    let ins = names(host.input_devices().ok().map(|d| d.filter_map(|x| x.name().ok()).collect()));
    let outs = names(host.output_devices().ok().map(|d| d.filter_map(|x| x.name().ok()).collect()));
    (ins, outs)
}

/// Lowercase with every digit run zero-padded, so numbers sort by value.
fn sort_key(name: &str) -> String {
    let mut out = String::new();
    let mut digits = String::new();
    for ch in name.chars().chain(Some('\0')) {
        if ch.is_ascii_digit() {
            digits.push(ch);
            continue;
        }
        if !digits.is_empty() {
            out.push_str(&format!("{digits:0>8}"));
            digits.clear();
        }
        if ch != '\0' {
            out.extend(ch.to_lowercase());
        }
    }
    out
}

/// Installed ASIO drivers (usable on A1 only).
pub fn list_asio() -> Vec<String> {
    asio::list_drivers()
}

fn find_device(input: bool, name: &str) -> Option<cpal::Device> {
    let host = cpal::default_host();
    let mut it: Box<dyn Iterator<Item = cpal::Device>> =
        if input { Box::new(host.input_devices().ok()?) } else { Box::new(host.output_devices().ok()?) };
    it.find(|d| d.name().map(|n| n == name).unwrap_or(false))
}

/// Everything the mixing clock needs; moved into the A1 callback or the clock thread.
struct Core {
    mixer: Mixer,
    shared: Arc<Shared>,
    local: MixParams,
    inputs: Vec<(usize, AdaptiveReader)>,
    outputs: Vec<(usize, usize, Producer)>,
    vban_in: Vec<(usize, usize, AdaptiveReader)>,
    vban_out: Vec<(usize, usize, Producer)>,
    tape: TapeCore,
    tmp: Vec<f32>,
    sr: u32,
    /// Strips fed straight from ASIO input channels: (strip, [left, right] channel).
    direct: Vec<(usize, [usize; 2])>,
    /// One buffer per ASIO input channel, filled by the ASIO callback; read from `direct_off`.
    direct_buf: Vec<Vec<f32>>,
    direct_off: usize,
}

impl Core {
    /// Produce `frames` (≤ MAX_BLOCK) of output; results stay in `self.mixer.buses`.
    fn block(&mut self, frames: usize) {
        if let Some(p) = self.shared.params.try_lock() {
            self.local = *p;
        }
        let m = &mut self.mixer;
        m.clear_inputs(frames);
        for (strip, src) in &self.direct {
            for (c, &ch) in src.iter().enumerate() {
                let s = &self.direct_buf[ch][self.direct_off..self.direct_off + frames];
                let dst = &mut m.inputs[*strip][c];
                for i in 0..frames {
                    dst[i] = s[i];
                }
            }
        }
        for (strip, rd) in self.inputs.iter_mut() {
            let ch = rd.channels();
            rd.read(&mut self.tmp, frames);
            for c in 0..ch {
                let dst = &mut m.inputs[*strip][c];
                for i in 0..frames {
                    dst[i] = self.tmp[i * ch + c];
                }
            }
        }
        for (stream, strip, rd) in self.vban_in.iter_mut() {
            let sr = self.shared.vban_stats.inputs[*stream].sr.load(Ordering::Relaxed);
            if sr > 0 {
                rd.set_ratio(sr as f64 / self.sr as f64);
            }
            let ch = rd.channels();
            rd.read(&mut self.tmp, frames);
            for c in 0..ch {
                let dst = &mut m.inputs[*strip][c];
                for i in 0..frames {
                    dst[i] += self.tmp[i * ch + c];
                }
            }
        }
        self.tape.playback(m, frames);
        m.process(&self.local, frames, &self.shared.meters);
        self.tape.capture(m, frames);

        for list in [&mut self.outputs, &mut self.vban_out] {
            for (bus, ch, prod) in list.iter_mut() {
                let ch = *ch;
                for i in 0..frames {
                    for c in 0..ch {
                        self.tmp[i * ch + c] = m.buses[*bus][c][i];
                    }
                }
                prod.push(&self.tmp[..frames * ch]);
            }
        }
    }
}

pub struct Engine {
    streams: Vec<cpal::Stream>,
    asio: Option<asio::AsioStream>,
    clock: Option<(Arc<AtomicBool>, JoinHandle<()>)>,
    vban: Option<VbanRuntime>,
}

impl Engine {
    pub fn start(settings: &Settings, shared: &Arc<Shared>, text_tx: Sender<String>, wake: Arc<dyn Fn() + Send + Sync>) -> Engine {
        let mut status = EngineStatus::default();
        let mut streams = Vec::new();
        shared.device_error.store(false, Ordering::Relaxed);

        // ---- master clock: A1 (ASIO driver or WASAPI device)
        let a1_asio = settings.device.outputs[0].as_ref().filter(|d| d.kind == DEV_ASIO).and_then(|d| {
            let want_buffer = settings.device.buffer_asio.clamp(0, MAX_BLOCK as i32) as usize;
            match asio::open(&d.name, settings.device.sr_asio.max(0) as u32, want_buffer) {
                Ok(a) if A1_RATES.contains(&a.info.sample_rate) => Some(a),
                Ok(a) => {
                    status.errors.push(format!(
                        "Failed to Open Main Output Device (A1)\nOutput A1 supports standard SampleRate only ({} Hz).",
                        a.info.sample_rate
                    ));
                    None
                }
                Err(e) => {
                    status.errors.push(format!("Failed to Open Main Output Device (A1)\n{e}"));
                    None
                }
            }
        });
        let a1 = settings.device.outputs[0].as_ref().filter(|d| d.kind != DEV_ASIO).and_then(|d| {
            let dev = find_device(false, &d.name);
            if dev.is_none() {
                status.errors.push(format!("A1 device not found: {}", d.name));
            }
            dev
        });
        let a1_cfg = a1.as_ref().and_then(|d| match d.default_output_config() {
            Ok(c) => Some(c),
            Err(e) => {
                status.errors.push(format!("A1: {e}"));
                None
            }
        });
        let mut sr = match &a1_asio {
            Some(a) => a.info.sample_rate,
            None => a1_cfg.as_ref().map(|c| c.sample_rate().0).unwrap_or(INTERNAL_SR),
        };
        if !A1_RATES.contains(&sr) {
            status.errors.push(format!(
                "Failed to Open Main Output Device (A1)\nOutput A1 supports standard SampleRate only ({sr} Hz)."
            ));
            sr = INTERNAL_SR;
        }
        shared.tape.engine_sr.store(sr, Ordering::Relaxed);
        let latency = (settings.device.buffer_wdm.clamp(128, 2048) as usize) * 3 / 2;

        let mut core = Core {
            mixer: Mixer::new(sr as f32),
            shared: shared.clone(),
            local: *shared.params.lock(),
            inputs: Vec::new(),
            outputs: Vec::new(),
            vban_in: Vec::new(),
            vban_out: Vec::new(),
            tape: TapeCore::new(shared.tape.clone()),
            tmp: vec![0.0; MAX_BLOCK * MAX_CH],
            sr,
            direct: Vec::new(),
            direct_buf: Vec::new(),
            direct_off: 0,
        };

        // ---- inputs (strips)
        for s in 0..NUM_STRIPS {
            let Some(d) = &settings.device.inputs[s] else { continue };
            if d.kind == DEV_ASIO {
                match &a1_asio {
                    Some(a) if a.info.driver == d.name && s < NUM_HW_STRIPS => {
                        let src = settings.device.asio_patch[s].map(|c| c.max(0) as usize);
                        let ins = &a.info.inputs;
                        if src.iter().all(|&c| c < ins.len()) {
                            core.direct.push((s, src));
                            let names = if src[0] == src[1] { ins[src[0]].clone() } else { format!("{} + {}", ins[src[0]], ins[src[1]]) };
                            status.points[s] = Some(format!("ASIO: {} ({names})", d.name));
                        } else {
                            status.errors.push(format!("Failed to Open Input Device: {}\nASIO input channel not available.", d.name));
                        }
                    }
                    _ => status.errors.push(format!("Failed to Open Input Device: {}\nASIO inputs need A1 on the same ASIO driver.", d.name)),
                }
                continue;
            }
            let Some(dev) = find_device(true, &d.name) else {
                status.errors.push(format!("Failed to Open Input Device: {}", d.name));
                continue;
            };
            let strip_ch = strip_channels(s);
            match open_input(&dev, strip_ch, shared.clone()) {
                Ok((stream, cons, dev_sr, dev_ch)) => {
                    core.inputs.push((s, AdaptiveReader::new(cons, strip_ch, dev_sr as f64 / sr as f64, latency * dev_sr as usize / sr as usize)));
                    status.points[s] = Some(format!("WDM: {} ({dev_sr} Hz, {dev_ch} ch)", d.name));
                    streams.push(stream);
                }
                Err(e) => status.errors.push(format!("Failed to Open Input Device: {} ({e})", d.name)),
            }
        }

        // ---- outputs A2..B2
        for b in 1..NUM_BUSES {
            let Some(d) = &settings.device.outputs[b] else { continue };
            let Some(dev) = find_device(false, &d.name) else {
                status.errors.push(format!("Failed to Open Output Device: {}", d.name));
                continue;
            };
            match open_output(&dev, sr, latency, shared.clone()) {
                Ok((stream, prod, dev_sr, dev_ch)) => {
                    core.outputs.push((b, dev_ch.min(MAX_CH), prod));
                    status.points[NUM_STRIPS + b] = Some(format!("WDM: {} ({dev_sr} Hz, {dev_ch} ch)", d.name));
                    streams.push(stream);
                }
                Err(e) => status.errors.push(format!("Failed to Open Output Device: {} ({e})", d.name)),
            }
        }

        // ---- VBAN
        let vcfg = &settings.vban;
        let mut sinks = Vec::new();
        let mut sources = Vec::new();
        if vcfg.enabled {
            for (i, s) in vcfg.inputs.iter().enumerate().filter(|(_, s)| s.enabled) {
                let strip = s.strip.min(NUM_STRIPS - 1);
                let ch = strip_channels(strip);
                let (prod, cons) = ring(sr as usize * ch);
                let target = 256 * (s.nq as usize + 2);
                sinks.push(InSink { stream: i, channels: ch, prod });
                core.vban_in.push((i, strip, AdaptiveReader::new(cons, ch, 1.0, target)));
            }
            for (i, s) in vcfg.outputs.iter().enumerate().filter(|(_, s)| s.enabled) {
                let ch = s.channels.clamp(1, 8) as usize;
                let (prod, cons) = ring(sr as usize * ch);
                core.vban_out.push((s.bus.min(NUM_BUSES - 1), ch, prod));
                sources.push(OutSource { stream: i, cons });
            }
        }
        let vban = VbanRuntime::start(vcfg, sr, sinks, sources, shared.vban_stats.clone(), text_tx, wake.clone());
        if shared.vban_stats.port_error.load(Ordering::Relaxed) {
            status.errors.push("VBAN UDP port is already used by another application.\nVBAN outgoing stream should work, but\nincoming VBAN streams could not be received.".into());
        }

        // ---- start the clock
        let mut clock = None;
        let mut asio_stream = None;
        if let Some(a) = a1_asio {
            let info = a.info.clone();
            let n_out = info.outputs.len().min(MAX_CH);
            core.direct_buf = vec![vec![0.0; info.buffer]; info.inputs.len()];
            let slot = Arc::new(Mutex::new(Some(core)));
            let sh = shared.clone();
            let wake2 = wake.clone();
            let on_reset: asio::OnReset = Box::new(move |why: &str| {
                sh.status.lock().errors.push(why.to_string());
                sh.restart_request.store(true, Ordering::Relaxed);
                wake2();
            });
            match a.start(asio_process(slot.clone()), n_out, on_reset) {
                Ok(s) => {
                    status.master = format!("A1: {}", info.driver);
                    status.points[NUM_STRIPS] = Some(format!("ASIO: {} ({} Hz, {n_out} ch, {} samples)", info.driver, info.sample_rate, info.buffer));
                    status.asio = Some(info);
                    asio_stream = Some(s);
                }
                Err(e) => {
                    status.errors.push(format!("Failed to Open Main Output Device (A1)\n{e}"));
                    let mut core = slot.lock().take().expect("core still present");
                    core.direct.clear();
                    for s in 0..NUM_HW_STRIPS {
                        if settings.device.inputs[s].as_ref().is_some_and(|d| d.kind == DEV_ASIO) {
                            status.points[s] = None;
                        }
                    }
                    clock = Some(internal_clock(core));
                    status.master = "Internal Master CLOCK".into();
                }
            }
        } else {
            match (a1, a1_cfg) {
            (Some(dev), Some(cfg)) if A1_RATES.contains(&cfg.sample_rate().0) => {
                let name = dev.name().unwrap_or_default();
                let dev_ch = cfg.channels() as usize;
                match open_master(&dev, cfg, core, shared.clone()) {
                    Ok(stream) => {
                        status.master = format!("A1: {name}");
                        status.points[NUM_STRIPS] = Some(format!("WDM: {name} ({sr} Hz, {dev_ch} ch)"));
                        streams.push(stream);
                    }
                    Err((e, core)) => {
                        status.errors.push(format!("Failed to Open Main Output Device (A1)\nDevice not ready or used by another process.\n{e}"));
                        clock = Some(internal_clock(core));
                        status.master = "Internal Master CLOCK".into();
                    }
                }
            }
            _ => {
                clock = Some(internal_clock(core));
                status.master = "Internal Master CLOCK".into();
            }
        }
        }
        for s in &streams {
            if let Err(e) = s.play() {
                status.errors.push(format!("Device Not Started: {e}"));
            }
        }
        status.running = true;
        status.sample_rate = sr;
        *shared.status.lock() = status;
        Engine { streams, asio: asio_stream, clock, vban: Some(vban) }
    }

    /// Open the ASIO driver's own settings panel (A1 on ASIO only).
    pub fn asio_control_panel(&self) {
        if let Some(a) = &self.asio {
            a.control_panel();
        }
    }

    pub fn stop(&mut self) {
        self.asio = None;
        self.streams.clear();
        if let Some((stop, h)) = self.clock.take() {
            stop.store(true, Ordering::Relaxed);
            let _ = h.join();
        }
        if let Some(mut v) = self.vban.take() {
            v.stop();
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.stop();
    }
}

/// ASIO callback: render A1 in MAX_BLOCK chunks, feeding ASIO inputs straight to their strips.
fn asio_process(slot: Arc<Mutex<Option<Core>>>) -> asio::Process {
    let mut core: Option<Core> = None;
    Box::new(move |ins: &[Vec<f32>], outs: &mut [Vec<f32>], frames: usize| {
        if core.is_none() {
            core = slot.lock().take();
        }
        let Some(core) = core.as_mut() else { return };
        for (dst, src) in core.direct_buf.iter_mut().zip(ins) {
            dst[..frames].copy_from_slice(&src[..frames]);
        }
        let mut done = 0;
        while done < frames {
            let n = (frames - done).min(MAX_BLOCK);
            core.direct_off = done;
            core.block(n);
            for (c, out) in outs.iter_mut().enumerate().take(MAX_CH) {
                out[done..done + n].copy_from_slice(&core.mixer.buses[0][c][..n]);
            }
            done += n;
        }
    })
}

fn internal_clock(mut core: Core) -> (Arc<AtomicBool>, JoinHandle<()>) {
    let stop = Arc::new(AtomicBool::new(false));
    let stop2 = stop.clone();
    let h = std::thread::Builder::new()
        .name("rustbus-clock".into())
        .spawn(move || {
            let frames = (core.sr / 100) as usize; // 10 ms
            let period = Duration::from_secs_f64(frames as f64 / core.sr as f64);
            let mut next = Instant::now();
            while !stop2.load(Ordering::Relaxed) {
                core.block(frames);
                next += period;
                let now = Instant::now();
                if next > now {
                    std::thread::sleep(next - now);
                } else if now - next > Duration::from_millis(200) {
                    next = now;
                }
            }
        })
        .expect("spawn clock thread");
    (stop, h)
}

fn err_fn(shared: Arc<Shared>) -> impl FnMut(cpal::StreamError) + Send + 'static {
    move |e| {
        shared.device_error.store(true, Ordering::Relaxed);
        shared.status.lock().errors.push(format!("Audio device error: {e}"));
    }
}

type InputOpen = (cpal::Stream, ring::Consumer, u32, usize);

fn open_input(dev: &cpal::Device, strip_ch: usize, shared: Arc<Shared>) -> Result<InputOpen, String> {
    let cfg = dev.default_input_config().map_err(|e| e.to_string())?;
    let sr = cfg.sample_rate().0;
    let dev_ch = cfg.channels() as usize;
    let (prod, cons) = ring(sr as usize * strip_ch);
    let sc = cfg.config();
    let stream = match cfg.sample_format() {
        cpal::SampleFormat::F32 => build_in::<f32>(dev, &sc, dev_ch, strip_ch, prod, shared),
        cpal::SampleFormat::I16 => build_in::<i16>(dev, &sc, dev_ch, strip_ch, prod, shared),
        cpal::SampleFormat::I32 => build_in::<i32>(dev, &sc, dev_ch, strip_ch, prod, shared),
        cpal::SampleFormat::U16 => build_in::<u16>(dev, &sc, dev_ch, strip_ch, prod, shared),
        f => return Err(format!("unsupported sample format {f:?}")),
    }?;
    Ok((stream, cons, sr, dev_ch))
}

fn build_in<T>(dev: &cpal::Device, sc: &cpal::StreamConfig, dev_ch: usize, strip_ch: usize, mut prod: Producer, shared: Arc<Shared>) -> Result<cpal::Stream, String>
where
    T: SizedSample + Send + 'static,
    f32: FromSample<T>,
{
    let mut tmp = vec![0.0f32; 16384 * strip_ch];
    dev.build_input_stream(
        sc,
        move |data: &[T], _: &cpal::InputCallbackInfo| {
            let frames = (data.len() / dev_ch).min(16384);
            for i in 0..frames {
                let f = &data[i * dev_ch..(i + 1) * dev_ch];
                for c in 0..strip_ch {
                    let v = if c < dev_ch {
                        f[c].to_sample::<f32>()
                    } else if dev_ch == 1 && c == 1 {
                        f[0].to_sample::<f32>()
                    } else {
                        0.0
                    };
                    tmp[i * strip_ch + c] = v;
                }
            }
            prod.push(&tmp[..frames * strip_ch]);
        },
        err_fn(shared),
        None,
    )
    .map_err(|e| e.to_string())
}

type OutputOpen = (cpal::Stream, Producer, u32, usize);

fn open_output(dev: &cpal::Device, engine_sr: u32, latency: usize, shared: Arc<Shared>) -> Result<OutputOpen, String> {
    let cfg = dev.default_output_config().map_err(|e| e.to_string())?;
    let sr = cfg.sample_rate().0;
    let dev_ch = cfg.channels() as usize;
    let ch = dev_ch.min(MAX_CH);
    let (prod, cons) = ring(engine_sr as usize * ch);
    let reader = AdaptiveReader::new(cons, ch, engine_sr as f64 / sr as f64, latency);
    let sc = cfg.config();
    let stream = match cfg.sample_format() {
        cpal::SampleFormat::F32 => build_out::<f32>(dev, &sc, dev_ch, reader, shared),
        cpal::SampleFormat::I16 => build_out::<i16>(dev, &sc, dev_ch, reader, shared),
        cpal::SampleFormat::I32 => build_out::<i32>(dev, &sc, dev_ch, reader, shared),
        cpal::SampleFormat::U16 => build_out::<u16>(dev, &sc, dev_ch, reader, shared),
        f => return Err(format!("unsupported sample format {f:?}")),
    }?;
    Ok((stream, prod, sr, dev_ch))
}

fn build_out<T>(dev: &cpal::Device, sc: &cpal::StreamConfig, dev_ch: usize, mut reader: AdaptiveReader, shared: Arc<Shared>) -> Result<cpal::Stream, String>
where
    T: SizedSample + FromSample<f32> + Send + 'static,
{
    let ch = reader.channels();
    let mut tmp = vec![0.0f32; 16384 * ch];
    dev.build_output_stream(
        sc,
        move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
            let frames = (data.len() / dev_ch).min(16384);
            reader.read(&mut tmp, frames);
            for i in 0..frames {
                for c in 0..dev_ch {
                    let v = if c < ch { tmp[i * ch + c] } else { 0.0 };
                    data[i * dev_ch + c] = T::from_sample(v);
                }
            }
        },
        err_fn(shared),
        None,
    )
    .map_err(|e| e.to_string())
}

fn open_master(dev: &cpal::Device, cfg: cpal::SupportedStreamConfig, core: Core, shared: Arc<Shared>) -> Result<cpal::Stream, (String, Core)> {
    let sc = cfg.config();
    let dev_ch = cfg.channels() as usize;
    // The core is needed back if building fails, so it goes through a shared slot.
    let slot = Arc::new(Mutex::new(Some(core)));
    let r = match cfg.sample_format() {
        cpal::SampleFormat::F32 => build_master::<f32>(dev, &sc, dev_ch, slot.clone(), shared),
        cpal::SampleFormat::I16 => build_master::<i16>(dev, &sc, dev_ch, slot.clone(), shared),
        cpal::SampleFormat::I32 => build_master::<i32>(dev, &sc, dev_ch, slot.clone(), shared),
        cpal::SampleFormat::U16 => build_master::<u16>(dev, &sc, dev_ch, slot.clone(), shared),
        f => Err(format!("unsupported sample format {f:?}")),
    };
    r.map_err(|e| (e, slot.lock().take().expect("core still present")))
}

fn build_master<T>(dev: &cpal::Device, sc: &cpal::StreamConfig, dev_ch: usize, slot: Arc<Mutex<Option<Core>>>, shared: Arc<Shared>) -> Result<cpal::Stream, String>
where
    T: SizedSample + FromSample<f32> + Send + 'static,
{
    let mut core: Option<Core> = None;
    let ch = dev_ch.min(MAX_CH);
    dev.build_output_stream(
        sc,
        move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
            if core.is_none() {
                core = slot.lock().take();
            }
            let Some(core) = core.as_mut() else {
                data.fill(T::EQUILIBRIUM);
                return;
            };
            let total = data.len() / dev_ch;
            let mut done = 0;
            while done < total {
                let n = (total - done).min(MAX_BLOCK);
                core.block(n);
                let a1 = &core.mixer.buses[0];
                for i in 0..n {
                    let base = (done + i) * dev_ch;
                    for c in 0..dev_ch {
                        let v = if c < ch { a1[c][i] } else { 0.0 };
                        data[base + c] = T::from_sample(v);
                    }
                }
                done += n;
            }
        },
        err_fn(shared),
        None,
    )
    .map_err(|e| e.to_string())
}

#[cfg(test)]
mod golden_tests {
    use super::*;
    use golden::{level, params, Fnv, Lcg};

    // Characterization golden: must not change during refactor Phases 2-5.
    const GOLDEN_CORE_BLOCK_HASH: u64 = 0x0f52_8d6b_4280_6e51;
    // Characterization golden: must not change during refactor Phases 2-5.
    const GOLDEN_ASIO_PROCESS_HASH: u64 = 0x0535_f28c_1786_bb6d;

    const SR: u32 = 48000;

    fn core(shared: &Arc<Shared>) -> Core {
        Core {
            mixer: Mixer::new(SR as f32),
            shared: shared.clone(),
            local: *shared.params.lock(),
            inputs: Vec::new(),
            outputs: Vec::new(),
            vban_in: Vec::new(),
            vban_out: Vec::new(),
            tape: TapeCore::new(shared.tape.clone()),
            tmp: vec![0.0; MAX_BLOCK * MAX_CH],
            sr: SR,
            direct: Vec::new(),
            direct_buf: Vec::new(),
            direct_off: 0,
        }
    }

    fn noise(rng: &mut Lcg, a: f32, n: usize) -> Vec<f32> {
        (0..n).map(|_| a * rng.next()).collect()
    }

    /// `Core::block` with hand-built rings: a WDM-style AdaptiveReader on strip 1, an ASIO
    /// direct buffer on strip 2, a 44.1 kHz VBAN input on virtual strip 1, one device output
    /// (B1, 2 ch) and one VBAN output (A1, 2 ch).
    #[test]
    fn golden_core_block() {
        const FRAMES: usize = 256;
        let shared = Shared::new(params());
        shared.vban_stats.inputs[0].sr.store(44100, Ordering::Relaxed);
        let mut c = core(&shared);
        let mut rng = Lcg::new(0x5eed_0002);

        let (mut in_prod, cons) = ring(SR as usize * 2);
        c.inputs.push((0, AdaptiveReader::new(cons, 2, 1.0, 512)));
        in_prod.push(&noise(&mut rng, 0.3, 512 * 2));

        c.direct.push((1, [1, 0]));
        c.direct_buf = vec![vec![0.0; FRAMES]; 2];

        let vch = strip_channels(3);
        let (mut vban_prod, cons) = ring(SR as usize * vch);
        c.vban_in.push((0, 3, AdaptiveReader::new(cons, vch, 1.0, 512)));
        vban_prod.push(&noise(&mut rng, 0.3, 512 * vch));

        let (prod, mut out_cons) = ring(SR as usize * 2);
        c.outputs.push((3, 2, prod));
        let (prod, mut vban_out_cons) = ring(SR as usize * 2);
        c.vban_out.push((0, 2, prod));

        let mut h = Fnv::new();
        let mut out = vec![0.0; FRAMES * 2];
        let mut peak = 0.0f32;
        for b in 0..64 {
            let a = level(b);
            in_prod.push(&noise(&mut rng, a, FRAMES * 2));
            // 44.1 kHz source: 235 or 236 frames per 256-frame engine block.
            let vf = if b % 2 == 0 { 235 } else { 236 };
            vban_prod.push(&noise(&mut rng, a, vf * vch));
            for d in c.direct_buf.iter_mut() {
                for x in d.iter_mut() {
                    *x = a * rng.next();
                }
            }
            c.block(FRAMES);
            for cons in [&mut out_cons, &mut vban_out_cons] {
                assert_eq!(cons.pop(&mut out), FRAMES * 2);
                h.samples(&out);
                peak = out.iter().fold(peak, |m, x| m.max(x.abs()));
            }
            for ch in c.mixer.buses.iter().flat_map(|b| b.iter()) {
                h.samples(&ch[..FRAMES]);
            }
        }
        assert!(peak > 1e-3);
        assert_eq!(c.inputs[0].1.underruns, 0);
        assert_eq!(c.vban_in[0].2.underruns, 0);
        assert_eq!(h.finish(), GOLDEN_CORE_BLOCK_HASH, "got {:#018x}", h.finish());
    }

    /// `asio_process` with callbacks larger than MAX_BLOCK (and the plan's 700) must equal
    /// rendering the same input with `Core::block` in MAX_BLOCK chunks.
    #[test]
    fn golden_asio_process_chunking() {
        const CALLS: [usize; 4] = [700, 2 * MAX_BLOCK + 700, MAX_BLOCK, 700];
        const N_OUT: usize = MAX_CH;
        let max = *CALLS.iter().max().unwrap();
        let shared = Shared::new(params());
        let make = || {
            let mut c = core(&shared);
            c.direct.push((0, [0, 1]));
            c.direct.push((2, [1, 1]));
            c.direct_buf = vec![vec![0.0; max]; 2];
            c
        };
        let mut process = asio_process(Arc::new(Mutex::new(Some(make()))));
        let mut reference = make();

        let mut rng = Lcg::new(0x5eed_0003);
        let mut h = Fnv::new();
        let mut peak = 0.0f32;
        for (k, &frames) in CALLS.iter().enumerate() {
            let a = level(k * 3 + 1);
            let ins: Vec<Vec<f32>> = (0..2).map(|_| noise(&mut rng, a, frames)).collect();
            let mut outs = vec![vec![0.0; frames]; N_OUT];
            process(&ins, &mut outs, frames);

            // Reference: the same chunking done by hand.
            let mut want = vec![vec![0.0; frames]; N_OUT];
            for (d, s) in reference.direct_buf.iter_mut().zip(&ins) {
                d[..frames].copy_from_slice(s);
            }
            let mut done = 0;
            while done < frames {
                let n = (frames - done).min(MAX_BLOCK);
                reference.direct_off = done;
                reference.block(n);
                for (c, w) in want.iter_mut().enumerate() {
                    w[done..done + n].copy_from_slice(&reference.mixer.buses[0][c][..n]);
                }
                done += n;
            }
            assert_eq!(outs, want, "callback {k} ({frames} frames)");
            for o in &outs {
                h.samples(o);
                peak = o.iter().fold(peak, |m, x| m.max(x.abs()));
            }
        }
        assert!(peak > 1e-3);
        assert_eq!(h.finish(), GOLDEN_ASIO_PROCESS_HASH, "got {:#018x}", h.finish());
    }
}
