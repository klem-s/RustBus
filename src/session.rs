//! The running mixer behind the window: settings, the audio engine, options and the device
//! lists, with a single coalesced path for engine restarts. The GUI owns one `Session` and
//! keeps only its own view state.

use std::sync::atomic::Ordering;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::engine::{list_asio, list_devices, Engine, Shared};
use crate::options::AppOptions;
use crate::settings::{DeviceRef, Settings, DEV_WDM};

pub struct Session {
    pub settings: Settings,
    pub shared: Arc<Shared>,
    engine: Option<Engine>,
    pub options: AppOptions,
    /// WDM (input, output) device names and the installed ASIO drivers.
    pub devices: (Vec<String>, Vec<String>),
    pub asio_drivers: Vec<String>,
    /// Engine, MIDI and script messages shown in the command console.
    pub console_log: Vec<String>,
    text_tx: Sender<String>,
    /// VBAN-TEXT commands received by the engine.
    pub text_rx: Receiver<String>,
    pub wake: Arc<dyn Fn() + Send + Sync>,
    restart_pending: bool,
    restart_at: Option<Instant>,
}

impl Session {
    /// A session for `settings`. Nothing is started and no device list is read yet.
    pub fn new(settings: Settings, options: AppOptions, wake: Arc<dyn Fn() + Send + Sync>) -> Session {
        let shared = Shared::new(settings.params);
        shared.tape.set_arm(&settings.recorder);
        shared.tape.loop_play.store(settings.recorder.loop_play, Ordering::Relaxed);
        let (text_tx, text_rx) = channel();
        Session {
            settings,
            shared,
            engine: None,
            options,
            devices: (Vec::new(), Vec::new()),
            asio_drivers: Vec::new(),
            console_log: Vec::new(),
            text_tx,
            text_rx,
            wake,
            restart_pending: false,
            restart_at: None,
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

    /// Restart the engine on the next `poll_restart`; any number of requests before then
    /// cause a single restart.
    pub fn request_restart(&mut self) {
        self.restart_pending = true;
    }

    /// Run the pending restarts: the delayed ones after a device failure or a driver reset
    /// request, then the one asked for with `request_restart`.
    pub fn poll_restart(&mut self) {
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

    // ------------------------------------------------------------ devices

    pub fn refresh_devices(&mut self) {
        self.devices = list_devices();
        self.asio_drivers = list_asio();
    }

    pub fn set_strip_device(&mut self, strip: usize, name: Option<String>) {
        self.settings.device.inputs[strip] = name.map(|n| DeviceRef { kind: DEV_WDM, name: n });
        self.request_restart();
    }

    pub fn set_bus_device(&mut self, bus: usize, name: Option<String>) {
        self.settings.device.outputs[bus] = name.map(|n| DeviceRef { kind: DEV_WDM, name: n });
        self.request_restart();
    }
}
