//! M.I.D.I. mapping (ANALYSIS §9): control IDs, learn, linear / encoder control types,
//! feedback, and the `VoiceMeeterMidiMap` XML items.

use std::fmt::Write as _;
use std::sync::mpsc::Sender;
use std::sync::Arc;

use midir::{MidiInput, MidiInputConnection, MidiOutput, MidiOutputConnection};
use parking_lot::Mutex;
use roxmltree::Node;

use crate::params::*;
use crate::script::{Effect, TapeCmd};
use crate::xml::{ab, ai};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Target {
    StripGain(usize),
    StripPan(usize),
    StripPanReset(usize),
    StripMute(usize),
    StripSolo(usize),
    StripMuteCenter(usize),
    StripDenoise(usize),
    StripKaraoke(usize),
    StripComp(usize),
    StripGate(usize),
    StripLimit(usize),
    StripRoute(usize, usize),
    StripEq(usize, usize),
    ToggleB1B2(usize),
    BusGain(usize),
    BusMute(usize),
    BusEq(usize),
    BusLimit(usize),
    Tape(TapeCmd),
    TapeGain,
}

impl Target {
    pub fn is_button(self) -> bool {
        matches!(
            self,
            Target::StripPanReset(_)
                | Target::StripMute(_)
                | Target::StripSolo(_)
                | Target::StripMuteCenter(_)
                | Target::StripDenoise(_)
                | Target::StripKaraoke(_)
                | Target::StripRoute(..)
                | Target::ToggleB1B2(_)
                | Target::BusMute(_)
                | Target::BusEq(_)
                | Target::Tape(_)
        )
    }
}

/// All control IDs with a description, in dialog order (strings 1337–1494).
pub fn controls() -> Vec<(String, String, Target)> {
    let mut v: Vec<(String, String, Target)> = Vec::new();
    let mut add = |id: String, d: String, t: Target| v.push((id, d, t));
    for i in 0..3 {
        add(format!("InGainFader{}", i + 1), format!("Gain Fader Strip #{}", i + 1), Target::StripGain(i));
    }
    for k in 0..NUM_STRIPS - NUM_HW_STRIPS {
        add(format!("VInGainFader{}", k + 1), format!("Gain of Virtual Strip #{}", k + 4), Target::StripGain(3 + k));
    }
    for i in 0..3 {
        add(format!("LRPanIn{}", i + 1), format!("L/R PAN Strip #{}", i + 1), Target::StripPan(i));
    }
    for k in 0..NUM_STRIPS - NUM_HW_STRIPS {
        add(format!("LRPanVIn{}", k + 1), format!("L/R PAN Virtual Strip #{}", k + 4), Target::StripPan(3 + k));
    }
    for i in 0..3 {
        add(format!("ResetLRPanIn{}", i + 1), format!("Reset L/R PAN Strip #{}", i + 1), Target::StripPanReset(i));
    }
    for k in 0..NUM_STRIPS - NUM_HW_STRIPS {
        add(format!("ResetLRPanVIn{}", k + 1), format!("Reset L/R PAN VStrip #{}", k + 4), Target::StripPanReset(3 + k));
    }
    for i in 0..3 {
        add(format!("InMute{}", i + 1), format!("Mute Strip #{}", i + 1), Target::StripMute(i));
    }
    for k in 0..NUM_STRIPS - NUM_HW_STRIPS {
        add(format!("VInMute{}", k + 1), format!("Mute Virtual Strip #{}", k + 4), Target::StripMute(3 + k));
    }
    for i in 0..3 {
        add(format!("InSolo{}", i + 1), format!("Solo Strip #{}", i + 1), Target::StripSolo(i));
    }
    for k in 0..NUM_STRIPS - NUM_HW_STRIPS {
        add(format!("VInSolo{}", k + 1), format!("Solo Virtual Strip #{}", k + 4), Target::StripSolo(3 + k));
    }
    add("VInMuteC1".into(), "M.C. Virtual Strip #4".into(), Target::StripMuteCenter(3));
    add("VInMuteC2".into(), "K Button Virtual Strip #5".into(), Target::StripKaraoke(4));
    for i in 0..NUM_HW_STRIPS {
        add(format!("InDenoise{}", i + 1), format!("Denoise Strip #{}", i + 1), Target::StripDenoise(i));
    }
    for i in 0..3 {
        add(format!("InComp{}", i + 1), format!("Compression Strip #{}", i + 1), Target::StripComp(i));
    }
    for i in 0..3 {
        add(format!("InGate{}", i + 1), format!("Gate Strip #{}", i + 1), Target::StripGate(i));
    }
    for i in 0..3 {
        add(format!("InLimit{}", i + 1), format!("Limit Strip #{}", i + 1), Target::StripLimit(i));
    }
    for k in 0..NUM_STRIPS - NUM_HW_STRIPS {
        add(format!("VInLimit{}", k + 1), format!("Limit Virtual Strip #{}", k + 4), Target::StripLimit(3 + k));
    }
    for s in 0..NUM_STRIPS {
        for (b, name) in BUS_NAMES.iter().enumerate() {
            add(format!("Strip{}{}", s + 1, name), format!("{} Strip #{}", name, s + 1), Target::StripRoute(s, b));
        }
    }
    for k in 0..NUM_STRIPS - NUM_HW_STRIPS {
        for (band, n) in [(2, "High"), (1, "Med"), (0, "Bass")] {
            add(format!("VIn{}EQ{}", k + 1, n), format!("EQ {} Virtual Strip #{}", n, k + 4), Target::StripEq(3 + k, band));
        }
    }
    for b in 0..NUM_BUSES {
        let id = if b < 3 { format!("OutGainFader{}", b + 1) } else { format!("VOutGainFader{}", b - 2) };
        add(id, format!("Gain of BUS {}", BUS_NAMES[b]), Target::BusGain(b));
    }
    for b in 0..NUM_BUSES {
        let id = if b < 3 { format!("OutMute{}", b + 1) } else { format!("VOutMute{}", b - 2) };
        add(id, format!("Mute BUS {}", BUS_NAMES[b]), Target::BusMute(b));
    }
    add("ToggleB1B2s1".into(), "Toggle B1/B2 on Strip #1".into(), Target::ToggleB1B2(0));
    for b in 0..NUM_BUSES {
        let id = if b < 3 { format!("OutEqOn{}", b + 1) } else { format!("VOutEqOn{}", b - 2) };
        add(id, format!("Master EQ BUS {}", BUS_NAMES[b]), Target::BusEq(b));
    }
    for b in 0..NUM_BUSES {
        let id = if b < 3 { format!("BusLimit{}", b + 1) } else { format!("VBusLimit{}", b - 2) };
        add(id, format!("Limit Bus {}", BUS_NAMES[b]), Target::BusLimit(b));
    }
    for (id, d, c) in [
        ("TapePlay", "PLAY TAPE", TapeCmd::Play),
        ("TapeRec", "REC TAPE", TapeCmd::Record),
        ("TapeStop", "STOP TAPE", TapeCmd::Stop),
        ("TapeREW", "REW TAPE", TapeCmd::Rew),
        ("TapeFF", "FF TAPE", TapeCmd::Ff),
        ("TapeRePlay", "REPLAY TAPE", TapeCmd::Replay),
    ] {
        add(id.into(), d.into(), Target::Tape(c));
    }
    add("TapeGain".into(), "TAPE Playback Gain".into(), Target::TapeGain);
    v
}

/// Control type: 0 = linear, 1 = coder offset (64 = no change), 2 = coder sign bit (Mackie).
#[derive(Clone, Debug, PartialEq)]
pub struct MidiItem {
    pub id: String,
    pub bytes: [u8; 6],
    pub coder: u8,
    pub coder_type: u8,
    pub ptt: bool,
    pub feed1: bool,
    pub feed2: bool,
    pub omni: bool,
    pub disabled: bool,
    pub feed_on: u8,
    pub feed_off: u8,
}

impl MidiItem {
    fn new(id: &str) -> Self {
        MidiItem { id: id.into(), bytes: [0; 6], coder: 0, coder_type: 0, ptt: false, feed1: false, feed2: false, omni: false, disabled: false, feed_on: 127, feed_off: 0 }
    }

    pub fn is_mapped(&self) -> bool {
        self.bytes[0] & 0x80 != 0 && !self.disabled
    }

    /// Does an incoming message (status, data1) trigger this item?
    pub fn matches(&self, status: u8, data1: u8) -> bool {
        if !self.is_mapped() {
            return false;
        }
        let (want, got) = if self.omni { (self.bytes[0] & 0xF0, status & 0xF0) } else { (self.bytes[0], status) };
        if want != got {
            return false;
        }
        // Program change / pitch bend have no data1 selector.
        matches!(status & 0xF0, 0xC0 | 0xE0) || self.bytes[1] == data1
    }

    pub fn describe(&self) -> String {
        if !self.is_mapped() {
            return String::from("-");
        }
        let ch = (self.bytes[0] & 0x0F) + 1;
        match self.bytes[0] & 0xF0 {
            0x90 => format!("#{ch} Note On {}", self.bytes[1]),
            0x80 => format!("#{ch} Note Off {}", self.bytes[1]),
            0xB0 => format!("#{ch} Ctrl Ch. {}", self.bytes[1]),
            0xC0 => format!("#{ch} Prg Ch. {}", self.bytes[1]),
            0xE0 => format!("#{ch} P.Bend"),
            _ => format!("{:02X} {:02X}", self.bytes[0], self.bytes[1]),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct MidiMap {
    pub name: String,
    pub items: Vec<MidiItem>,
}

impl Default for MidiMap {
    fn default() -> Self {
        MidiMap { name: "-noname-".into(), items: controls().iter().map(|(id, _, _)| MidiItem::new(id)).collect() }
    }
}

impl MidiMap {
    pub fn write_items(&self, o: &mut String) {
        let _ = writeln!(o, "\t<MidiMapName>{}</MidiMapName>", self.name.replace('&', "&amp;").replace('<', "&lt;"));
        for it in &self.items {
            let b = &it.bytes;
            let _ = writeln!(
                o,
                "\t<MidiMapItem id='{}' b1='{:02X}' b2='{:02X}' b3='{:02X}' b4='{:02X}' b5='{:02X}' b6='{:02X}' coder='{}' codertype='{}' ptt='{}' feed1='{}' feed2='{}' omni='{}' disabled='{}' feedOn='{}' feedOff='{}'/>",
                it.id,
                b[0],
                b[1],
                b[2],
                b[3],
                b[4],
                b[5],
                it.coder,
                it.coder_type,
                it.ptt as i32,
                it.feed1 as i32,
                it.feed2 as i32,
                it.omni as i32,
                it.disabled as i32,
                it.feed_on,
                it.feed_off
            );
        }
    }

    pub fn read(sec: Node) -> MidiMap {
        let mut m = MidiMap::default();
        for n in sec.children().filter(Node::is_element) {
            match n.tag_name().name() {
                "MidiMapName" => m.name = n.text().unwrap_or("").to_string(),
                "MidiMapItem" => {
                    let Some(id) = n.attribute("id") else { continue };
                    let Some(it) = m.items.iter_mut().find(|x| x.id.eq_ignore_ascii_case(id)) else { continue };
                    for k in 0..6 {
                        if let Some(v) = n.attribute(format!("b{}", k + 1).as_str()) {
                            it.bytes[k] = u8::from_str_radix(v.trim(), 16).unwrap_or(0);
                        }
                    }
                    it.coder = ai(n, "coder").unwrap_or(0) as u8;
                    it.coder_type = ai(n, "codertype").unwrap_or(0) as u8;
                    it.ptt = ab(n, "ptt").unwrap_or(false);
                    it.feed1 = ab(n, "feed1").unwrap_or(false);
                    it.feed2 = ab(n, "feed2").unwrap_or(false);
                    it.omni = ab(n, "omni").unwrap_or(false);
                    it.disabled = ab(n, "disabled").unwrap_or(false);
                    it.feed_on = ai(n, "feedOn").unwrap_or(127) as u8;
                    it.feed_off = ai(n, "feedOff").unwrap_or(0) as u8;
                }
                _ => {}
            }
        }
        m
    }

    /// Standalone M.I.D.I. map file (`VBAudioVoicemeeterMIDIMapping`).
    pub fn to_file_xml(&self) -> String {
        let mut o = String::from("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<VBAudioVoicemeeterMIDIMapping>\n<VoiceMeeterMidiMap>\n");
        self.write_items(&mut o);
        o.push_str("</VoiceMeeterMidiMap>\n</VBAudioVoicemeeterMIDIMapping>\n");
        o
    }

    pub fn from_file_xml(text: &str) -> Result<MidiMap, String> {
        let doc = roxmltree::Document::parse(text.trim_start_matches('\u{feff}')).map_err(|e| e.to_string())?;
        let sec = doc
            .descendants()
            .find(|n| n.has_tag_name("VoiceMeeterMidiMap"))
            .ok_or("no VoiceMeeterMidiMap element")?;
        Ok(MidiMap::read(sec))
    }
}

fn scale(v: u8, lo: f32, hi: f32) -> f32 {
    lo + (hi - lo) * (v.min(127) as f32 / 127.0)
}

fn unscale(x: f32, lo: f32, hi: f32) -> u8 {
    (((x - lo) / (hi - lo)).clamp(0.0, 1.0) * 127.0).round() as u8
}

/// Range of a continuous target: (min, max, encoder step).
fn range(t: Target) -> (f32, f32, f32) {
    match t {
        Target::StripGain(_) | Target::BusGain(_) | Target::TapeGain => (GAIN_MIN_DB, GAIN_MAX_DB, 0.5),
        Target::StripPan(_) => (-0.5, 0.5, 0.01),
        Target::StripComp(_) | Target::StripGate(_) => (0.0, 10.0, 0.1),
        Target::StripLimit(_) | Target::BusLimit(_) => (LIMIT_MIN_DB, LIMIT_MAX_DB, 0.5),
        Target::StripEq(..) => (-12.0, 12.0, 0.25),
        _ => (0.0, 1.0, 1.0),
    }
}

fn get(p: &MixParams, t: Target) -> f32 {
    let bf = |b: bool| b as i32 as f32;
    match t {
        Target::StripGain(s) => p.strips[s].gain_db,
        Target::StripPan(s) => p.strips[s].pan_x,
        Target::StripPanReset(_) => 0.0,
        Target::StripMute(s) => bf(p.strips[s].mute),
        Target::StripSolo(s) => bf(p.strips[s].solo),
        Target::StripMuteCenter(s) => bf(p.strips[s].mute_center),
        Target::StripDenoise(s) => bf(p.strips[s].denoise_on),
        Target::StripKaraoke(s) => bf(p.strips[s].karaoke > 0),
        Target::StripComp(s) => p.strips[s].comp.knob(),
        Target::StripGate(s) => p.strips[s].gate.knob(),
        Target::StripLimit(s) => p.strips[s].limit_db,
        Target::StripRoute(s, b) => bf(p.strips[s].route[b]),
        Target::StripEq(s, band) => p.strips[s].eq_gain[band],
        Target::ToggleB1B2(s) => bf(p.strips[s].route[4]),
        Target::BusGain(b) => p.buses[b].gain_db,
        Target::BusMute(b) => bf(p.buses[b].mute),
        Target::BusEq(b) => bf(p.buses[b].eq_on),
        Target::BusLimit(b) => p.buses[b].limit_db,
        Target::Tape(_) => 0.0,
        Target::TapeGain => p.tape.gain_db,
    }
}

fn set(p: &mut MixParams, t: Target, v: f32) {
    let on = v >= 0.5;
    match t {
        Target::StripGain(s) => p.strips[s].gain_db = clamp_gain(v),
        Target::StripPan(s) => p.strips[s].pan_x = v.clamp(-0.5, 0.5),
        Target::StripPanReset(s) => {
            if on {
                p.strips[s].pan_x = 0.0;
                p.strips[s].pan_y = 0.0;
            }
        }
        Target::StripMute(s) => p.strips[s].mute = on,
        Target::StripSolo(s) => p.strips[s].solo = on,
        Target::StripMuteCenter(s) => p.strips[s].mute_center = on,
        Target::StripDenoise(s) => p.strips[s].denoise_on = on,
        Target::StripKaraoke(s) => p.strips[s].karaoke = if on { p.strips[s].karaoke.max(1) } else { 0 },
        Target::StripComp(s) => p.strips[s].comp.set_knob(v),
        Target::StripGate(s) => p.strips[s].gate.set_knob(v),
        Target::StripLimit(s) => p.strips[s].limit_db = v.clamp(LIMIT_MIN_DB, LIMIT_MAX_DB),
        Target::StripRoute(s, b) => p.strips[s].route[b] = on,
        Target::StripEq(s, band) => p.strips[s].eq_gain[band] = v.clamp(-12.0, 12.0),
        Target::ToggleB1B2(s) => {
            p.strips[s].route[3] = !on;
            p.strips[s].route[4] = on;
        }
        Target::BusGain(b) => p.buses[b].gain_db = clamp_gain(v),
        Target::BusMute(b) => p.buses[b].mute = on,
        Target::BusEq(b) => p.buses[b].eq_on = on,
        Target::BusLimit(b) => p.buses[b].limit_db = v.clamp(LIMIT_MIN_DB, LIMIT_MAX_DB),
        Target::Tape(_) => {}
        Target::TapeGain => p.tape.gain_db = clamp_gain(v),
    }
}

/// Apply an incoming 3-byte message to the parameters. Returns the side effect (tape)
/// and, for feedback, the 3-byte reply to send.
pub fn handle(map: &MidiMap, msg: &[u8], p: &mut MixParams) -> (Vec<Effect>, Vec<[u8; 3]>) {
    let mut fx = Vec::new();
    let mut fb = Vec::new();
    if msg.is_empty() || msg[0] & 0x80 == 0 {
        return (fx, fb);
    }
    let status = msg[0];
    let d1 = msg.get(1).copied().unwrap_or(0);
    let d2 = msg.get(2).copied().unwrap_or(0);
    let kind = status & 0xF0;
    // Normalised 7-bit value: note-off = 0, pitch bend uses MSB.
    let value = match kind {
        0x80 => 0,
        0xC0 => 127,
        0xE0 => d2,
        _ => d2,
    };
    let ctrl = controls();
    for it in map.items.iter() {
        if !(it.matches(status, d1) || (kind == 0x80 && it.bytes[0] & 0xF0 == 0x90 && it.matches(0x90 | (status & 0x0F), d1))) {
            continue;
        }
        let Some((_, _, t)) = ctrl.iter().find(|(id, _, _)| *id == it.id) else { continue };
        let t = *t;
        if t.is_button() {
            let pressed = value > 0;
            if let Target::Tape(c) = t {
                if pressed {
                    fx.push(Effect::Tape(c));
                }
                continue;
            }
            if it.ptt {
                set(p, t, if pressed { 1.0 } else { 0.0 });
            } else if pressed {
                let cur = get(p, t);
                set(p, t, if cur >= 0.5 { 0.0 } else { 1.0 });
            } else {
                continue;
            }
            let on = get(p, t) >= 0.5;
            fb.push([it.bytes[0], it.bytes[1], if on { it.feed_on } else { it.feed_off }]);
        } else {
            let (lo, hi, step) = range(t);
            let nv = if it.coder > 0 {
                let delta = match it.coder_type {
                    1 => {
                        // Sign bit (Mackie): bit 6 set = negative.
                        let mag = (value & 0x3F) as f32;
                        if value & 0x40 != 0 { -mag } else { mag }
                    }
                    _ => value as f32 - 64.0,
                };
                (get(p, t) + delta * step).clamp(lo, hi)
            } else {
                scale(value, lo, hi)
            };
            set(p, t, nv);
            if it.feed1 || it.feed2 || it.coder == 0 {
                fb.push([it.bytes[0], it.bytes[1], unscale(get(p, t), lo, hi)]);
            }
        }
    }
    (fx, fb)
}

/// Current feedback state for every mapped item (for "Refresh" / after a preset load).
pub fn feedback_all(map: &MidiMap, p: &MixParams) -> Vec<[u8; 3]> {
    let ctrl = controls();
    map.items
        .iter()
        .filter(|it| it.is_mapped() && (it.feed1 || it.feed2))
        .filter_map(|it| {
            let (_, _, t) = ctrl.iter().find(|(id, _, _)| *id == it.id)?;
            let t = *t;
            let v = if t.is_button() {
                if get(p, t) >= 0.5 { it.feed_on } else { it.feed_off }
            } else {
                let (lo, hi, _) = range(t);
                unscale(get(p, t), lo, hi)
            };
            Some([it.bytes[0], it.bytes[1], v])
        })
        .collect()
}

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

#[cfg(test)]
mod tests {
    use super::*;

    fn map_with(id: &str, f: impl FnOnce(&mut MidiItem)) -> MidiMap {
        let mut m = MidiMap::default();
        let it = m.items.iter_mut().find(|x| x.id == id).unwrap();
        f(it);
        m
    }

    #[test]
    fn ids_unique_and_complete() {
        let c = controls();
        let mut ids: Vec<_> = c.iter().map(|x| x.0.clone()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), c.len());
        assert!(ids.contains(&"Strip5B2".to_string()));
        assert!(ids.contains(&"VOutGainFader2".to_string()));
    }

    #[test]
    fn linear_fader() {
        let m = map_with("InGainFader1", |it| it.bytes[..2].copy_from_slice(&[0xB0, 7]));
        let mut p = MixParams::default();
        handle(&m, &[0xB0, 7, 127], &mut p);
        assert_eq!(p.strips[0].gain_db, 12.0);
        handle(&m, &[0xB0, 7, 0], &mut p);
        assert_eq!(p.strips[0].gain_db, -60.0);
        handle(&m, &[0xB1, 7, 127], &mut p); // other channel, no omni
        assert_eq!(p.strips[0].gain_db, -60.0);
    }

    #[test]
    fn toggle_and_ptt() {
        let m = map_with("InMute2", |it| it.bytes[..2].copy_from_slice(&[0x90, 36]));
        let mut p = MixParams::default();
        let (_, fb) = handle(&m, &[0x90, 36, 100], &mut p);
        assert!(p.strips[1].mute);
        assert_eq!(fb, vec![[0x90, 36, 127]]);
        handle(&m, &[0x80, 36, 0], &mut p);
        assert!(p.strips[1].mute, "note off must not toggle");
        handle(&m, &[0x90, 36, 100], &mut p);
        assert!(!p.strips[1].mute);

        let m = map_with("InMute1", |it| {
            it.bytes[..2].copy_from_slice(&[0x90, 40]);
            it.ptt = true;
        });
        handle(&m, &[0x90, 40, 100], &mut p);
        assert!(p.strips[0].mute);
        handle(&m, &[0x80, 40, 0], &mut p);
        assert!(!p.strips[0].mute);
    }

    #[test]
    fn encoder_modes() {
        let m = map_with("OutGainFader1", |it| {
            it.bytes[..2].copy_from_slice(&[0xB0, 16]);
            it.coder = 1;
            it.coder_type = 0;
        });
        let mut p = MixParams::default();
        handle(&m, &[0xB0, 16, 66], &mut p);
        assert_eq!(p.buses[0].gain_db, 1.0);
        let m = map_with("OutGainFader1", |it| {
            it.bytes[..2].copy_from_slice(&[0xB0, 16]);
            it.coder = 1;
            it.coder_type = 1;
        });
        handle(&m, &[0xB0, 16, 0x41], &mut p);
        assert_eq!(p.buses[0].gain_db, 0.5);
    }

    #[test]
    fn tape_button_emits_effect() {
        let m = map_with("TapePlay", |it| it.bytes[..2].copy_from_slice(&[0x90, 1]));
        let mut p = MixParams::default();
        let (fx, _) = handle(&m, &[0x90, 1, 127], &mut p);
        assert_eq!(fx, vec![Effect::Tape(TapeCmd::Play)]);
    }

    #[test]
    fn xml_roundtrip() {
        let m = map_with("Strip3B1", |it| {
            it.bytes = [0xB2, 0x10, 0, 0, 0, 0];
            it.omni = true;
            it.feed_on = 64;
        });
        let back = MidiMap::from_file_xml(&m.to_file_xml()).unwrap();
        assert_eq!(back, m);
    }
}
