//! Measures the peak level on a named capture device for a few seconds.
//! Usage: cargo run --example probe_input -- "Line 2 (Virtual Audio Cable)"
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::{Arc, Mutex};

fn main() {
    let name = std::env::args().nth(1).expect("device name");
    let host = cpal::default_host();
    let dev = host.input_devices().unwrap().find(|d| d.name().map(|n| n == name).unwrap_or(false)).expect("device not found");
    let cfg = dev.default_input_config().unwrap();
    println!("{name}: {} Hz, {} ch, {:?}", cfg.sample_rate().0, cfg.channels(), cfg.sample_format());
    let peak = Arc::new(Mutex::new(0f32));
    let p2 = peak.clone();
    let stream = dev
        .build_input_stream(&cfg.config(), move |d: &[f32], _: &cpal::InputCallbackInfo| {
            let m = d.iter().fold(0f32, |a, x| a.max(x.abs()));
            let mut p = p2.lock().unwrap();
            *p = p.max(m);
        }, |e| eprintln!("stream error: {e}"), None)
        .unwrap();
    stream.play().unwrap();
    for _ in 0..6 {
        std::thread::sleep(std::time::Duration::from_millis(500));
        let mut p = peak.lock().unwrap();
        println!("  peak {:.4}", *p);
        *p = 0.0;
    }
}
