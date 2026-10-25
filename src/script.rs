//! Parameter script language (ANALYSIS §7), e.g.
//! `Strip[0].Gain = -6.0; Bus[1].mode.TVMix = 1; Recorder.play = 1; Strip[2].Label = "Mic";`
//! Supports `=`, `+=`, `-=`; statements separated by `;`, `,` or newlines.
//! Names are case-insensitive.

use crate::params::*;

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Num(f32),
    Str(String),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Op {
    Set,
    Add,
    Sub,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Segment {
    pub name: String,
    pub index: Option<i64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Statement {
    pub path: Vec<Segment>,
    pub op: Op,
    pub value: Value,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TapeCmd {
    Play,
    Stop,
    Record,
    Pause,
    Rew,
    Ff,
    Replay,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    Tape(TapeCmd),
    TapeLoad(String),
    StripDevice { strip: usize, driver: String, name: String },
    BusDevice { bus: usize, driver: String, name: String },
    Restart,
    Shutdown,
    Show,
    Lock(bool),
    SaveSettings(String),
    LoadSettings(String),
    Reset,
}

pub fn parse(src: &str) -> Result<Vec<Statement>, String> {
    let mut out = Vec::new();
    for raw in split_statements(src) {
        let s = raw.trim();
        if s.is_empty() || s.starts_with("//") || s.starts_with('#') {
            continue;
        }
        out.push(parse_statement(s)?);
    }
    Ok(out)
}

fn split_statements(src: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut in_str = false;
    for ch in src.chars() {
        match ch {
            '"' => {
                in_str = !in_str;
                cur.push(ch);
            }
            ';' | ',' | '\n' | '\r' if !in_str => parts.push(std::mem::take(&mut cur)),
            _ => cur.push(ch),
        }
    }
    parts.push(cur);
    parts
}

fn parse_statement(s: &str) -> Result<Statement, String> {
    // Only look for the operator before any string literal.
    let head = &s[..s.find('"').unwrap_or(s.len())];
    let (lhs, op, rhs) = if let Some(i) = head.find("+=") {
        (&s[..i], Op::Add, &s[i + 2..])
    } else if let Some(i) = head.find("-=") {
        (&s[..i], Op::Sub, &s[i + 2..])
    } else if let Some(i) = head.find('=') {
        (&s[..i], Op::Set, &s[i + 1..])
    } else {
        (s, Op::Set, "1")
    };
    let path = parse_path(lhs.trim())?;
    let rhs = rhs.trim();
    let value = if let Some(inner) = rhs.strip_prefix('"') {
        Value::Str(inner.strip_suffix('"').ok_or_else(|| format!("unterminated string in '{s}'"))?.to_string())
    } else {
        Value::Num(rhs.parse::<f32>().map_err(|_| format!("bad value '{rhs}' in '{s}'"))?)
    };
    Ok(Statement { path, op, value })
}

fn parse_path(s: &str) -> Result<Vec<Segment>, String> {
    if s.is_empty() {
        return Err("empty parameter name".into());
    }
    s.split('.')
        .map(|seg| {
            let seg = seg.trim();
            if let Some(open) = seg.find('[') {
                let close = seg.rfind(']').ok_or_else(|| format!("missing ']' in '{seg}'"))?;
                let idx = seg[open + 1..close].trim().parse::<i64>().map_err(|_| format!("bad index in '{seg}'"))?;
                Ok(Segment { name: seg[..open].to_ascii_lowercase(), index: Some(idx) })
            } else if seg.is_empty() {
                Err(format!("empty segment in '{s}'"))
            } else {
                Ok(Segment { name: seg.to_ascii_lowercase(), index: None })
            }
        })
        .collect()
}

/// Execute script text against the parameters. Returns side effects the caller must
/// perform (tape transport, devices, engine restart…).
pub fn execute(src: &str, p: &mut MixParams, labels: &mut Labels) -> Result<Vec<Effect>, String> {
    let stmts = parse(src)?;
    let mut fx = Vec::new();
    for st in &stmts {
        apply(st, p, labels, &mut fx)?;
    }
    Ok(fx)
}

fn num(st: &Statement) -> Result<f32, String> {
    match &st.value {
        Value::Num(v) => Ok(*v),
        Value::Str(_) => Err(format!("{} expects a number", path_str(&st.path))),
    }
}

fn text(st: &Statement) -> Result<String, String> {
    match &st.value {
        Value::Str(s) => Ok(s.clone()),
        Value::Num(v) => Ok(v.to_string()),
    }
}

fn path_str(p: &[Segment]) -> String {
    p.iter()
        .map(|s| match s.index {
            Some(i) => format!("{}[{}]", s.name, i),
            None => s.name.clone(),
        })
        .collect::<Vec<_>>()
        .join(".")
}

fn upd(cur: f32, st: &Statement) -> Result<f32, String> {
    let v = num(st)?;
    Ok(match st.op {
        Op::Set => v,
        Op::Add => cur + v,
        Op::Sub => cur - v,
    })
}

fn updb(cur: bool, st: &Statement) -> Result<bool, String> {
    Ok(upd(if cur { 1.0 } else { 0.0 }, st)? >= 0.5)
}

fn index(seg: &Segment, max: usize) -> Result<usize, String> {
    let i = seg.index.ok_or_else(|| format!("'{}' needs an index", seg.name))?;
    if i < 0 || i as usize >= max {
        return Err(format!("index {i} out of range for '{}'", seg.name));
    }
    Ok(i as usize)
}

fn route_index(name: &str) -> Option<usize> {
    match name {
        "a1" => Some(0),
        "a2" => Some(1),
        "a3" => Some(2),
        "b1" => Some(3),
        "b2" => Some(4),
        "b3" => Some(5),
        _ => None,
    }
}

fn apply(st: &Statement, p: &mut MixParams, labels: &mut Labels, fx: &mut Vec<Effect>) -> Result<(), String> {
    let path = &st.path;
    let bad = || format!("unknown parameter '{}'", path_str(path));
    let head = &path[0];
    match head.name.as_str() {
        "strip" => {
            let i = index(head, NUM_STRIPS)?;
            let s = &mut p.strips[i];
            let key = path.get(1).ok_or_else(bad)?.name.as_str();
            let sub = path.get(2).map(|x| x.name.as_str());
            match (key, sub) {
                ("gain", None) => s.gain_db = clamp_gain(upd(s.gain_db, st)?),
                ("limit", None) => s.limit_db = upd(s.limit_db, st)?.clamp(LIMIT_MIN_DB, LIMIT_MAX_DB),
                ("mute", None) => s.mute = updb(s.mute, st)?,
                ("solo", None) => s.solo = updb(s.solo, st)?,
                ("mono", None) => s.mono = updb(s.mono, st)?,
                ("mc", None) => s.mute_center = updb(s.mute_center, st)?,
                // RustBus extension: Strip[i].Denoise = 1, Strip[i].Denoise.Amount = 0.8
                ("denoise", None) if i < NUM_HW_STRIPS => s.denoise_on = updb(s.denoise_on, st)?,
                ("denoise", Some("amount")) if i < NUM_HW_STRIPS => s.denoise = upd(s.denoise, st)?.clamp(0.0, 1.0),
                ("karaoke", None) => s.karaoke = upd(s.karaoke as f32, st)?.clamp(0.0, 4.0) as u8,
                ("mode", None) => s.strip_mode = upd(s.strip_mode as f32, st)?.clamp(0.0, 15.0) as u8,
                ("comp", None) => {
                    let v = upd(s.comp.knob(), st)?;
                    s.comp.set_knob(v);
                }
                ("gate", None) => {
                    let v = upd(s.gate.knob(), st)?;
                    s.gate.set_knob(v);
                }
                ("comp", Some(k)) => {
                    let c = &mut s.comp;
                    match k {
                        "gainin" => c.gain_in = upd(c.gain_in, st)?.clamp(-24.0, 24.0),
                        "ratio" => c.ratio = upd(c.ratio, st)?.clamp(1.0, 8.0),
                        "threshold" => c.threshold_db = upd(c.threshold_db, st)?.clamp(-40.0, -3.0),
                        "attack" => c.attack_ms = upd(c.attack_ms, st)?.clamp(0.0, 200.0),
                        "release" => c.release_ms = upd(c.release_ms, st)?.clamp(0.0, 5000.0),
                        "knee" => c.knee = upd(c.knee, st)?.clamp(0.0, 1.0),
                        "gainout" => c.gain_out = upd(c.gain_out, st)?.clamp(-24.0, 24.0),
                        "makeup" => c.auto_makeup = updb(c.auto_makeup, st)?,
                        _ => return Err(bad()),
                    }
                }
                ("gate", Some(k)) => {
                    let g = &mut s.gate;
                    match k {
                        "threshold" => g.threshold_db = upd(g.threshold_db, st)?.clamp(-60.0, -10.0),
                        "damping" => g.damping_db = upd(g.damping_db, st)?.clamp(-60.0, 0.0),
                        "bpsidechain" => g.bp_sidechain_hz = upd(g.bp_sidechain_hz, st)?.clamp(100.0, 4000.0),
                        "attack" => g.attack_ms = upd(g.attack_ms, st)?.clamp(0.0, 1000.0),
                        "hold" => g.hold_ms = upd(g.hold_ms, st)?.clamp(0.0, 5000.0),
                        "release" => g.release_ms = upd(g.release_ms, st)?.clamp(0.0, 5000.0),
                        _ => return Err(bad()),
                    }
                }
                ("eqgain1", None) => s.eq_gain[0] = upd(s.eq_gain[0], st)?.clamp(-12.0, 12.0),
                ("eqgain2", None) => s.eq_gain[1] = upd(s.eq_gain[1], st)?.clamp(-12.0, 12.0),
                ("eqgain3", None) => s.eq_gain[2] = upd(s.eq_gain[2], st)?.clamp(-12.0, 12.0),
                ("pan_x", None) => s.pan_x = upd(s.pan_x, st)?.clamp(-0.5, 0.5),
                ("pan_y", None) => s.pan_y = upd(s.pan_y, st)?.clamp(-0.5, 0.5),
                ("color_x", None) => s.color_x = upd(s.color_x, st)?.clamp(-0.5, 0.5),
                ("color_y", None) => s.color_y = upd(s.color_y, st)?.clamp(-0.5, 0.5),
                ("fx_x", None) => s.fx_x = upd(s.fx_x, st)?.clamp(-0.5, 0.5),
                ("fx_y", None) => s.fx_y = upd(s.fx_y, st)?.clamp(0.0, 1.0),
                ("label", None) => labels.strips[i] = text(st)?,
                ("device", Some(drv)) => fx.push(Effect::StripDevice { strip: i, driver: drv.to_string(), name: text(st)? }),
                (r, None) if route_index(r).is_some() => {
                    let b = route_index(r).unwrap();
                    s.route[b] = updb(s.route[b], st)?;
                }
                // Banana has no A4/A5; accept and ignore for script compatibility.
                ("a4" | "a5", None) => {}
                _ => return Err(bad()),
            }
        }
        "bus" => {
            let i = index(head, NUM_BUSES)?;
            let b = &mut p.buses[i];
            let key = path.get(1).ok_or_else(bad)?;
            match key.name.as_str() {
                "gain" => b.gain_db = clamp_gain(upd(b.gain_db, st)?),
                "limit" => b.limit_db = upd(b.limit_db, st)?.clamp(LIMIT_MIN_DB, LIMIT_MAX_DB),
                "mute" => b.mute = updb(b.mute, st)?,
                "mono" => b.mono = upd(b.mono as f32, st)?.clamp(0.0, 2.0) as u8,
                "sel" => b.sel = updb(b.sel, st)?,
                "monitor" => b.monitor = updb(b.monitor, st)?,
                "label" => labels.buses[i] = text(st)?,
                "device" => {
                    let drv = path.get(2).ok_or_else(bad)?.name.clone();
                    fx.push(Effect::BusDevice { bus: i, driver: drv, name: text(st)? });
                }
                "mode" => {
                    let m = path.get(2).ok_or_else(bad)?.name.as_str();
                    let mode = BusMode::ALL.iter().copied().find(|x| x.script_name() == m).ok_or_else(bad)?;
                    if num(st)? >= 0.5 {
                        b.mode = mode;
                    } else if b.mode == mode {
                        b.mode = BusMode::Normal;
                    }
                }
                "eq" => match path.get(2) {
                    None => b.eq_on = updb(b.eq_on, st)?,
                    Some(seg) if seg.name == "on" => b.eq_on = updb(b.eq_on, st)?,
                    Some(seg) if seg.name == "channel" => {
                        let c = index(seg, MAX_CH)?;
                        let k = path.get(3).ok_or_else(bad)?;
                        let ch = &mut b.eq[c];
                        match k.name.as_str() {
                            "trim" => ch.trim_db = upd(ch.trim_db, st)?.clamp(-24.0, 24.0),
                            "delay" => ch.delay_ms = upd(ch.delay_ms, st)?.clamp(0.0, 500.0),
                            "cell" => {
                                let n = index(k, EQ_CELLS)?;
                                let cell = &mut ch.cells[n];
                                match path.get(4).ok_or_else(bad)?.name.as_str() {
                                    "on" => cell.on = updb(cell.on, st)?,
                                    "type" => cell.kind = EqType::from_index(upd(cell.kind as i64 as f32, st)? as i64),
                                    "gain" => cell.gain_db = upd(cell.gain_db, st)?.clamp(-36.0, 18.0),
                                    "f" | "freq" => cell.freq = upd(cell.freq, st)?.clamp(20.0, 20000.0),
                                    "q" => cell.q = upd(cell.q, st)?.clamp(0.3, 100.0),
                                    _ => return Err(bad()),
                                }
                            }
                            _ => return Err(bad()),
                        }
                    }
                    _ => return Err(bad()),
                },
                _ => return Err(bad()),
            }
        }
        "recorder" => {
            let key = path.get(1).ok_or_else(bad)?.name.as_str();
            let on = || -> Result<bool, String> { Ok(num(st)? >= 0.5) };
            match key {
                "play" if on()? => fx.push(Effect::Tape(TapeCmd::Play)),
                "stop" if on()? => fx.push(Effect::Tape(TapeCmd::Stop)),
                "record" if on()? => fx.push(Effect::Tape(TapeCmd::Record)),
                "pause" if on()? => fx.push(Effect::Tape(TapeCmd::Pause)),
                "rew" if on()? => fx.push(Effect::Tape(TapeCmd::Rew)),
                "ff" if on()? => fx.push(Effect::Tape(TapeCmd::Ff)),
                "replay" if on()? => fx.push(Effect::Tape(TapeCmd::Replay)),
                "play" | "stop" | "record" | "pause" | "rew" | "ff" | "replay" => {}
                "gain" => p.tape.gain_db = clamp_gain(upd(p.tape.gain_db, st)?),
                "load" => fx.push(Effect::TapeLoad(text(st)?)),
                r if route_index(r).is_some() => {
                    let b = route_index(r).unwrap();
                    p.tape.route[b] = updb(p.tape.route[b], st)?;
                }
                _ => return Err(bad()),
            }
        }
        "command" => {
            let key = path.get(1).ok_or_else(bad)?.name.as_str();
            match key {
                "restart" => fx.push(Effect::Restart),
                "shutdown" => fx.push(Effect::Shutdown),
                "show" => fx.push(Effect::Show),
                "lock" => fx.push(Effect::Lock(num(st)? >= 0.5)),
                "save" => fx.push(Effect::SaveSettings(text(st)?)),
                "load" => fx.push(Effect::LoadSettings(text(st)?)),
                "reset" => fx.push(Effect::Reset),
                _ => return Err(bad()),
            }
        }
        _ => return Err(bad()),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(src: &str) -> (MixParams, Labels, Vec<Effect>) {
        let mut p = MixParams::default();
        let mut l = Labels::default();
        let fx = execute(src, &mut p, &mut l).unwrap();
        (p, l, fx)
    }

    #[test]
    fn strip_basics() {
        let (p, l, _) = run("Strip[0].Gain = -6.5; Strip[1].Mute = 1\nstrip[2].a2=1, Strip[3].B1 = 0; Strip[2].Label = \"My; Mic\"");
        assert_eq!(p.strips[0].gain_db, -6.5);
        assert!(p.strips[1].mute);
        assert!(p.strips[2].route[1]);
        assert!(!p.strips[3].route[3]);
        assert_eq!(l.strips[2], "My; Mic");
    }

    #[test]
    fn relative_and_clamp() {
        let (p, _, _) = run("Strip[0].Gain = 10; Strip[0].Gain += 5; Bus[0].Gain -= 100");
        assert_eq!(p.strips[0].gain_db, 12.0);
        assert_eq!(p.buses[0].gain_db, -60.0);
    }

    #[test]
    fn bus_mode_and_eq() {
        let (p, _, _) = run("Bus[2].mode.TVMix = 1; Bus[1].EQ = 1; Bus[1].EQ.channel[0].cell[2].gain = 4.5");
        assert_eq!(p.buses[2].mode, BusMode::TvMix);
        assert!(p.buses[1].eq_on);
        assert_eq!(p.buses[1].eq[0].cells[2].gain_db, 4.5);
    }

    #[test]
    fn recorder_and_commands() {
        let (p, _, fx) = run("Recorder.play = 1; Recorder.B2 = 1; Recorder.load = \"c:\\a.wav\"; Command.Restart = 1; Recorder.stop = 0");
        assert!(p.tape.route[4]);
        assert_eq!(
            fx,
            vec![Effect::Tape(TapeCmd::Play), Effect::TapeLoad("c:\\a.wav".into()), Effect::Restart]
        );
    }

    #[test]
    fn errors() {
        let mut p = MixParams::default();
        let mut l = Labels::default();
        assert!(execute("Strip[9].Gain = 0", &mut p, &mut l).is_err());
        assert!(execute("Foo.Bar = 1", &mut p, &mut l).is_err());
        assert!(execute("Strip[0].Gain = abc", &mut p, &mut l).is_err());
    }

    #[test]
    fn comp_knob() {
        let (p, _, _) = run("Strip[0].Comp = 4");
        assert!((p.strips[0].comp.knob() - 4.0).abs() < 1e-5);
    }
}

#[cfg(test)]
mod b3_tests {
    use super::*;

    #[test]
    fn b3_bus_is_addressable() {
        let mut p = MixParams::default();
        let mut l = Labels::default();
        execute("Strip[3].B3 = 1; Bus[5].Gain = -3; Bus[5].Label = \"Stream\"; Recorder.B3 = 1", &mut p, &mut l).unwrap();
        assert!(p.strips[3].route[5]);
        assert_eq!(p.buses[5].gain_db, -3.0);
        assert_eq!(l.buses[5], "Stream");
        assert!(p.tape.route[5]);
    }
}
