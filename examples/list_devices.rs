//! Lists every WASAPI device with its default (shared-mode) format.
use cpal::traits::{DeviceTrait, HostTrait};

fn main() {
    let host = cpal::default_host();
    for (kind, devs) in [("IN ", host.input_devices().unwrap()), ("OUT", host.output_devices().unwrap())] {
        for d in devs {
            let name = d.name().unwrap_or_default();
            let cfg = if kind == "IN " { d.default_input_config() } else { d.default_output_config() };
            match cfg {
                Ok(c) => println!("{kind} {name:<50} {} Hz  {} ch  {:?}", c.sample_rate().0, c.channels(), c.sample_format()),
                Err(e) => println!("{kind} {name:<50} error: {e}"),
            }
        }
    }
}
