//! Smoke test of the device engine against real WASAPI devices.
//! A1 = default output (bus muted, nothing routed → silence), strip 1 = default input.
//! Prints engine status and input meter peaks for ~2 s.

use std::sync::atomic::Ordering;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait};
use rustbus::engine::{mixer::Meters, Engine, Shared};
use rustbus::settings::{DeviceRef, Settings, DEV_WDM};

fn main() {
    let host = cpal::default_host();
    let out = host.default_output_device().and_then(|d| d.name().ok());
    let inp = host.default_input_device().and_then(|d| d.name().ok());
    println!("default output: {out:?}\ndefault input:  {inp:?}");

    let mut s = Settings::default();
    s.device.outputs[0] = out.map(|n| DeviceRef { kind: DEV_WDM, name: n });
    s.device.inputs[0] = inp.map(|n| DeviceRef { kind: DEV_WDM, name: n });
    s.params.strips[0].route = [false; rustbus::params::NUM_BUSES];
    s.params.buses[0].mute = true;

    let shared = Shared::new(s.params);
    let (tx, _rx) = std::sync::mpsc::channel();
    let mut engine = Engine::start(&s, &shared, tx, std::sync::Arc::new(|| {}));
    let st = shared.status.lock().clone();
    println!("running={} sr={} master={}", st.running, st.sample_rate, st.master);
    for (i, p) in st.points.iter().enumerate() {
        if let Some(p) = p {
            println!("  point {i}: {p}");
        }
    }
    for e in &st.errors {
        println!("  error: {e}");
    }
    for _ in 0..4 {
        std::thread::sleep(Duration::from_millis(500));
        let l = Meters::take(&shared.meters.strips[0][0]);
        let r = Meters::take(&shared.meters.strips[0][1]);
        println!("  strip1 peak L={l:.5} R={r:.5}  device_error={}", shared.device_error.load(Ordering::Relaxed));
    }
    engine.stop();
    println!("stopped cleanly");
}
