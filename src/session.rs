//! The running mixer behind the window: settings, the audio engine, options and the device
//! lists, with a single coalesced path for engine restarts. The GUI owns one `Session` and
//! keeps only its own view state.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use crate::engine::{list_asio, list_devices, Engine, Shared};
use crate::hotkey::HotkeyRuntime;
use crate::midi::{self, LearnState, MidiMap, MidiRuntime};
use crate::options::AppOptions;
use crate::recorder::{self, RecorderJob, SEEK_SECONDS};
use crate::script::{self, Effect, TapeCmd};
use crate::settings::{self, DeviceRef, Settings, DEV_WDM};

pub struct Session {
    pub settings: Settings,
    pub shared: Arc<Shared>,
    engine: Option<Engine>,
    pub options: AppOptions,
    /// WDM (input, output) device names and the installed ASIO drivers.
    pub devices: (Vec<String>, Vec<String>),
    pub asio_drivers: Vec<String>,
    /// MIDI (input, output) port names.
    pub midi_ports: (Vec<String>, Vec<String>),
    pub midi_map: Arc<Mutex<MidiMap>>,
    pub midi_learn: Arc<Mutex<LearnState>>,
    midi: Option<MidiRuntime>,
    hotkey: Option<HotkeyRuntime>,
    /// Strip the tray "Mute mic" item toggles (follows the hotkey strip).
    pub mic_strip: Arc<AtomicIsize>,
    /// Engine, MIDI and script messages shown in the command console.
    pub console_log: Vec<String>,
    text_tx: Sender<String>,
    /// VBAN-TEXT commands received by the engine.
    text_rx: Receiver<String>,
    fx_tx: Sender<Effect>,
    /// Script effects of MIDI-mapped controls.
    fx_rx: Receiver<Effect>,
    wake: Arc<dyn Fn() + Send + Sync>,
    recorder_job: Option<RecorderJob>,
    loader: Option<JoinHandle<Result<(String, Vec<f32>, bool), String>>>,
    restart_pending: bool,
    restart_at: Option<Instant>,
    last_autosave: Instant,
    /// An error for the user (title, text); the GUI takes it and shows it.
    pub message: Option<(String, String)>,
    /// A script asked for "Reset Settings"; the GUI takes it and asks for confirmation.
    pub reset_requested: bool,
}

impl Session {
    /// A session for `settings`. Nothing is started and no device list is read yet.
    pub fn new(settings: Settings, options: AppOptions, wake: Arc<dyn Fn() + Send + Sync>) -> Session {
        let shared = Shared::new(settings.params);
        shared.tape.set_arm(&settings.recorder);
        shared.tape.loop_play.store(settings.recorder.loop_play, Ordering::Relaxed);
        let (text_tx, text_rx) = channel();
        let (fx_tx, fx_rx) = channel();
        Session {
            midi_map: Arc::new(Mutex::new(settings.midi.clone())),
            settings,
            shared,
            engine: None,
            options,
            devices: (Vec::new(), Vec::new()),
            asio_drivers: Vec::new(),
            midi_ports: (Vec::new(), Vec::new()),
            midi_learn: Arc::new(Mutex::new(LearnState::default())),
            midi: None,
            hotkey: None,
            mic_strip: Arc::new(AtomicIsize::new(0)),
            console_log: Vec::new(),
            text_tx,
            text_rx,
            fx_tx,
            fx_rx,
            wake,
            recorder_job: None,
            loader: None,
            restart_pending: false,
            restart_at: None,
            last_autosave: Instant::now(),
            message: None,
            reset_requested: false,
        }
    }

    // ------------------------------------------------------------ engine

    /// Stop the engine (if running) and start it again from the current settings, now.
    pub fn restart_engine(&mut self) {
        if let Some(mut e) = self.engine.take() {
            e.stop();
        }
        self.settings.params = *self.shared.params.lock();
        self.engine = Some(Engine::start(&self.settings.device, &self.settings.vban, &self.shared, self.text_tx.clone(), self.wake.clone()));
        let st = self.shared.status.lock();
        for e in &st.errors {
            self.console_log.push(format!("[engine] {}", e.replace('\n', " ")));
        }
    }

    /// Restart the engine on the next `tick`; any number of requests before then cause a
    /// single restart.
    pub fn request_restart(&mut self) {
        self.restart_pending = true;
    }

    /// Run the pending restarts: the delayed ones after a device failure or a driver reset
    /// request, then the one asked for with `request_restart`.
    fn poll_restart(&mut self) {
        // Device failure → auto restart (menu "Auto Restart Audio Engine").
        if self.shared.device_error.load(Ordering::Relaxed) && self.options.auto_restart && self.restart_at.is_none() {
            self.restart_at = Some(Instant::now() + Duration::from_secs(2));
        }
        // A driver asked to be reopened (ASIO reset after a control-panel change).
        if self.shared.restart_request.swap(false, Ordering::Relaxed) && self.restart_at.is_none() {
            self.restart_at = Some(Instant::now() + Duration::from_millis(500));
        }
        if let Some(t) = self.restart_at {
            if Instant::now() >= t {
                self.restart_at = None;
                self.refresh_devices();
                self.restart_engine();
            }
        }
        if self.restart_pending {
            self.restart_pending = false;
            self.restart_engine();
        }
    }

    /// Poll the background work (call once per GUI frame): VBAN-TEXT commands, MIDI effects,
    /// the tape file loader and recorder, pending restarts and the periodic autosave.
    pub fn tick(&mut self) {
        while let Ok(t) = self.text_rx.try_recv() {
            self.run_script(&t, "vban");
        }
        while let Ok(e) = self.fx_rx.try_recv() {
            self.handle_effect(e);
        }
        if self.loader.as_ref().map(|h| h.is_finished()).unwrap_or(false) {
            match self.loader.take().unwrap().join() {
                Ok(Ok((name, data, play))) => {
                    recorder::set_loaded_file(&self.shared.tape, &name, data, play);
                    *self.shared.tape.status.lock() = String::new();
                }
                Ok(Err(e)) => {
                    *self.shared.tape.status.lock() = e.clone();
                    self.message = Some(("Tape".into(), e));
                }
                Err(_) => {}
            }
        }
        if self.recorder_job.as_ref().map(|j| j.is_finished()).unwrap_or(false) {
            self.finish_recording();
        }
        self.poll_restart();
        // Periodic autosave of the last settings (crash safety).
        if self.last_autosave.elapsed() > Duration::from_secs(30) {
            self.last_autosave = Instant::now();
            let _ = self.save_settings_to(&settings::last_settings_path());
        }
    }

    pub fn stop_engine(&mut self) {
        if let Some(mut e) = self.engine.take() {
            e.stop();
        }
    }

    /// Open the running ASIO driver's own settings panel.
    pub fn asio_control_panel(&self) {
        if let Some(e) = &self.engine {
            e.asio_control_panel();
        }
    }

    // ------------------------------------------------------------ MIDI / hotkey

    pub fn restart_midi(&mut self) {
        let d = &self.settings.device;
        let rt = MidiRuntime::start(
            &d.midi_in,
            &d.midi_out,
            self.midi_map.clone(),
            self.shared.params.clone(),
            self.midi_learn.clone(),
            self.fx_tx.clone(),
            self.wake.clone(),
        );
        for e in &rt.errors {
            self.console_log.push(format!("[midi] {e}"));
        }
        self.midi = Some(rt);
    }

    pub fn send_midi_feedback(&self) {
        if let Some(m) = &self.midi {
            let p = *self.shared.params.lock();
            m.send(&midi::feedback_all(&self.midi_map.lock(), &p));
        }
    }

    pub fn restart_hotkey(&mut self) {
        self.hotkey = None;
        let cfg = self.options.hotkey;
        self.mic_strip.store(cfg.strip as isize, Ordering::Relaxed);
        self.hotkey = HotkeyRuntime::start(cfg, self.shared.params.clone(), self.wake.clone());
    }

    // ------------------------------------------------------------ settings

    /// The settings with the live mixer parameters and MIDI map.
    pub fn snapshot_settings(&mut self) -> Settings {
        self.settings.params = *self.shared.params.lock();
        self.settings.midi = self.midi_map.lock().clone();
        self.settings.clone()
    }

    pub fn save_settings_to(&mut self, path: &Path) -> Result<(), String> {
        self.snapshot_settings().save(path).map_err(|e| e.to_string())
    }

    /// Make `s` the current settings and restart the engine and MIDI with them, now.
    pub fn apply_settings(&mut self, s: Settings) {
        *self.shared.params.lock() = s.params;
        *self.midi_map.lock() = s.midi.clone();
        self.shared.tape.set_arm(&s.recorder);
        self.shared.tape.loop_play.store(s.recorder.loop_play, Ordering::Relaxed);
        self.settings = s;
        self.restart_engine();
        self.restart_midi();
    }

    // ------------------------------------------------------------ devices

    pub fn refresh_devices(&mut self) {
        self.devices = list_devices();
        self.asio_drivers = list_asio();
    }

    pub fn refresh_midi_ports(&mut self) {
        self.midi_ports = (midi::list_inputs(), midi::list_outputs());
    }

    pub fn set_strip_device(&mut self, strip: usize, name: Option<String>) {
        self.settings.device.inputs[strip] = name.map(|n| DeviceRef { kind: DEV_WDM, name: n });
        self.request_restart();
    }

    pub fn set_bus_device(&mut self, bus: usize, name: Option<String>) {
        self.settings.device.outputs[bus] = name.map(|n| DeviceRef { kind: DEV_WDM, name: n });
        self.request_restart();
    }

    // ------------------------------------------------------------ tape

    pub fn tape_cmd(&mut self, c: TapeCmd) {
        let t = self.shared.tape.clone();
        match c {
            TapeCmd::Play => {
                if t.has_file() {
                    if t.pos.load(Ordering::Relaxed) >= t.len.load(Ordering::Relaxed) {
                        t.pos.store(0, Ordering::Relaxed);
                    }
                    t.playing.store(true, Ordering::Relaxed);
                }
            }
            TapeCmd::Replay => {
                t.pos.store(0, Ordering::Relaxed);
                t.playing.store(t.has_file(), Ordering::Relaxed);
            }
            TapeCmd::Stop => {
                let was_active = t.playing.load(Ordering::Relaxed) || self.recorder_job.is_some();
                t.playing.store(false, Ordering::Relaxed);
                self.finish_recording();
                if !was_active {
                    t.pos.store(0, Ordering::Relaxed);
                }
            }
            TapeCmd::Record => {
                if self.recorder_job.is_some() {
                    let p = t.rec_paused.load(Ordering::Relaxed);
                    t.rec_paused.store(!p, Ordering::Relaxed);
                } else {
                    let sr = self.shared.status.lock().sample_rate.max(8000);
                    match RecorderJob::start(&t, &self.settings.recorder, sr) {
                        Ok(job) => self.recorder_job = Some(job),
                        Err(e) => self.message = Some(("Recorder".into(), e)),
                    }
                }
            }
            TapeCmd::Pause => {
                if self.recorder_job.is_some() {
                    let p = t.rec_paused.load(Ordering::Relaxed);
                    t.rec_paused.store(!p, Ordering::Relaxed);
                }
            }
            TapeCmd::Rew => t.seek_by(-SEEK_SECONDS),
            TapeCmd::Ff => t.seek_by(SEEK_SECONDS),
        }
    }

    pub fn finish_recording(&mut self) {
        if let Some(job) = self.recorder_job.take() {
            self.shared.tape.recording.store(false, Ordering::Relaxed);
            match job.finish() {
                Ok(files) => {
                    if let Some(f) = files.first() {
                        self.settings.recorder.last_file = f.to_string_lossy().into();
                        *self.shared.tape.status.lock() = format!("Saved {}", f.display());
                        // Like the original: the new recording becomes the loaded cassette.
                        if !self.settings.recorder.multitrack {
                            self.load_tape_file(f.clone(), false);
                        }
                    }
                }
                Err(e) => self.message = Some(("Recorder".into(), e)),
            }
        }
    }

    pub fn load_tape_file(&mut self, path: PathBuf, play: bool) {
        let sr = self.shared.status.lock().sample_rate.max(8000);
        *self.shared.tape.status.lock() = format!("Loading {}...", path.display());
        self.loader = Some(std::thread::spawn(move || {
            let data = recorder::load_audio_file(&path, sr)?;
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            Ok((name, data, play))
        }));
    }

    // ------------------------------------------------------------ effects

    pub fn run_script(&mut self, text: &str, source: &str) {
        let result = {
            let mut p = self.shared.params.lock();
            let mut copy = *p;
            let r = script::execute(text, &mut copy, &mut self.settings.labels);
            if r.is_ok() {
                if self.options.limit_remote_gain && source != "console" {
                    for s in copy.strips.iter_mut() {
                        s.gain_db = s.gain_db.min(0.0);
                    }
                    for b in copy.buses.iter_mut() {
                        b.gain_db = b.gain_db.min(0.0);
                    }
                }
                *p = copy;
            }
            r
        };
        match result {
            Ok(fx) => {
                self.console_log.push(format!("[{source}] ok: {}", text.trim()));
                for e in fx {
                    self.handle_effect(e);
                }
            }
            Err(e) => self.console_log.push(format!("[{source}] error: {e}")),
        }
        if self.console_log.len() > 500 {
            self.console_log.drain(..100);
        }
    }

    fn handle_effect(&mut self, e: Effect) {
        match e {
            Effect::Tape(c) => self.tape_cmd(c),
            Effect::TapeLoad(p) => self.load_tape_file(PathBuf::from(p), false),
            Effect::StripDevice { strip, name, .. } => self.set_strip_device(strip, (!name.is_empty()).then_some(name)),
            Effect::BusDevice { bus, name, .. } => self.set_bus_device(bus, (!name.is_empty()).then_some(name)),
            Effect::Restart => self.request_restart(),
            Effect::Shutdown => self.console_log.push("[script] Command.Shutdown requested".into()),
            Effect::Show => {}
            Effect::Lock(b) => self.options.lock_gui = b,
            Effect::SaveSettings(p) => {
                if let Err(e) = self.save_settings_to(Path::new(&p)) {
                    self.console_log.push(format!("save failed: {e}"));
                }
            }
            Effect::LoadSettings(p) => match Settings::load(Path::new(&p)) {
                Ok(s) => self.apply_settings(s),
                Err(e) => self.console_log.push(format!("load failed: {e}")),
            },
            Effect::Reset => self.reset_requested = true,
        }
    }
}
