//! M.I.D.I. device I/O via midir: port listing, input callbacks (learn + mapping), feedback output.

use std::sync::mpsc::Sender;
use std::sync::Arc;

use midir::{MidiInput, MidiInputConnection, MidiOutput, MidiOutputConnection};
use parking_lot::Mutex;

use super::map::{handle, MidiMap};
use crate::params::MixParams;
use crate::script::Effect;

pub fn list_inputs() -> Vec<String> {
    let Ok(m) = MidiInput::new("rustbus-list") else { return vec![] };
    m.ports().iter().filter_map(|p| m.port_name(p).ok()).collect()
}

pub fn list_outputs() -> Vec<String> {
    let Ok(m) = MidiOutput::new("rustbus-list") else { return vec![] };
    m.ports().iter().filter_map(|p| m.port_name(p).ok()).collect()
}

/// Learn state shared with the MIDI callback.
#[derive(Default)]
pub struct LearnState {
    /// Index of the item waiting for a message.
    pub target: Option<usize>,
    pub last_message: String,
}

pub struct MidiRuntime {
    _inputs: Vec<MidiInputConnection<()>>,
    pub output: Arc<Mutex<Option<MidiOutputConnection>>>,
    pub errors: Vec<String>,
}

impl MidiRuntime {
    pub fn start(
        in_names: &[String],
        out_name: &str,
        map: Arc<Mutex<MidiMap>>,
        params: Arc<Mutex<MixParams>>,
        learn: Arc<Mutex<LearnState>>,
        fx_tx: Sender<Effect>,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> MidiRuntime {
        let mut errors = Vec::new();
        let output: Arc<Mutex<Option<MidiOutputConnection>>> = Arc::new(Mutex::new(None));
        if !out_name.is_empty() {
            match MidiOutput::new("rustbus-out") {
                Ok(m) => match m.ports().into_iter().find(|p| m.port_name(p).ok().as_deref() == Some(out_name)) {
                    Some(port) => match m.connect(&port, "rustbus-feedback") {
                        Ok(c) => *output.lock() = Some(c),
                        Err(e) => errors.push(format!("MIDI out '{out_name}': {e}")),
                    },
                    None => errors.push(format!("MIDI out '{out_name}' not found")),
                },
                Err(e) => errors.push(e.to_string()),
            }
        }
        let mut inputs = Vec::new();
        for name in in_names.iter().filter(|n| !n.is_empty()) {
            let Ok(m) = MidiInput::new("rustbus-in") else { continue };
            let Some(port) = m.ports().into_iter().find(|p| m.port_name(p).ok().as_deref() == Some(name.as_str())) else {
                errors.push(format!("MIDI in '{name}' not found"));
                continue;
            };
            let (map, params, learn, fx_tx, wake, output) = (map.clone(), params.clone(), learn.clone(), fx_tx.clone(), wake.clone(), output.clone());
            let cb = move |_ts: u64, msg: &[u8], _: &mut ()| {
                if msg.is_empty() || msg[0] >= 0xF0 {
                    return;
                }
                {
                    let mut l = learn.lock();
                    l.last_message = msg.iter().map(|b| format!("{b:02X} ")).collect();
                    if let Some(idx) = l.target.take() {
                        let mut m = map.lock();
                        if let Some(it) = m.items.get_mut(idx) {
                            it.bytes = [0; 6];
                            let st = if msg[0] & 0xF0 == 0x80 { 0x90 | (msg[0] & 0x0F) } else { msg[0] };
                            it.bytes[0] = st;
                            it.bytes[1] = msg.get(1).copied().unwrap_or(0);
                        }
                        wake();
                        return;
                    }
                }
                let (fx, fb) = {
                    let m = map.lock();
                    let mut p = params.lock();
                    handle(&m, msg, &mut p)
                };
                if let Some(out) = output.lock().as_mut() {
                    for f in &fb {
                        let _ = out.send(f);
                    }
                }
                for e in fx {
                    let _ = fx_tx.send(e);
                }
                wake();
            };
            match m.connect(&port, "rustbus-midi-in", cb, ()) {
                Ok(c) => inputs.push(c),
                Err(e) => errors.push(format!("MIDI in '{name}': {e}")),
            }
        }
        MidiRuntime { _inputs: inputs, output, errors }
    }

    pub fn send(&self, msgs: &[[u8; 3]]) {
        if let Some(out) = self.output.lock().as_mut() {
            for m in msgs {
                let _ = out.send(m);
            }
        }
    }
}
