//! `VBAudioVoicemeeterSettings` XML reader/writer (ANALYSIS §6, §6.1).
//!
//! The writer reproduces the original's hand-formatted layout (tab indent, single-quoted
//! attributes, section order from decompiled `FUN_0053a310` / `FUN_005391b0`). The reader
//! is tolerant: unknown elements are ignored and missing attributes keep defaults, so files
//! written by the original application load here and vice versa. Devices for the virtual
//! strips / B buses (which the original binds to its own drivers) are stored in an extra
//! `RustBusDevice` element that the original ignores (older `VMRSDevice` is still read).

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use roxmltree::{Document, Node};

use crate::midi::MidiMap;
use crate::params::*;
use crate::vban::VbanConfig;

/// Device driver type codes (public Remote API numbering).
pub const DEV_MME: i32 = 1;
pub const DEV_WDM: i32 = 3;
pub const DEV_KS: i32 = 4;
pub const DEV_ASIO: i32 = 5;

#[derive(Clone, Debug, PartialEq)]
pub struct DeviceRef {
    pub kind: i32,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DeviceConfig {
    pub buffer_mme: i32,
    pub buffer_wdm: i32,
    pub buffer_ks: i32,
    pub buffer_asio: i32,
    pub sr_asio: i32,
    /// ASIO input channels [left, right] feeding each hardware strip whose device is ASIO.
    pub asio_patch: [[i32; 2]; NUM_HW_STRIPS],
    pub monitor_delay_ms: [f32; 3],
    /// Strip input devices: 0..2 hardware strips, 3..4 virtual strips (rebuild extension).
    pub inputs: [Option<DeviceRef>; NUM_STRIPS],
    /// Bus output devices A1, A2, A3, B1, B2, B3 (B1..B3 are a rebuild extension).
    pub outputs: [Option<DeviceRef>; NUM_BUSES],
    pub midi_in: [String; 2],
    pub midi_out: String,
}

impl Default for DeviceConfig {
    fn default() -> Self {
        DeviceConfig {
            buffer_mme: 1024,
            buffer_wdm: 512,
            buffer_ks: 512,
            buffer_asio: 0,
            sr_asio: 0,
            asio_patch: std::array::from_fn(|s| [2 * s as i32, 2 * s as i32 + 1]),
            monitor_delay_ms: [0.0; 3],
            inputs: Default::default(),
            outputs: Default::default(),
            midi_in: Default::default(),
            midi_out: String::new(),
        }
    }
}

/// Recorder file type (`RecordParam FileType`) [I numbering].
pub const FILE_WAV: i32 = 1;
pub const FILE_AIFF: i32 = 2;
pub const FILE_BWF: i32 = 3;
pub const FILE_MP3: i32 = 4;

#[derive(Clone, Debug, PartialEq)]
pub struct RecorderConfig {
    /// Bus used as recording source when no input/bus is armed (0 = A1).
    pub rec_bus: i32,
    pub dont_play: bool,
    pub loop_play: bool,
    pub multitrack: bool,
    pub timer_stop: i32,
    pub arm_inputs: [bool; NUM_STRIPS],
    pub arm_buses: [bool; NUM_BUSES],
    pub last_file: String,
    pub directory: String,
    pub prefix: String,
    pub file_type: i32,
    pub bit_res: i32,
    pub target_sr: i32,
    pub channels: i32,
    pub kbps: i32,
    pub pre_rec_time: i32,
}

impl Default for RecorderConfig {
    fn default() -> Self {
        RecorderConfig {
            rec_bus: 0,
            dont_play: false,
            loop_play: false,
            multitrack: false,
            timer_stop: 0,
            arm_inputs: [false; NUM_STRIPS],
            arm_buses: [true, false, false, false, false, false],
            last_file: String::new(),
            directory: r"C:\VoicemeeterRecord".into(),
            prefix: String::new(),
            file_type: FILE_WAV,
            bit_res: 24,
            target_sr: 0,
            channels: 2,
            kbps: 192,
            pre_rec_time: 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Settings {
    pub device: DeviceConfig,
    pub params: MixParams,
    pub labels: Labels,
    pub monitor_on_sel: bool,
    pub slider_relative: bool,
    pub insert_patch: [i8; 22],
    pub insert_mode: bool,
    pub recorder: RecorderConfig,
    pub midi: MidiMap,
    pub vban: VbanConfig,
}

// ---------------------------------------------------------------- paths

fn config_base() -> PathBuf {
    dirs::config_dir().unwrap_or_else(|| PathBuf::from("."))
}

pub fn config_dir() -> PathBuf {
    config_base().join("RustBus")
}

pub fn last_settings_path() -> PathBuf {
    config_dir().join("RustBus_LastSettings.xml")
}
pub fn today_settings_path() -> PathBuf {
    config_dir().join("RustBus_TodaySettings.xml")
}
pub fn yesterday_settings_path() -> PathBuf {
    config_dir().join("RustBus_YesterdaySettings.xml")
}

/// One-time migration from the pre-rename folder `%APPDATA%\VoicemeeterRS`.
/// Copies everything (renaming `VoicemeeterBanana_*` files and the `Scene_Banana`
/// preset folder); the old folder is left untouched as a backup.
pub fn migrate_legacy_config() -> std::io::Result<bool> {
    migrate_dir(&config_base().join("VoicemeeterRS"), &config_dir())
}

fn migrate_dir(old: &Path, new: &Path) -> std::io::Result<bool> {
    if new.exists() || !old.is_dir() {
        return Ok(false);
    }
    copy_renamed(old, new)?;
    Ok(true)
}

fn copy_renamed(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let new_name = if name == "Scene_Banana" {
            "Scenes".to_string()
        } else if let Some(rest) = name.strip_prefix("VoicemeeterBanana_") {
            format!("RustBus_{rest}")
        } else {
            name
        };
        let dest = to.join(new_name);
        if entry.file_type()?.is_dir() {
            copy_renamed(&entry.path(), &dest)?;
        } else {
            std::fs::copy(entry.path(), dest)?;
        }
    }
    Ok(())
}

/// Rotate Today → Yesterday once per day, then write Today (mirrors the original's
/// Today/Yesterday backups, ANALYSIS §6).
pub fn rotate_daily_backup(xml: &str) -> std::io::Result<()> {
    let today = today_settings_path();
    if let Ok(meta) = std::fs::metadata(&today) {
        let modified: chrono::DateTime<chrono::Local> = meta.modified()?.into();
        if modified.date_naive() != chrono::Local::now().date_naive() {
            let _ = std::fs::copy(&today, yesterday_settings_path());
        }
    }
    std::fs::write(today, xml)
}

// ---------------------------------------------------------------- writer

fn b(v: bool) -> i32 {
    v as i32
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&apos;")
}

impl Settings {
    pub fn to_xml(&self) -> String {
        let mut o = String::with_capacity(64 * 1024);
        o.push_str("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<VBAudioVoicemeeterSettings>\n");
        self.write_devices(&mut o);
        self.write_parameters(&mut o);
        self.write_recorder(&mut o);
        o.push_str("<VoiceMeeterMidiMap>\n");
        self.midi.write_items(&mut o);
        o.push_str("</VoiceMeeterMidiMap>\n");
        o.push_str("<VBANConfiguration>\n");
        self.vban.write_items(&mut o);
        o.push_str("</VBANConfiguration>\n");
        o.push_str("</VBAudioVoicemeeterSettings>\n");
        o
    }

    fn write_devices(&self, o: &mut String) {
        let d = &self.device;
        o.push_str("<VoiceMeeterDeviceConfiguration>\n");
        let _ = writeln!(
            o,
            "\t<OptionDev mme='{}' wdm='{}' ks='{}' asio='{}' srasio='{}' msA1='{:.2}' msA2='{:.2}' msA3='{:.2}' />",
            d.buffer_mme, d.buffer_wdm, d.buffer_ks, d.buffer_asio, d.sr_asio, d.monitor_delay_ms[0], d.monitor_delay_ms[1], d.monitor_delay_ms[2]
        );
        for i in 0..NUM_HW_STRIPS {
            match &d.inputs[i] {
                Some(dev) => {
                    let _ = writeln!(o, "\t<InputDev index='{}' type='{}' name=\"{}\" />", i + 1, dev.kind, esc(&dev.name));
                }
                None => {
                    let _ = writeln!(o, "\t<InputDev index='{}' name=\"-\" />", i + 1);
                }
            }
        }
        for i in 0..3 {
            match &d.outputs[i] {
                Some(dev) => {
                    let _ = writeln!(o, "\t<OutputDev index='{}' type='{}' name=\"{}\" />", i + 1, dev.kind, esc(&dev.name));
                }
                None => {
                    let _ = write!(o, "\t<OutputDev index='{}' name=\"-\" ", i + 1);
                    for k in 1..=8 {
                        let _ = write!(o, "pOut{k}='0' ");
                    }
                    o.push_str(" />\n");
                }
            }
        }
        for (i, name) in d.midi_in.iter().enumerate() {
            let n = if name.is_empty() { "-".to_string() } else { esc(name) };
            let _ = writeln!(o, "\t<MIDIDevIn index='{}' name=\"{}\" />", i + 1, n);
        }
        let n = if d.midi_out.is_empty() { "-".to_string() } else { esc(&d.midi_out) };
        let _ = writeln!(o, "\t<MIDIDevOut index='1' name=\"{n}\" />");
        // Rebuild extension: ASIO input channels per hardware strip.
        for (i, [l, r]) in d.asio_patch.iter().enumerate() {
            let _ = writeln!(o, "\t<RustBusAsioPatch index='{}' left='{l}' right='{r}' />", i + 1);
        }
        // Rebuild extension: devices standing in for the VAIO virtual endpoints.
        for (i, dev) in d.inputs.iter().enumerate().skip(NUM_HW_STRIPS) {
            if let Some(dev) = dev {
                let _ = writeln!(o, "\t<RustBusDevice dir='in' index='{}' type='{}' name=\"{}\" />", i + 1, dev.kind, esc(&dev.name));
            }
        }
        for (i, dev) in d.outputs.iter().enumerate().skip(3) {
            if let Some(dev) = dev {
                let _ = writeln!(o, "\t<RustBusDevice dir='out' index='{}' type='{}' name=\"{}\" />", i + 1, dev.kind, esc(&dev.name));
            }
        }
        o.push_str("</VoiceMeeterDeviceConfiguration>\n");
    }

    /// `<VoiceMeeterParameters>` block — also the payload of preset scenes.
    pub fn write_parameters(&self, o: &mut String) {
        write_parameters(o, &self.params, &self.labels, self.monitor_on_sel, self.slider_relative, &self.insert_patch, self.insert_mode);
    }

    fn write_recorder(&self, o: &mut String) {
        let r = &self.recorder;
        o.push_str("<VoiceMeeterRecorder>\n");
        let _ = writeln!(
            o,
            "\t<RecordMode RecBus='{}' DontPlay='{}' Loop='{}' MultiTrack='{}' TimerStop='{}' />",
            r.rec_bus,
            b(r.dont_play),
            b(r.loop_play),
            b(r.multitrack),
            r.timer_stop
        );
        let _ = writeln!(
            o,
            "\t<RecordArm ArmIn1='{}' ArmIn2='{}' ArmIn3='{}' ArmIn4='{}' ArmIn5='{}' ArmIn6='{}' ArmBus1='{}' ArmBus2='{}' ArmBus3='{}' ArmBus4='{}' ArmBus5='{}' ArmBus6='{}' />",
            b(r.arm_inputs[0]),
            b(r.arm_inputs[1]),
            b(r.arm_inputs[2]),
            b(r.arm_inputs[3]),
            b(r.arm_inputs[4]),
            b(r.arm_inputs[5]),
            b(r.arm_buses[0]),
            b(r.arm_buses[1]),
            b(r.arm_buses[2]),
            b(r.arm_buses[3]),
            b(r.arm_buses[4]),
            b(r.arm_buses[5])
        );
        let _ = writeln!(o, "\t<RecordLastFileName>{}</RecordLastFileName>", esc(&r.last_file));
        let _ = writeln!(o, "\t<RecordDirectory>{}</RecordDirectory>", esc(&r.directory));
        let _ = writeln!(o, "\t<RecordPrefix>{}</RecordPrefix>", esc(&r.prefix));
        let _ = writeln!(
            o,
            "\t<RecordParam FileType='{}' BitRes='{}' TargetSR='{}' nbChannel='{}' kbps='{}' preRecTime='{}' />",
            r.file_type, r.bit_res, r.target_sr, r.channels, r.kbps, r.pre_rec_time
        );
        o.push_str("</VoiceMeeterRecorder>\n");
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, self.to_xml())
    }

    pub fn load(path: &Path) -> Result<Settings, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Settings::from_xml(&text)
    }

    pub fn from_xml(text: &str) -> Result<Settings, String> {
        let text = text.trim_start_matches('\u{feff}');
        let doc = Document::parse(text).map_err(|e| format!("XML error: {e}"))?;
        let root = doc.root_element();
        let mut s = Settings::default();
        // A plain default MIDI map / VBAN config unless the file has them.
        for sec in root.children().filter(Node::is_element) {
            match sec.tag_name().name() {
                "VoiceMeeterDeviceConfiguration" => s.read_devices(sec),
                "VoiceMeeterParameters" => {
                    read_parameters(sec, &mut s.params, &mut s.labels, &mut s.monitor_on_sel, &mut s.slider_relative, &mut s.insert_patch, &mut s.insert_mode)
                }
                "VoiceMeeterRecorder" => s.read_recorder(sec),
                "VoiceMeeterMidiMap" => s.midi = MidiMap::read(sec),
                "VBANConfiguration" => s.vban = VbanConfig::read(sec),
                _ => {}
            }
        }
        Ok(s)
    }

    fn read_devices(&mut self, sec: Node) {
        let d = &mut self.device;
        for n in sec.children().filter(Node::is_element) {
            let idx = ai(n, "index").unwrap_or(0) as usize;
            let dev = || {
                let name = n.attribute("name").unwrap_or("-");
                (name != "-" && !name.is_empty()).then(|| DeviceRef { kind: ai(n, "type").unwrap_or(DEV_WDM as i64) as i32, name: name.to_string() })
            };
            match n.tag_name().name() {
                "OptionDev" => {
                    d.buffer_mme = ai(n, "mme").unwrap_or(d.buffer_mme as i64) as i32;
                    d.buffer_wdm = ai(n, "wdm").unwrap_or(d.buffer_wdm as i64) as i32;
                    d.buffer_ks = ai(n, "ks").unwrap_or(d.buffer_ks as i64) as i32;
                    d.buffer_asio = ai(n, "asio").unwrap_or(0) as i32;
                    d.sr_asio = ai(n, "srasio").unwrap_or(0) as i32;
                    for k in 0..3 {
                        d.monitor_delay_ms[k] = af(n, &format!("msA{}", k + 1)).unwrap_or(0.0);
                    }
                }
                "InputDev" if (1..=NUM_HW_STRIPS).contains(&idx) => d.inputs[idx - 1] = dev(),
                "OutputDev" if (1..=3).contains(&idx) => d.outputs[idx - 1] = dev(),
                "MIDIDevIn" if (1..=2).contains(&idx) => d.midi_in[idx - 1] = name_attr(n),
                "MIDIDevOut" => d.midi_out = name_attr(n),
                "RustBusAsioPatch" if (1..=NUM_HW_STRIPS).contains(&idx) => {
                    let p = &mut d.asio_patch[idx - 1];
                    p[0] = ai(n, "left").unwrap_or(p[0] as i64).max(0) as i32;
                    p[1] = ai(n, "right").unwrap_or(p[1] as i64).max(0) as i32;
                }
                "RustBusDevice" | "VMRSDevice" => match n.attribute("dir") {
                    Some("in") if (NUM_HW_STRIPS + 1..=NUM_STRIPS).contains(&idx) => d.inputs[idx - 1] = dev(),
                    Some("out") if (4..=NUM_BUSES).contains(&idx) => d.outputs[idx - 1] = dev(),
                    _ => {}
                },
                _ => {}
            }
        }
    }

    fn read_recorder(&mut self, sec: Node) {
        let r = &mut self.recorder;
        for n in sec.children().filter(Node::is_element) {
            match n.tag_name().name() {
                "RecordMode" => {
                    r.rec_bus = ai(n, "RecBus").unwrap_or(0) as i32;
                    r.dont_play = ab(n, "DontPlay").unwrap_or(false);
                    r.loop_play = ab(n, "Loop").unwrap_or(false);
                    r.multitrack = ab(n, "MultiTrack").unwrap_or(false);
                    r.timer_stop = ai(n, "TimerStop").unwrap_or(0) as i32;
                }
                "RecordArm" => {
                    for k in 0..NUM_STRIPS {
                        r.arm_inputs[k] = ab(n, &format!("ArmIn{}", k + 1)).unwrap_or(false);
                    }
                    for k in 0..NUM_BUSES {
                        r.arm_buses[k] = ab(n, &format!("ArmBus{}", k + 1)).unwrap_or(false);
                    }
                }
                "RecordLastFileName" => r.last_file = n.text().unwrap_or("").to_string(),
                "RecordDirectory" => r.directory = n.text().unwrap_or("").to_string(),
                "RecordPrefix" => r.prefix = n.text().unwrap_or("").to_string(),
                "RecordParam" => {
                    r.file_type = ai(n, "FileType").unwrap_or(r.file_type as i64) as i32;
                    r.bit_res = ai(n, "BitRes").unwrap_or(r.bit_res as i64) as i32;
                    r.target_sr = ai(n, "TargetSR").unwrap_or(0) as i32;
                    r.channels = ai(n, "nbChannel").unwrap_or(2) as i32;
                    r.kbps = ai(n, "kbps").unwrap_or(192) as i32;
                    r.pre_rec_time = ai(n, "preRecTime").unwrap_or(0) as i32;
                }
                _ => {}
            }
        }
    }
}

fn name_attr(n: Node) -> String {
    match n.attribute("name") {
        Some("-") | None => String::new(),
        Some(s) => s.to_string(),
    }
}

pub fn write_parameters(o: &mut String, p: &MixParams, labels: &Labels, monitor_on_sel: bool, slider_relative: bool, insert: &[i8; 22], insert_mode: bool) {
    o.push_str("<VoiceMeeterParameters>\n");
    for i in 0..NUM_HW_STRIPS {
        let _ = writeln!(o, "\t<LabelStrip{0}>{1}</LabelStrip{0}>", i + 1, esc(&labels.strips[i]));
    }
    let _ = writeln!(o, "\t<LabelVirtualStrip1>{}</LabelVirtualStrip1>", esc(&labels.strips[3]));
    let _ = writeln!(o, "\t<LabelVirtualStrip3>{}</LabelVirtualStrip3>", esc(&labels.strips[5]));
    let _ = writeln!(o, "\t<LabelVirtualStrip2>{}</LabelVirtualStrip2>", esc(&labels.strips[4]));
    let _ = writeln!(o, "\t<VMOptions MonitorOnSEL='{}' SliderRelative='{}' />", b(monitor_on_sel), b(slider_relative));
    let t = &p.tape;
    let _ = writeln!(
        o,
        "\t<K7BUSRoute busa='{}' busa2='{}' busa3='{}' busb='{}' busb2='{}' busb3='{}' dblevel='{:.2}' />",
        b(t.route[0]),
        b(t.route[1]),
        b(t.route[2]),
        b(t.route[3]),
        b(t.route[4]),
        b(t.route[5]),
        t.gain_db
    );
    for (i, s) in p.strips.iter().enumerate() {
        let idx = i + 1;
        let _ = writeln!(
            o,
            "\t<Strip index='{idx}' mute='{}' vaio='0' solo='{}' mono='{}' muc='{}' prg_c='{}' prg_g='{}' busa='{}' busa2='{}' busa3='{}' busb='{}' busb2='{}' busb3='{}' greenmode='0' karaoke='{}' stripmode='{}' dblevel='{:.2}' dblimit='{:.2}' />",
            b(s.mute),
            b(s.solo),
            b(s.mono),
            b(s.mute_center),
            b(s.comp_on),
            b(s.gate_on),
            b(s.route[0]),
            b(s.route[1]),
            b(s.route[2]),
            b(s.route[3]),
            b(s.route[4]),
            b(s.route[5]),
            s.karaoke,
            s.strip_mode,
            s.gain_db,
            s.limit_db
        );
        // RustBus extension kept off the original <Strip> line so that line stays byte-compatible.
        if !is_virtual_strip(i) {
            let _ = writeln!(o, "\t<RustBusStrip index='{idx}' denoise='{}' denoiseamount='{:.2}' />", b(s.denoise_on), s.denoise);
        }
        let c = &s.comp;
        let _ = writeln!(
            o,
            "\t<StripComp index='{idx}' gainin='{:.2}' attack='{:.1}' release='{:.1}' knee='{:.2}' comprate='{:.3}' threshold='{:.2}' automakeup='{}' gainout='{:.2}' />",
            c.gain_in,
            c.attack_ms,
            c.release_ms,
            c.knee,
            c.ratio,
            c.threshold_db,
            b(c.auto_makeup),
            c.gain_out
        );
        let g = &s.gate;
        let _ = writeln!(
            o,
            "\t<StripGate index='{idx}' thresin='{:.2}' damping='{:.2}' bpsidechain='{:.0}' attack='{:.1}' hold='{:.1}' release='{:.1}' />",
            g.threshold_db, g.damping_db, g.bp_sidechain_hz, g.attack_ms, g.hold_ms, g.release_ms
        );
        if is_virtual_strip(i) {
            let _ = writeln!(
                o,
                "\t<Strip index='{idx}' EQGain1='{:.2}' EQGain2='{:.2}' EQGain3='{:.2}' Panel3Dx='{:.3}' Panel3Dy='{:.3}' />",
                s.eq_gain[0], s.eq_gain[1], s.eq_gain[2], s.pan_x, s.pan_y
            );
        } else {
            let _ = writeln!(
                o,
                "\t<Strip index='{idx}' paneltype='{}' Panel3Dx='{:.3}' Panel3Dy='{:.3}' ColorPanelx='{:.3}' ColorPanely='{:.3}' ModPanelx='{:.3}' ModPanely='{:.3}' audibility_c='{:.2}' audibility_g='{:.2}' />",
                s.panel_type as i32,
                s.pan_x,
                s.pan_y,
                s.color_x,
                s.color_y,
                s.fx_x,
                s.fx_y,
                s.comp.knob(),
                s.gate.knob()
            );
        }
    }
    for (i, bp) in p.buses.iter().enumerate() {
        let _ = writeln!(
            o,
            "\t<Bus index='{}' mute='{}' vaio='0' mono='{}' cross='0' BusMode='{}' EQon='{}' SEL='{}' monitor='{}' dblevel='{:.2}' greenmode='0' dblimit='{:.2}' />",
            i + 1,
            b(bp.mute),
            bp.mono,
            bp.mode as i32,
            b(bp.eq_on),
            b(bp.sel),
            b(bp.monitor),
            bp.gain_db,
            bp.limit_db
        );
    }
    for i in 0..3 {
        let _ = writeln!(o, "\t<LabelBus{0}>{1}</LabelBus{0}>", i + 1, esc(&labels.buses[i]));
    }
    let _ = writeln!(o, "\t<LabelVirtualBus1>{}</LabelVirtualBus1>", esc(&labels.buses[3]));
    let _ = writeln!(o, "\t<LabelVirtualBus2>{}</LabelVirtualBus2>", esc(&labels.buses[4]));
    let _ = writeln!(o, "\t<LabelVirtualBus3>{}</LabelVirtualBus3>", esc(&labels.buses[5]));
    o.push_str("<VoiceMeeterBUSEQ>\n");
    for (i, bp) in p.buses.iter().enumerate() {
        write_bus_eq(o, "Bus", Some(i + 1), bp);
    }
    o.push_str("</VoiceMeeterBUSEQ>\n");
    for (i, bp) in p.buses.iter().enumerate() {
        let _ = writeln!(o, "<VoiceMeeterBUSEQMem index='{}' abmem='0' abselected='0' >", i + 1);
        write_bus_eq(o, "BusEQmem", None, bp);
        o.push_str("</VoiceMeeterBUSEQMem>\n");
        let _ = writeln!(o, "<VoiceMeeterBUSEQMem index='{}' abmem='1' >", i + 1);
        write_bus_eq(o, "BusEQmem", None, bp);
        o.push_str("</VoiceMeeterBUSEQMem>\n");
    }
    let cp = &p.composite;
    let _ = write!(o, "\t<PatchComposite");
    for (k, c) in cp.channels.iter().enumerate() {
        let _ = write!(o, " c{}='{}'", k + 1, c);
    }
    let _ = writeln!(o, " mode='{}' />", b(cp.post_fader));
    let _ = write!(o, "\t<PatchInsert");
    for (k, c) in insert.iter().enumerate() {
        let _ = write!(o, " c{}='{}'", k + 1, c);
    }
    let _ = writeln!(o, " mode='{}' />", b(insert_mode));
    o.push_str("</VoiceMeeterParameters>\n");
}

fn write_bus_eq(o: &mut String, tag: &str, index: Option<usize>, bp: &BusParams) {
    let idx = index.map(|i| format!(" index='{i}'")).unwrap_or_default();
    for (c, ch) in bp.eq.iter().enumerate() {
        for (k, cell) in ch.cells.iter().enumerate() {
            let _ = writeln!(
                o,
                "\t<{tag}{idx} channel='{c}' cell='{k}' EQon='{}' EQtype='{}' dblevel='{:.2}' freq='{:.2}' Q='{:.2}' />",
                b(cell.on),
                cell.kind as i32,
                cell.gain_db,
                cell.freq,
                cell.q
            );
        }
        let _ = writeln!(o, "\t<{tag}{idx} channel='{c}' trim='{:.2}' delay='{:.2}' />", ch.trim_db, ch.delay_ms);
    }
}

pub fn read_parameters(
    sec: Node,
    p: &mut MixParams,
    labels: &mut Labels,
    monitor_on_sel: &mut bool,
    slider_relative: &mut bool,
    insert: &mut [i8; 22],
    insert_mode: &mut bool,
) {
    for n in sec.children().filter(Node::is_element) {
        let tag = n.tag_name().name();
        let text = || n.text().unwrap_or("").to_string();
        match tag {
            "LabelVirtualStrip1" => labels.strips[3] = text(),
            "LabelVirtualStrip2" => labels.strips[4] = text(),
            "LabelVirtualStrip3" => labels.strips[5] = text(),
            "LabelVirtualBus1" => labels.buses[3] = text(),
            "LabelVirtualBus2" => labels.buses[4] = text(),
            "LabelVirtualBus3" => labels.buses[5] = text(),
            t if t.starts_with("LabelStrip") => {
                if let Ok(i) = t["LabelStrip".len()..].parse::<usize>() {
                    if (1..=NUM_HW_STRIPS).contains(&i) {
                        labels.strips[i - 1] = text();
                    }
                }
            }
            t if t.starts_with("LabelBus") => {
                if let Ok(i) = t["LabelBus".len()..].parse::<usize>() {
                    if (1..=3).contains(&i) {
                        labels.buses[i - 1] = text();
                    }
                }
            }
            "VMOptions" => {
                *monitor_on_sel = ab(n, "MonitorOnSEL").unwrap_or(false);
                *slider_relative = ab(n, "SliderRelative").unwrap_or(false);
            }
            "K7BUSRoute" => {
                read_route(n, &mut p.tape.route);
                if let Some(v) = af(n, "dblevel") {
                    p.tape.gain_db = clamp_gain(v);
                }
            }
            "Strip" | "StripComp" | "StripGate" | "RustBusStrip" => {
                let Some(i) = ai(n, "index").map(|v| v as usize).filter(|i| (1..=NUM_STRIPS).contains(i)) else { continue };
                read_strip_attrs(n, &mut p.strips[i - 1]);
            }
            "Bus" => {
                let Some(i) = ai(n, "index").map(|v| v as usize).filter(|i| (1..=NUM_BUSES).contains(i)) else { continue };
                let bp = &mut p.buses[i - 1];
                if let Some(v) = ab(n, "mute") {
                    bp.mute = v;
                }
                if let Some(v) = ai(n, "mono") {
                    bp.mono = v.clamp(0, 2) as u8;
                }
                if let Some(v) = ai(n, "BusMode") {
                    bp.mode = BusMode::from_index(v);
                }
                if let Some(v) = ab(n, "EQon") {
                    bp.eq_on = v;
                }
                if let Some(v) = ab(n, "SEL") {
                    bp.sel = v;
                }
                if let Some(v) = ab(n, "monitor") {
                    bp.monitor = v;
                }
                if let Some(v) = af(n, "dblevel") {
                    bp.gain_db = clamp_gain(v);
                }
                if let Some(v) = af(n, "dblimit") {
                    bp.limit_db = v.clamp(LIMIT_MIN_DB, LIMIT_MAX_DB);
                }
            }
            "VoiceMeeterBUSEQ" => {
                for e in n.children().filter(|e| e.is_element() && e.tag_name().name() == "Bus") {
                    let Some(i) = ai(e, "index").map(|v| v as usize).filter(|i| (1..=NUM_BUSES).contains(i)) else { continue };
                    read_eq_line(e, &mut p.buses[i - 1]);
                }
            }
            "PatchComposite" => {
                for k in 0..MAX_CH {
                    if let Some(v) = ai(n, &format!("c{}", k + 1)) {
                        p.composite.channels[k] = v.clamp(0, COMPOSITE_INPUTS as i64) as u8;
                    }
                }
                p.composite.post_fader = ab(n, "mode").unwrap_or(false);
            }
            "PatchInsert" => {
                for (k, slot) in insert.iter_mut().enumerate() {
                    if let Some(v) = ai(n, &format!("c{}", k + 1)) {
                        *slot = v.clamp(-1, 1) as i8;
                    }
                }
                *insert_mode = ab(n, "mode").unwrap_or(false);
            }
            _ => {}
        }
    }
}

fn read_eq_line(e: Node, bp: &mut BusParams) {
    let Some(c) = ai(e, "channel").map(|v| v as usize).filter(|c| *c < MAX_CH) else { return };
    let ch = &mut bp.eq[c];
    if let Some(k) = ai(e, "cell").map(|v| v as usize) {
        if k < EQ_CELLS {
            let cell = &mut ch.cells[k];
            cell.on = ab(e, "EQon").unwrap_or(cell.on);
            cell.kind = ai(e, "EQtype").map(EqType::from_index).unwrap_or(cell.kind);
            cell.gain_db = af(e, "dblevel").unwrap_or(cell.gain_db);
            cell.freq = af(e, "freq").unwrap_or(cell.freq);
            cell.q = af(e, "Q").unwrap_or(cell.q);
        }
    } else {
        ch.trim_db = af(e, "trim").unwrap_or(ch.trim_db);
        ch.delay_ms = af(e, "delay").unwrap_or(ch.delay_ms);
    }
}

fn read_route(n: Node, route: &mut [bool; NUM_BUSES]) {
    for (k, name) in ["busa", "busa2", "busa3", "busb", "busb2", "busb3"].iter().enumerate() {
        if let Some(v) = ab(n, name) {
            route[k] = v;
        }
    }
}

fn read_strip_attrs(n: Node, s: &mut StripParams) {
    for a in n.attributes() {
        let v = a.value();
        let f = || v.trim().parse::<f32>().ok();
        let bo = || f().map(|x| x != 0.0);
        match a.name() {
            "mute" => s.mute = bo().unwrap_or(s.mute),
            "solo" => s.solo = bo().unwrap_or(s.solo),
            "mono" => s.mono = bo().unwrap_or(s.mono),
            "muc" => s.mute_center = bo().unwrap_or(s.mute_center),
            "prg_c" => s.comp_on = bo().unwrap_or(s.comp_on),
            "prg_g" => s.gate_on = bo().unwrap_or(s.gate_on),
            "karaoke" => s.karaoke = f().map(|x| x.clamp(0.0, 4.0) as u8).unwrap_or(s.karaoke),
            "denoise" => s.denoise_on = bo().unwrap_or(s.denoise_on),
            "denoiseamount" => s.denoise = f().map(|x| x.clamp(0.0, 1.0)).unwrap_or(s.denoise),
            "stripmode" => s.strip_mode = f().map(|x| x as u8).unwrap_or(s.strip_mode),
            "dblevel" => s.gain_db = f().map(clamp_gain).unwrap_or(s.gain_db),
            "dblimit" => s.limit_db = f().map(|x| x.clamp(LIMIT_MIN_DB, LIMIT_MAX_DB)).unwrap_or(s.limit_db),
            "gainin" => s.comp.gain_in = f().unwrap_or(s.comp.gain_in),
            "attack" if n.tag_name().name() == "StripComp" => s.comp.attack_ms = f().unwrap_or(s.comp.attack_ms),
            "release" if n.tag_name().name() == "StripComp" => s.comp.release_ms = f().unwrap_or(s.comp.release_ms),
            "knee" => s.comp.knee = f().unwrap_or(s.comp.knee),
            "comprate" => s.comp.ratio = f().map(|x| x.max(1.0)).unwrap_or(s.comp.ratio),
            "threshold" => s.comp.threshold_db = f().unwrap_or(s.comp.threshold_db),
            "automakeup" => s.comp.auto_makeup = bo().unwrap_or(s.comp.auto_makeup),
            "gainout" => s.comp.gain_out = f().unwrap_or(s.comp.gain_out),
            "thresin" => s.gate.threshold_db = f().unwrap_or(s.gate.threshold_db),
            "damping" => s.gate.damping_db = f().unwrap_or(s.gate.damping_db),
            "bpsidechain" => s.gate.bp_sidechain_hz = f().unwrap_or(s.gate.bp_sidechain_hz),
            "attack" => s.gate.attack_ms = f().unwrap_or(s.gate.attack_ms),
            "hold" => s.gate.hold_ms = f().unwrap_or(s.gate.hold_ms),
            "release" => s.gate.release_ms = f().unwrap_or(s.gate.release_ms),
            "paneltype" => s.panel_type = f().map(|x| PanelType::from_index(x as i64)).unwrap_or(s.panel_type),
            "Panel3Dx" | "panelx" | "3DPanelx" => s.pan_x = f().unwrap_or(s.pan_x),
            "Panel3Dy" | "panely" | "3DPanely" => s.pan_y = f().unwrap_or(s.pan_y),
            "ColorPanelx" => s.color_x = f().unwrap_or(s.color_x),
            "ColorPanely" => s.color_y = f().unwrap_or(s.color_y),
            "ModPanelx" => s.fx_x = f().unwrap_or(s.fx_x),
            "ModPanely" => s.fx_y = f().unwrap_or(s.fx_y),
            "EQGain1" => s.eq_gain[0] = f().unwrap_or(0.0),
            "EQGain2" => s.eq_gain[1] = f().unwrap_or(0.0),
            "EQGain3" => s.eq_gain[2] = f().unwrap_or(0.0),
            n => {
                if let Some(k) = ["busa", "busa2", "busa3", "busb", "busb2", "busb3"].iter().position(|x| *x == n) {
                    s.route[k] = bo().unwrap_or(s.route[k]);
                }
            }
        }
    }
}

pub fn ai(n: Node, name: &str) -> Option<i64> {
    let v = n.attribute(name)?.trim();
    v.parse::<i64>().ok().or_else(|| v.parse::<f64>().ok().map(|f| f as i64))
}

pub fn af(n: Node, name: &str) -> Option<f32> {
    n.attribute(name)?.trim().parse::<f32>().ok()
}

pub fn ab(n: Node, name: &str) -> Option<bool> {
    ai(n, name).map(|v| v != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shape of a file written by the original application (from a real
    /// `VoiceMeeterDefault.xml`, trimmed).
    const ORIGINAL: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<VBAudioVoicemeeterSettings>
<VoiceMeeterDeviceConfiguration>
	<OptionDev mme='1024' wdm='512' ks='512' asio='0' srasio='0' msA1='0.00' msA2='0.00' />
	<InputDev index='1' type='3' name="Microphone (USB Audio)" />
	<InputDev index='2' name="-" />
	<OutputDev index='1' type='3' name="Speakers (Realtek)" />
	<OutputDev index='2' name="-" pOut1='0' pOut2='0' pOut3='0' pOut4='0' pOut5='0' pOut6='0' pOut7='0' pOut8='0'  />
	<MIDIDevIn index='1' name="-" />
	<MIDIDevOut index='1' name="nanoKONTROL2" />
</VoiceMeeterDeviceConfiguration>
<VoiceMeeterParameters>
	<LabelStrip1>Mic</LabelStrip1>
	<LabelVirtualStrip1>Desktop</LabelVirtualStrip1>
	<VMOptions MonitorOnSEL='0' SliderRelative='1' />
	<Strip index='1' mute='1' vaio='0' solo='0' mono='0' muc='0' busa='1' busb='0' dblevel='-3.50' />
	<Strip index='1' paneltype='0' Panel3Dx='0.100' Panel3Dy='0.000' ColorPanelx='-0.200' ColorPanely='0.000' audibility='0.00' />
	<Strip index='4' EQGain1='3.00' EQGain2='0.00' EQGain3='-2.00' Panel3Dx='0.000' Panel3Dy='0.000' />
	<Bus index='1' mute='0' vaio='0' mono='0' cross='0' BusMode='5' dblevel='-1.00' />
	<LabelBus1>Speakers</LabelBus1>
</VoiceMeeterParameters>
<VBANConfiguration>
	<VBAN status="1" username="me" color="00AA9678" />
	<VBANStreamIn index='1' in='2' status='1' name='Stream1' ip='192.168.1.20' port='6980' NQ='1' />
	<VBANStreamOut index='1' bus='0' status='0' name='Stream1' ip='' port='6980' SR='48000' ch='2' NQ='0' format='1' />
</VBANConfiguration>
</VBAudioVoicemeeterSettings>
"#;

    #[test]
    fn reads_original_format() {
        let s = Settings::from_xml(ORIGINAL).unwrap();
        assert_eq!(s.device.inputs[0].as_ref().unwrap().name, "Microphone (USB Audio)");
        assert_eq!(s.device.inputs[1], None);
        assert_eq!(s.device.outputs[0].as_ref().unwrap().kind, DEV_WDM);
        assert_eq!(s.device.midi_out, "nanoKONTROL2");
        assert_eq!(s.labels.strips[0], "Mic");
        assert_eq!(s.labels.strips[3], "Desktop");
        assert_eq!(s.labels.buses[0], "Speakers");
        assert!(s.slider_relative);
        let st = &s.params.strips[0];
        assert!(st.mute);
        assert_eq!(st.gain_db, -3.5);
        assert!(st.route[0] && !st.route[3]);
        assert_eq!(st.panel_type, PanelType::Color);
        assert_eq!(st.color_x, -0.2);
        assert_eq!(s.params.strips[3].eq_gain, [3.0, 0.0, -2.0]);
        assert_eq!(s.params.buses[0].mode, BusMode::TvMix);
        assert_eq!(s.params.buses[0].gain_db, -1.0);
        assert!(s.vban.enabled);
        assert_eq!(s.vban.inputs[0].ip, "192.168.1.20");
        assert_eq!(s.vban.inputs[0].strip, 2);
    }

    #[test]
    fn roundtrip() {
        let mut s = Settings::from_xml(ORIGINAL).unwrap();
        s.params.strips[2].comp.set_knob(3.0);
        s.params.strips[4].karaoke = 2;
        s.params.buses[3].eq_on = true;
        s.params.buses[3].eq[1].cells[2].gain_db = -4.0;
        s.params.buses[3].eq[1].delay_ms = 12.5;
        s.params.tape.route = [false, true, false, false, true, true];
        s.params.composite.channels[3] = 7;
        s.device.inputs[3] = Some(DeviceRef { kind: DEV_WDM, name: "CABLE Output".into() });
        s.device.outputs[4] = Some(DeviceRef { kind: DEV_WDM, name: "CABLE Input".into() });
        s.recorder.arm_inputs[1] = true;
        s.recorder.directory = r"D:\rec & co".into();
        s.labels.buses[2] = "Head<phones>".into();
        s.labels.buses[5] = "Stream".into();
        s.params.strips[1].route[5] = true;
        s.device.outputs[5] = Some(DeviceRef { kind: DEV_WDM, name: "CABLE-B Input".into() });
        // Third virtual input strip.
        s.labels.strips[5] = "Game".into();
        s.params.strips[5].eq_gain = [2.0, 0.0, -1.0];
        s.params.strips[5].route = [false, true, false, false, false, true];
        s.device.inputs[5] = Some(DeviceRef { kind: DEV_WDM, name: "CABLE-C Output".into() });
        s.recorder.arm_inputs[5] = true;
        s.params.strips[0].denoise_on = true;
        s.params.strips[0].denoise = 0.75;
        s.device.asio_patch[0] = [0, 0];
        let xml = s.to_xml();
        let back = Settings::from_xml(&xml).unwrap();
        assert_eq!(back, s, "\n{xml}");
    }

    #[test]
    fn writer_matches_original_line_shapes() {
        let xml = Settings::default().to_xml();
        assert!(xml.starts_with("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<VBAudioVoicemeeterSettings>\n<VoiceMeeterDeviceConfiguration>\n\t<OptionDev mme='1024' wdm='512' ks='512' asio='0' srasio='0' msA1='0.00' msA2='0.00' msA3='0.00' />\n"));
        assert!(xml.contains("\t<Strip index='1' mute='0' vaio='0' solo='0' mono='0' muc='0' prg_c='1' prg_g='1' busa='1' busa2='0' busa3='0' busb='1' busb2='0' busb3='0' greenmode='0' karaoke='0' stripmode='0' dblevel='0.00' dblimit='12.00' />\n"));
        assert!(xml.contains("\t<K7BUSRoute busa='1' busa2='0' busa3='0' busb='0' busb2='0' busb3='0' dblevel='0.00' />\n"));
        assert!(xml.contains("\t<InputDev index='1' name=\"-\" />\n"));
    }
}

#[cfg(test)]
mod migrate_tests {
    use super::*;

    #[test]
    fn copies_and_renames_legacy_folder() {
        let root = std::env::temp_dir().join(format!("rustbus-migrate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let old = root.join("VoicemeeterRS");
        std::fs::create_dir_all(old.join("Scene_Banana")).unwrap();
        std::fs::write(old.join("VoicemeeterBanana_LastSettings.xml"), "x").unwrap();
        std::fs::write(old.join("options.ini"), "y").unwrap();
        std::fs::write(old.join("Scene_Banana").join("PresetScene_01.xml"), "z").unwrap();
        let new = root.join("RustBus");
        assert!(migrate_dir(&old, &new).unwrap());
        assert_eq!(std::fs::read_to_string(new.join("RustBus_LastSettings.xml")).unwrap(), "x");
        assert_eq!(std::fs::read_to_string(new.join("options.ini")).unwrap(), "y");
        assert_eq!(std::fs::read_to_string(new.join("Scenes").join("PresetScene_01.xml")).unwrap(), "z");
        assert!(old.exists(), "old folder is kept as a backup");
        // Second run is a no-op.
        assert!(!migrate_dir(&old, &new).unwrap());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn reads_legacy_device_tag() {
        let xml = "<VBAudioVoicemeeterSettings><VoiceMeeterDeviceConfiguration>\
            <VMRSDevice dir='in' index='4' type='3' name=\"Line 1\" />\
            </VoiceMeeterDeviceConfiguration></VBAudioVoicemeeterSettings>";
        let s = Settings::from_xml(xml).unwrap();
        assert_eq!(s.device.inputs[3].as_ref().unwrap().name, "Line 1");
    }
}
