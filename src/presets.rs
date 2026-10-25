//! Preset scenes (`PresetScene_NN.xml`, root `VBAudioVoicemeeterPresetScene`), ANALYSIS §6.
//! A scene stores name, comment, timestamp and a `VoiceMeeterParameters` block, plus the
//! device configuration when `fdevice='1'`.

use std::fmt::Write as _;
use std::path::PathBuf;

use roxmltree::{Document, Node};

use crate::params::{Labels, MixParams};
use crate::settings::{self, ai, DeviceConfig, Settings};

pub const NUM_PRESETS: usize = 16;

#[derive(Clone, Debug, PartialEq)]
pub struct Preset {
    pub name: String,
    pub comment: String,
    pub timestamp: String,
    pub params: MixParams,
    pub labels: Labels,
    pub device: Option<DeviceConfig>,
}

pub fn preset_dir() -> PathBuf {
    settings::config_dir().join("Scenes")
}

pub fn preset_path(i: usize) -> PathBuf {
    preset_dir().join(format!("PresetScene_{:02}.xml", i + 1))
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

impl Preset {
    pub fn capture(name: &str, settings: &Settings, with_devices: bool) -> Preset {
        Preset {
            name: name.to_string(),
            comment: String::new(),
            timestamp: chrono::Local::now().format("%Y/%m/%d - %Hh%M:%S").to_string(),
            params: settings.params,
            labels: settings.labels.clone(),
            device: with_devices.then(|| settings.device.clone()),
        }
    }

    pub fn to_xml(&self, index: usize) -> String {
        let mut o = String::from("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<VBAudioVoicemeeterPresetScene>\n");
        let _ = writeln!(o, "\t<PresetName index='{}' fdevice='{}' >{}</PresetName>", index + 1, self.device.is_some() as i32, esc(&self.name));
        let _ = writeln!(o, "\t<PresetComment>{}</PresetComment>", esc(&self.comment));
        let _ = writeln!(o, "\t<PresetTimeStamp>{}</PresetTimeStamp>", esc(&self.timestamp));
        let mut s = Settings { params: self.params, labels: self.labels.clone(), ..Default::default() };
        if let Some(d) = &self.device {
            s.device = d.clone();
            let full = s.to_xml();
            // Reuse the device section exactly as the settings writer formats it.
            if let (Some(a), Some(b)) = (full.find("<VoiceMeeterDeviceConfiguration>"), full.find("</VoiceMeeterDeviceConfiguration>\n")) {
                o.push_str(&full[a..b + "</VoiceMeeterDeviceConfiguration>\n".len()]);
            }
        }
        s.write_parameters(&mut o);
        o.push_str("</VBAudioVoicemeeterPresetScene>\n");
        o
    }

    pub fn from_xml(text: &str) -> Result<Preset, String> {
        let doc = Document::parse(text.trim_start_matches('\u{feff}')).map_err(|e| e.to_string())?;
        let root = doc.root_element();
        let mut p = Preset { name: String::new(), comment: String::new(), timestamp: String::new(), params: MixParams::default(), labels: Labels::default(), device: None };
        let mut fdevice = false;
        // Parse the whole thing as a settings document for the shared sections.
        let wrapped = Settings::from_xml(text)?;
        for n in root.children().filter(Node::is_element) {
            match n.tag_name().name() {
                "PresetName" => {
                    p.name = n.text().unwrap_or("").to_string();
                    fdevice = ai(n, "fdevice").unwrap_or(0) != 0;
                }
                "PresetComment" => p.comment = n.text().unwrap_or("").to_string(),
                "PresetTimeStamp" => p.timestamp = n.text().unwrap_or("").to_string(),
                _ => {}
            }
        }
        p.params = wrapped.params;
        p.labels = wrapped.labels;
        if fdevice {
            p.device = Some(wrapped.device);
        }
        Ok(p)
    }

    pub fn store(&self, index: usize) -> std::io::Result<()> {
        std::fs::create_dir_all(preset_dir())?;
        std::fs::write(preset_path(index), self.to_xml(index))
    }

    pub fn load(index: usize) -> Option<Preset> {
        let text = std::fs::read_to_string(preset_path(index)).ok()?;
        Preset::from_xml(&text).ok()
    }

    pub fn delete(index: usize) {
        let _ = std::fs::remove_file(preset_path(index));
    }

    /// Apply to settings (parameters/labels, and devices if the scene has them).
    /// Returns true if the device configuration changed (engine restart needed).
    pub fn apply_to(&self, s: &mut Settings) -> bool {
        s.params = self.params;
        s.labels = self.labels.clone();
        match &self.device {
            Some(d) if *d != s.device => {
                s.device = d.clone();
                true
            }
            _ => false,
        }
    }
}

pub fn list() -> Vec<Option<Preset>> {
    (0..NUM_PRESETS).map(Preset::load).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::DeviceRef;

    #[test]
    fn roundtrip_with_devices() {
        let mut s = Settings::default();
        s.params.strips[1].gain_db = -9.0;
        s.labels.strips[1] = "Guitar".into();
        s.device.inputs[0] = Some(DeviceRef { kind: 3, name: "Mic".into() });
        let mut p = Preset::capture("Live", &s, true);
        p.comment = "a & b".into();
        let back = Preset::from_xml(&p.to_xml(3)).unwrap();
        assert_eq!(back, p);

        let p2 = Preset::capture("NoDev", &s, false);
        let back2 = Preset::from_xml(&p2.to_xml(0)).unwrap();
        assert_eq!(back2.device, None);
        assert_eq!(back2.params.strips[1].gain_db, -9.0);
    }
}
