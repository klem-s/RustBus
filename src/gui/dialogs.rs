//! Secondary windows: System Settings, Recorder Options, M.I.D.I. Mapping, VBAN,
//! Preset Scenes, Bus EQ, Command Console, About and small prompts.

use std::sync::atomic::Ordering;

use eframe::egui::{self, Color32, Pos2, RichText, Stroke, Ui, Vec2};

use rustbus::dsp::biquad::Coefs;
use rustbus::engine::mixer::composite_label;
use rustbus::hotkey::{self, HotkeyMode};
use rustbus::midi::{self, MidiMap};
use rustbus::params::*;
use rustbus::presets::NUM_PRESETS;
use rustbus::session;
use rustbus::settings::{DeviceRef, Settings, DEV_ASIO, DEV_WDM, FILE_BWF, FILE_WAV};
use rustbus::vban::{self, FORMAT_NAMES, NQ_NAMES, SR_TABLE};

use super::theme::RED;
use super::widgets::*;
use super::{short, App};

pub(crate) fn show(app: &mut App, ctx: &egui::Context, p: &mut MixParams) {
    system(app, ctx, p);
    recorder(app, ctx);
    midi_dlg(app, ctx);
    vban_dlg(app, ctx);
    presets(app, ctx);
    bus_eq(app, ctx, p);
    console(app, ctx);
    about(app, ctx);
    rename(app, ctx);
    app.take_session_notices();
    message(app, ctx);
    confirm_reset(app, ctx);
    hotkey_dlg(app, ctx);
}

// ---------------------------------------------------------------- Mic hotkey

fn hotkey_dlg(app: &mut App, ctx: &egui::Context) {
    const ESC: u16 = 0x1B;
    if !app.dlg.hotkey {
        app.hotkey_capture = None;
        return;
    }
    let mut open = true;
    egui::Window::new("Mic Hotkey").open(&mut open).resizable(false).show(ctx, |ui| {
        let mut cfg = app.session.options.hotkey;
        egui::Grid::new("hotkey").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
            ui.label("Mode");
            egui::ComboBox::from_id_salt("hk-mode").width(180.0).selected_text(cfg.mode.label()).show_ui(ui, |ui| {
                for m in HotkeyMode::ALL {
                    ui.selectable_value(&mut cfg.mode, m, m.label());
                }
            });
            ui.end_row();

            ui.label("Strip");
            egui::ComboBox::from_id_salt("hk-strip").width(180.0).selected_text(app.session.settings.labels.strip_display(cfg.strip)).show_ui(ui, |ui| {
                for s in 0..NUM_STRIPS {
                    ui.selectable_value(&mut cfg.strip, s, app.session.settings.labels.strip_display(s));
                }
            });
            ui.end_row();

            ui.label("Key");
            ui.horizontal(|ui| {
                if let Some(prev) = &mut app.hotkey_capture {
                    ui.label(RichText::new("Press a key or mouse button… (Esc cancels)").color(ON_YELLOW));
                    let now = hotkey::keys_down();
                    match now.iter().copied().find(|k| !prev.contains(k)) {
                        Some(ESC) => app.hotkey_capture = None,
                        Some(k) => {
                            cfg.vk = k;
                            app.hotkey_capture = None;
                        }
                        // Keys held when capture started count again once released.
                        None => prev.retain(|k| now.contains(k)),
                    }
                    ctx.request_repaint();
                } else {
                    ui.label(RichText::new(hotkey::key_name(cfg.vk)).strong());
                    if ui.button("Set key…").clicked() {
                        app.hotkey_capture = Some(hotkey::keys_down());
                    }
                    if cfg.vk != 0 && ui.button("Clear").clicked() {
                        cfg.vk = 0;
                    }
                }
            });
            ui.end_row();
        });
        ui.separator();
        let key = hotkey::key_name(cfg.vk);
        let strip = app.session.settings.labels.strip_display(cfg.strip);
        let status = match cfg.mode {
            _ if cfg.vk == 0 && cfg.mode != HotkeyMode::Off => "Choose a key to turn it on.".to_string(),
            HotkeyMode::Off => "Hotkey is off.".to_string(),
            HotkeyMode::PushToTalk => format!("{strip} is muted unless you hold {key}."),
            HotkeyMode::PushToMute => format!("{strip} is muted while you hold {key}."),
            HotkeyMode::Toggle => format!("Press {key} to mute / unmute {strip}."),
        };
        ui.label(status);
        ui.label(
            RichText::new("Works while a game has focus. Mouse side buttons (Mouse 4/5) or F13–F24 make good keys, since games rarely use them.")
                .size(11.0)
                .color(TEXT_DIM),
        );
        if cfg != app.session.options.hotkey {
            app.session.options.hotkey = cfg;
            app.session.options.save();
            app.session.restart_hotkey();
        }
    });
    if !open {
        app.dlg.hotkey = false;
    }
}

/// Run a window whose open flag lives in `app.dlg` without borrowing `app` twice.
macro_rules! window {
    ($app:expr, $ctx:expr, $flag:ident, $title:expr, |$ui:ident| $body:block) => {{
        if $app.dlg.$flag {
            let mut open = true;
            egui::Window::new($title).open(&mut open).resizable(true).show($ctx, |$ui| $body);
            if !open {
                $app.dlg.$flag = false;
            }
        }
    }};
}

/// Device picker: ASIO drivers (A1 only, `asio` empty elsewhere) then WDM devices.
fn device_combo(ui: &mut Ui, id: &str, current: &Option<DeviceRef>, asio: &[String], names: &[String]) -> Option<Option<DeviceRef>> {
    let mut out = None;
    let sel = match current {
        Some(d) if d.kind == DEV_ASIO => format!("ASIO: {}", short(&d.name)),
        Some(d) => short(&d.name),
        None => "- none -".into(),
    };
    egui::ComboBox::from_id_salt(id).width(260.0).selected_text(sel).show_ui(ui, |ui| {
        if ui.selectable_label(current.is_none(), "- none -").clicked() {
            out = Some(None);
        }
        for (kind, prefix, list) in [(DEV_ASIO, "ASIO", asio), (DEV_WDM, "WDM", names)] {
            for n in list {
                let on = current.as_ref().is_some_and(|d| &d.name == n && (d.kind == DEV_ASIO) == (kind == DEV_ASIO));
                if ui.selectable_label(on, format!("{prefix}: {n}")).clicked() {
                    out = Some(Some(DeviceRef { kind, name: n.clone() }));
                }
            }
        }
    });
    out
}

// ---------------------------------------------------------------- System settings

fn system(app: &mut App, ctx: &egui::Context, p: &mut MixParams) {
    window!(app, ctx, system, "System Settings / Options", |ui| {
        let st = app.session.status();
        ui.heading("Audio Engine");
        ui.label(format!("Sample rate: {} Hz    Master clock: {}", st.sample_rate, st.master));
        ui.horizontal(|ui| {
            ui.label("Buffering (WDM/WASAPI latency target, samples):");
            ui.add(egui::DragValue::new(&mut app.session.settings.device.buffer_wdm).range(128..=2048));
            ui.label(RichText::new("(default: 512)").color(TEXT_DIM));
        });
        ui.horizontal(|ui| {
            ui.label("Buffering ASIO (samples, 0 = driver default):");
            ui.add(egui::DragValue::new(&mut app.session.settings.device.buffer_asio).range(0..=2048));
            ui.label("Preferred ASIO sample rate:");
            let sr = &mut app.session.settings.device.sr_asio;
            egui::ComboBox::from_id_salt("sr-asio").selected_text(if *sr == 0 { "driver default".to_string() } else { format!("{sr} Hz") }).show_ui(ui, |ui| {
                ui.selectable_value(sr, 0, "driver default");
                for r in [44100, 48000, 88200, 96000, 176400, 192000] {
                    ui.selectable_value(sr, r, format!("{r} Hz"));
                }
            });
        });
        ui.horizontal(|ui| {
            if ui.button("Restart Audio Engine").clicked() {
                app.session.restart_engine();
            }
            if let Some(a) = &st.asio {
                if ui.button("ASIO Control Panel...").on_hover_text("Opens the driver's own settings (buffer size, clock)").clicked() {
                    app.session.asio_control_panel();
                }
                ui.label(RichText::new(format!("{}: {} samples ({:.1} ms)", a.driver, a.buffer, a.buffer as f32 * 1000.0 / a.sample_rate.max(1) as f32)).color(TEXT_DIM));
            }
        });
        if !st.errors.is_empty() {
            ui.group(|ui| {
                ui.label(RichText::new("Engine messages").color(ON_YELLOW));
                for e in &st.errors {
                    ui.label(RichText::new(e).size(11.0));
                }
            });
        }
        ui.separator();
        ui.heading("Audio Points");
        let (ins, outs) = app.session.devices.clone();
        let mut restart = false;
        egui::Grid::new("points").num_columns(3).striped(true).show(ui, |ui| {
            for s in 0..NUM_STRIPS {
                ui.label(if is_virtual_strip(s) { format!("Virtual Input #{}", s - NUM_HW_STRIPS + 1) } else { format!("Hardware Input #{}", s + 1) });
                if let Some(sel) = device_combo(ui, &format!("in{s}"), &app.session.settings.device.inputs[s], &[], &ins) {
                    app.session.settings.device.inputs[s] = sel;
                    restart = true;
                }
                ui.label(RichText::new(st.points[s].clone().unwrap_or("-".into())).size(10.5).color(TEXT_DIM));
                ui.end_row();
            }
            for b in 0..NUM_BUSES {
                ui.label(format!("Output {}", BUS_NAMES[b]));
                let asio = if b == 0 { app.session.asio_drivers.clone() } else { Vec::new() };
                if let Some(sel) = device_combo(ui, &format!("out{b}"), &app.session.settings.device.outputs[b], &asio, &outs) {
                    app.session.settings.device.outputs[b] = sel;
                    restart = true;
                }
                ui.label(RichText::new(st.points[NUM_STRIPS + b].clone().unwrap_or("-".into())).size(10.5).color(TEXT_DIM));
                ui.end_row();
            }
        });
        if restart {
            app.session.restart_engine();
        }
        ui.separator();
        ui.heading("PATCH COMPOSITE");
        ui.label(RichText::new("Input channel used by each BUS channel in COMPOSITE mode").color(TEXT_DIM));
        ui.horizontal(|ui| {
            ui.radio_value(&mut p.composite.post_fader, false, "PRE-Fader inputs");
            ui.radio_value(&mut p.composite.post_fader, true, "POST-Fader inputs");
        });
        egui::Grid::new("composite").num_columns(4).show(ui, |ui| {
            for c in 0..MAX_CH {
                ui.label(format!("BUS ch{}", c + 1));
                let cur = p.composite.channels[c] as usize;
                egui::ComboBox::from_id_salt(format!("comp{c}")).width(130.0).selected_text(composite_label(cur)).show_ui(ui, |ui| {
                    for k in 0..=COMPOSITE_INPUTS {
                        if ui.selectable_label(cur == k, composite_label(k)).clicked() {
                            p.composite.channels[c] = k as u8;
                        }
                    }
                });
                if c % 2 == 1 {
                    ui.end_row();
                }
            }
        });
        ui.separator();
        ui.checkbox(&mut app.session.settings.monitor_on_sel, "Monitor on SEL (stored for compatibility)");
        ui.checkbox(&mut app.session.settings.slider_relative, "Relative sliders (stored for compatibility)");
    });
}

// ---------------------------------------------------------------- Recorder

fn recorder(app: &mut App, ctx: &egui::Context) {
    window!(app, ctx, recorder, "Tape Recorder Options", |ui| {
        let r = &mut app.session.settings.recorder;
        egui::Grid::new("rec").num_columns(2).show(ui, |ui| {
            ui.label("Target Directory:");
            ui.horizontal(|ui| {
                ui.text_edit_singleline(&mut r.directory);
                if ui.button("...").clicked() {
                    if let Some(d) = rfd::FileDialog::new().set_title("Select Target Folder").pick_folder() {
                        r.directory = d.to_string_lossy().into();
                    }
                }
            });
            ui.end_row();
            ui.label("Prefix Name:");
            ui.text_edit_singleline(&mut r.prefix);
            ui.end_row();
            ui.label("File Type:");
            egui::ComboBox::from_id_salt("ftype").selected_text(if r.file_type == FILE_BWF { "BWF" } else { "WAV" }).show_ui(ui, |ui| {
                ui.selectable_value(&mut r.file_type, FILE_WAV, "WAV");
                ui.selectable_value(&mut r.file_type, FILE_BWF, "BWF (written as WAV)");
            });
            ui.end_row();
            ui.label("Sample Rate:");
            egui::ComboBox::from_id_salt("tsr").selected_text(if r.target_sr == 0 { "Default SR".to_string() } else { format!("{} Hz", r.target_sr) }).show_ui(ui, |ui| {
                for sr in [0, 22050, 32000, 44100, 48000, 88200, 96000] {
                    let t = if sr == 0 { "Default SR".to_string() } else { format!("{sr} Hz") };
                    ui.selectable_value(&mut r.target_sr, sr, t);
                }
            });
            ui.end_row();
            ui.label("Bit Resolution:");
            egui::ComboBox::from_id_salt("bits").selected_text(format!("{} bits", r.bit_res)).show_ui(ui, |ui| {
                ui.selectable_value(&mut r.bit_res, 16, "16 bits");
                ui.selectable_value(&mut r.bit_res, 24, "24 bits");
                ui.selectable_value(&mut r.bit_res, 32, "32 bits float");
            });
            ui.end_row();
            ui.label("Channels:");
            egui::ComboBox::from_id_salt("nch").selected_text(format!("{} channels", r.channels)).show_ui(ui, |ui| {
                for (n, t) in [(2, "2 channels (Stereo)"), (4, "4 channels (Quadri)"), (6, "6 channels (5.1)"), (8, "8 channels (7.1)")] {
                    ui.selectable_value(&mut r.channels, n, t);
                }
            });
            ui.end_row();
            ui.label("Stop Record After (s, 0 = never):");
            ui.add(egui::DragValue::new(&mut r.timer_stop).range(0..=86400));
            ui.end_row();
        });
        ui.checkbox(&mut r.multitrack, format!("MULTITRACK OPTION > Generates {} Mono WAV files", r.channels));
        let mut play_on_load = !r.dont_play;
        if ui.checkbox(&mut play_on_load, "Play On Load").changed() {
            r.dont_play = !play_on_load;
        }
        if ui.checkbox(&mut r.loop_play, "Loop playback").changed() {
            app.session.shared.tape.loop_play.store(r.loop_play, Ordering::Relaxed);
        }
        ui.separator();
        ui.label(RichText::new("Recorded sources (mixed together):").strong());
        ui.horizontal(|ui| {
            ui.label("Inputs:");
            for s in 0..NUM_STRIPS {
                ui.checkbox(&mut r.arm_inputs[s], format!("IN{}", s + 1));
            }
        });
        ui.horizontal(|ui| {
            ui.label("Buses:");
            for b in 0..NUM_BUSES {
                ui.checkbox(&mut r.arm_buses[b], BUS_NAMES[b]);
            }
        });
        let r = r.clone();
        app.session.shared.tape.set_arm(&r);
        ui.label(RichText::new("MP3 / AIFF encoding and pre-recording are not implemented.").color(TEXT_DIM).size(11.0));
    });
}

// ---------------------------------------------------------------- MIDI

fn midi_dlg(app: &mut App, ctx: &egui::Context) {
    window!(app, ctx, midi, "M.I.D.I. Mapping", |ui| {
        let (ins, outs) = app.session.midi_ports.clone();
        let mut restart = false;
        let mut pick = |ui: &mut Ui, id: &str, label: &str, cur: &mut String, list: &[String]| {
            ui.label(label);
            egui::ComboBox::from_id_salt(id).width(240.0).selected_text(if cur.is_empty() { "Not Selected".into() } else { cur.clone() }).show_ui(ui, |ui| {
                if ui.selectable_label(cur.is_empty(), "Not Selected").clicked() {
                    cur.clear();
                    restart = true;
                }
                for n in list {
                    if ui.selectable_label(cur == n, n).clicked() {
                        *cur = n.clone();
                        restart = true;
                    }
                }
            });
            ui.end_row();
        };
        egui::Grid::new("mididev").num_columns(2).show(ui, |ui| {
            let d = &mut app.session.settings.device;
            pick(ui, "mi1", "M.I.D.I. Input Device:", &mut d.midi_in[0], &ins);
            pick(ui, "mi2", "M.I.D.I. Extra Input Device:", &mut d.midi_in[1], &ins);
            pick(ui, "mo1", "M.I.D.I. Output Device (feedback):", &mut d.midi_out, &outs);
        });
        if restart {
            app.session.restart_midi();
        }
        let last = app.session.midi_learn.lock().last_message.clone();
        let learning = app.session.midi_learn.lock().target;
        ui.horizontal(|ui| {
            ui.label("M.I.D.I. Map Name:");
            ui.text_edit_singleline(&mut app.session.midi_map.lock().name);
            ui.label(RichText::new(format!("Last message: {last}")).monospace().color(TEXT_DIM));
        });
        ui.horizontal(|ui| {
            if ui.button("Reset Map").clicked() {
                *app.session.midi_map.lock() = MidiMap::default();
            }
            if ui.button("Load M.I.D.I. Map...").clicked() {
                if let Some(f) = rfd::FileDialog::new().add_filter("XML", &["xml"]).pick_file() {
                    if let Err(e) = app.session.load_midi_map(&f) {
                        app.dlg.message = Some(("M.I.D.I.".into(), e));
                    }
                }
            }
            if ui.button("Save Current M.I.D.I. Map...").clicked() {
                if let Some(f) = rfd::FileDialog::new().add_filter("XML", &["xml"]).set_file_name("MidiMap.xml").save_file() {
                    let _ = app.session.save_midi_map(&f);
                }
            }
            if ui.button("Refresh (send feedback)").clicked() {
                app.session.send_midi_feedback();
            }
            if ui.button("Refresh device list").clicked() {
                app.session.refresh_midi_ports();
            }
        });
        ui.separator();
        let controls = midi::controls();
        egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
            egui::Grid::new("midimap").num_columns(8).striped(true).show(ui, |ui| {
                for h in ["Control", "Message", "", "Control Type", "PTT", "Omni", "Feedback", "Disabled"] {
                    ui.label(RichText::new(h).strong());
                }
                ui.end_row();
                let mut map = app.session.midi_map.lock();
                for (i, (_, desc, t)) in controls.iter().enumerate() {
                    let Some(it) = map.items.get_mut(i) else { continue };
                    ui.label(desc);
                    ui.label(RichText::new(it.describe()).monospace());
                    ui.horizontal(|ui| {
                        let is_learning = learning == Some(i);
                        if lit_button(ui, is_learning, if is_learning { "Waiting" } else { "Learn" }, ON_YELLOW, 56.0).clicked() {
                            app.session.midi_learn.lock().target = if is_learning { None } else { Some(i) };
                        }
                        if ui.small_button("x").on_hover_text("Clear").clicked() {
                            it.bytes = [0; 6];
                        }
                    });
                    if t.is_button() {
                        ui.label("Button");
                    } else {
                        let cur = match (it.coder, it.coder_type) {
                            (0, _) => 0,
                            (_, 1) => 2,
                            _ => 1,
                        };
                        let names = ["Linear (regular)", "Coder offset", "Coder sign bit (Mackie)"];
                        egui::ComboBox::from_id_salt(format!("ct{i}")).width(150.0).selected_text(names[cur]).show_ui(ui, |ui| {
                            for (k, n) in names.iter().enumerate() {
                                if ui.selectable_label(cur == k, *n).clicked() {
                                    (it.coder, it.coder_type) = match k {
                                        0 => (0, 0),
                                        1 => (1, 0),
                                        _ => (1, 1),
                                    };
                                }
                            }
                        });
                    }
                    ui.checkbox(&mut it.ptt, "");
                    ui.checkbox(&mut it.omni, "");
                    ui.checkbox(&mut it.feed1, "");
                    ui.checkbox(&mut it.disabled, "");
                    ui.end_row();
                }
            });
        });
        ui.label(RichText::new("Click Learn, then move a control on your MIDI device.").color(TEXT_DIM).size(11.0));
    });
}

// ---------------------------------------------------------------- VBAN

fn vban_dlg(app: &mut App, ctx: &egui::Context) {
    window!(app, ctx, vban, "VB-Audio Network Configuration (VBAN)", |ui| {
        let stats = app.session.shared.vban_stats.clone();
        let c = &mut app.session.settings.vban;
        ui.horizontal(|ui| {
            ui.checkbox(&mut c.enabled, RichText::new("VBAN is ON").strong());
            ui.label("Username:");
            ui.text_edit_singleline(&mut c.username);
            if stats.port_error.load(Ordering::Relaxed) {
                ui.label(RichText::new("UDP port already in use!").color(RED));
            }
        });
        ui.label(RichText::new("Incoming Streams:").strong());
        egui::Grid::new("vin").num_columns(8).striped(true).show(ui, |ui| {
            for h in ["On", "Stream Name", "IP Address From", "Port", "Destination", "Net Quality", "SR / ch / format", "Packets / Errors"] {
                ui.label(RichText::new(h).size(11.0).color(TEXT_DIM));
            }
            ui.end_row();
            for (i, s) in c.inputs.iter_mut().enumerate() {
                ui.checkbox(&mut s.enabled, "");
                ui.add(egui::TextEdit::singleline(&mut s.name).desired_width(90.0).char_limit(16));
                ui.add(egui::TextEdit::singleline(&mut s.ip).desired_width(100.0).hint_text("any"));
                ui.add(egui::DragValue::new(&mut s.port));
                egui::ComboBox::from_id_salt(format!("vin{i}")).width(70.0).selected_text(format!("IN #{}", s.strip + 1)).show_ui(ui, |ui| {
                    for k in 0..NUM_STRIPS {
                        ui.selectable_value(&mut s.strip, k, format!("IN #{}", k + 1));
                    }
                });
                egui::ComboBox::from_id_salt(format!("vnq{i}")).width(80.0).selected_text(NQ_NAMES[s.nq as usize % 5]).show_ui(ui, |ui| {
                    for (k, n) in NQ_NAMES.iter().enumerate() {
                        ui.selectable_value(&mut s.nq, k as u8, *n);
                    }
                });
                let st = &stats.inputs[i];
                let sr = st.sr.load(Ordering::Relaxed);
                ui.label(if sr > 0 {
                    format!("{sr} / {} / {}", st.channels.load(Ordering::Relaxed), FORMAT_NAMES.get(st.format.load(Ordering::Relaxed) as usize).unwrap_or(&"?"))
                } else {
                    "-".into()
                });
                ui.label(format!("{} / {}", st.packets.load(Ordering::Relaxed), st.errors.load(Ordering::Relaxed)));
                ui.end_row();
            }
        });
        ui.separator();
        ui.label(RichText::new("Outgoing Streams:").strong());
        egui::Grid::new("vout").num_columns(9).striped(true).show(ui, |ui| {
            for h in ["On", "Stream Name", "IP Address To", "Port", "Source", "Sample Rate", "Ch", "Format", "Packets"] {
                ui.label(RichText::new(h).size(11.0).color(TEXT_DIM));
            }
            ui.end_row();
            for (i, s) in c.outputs.iter_mut().enumerate() {
                ui.checkbox(&mut s.enabled, "");
                ui.add(egui::TextEdit::singleline(&mut s.name).desired_width(90.0).char_limit(16));
                ui.add(egui::TextEdit::singleline(&mut s.ip).desired_width(100.0).hint_text("e.g. 192.168.1.255"));
                ui.add(egui::DragValue::new(&mut s.port));
                egui::ComboBox::from_id_salt(format!("vbus{i}")).width(60.0).selected_text(BUS_NAMES[s.bus.min(NUM_BUSES - 1)]).show_ui(ui, |ui| {
                    for (k, n) in BUS_NAMES.iter().enumerate() {
                        ui.selectable_value(&mut s.bus, k, *n);
                    }
                });
                egui::ComboBox::from_id_salt(format!("vsr{i}")).width(80.0).selected_text(format!("{} Hz", s.sr)).show_ui(ui, |ui| {
                    let mut rates: Vec<u32> = SR_TABLE.iter().copied().filter(|r| (8000..=192000).contains(r)).collect();
                    rates.sort();
                    for r in rates {
                        ui.selectable_value(&mut s.sr, r, format!("{r} Hz"));
                    }
                });
                ui.add(egui::DragValue::new(&mut s.channels).range(1..=8));
                egui::ComboBox::from_id_salt(format!("vfmt{i}")).width(90.0).selected_text(FORMAT_NAMES[s.format as usize % 6]).show_ui(ui, |ui| {
                    for k in [vban::FMT_I16, vban::FMT_I24, vban::FMT_I32, vban::FMT_F32] {
                        ui.selectable_value(&mut s.format, k, FORMAT_NAMES[k as usize]);
                    }
                });
                ui.label(stats.outputs[i].packets.load(Ordering::Relaxed).to_string());
                ui.end_row();
            }
        });
        ui.separator();
        ui.horizontal(|ui| {
            ui.checkbox(&mut c.command_in.enabled, "Incoming VBAN-TEXT commands");
            ui.label("Stream:");
            ui.add(egui::TextEdit::singleline(&mut c.command_in.name).desired_width(90.0));
            ui.label("From IP:");
            ui.add(egui::TextEdit::singleline(&mut c.command_in.ip).desired_width(100.0).hint_text("any"));
            ui.label("Port:");
            ui.add(egui::DragValue::new(&mut c.command_in.port));
        });
        ui.horizontal(|ui| {
            if ui.button(RichText::new("Apply (restart engine)").strong()).clicked() {
                app.session.restart_engine();
            }
            if ui.button("Reset VBAN").clicked() {
                app.session.settings.vban = Default::default();
            }
        });
        ui.separator();
        ui.collapsing("Send a VBAN-TEXT command", |ui| {
            ui.horizontal(|ui| {
                ui.label("To (ip:port):");
                ui.text_edit_singleline(&mut app.vban_text_dest);
            });
            ui.text_edit_singleline(&mut app.vban_text_body);
            if ui.button("Send").clicked() {
                let name = app.session.settings.vban.command_in.name.clone();
                let r = app
                    .vban_text_dest
                    .parse()
                    .map_err(|e: std::net::AddrParseError| e.to_string())
                    .and_then(|a| vban::send_text(a, &name, &app.vban_text_body).map_err(|e| e.to_string()));
                if let Err(e) = r {
                    app.dlg.message = Some(("VBAN".into(), e));
                }
            }
        });
    });
}

// ---------------------------------------------------------------- Presets

fn presets(app: &mut App, ctx: &egui::Context) {
    window!(app, ctx, presets, "Preset Scene", |ui| {
        ui.horizontal(|ui| {
            ui.label("Name for Store:");
            ui.text_edit_singleline(&mut app.preset_name);
            ui.checkbox(&mut app.preset_with_devices, "include device configuration");
        });
        let mut refresh = false;
        egui::Grid::new("presets").num_columns(5).striped(true).show(ui, |ui| {
            for i in 0..NUM_PRESETS {
                let entry = app.session.preset_names.get(i).cloned().flatten();
                let current = app.session.options.last_preset == i as i32;
                let label = match &entry {
                    Some((n, _)) => format!("{:02} - {}", i + 1, n),
                    None => format!("{:02} - Empty Preset", i + 1),
                };
                ui.label(if current { RichText::new(label).color(ACCENT).strong() } else { RichText::new(label) });
                ui.label(RichText::new(entry.as_ref().map(|e| e.1.clone()).unwrap_or_default()).size(10.5).color(TEXT_DIM));
                if ui.add_enabled(entry.is_some(), egui::Button::new("Recall")).clicked() {
                    app.session.recall_preset(i);
                }
                if ui.button(if entry.is_some() { "Overwrite" } else { "Store" }).clicked() {
                    if let Err(e) = app.session.store_preset(i, &app.preset_name, app.preset_with_devices) {
                        app.dlg.message = Some(("Preset".into(), e.to_string()));
                    }
                    refresh = true;
                }
                if ui.add_enabled(entry.is_some(), egui::Button::new("Delete")).clicked() {
                    app.session.delete_preset(i);
                    refresh = true;
                }
                ui.end_row();
            }
        });
        if refresh {
            app.session.refresh_presets();
        }
        ui.label(RichText::new(format!("Scenes are stored in {}", rustbus::presets::preset_dir().display())).size(10.5).color(TEXT_DIM));
    });
}

// ---------------------------------------------------------------- Bus EQ

fn bus_eq(app: &mut App, ctx: &egui::Context, p: &mut MixParams) {
    let Some(b) = app.dlg.bus_eq else { return };
    let mut open = true;
    let title = format!("EQPro-G6 BUS {} ({})", BUS_NAMES[b], if b < 3 { "Physical" } else { "Virtual" });
    egui::Window::new(title).open(&mut open).resizable(false).show(ctx, |ui| {
        let bp = &mut p.buses[b];
        ui.horizontal(|ui| {
            toggle(ui, &mut bp.eq_on, "EQ ON", ON_BLUE, 60.0);
            ui.label("Channel:");
            egui::ComboBox::from_id_salt("eqch").selected_text(if app.eq_channel >= MAX_CH { "All channels".into() } else { format!("Channel {}", app.eq_channel + 1) }).show_ui(ui, |ui| {
                ui.selectable_value(&mut app.eq_channel, MAX_CH, "All channels");
                for c in 0..MAX_CH {
                    ui.selectable_value(&mut app.eq_channel, c, format!("Channel {}", c + 1));
                }
            });
            if ui.button("Reset EQ").clicked() {
                bp.eq = [BusEqChannel::default(); MAX_CH];
            }
        });
        let ch = app.eq_channel.min(MAX_CH - 1);
        let mut edit = bp.eq[ch];
        let before = edit;
        eq_curve(ui, &edit);
        egui::Grid::new("eqcells").num_columns(EQ_CELLS + 1).show(ui, |ui| {
            ui.label("");
            for k in 0..EQ_CELLS {
                ui.label(RichText::new(format!("Band {}", k + 1)).strong());
            }
            ui.end_row();
            ui.label("On");
            for c in edit.cells.iter_mut() {
                ui.checkbox(&mut c.on, "");
            }
            ui.end_row();
            ui.label("Type");
            for (k, c) in edit.cells.iter_mut().enumerate() {
                egui::ComboBox::from_id_salt(format!("eqt{k}")).width(80.0).selected_text(c.kind.label()).show_ui(ui, |ui| {
                    for t in EqType::ALL {
                        ui.selectable_value(&mut c.kind, t, t.label());
                    }
                });
            }
            ui.end_row();
            ui.label("Freq (Hz)");
            for c in edit.cells.iter_mut() {
                let speed = (c.freq * 0.01) as f64;
                ui.add(egui::DragValue::new(&mut c.freq).range(20.0..=20000.0).speed(speed).max_decimals(0));
            }
            ui.end_row();
            ui.label("Gain (dB)");
            for c in edit.cells.iter_mut() {
                ui.add(egui::DragValue::new(&mut c.gain_db).range(-36.0..=18.0).speed(0.1).max_decimals(1));
            }
            ui.end_row();
            ui.label("Q");
            for c in edit.cells.iter_mut() {
                ui.add(egui::DragValue::new(&mut c.q).range(0.3..=100.0).speed(0.02).max_decimals(2));
            }
            ui.end_row();
        });
        ui.horizontal(|ui| {
            ui.label("Trim (dB):");
            ui.add(egui::DragValue::new(&mut edit.trim_db).range(-24.0..=24.0).speed(0.1));
            ui.label("Delay (ms):");
            ui.add(egui::DragValue::new(&mut edit.delay_ms).range(0.0..=500.0).speed(0.1));
        });
        if edit != before {
            if app.eq_channel >= MAX_CH {
                bp.eq = [edit; MAX_CH];
            } else {
                bp.eq[ch] = edit;
            }
        }
        ui.horizontal(|ui| {
            if ui.button("Save EQ Settings...").clicked() {
                if let Some(f) = rfd::FileDialog::new().add_filter("XML", &["xml"]).set_file_name("BusEQ.xml").save_file() {
                    let _ = session::save_bus_eq(&f, p, b);
                }
            }
            if ui.button("Load EQ Settings...").clicked() {
                if let Some(f) = rfd::FileDialog::new().add_filter("XML", &["xml"]).pick_file() {
                    match session::load_bus_eq(&f) {
                        Ok(eq) => p.buses[b].eq = eq,
                        Err(e) => app.dlg.message = Some(("EQ".into(), e)),
                    }
                }
            }
        });
    });
    if !open {
        app.dlg.bus_eq = None;
    }
}

fn eq_curve(ui: &mut Ui, ch: &BusEqChannel) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(560.0, 150.0), egui::Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 4.0, Color32::from_rgb(14, 18, 24));
    let sr = 48000.0;
    let coefs: Vec<Coefs> = ch.cells.iter().map(|c| Coefs::from_cell(sr, c)).collect();
    let (lo, hi) = (20f32.ln(), 20000f32.ln());
    let db_range = 24.0;
    let y_of = |db: f32| rect.center().y - (db / db_range).clamp(-1.0, 1.0) * rect.height() * 0.45;
    for db in [-18.0, -12.0, -6.0, 0.0, 6.0, 12.0, 18.0] {
        let y = y_of(db);
        p.line_segment([Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)], Stroke::new(if db == 0.0 { 1.0f32 } else { 0.5 }, Color32::from_rgb(45, 52, 64)));
    }
    for f in [100.0f32, 1000.0, 10000.0] {
        let x = rect.left() + (f.ln() - lo) / (hi - lo) * rect.width();
        p.line_segment([Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())], Stroke::new(0.5f32, Color32::from_rgb(45, 52, 64)));
    }
    let pts: Vec<Pos2> = (0..=200)
        .map(|i| {
            let f = (lo + (hi - lo) * i as f32 / 200.0).exp();
            let db: f32 = coefs.iter().map(|c| c.magnitude_db(sr, f)).sum::<f32>() + ch.trim_db;
            Pos2::new(rect.left() + rect.width() * i as f32 / 200.0, y_of(db))
        })
        .collect();
    p.add(egui::Shape::line(pts, Stroke::new(2.0f32, ACCENT)));
}

// ---------------------------------------------------------------- Console / About / prompts

fn console(app: &mut App, ctx: &egui::Context) {
    window!(app, ctx, console, "Command Console", |ui| {
        ui.label(RichText::new("Parameter script, e.g.  Strip[0].Gain = -6; Bus[1].mode.TVMix = 1; Recorder.play = 1;").color(TEXT_DIM));
        let r = ui.add(egui::TextEdit::multiline(&mut app.console_input).desired_rows(3).desired_width(f32::INFINITY).code_editor());
        let run = ui.button("Run  (Ctrl+Enter)").clicked() || (r.has_focus() && ui.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::Enter)));
        if run {
            let t = app.console_input.clone();
            app.session.run_script(&t, "console");
        }
        ui.separator();
        egui::ScrollArea::vertical().max_height(220.0).stick_to_bottom(true).show(ui, |ui| {
            for l in &app.session.console_log {
                let col = if l.contains("error") { RED } else { TEXT_DIM };
                ui.label(RichText::new(l).monospace().size(11.0).color(col));
            }
        });
    });
}

fn about(app: &mut App, ctx: &egui::Context) {
    window!(app, ctx, about, "About", |ui| {
        ui.heading("RustBus");
        ui.label(format!("Version {}", env!("CARGO_PKG_VERSION")));
        ui.label("A virtual audio mixer written in Rust — a clean-room reimplementation of Voicemeeter-Banana-style behaviour.");
        ui.label("Behaviour was mapped from the original application; no original code or artwork is used.");
        ui.label("Voicemeeter and VBAN are trademarks of VB-Audio Software.");
        ui.separator();
        ui.label(format!("Settings folder: {}", rustbus::settings::config_dir().display()));
    });
}

fn rename(app: &mut App, ctx: &egui::Context) {
    let Some((is_strip, idx, mut text)) = app.dlg.rename.take() else { return };
    let mut keep = true;
    let defaults = Labels::default();
    let default_name = if is_strip { defaults.strip_display(idx) } else { defaults.bus_display(idx) };
    let what = if is_strip { format!("input strip #{}", idx + 1) } else { format!("bus {}", BUS_NAMES[idx]) };
    egui::Window::new("Edit Label").collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
        ui.label(format!("Name for {what}:"));
        // The original stores labels in 64-character buffers.
        let r = ui.add(egui::TextEdit::singleline(&mut text).char_limit(64).desired_width(240.0).hint_text(&default_name));
        r.request_focus();
        ui.label(RichText::new(format!("Leave empty to restore \"{default_name}\".")).size(10.5).color(TEXT_DIM));
        ui.horizontal(|ui| {
            if ui.button("OK").clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                let t = text.trim();
                // Store an empty label when it equals the default, so defaults stay defaults.
                let stored = if t == default_name { String::new() } else { t.to_string() };
                if is_strip {
                    app.session.settings.labels.strips[idx] = stored;
                } else {
                    app.session.settings.labels.buses[idx] = stored;
                }
                keep = false;
            }
            if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                keep = false;
            }
        });
    });
    if keep {
        app.dlg.rename = Some((is_strip, idx, text));
    }
}

fn message(app: &mut App, ctx: &egui::Context) {
    let Some((title, body)) = app.dlg.message.clone() else { return };
    egui::Window::new(title).collapsible(false).resizable(false).show(ctx, |ui| {
        ui.label(body);
        if ui.button("OK").clicked() {
            app.dlg.message = None;
        }
    });
}

fn confirm_reset(app: &mut App, ctx: &egui::Context) {
    if !app.dlg.confirm_reset {
        return;
    }
    egui::Window::new("Re-Initialization").collapsible(false).resizable(false).show(ctx, |ui| {
        ui.label("Reset Current Settings !\nRustBus will be configured as it was after first installation.\n\nDo you want to continue ?");
        ui.horizontal(|ui| {
            if ui.button("Yes").clicked() {
                app.dlg.confirm_reset = false;
                app.session.apply_settings(Settings::default());
            }
            if ui.button("No").clicked() {
                app.dlg.confirm_reset = false;
            }
        });
    });
}
