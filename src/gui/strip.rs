//! Input strip cards (hardware and virtual) and the strip context menu.

use eframe::egui::{self, RichText, Sense, Ui, Vec2};

use rustbus::hotkey;
use rustbus::params::*;
use rustbus::settings::DEV_ASIO;

use super::help::help_text;
use super::theme::*;
use super::widgets::*;
use super::{pretty_device, stereo_levels, App};

impl App {
    /// Strip device caption: the WDM device name, or the patched ASIO channels.
    fn input_caption(&self, s: usize) -> String {
        match &self.session.settings.device.inputs[s] {
            None => "No device".into(),
            Some(d) if d.kind == DEV_ASIO => {
                let [l, r] = self.session.settings.device.asio_patch[s].map(|c| c.max(0) as usize);
                let st = self.session.shared.status.lock();
                let name = |c: usize| st.asio.as_ref().and_then(|a| a.inputs.get(c).cloned()).unwrap_or_else(|| format!("In {}", c + 1));
                if l == r {
                    format!("{} · ASIO", name(l))
                } else {
                    format!("{} + {} · ASIO", name(l), name(r))
                }
            }
            Some(d) => pretty_device(&d.name),
        }
    }

    pub(super) fn strip_ui(&mut self, ui: &mut Ui, s: usize, p: &mut MixParams) {
        let virt = is_virtual_strip(s);
        let locked = self.session.options.lock_gui;
        let accent = if virt { ACCENT_B } else { ACCENT_A };
        let idle = self.session.settings.device.inputs[s].is_none();
        let inner = STRIP_W - 2.0 * STRIP_PAD;
        card().show(ui, |ui| {
            ui.set_width(inner);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing = Vec2::new(8.0, 12.0);

                // Head: dot + name. Double-click renames, right-click opens the strip menu.
                let title = self.session.settings.labels.strip_display(s);
                let r = ui
                    .horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        ui.set_min_height(20.0);
                        dot(ui, if idle { IDLE_DOT } else { accent });
                        ui.add(egui::Label::new(text(&title, semi(14.0), if idle { TEXT_3 } else { TEXT })).truncate().sense(Sense::click()))
                    })
                    .inner
                    .on_hover_text("Double-click to rename · Right-click for strip menu");
                if r.double_clicked() {
                    self.dlg.rename = Some((true, s, title.clone()));
                }
                r.context_menu(|ui| self.strip_context_menu(ui, s, p));
                self.hover_help(&r, &format!("STRIP{s}"));

                // Device select.
                let dev = self.input_caption(s);
                let r = select(ui, ("in-dev", s), &dev, inner, |ui| self.input_device_menu(ui, s));
                self.hover_help(&r, &format!("STRIP{s}"));

                ui.add_enabled_ui(!locked, |ui| {
                    let sp = &mut p.strips[s];
                    // Knobs: hardware Comp / Gate, virtual Treble / Mid / Bass.
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        if virt {
                            let w = (inner - 8.0) / 3.0;
                            let fmt = |v: f32| format!("{} dB", fmt_db(v));
                            for (band, name, id) in [(2, "Treble", "STRIP_EQHI"), (1, "Mid", "STRIP_EQMED"), (0, "Bass", "STRIP_EQLO")] {
                                let (r, _) = knob(ui, &mut sp.eq_gain[band], (-12.0, 12.0), 0.0, name, 36.0, w, ACCENT_B, &fmt);
                                if r.hovered() {
                                    self.help = help_text(id).into();
                                }
                            }
                        } else {
                            let w = (inner - 4.0) / 2.0;
                            let fmt = |v: f32| format!("{v:.1}");
                            let mut c = sp.comp.knob();
                            let (r, ch) = knob(ui, &mut c, (0.0, 10.0), 0.0, "Comp", 40.0, w, ACCENT_A, &fmt);
                            if ch {
                                sp.comp.set_knob(c);
                            }
                            if r.hovered() {
                                self.help = help_text("STRIP_COMP").into();
                            }
                            let mut g = sp.gate.knob();
                            let (r, ch) = knob(ui, &mut g, (0.0, 10.0), 0.0, "Gate", 40.0, w, ACCENT_A, &fmt);
                            if ch {
                                sp.gate.set_knob(g);
                            }
                            if r.hovered() {
                                self.help = help_text("STRIP_GATE").into();
                            }
                        }
                    });

                    // Pan: label (+ Color/3D/FX on hardware) and the XY pad.
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 6.0;
                        ui.horizontal(|ui| {
                            ui.set_width(inner);
                            mini(ui, "Pan", TEXT_3);
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if virt {
                                    mini(ui, "Stereo", TEXT_FAINT);
                                } else {
                                    let types = [PanelType::Color, PanelType::Position3D, PanelType::Modulation];
                                    let cur = types.iter().position(|t| *t == sp.panel_type).unwrap_or(1);
                                    if let Some(i) = segmented(ui, &["Color", "3D", "FX"], cur) {
                                        sp.panel_type = types[i];
                                    }
                                }
                            });
                        });
                        let (r, help) = if virt {
                            (pan_pad(ui, &mut sp.pan_x, &mut sp.pan_y, (-0.5, 0.5), (-0.5, 0.5), inner, PUCK_B).0, "STRIP_PANPOT")
                        } else {
                            match sp.panel_type {
                                PanelType::Color => (pan_pad(ui, &mut sp.color_x, &mut sp.color_y, (-0.5, 0.5), (-0.5, 0.5), inner, TEXT).0, "STRIP_PANELC"),
                                PanelType::Position3D => (pan_pad(ui, &mut sp.pan_x, &mut sp.pan_y, (-0.5, 0.5), (-0.5, 0.5), inner, TEXT).0, "STRIP_PANELP"),
                                PanelType::Modulation => (pan_pad(ui, &mut sp.fx_x, &mut sp.fx_y, (-0.5, 0.5), (0.0, 1.0), inner, TEXT).0, "STRIP_PANELM"),
                            }
                        };
                        if r.hovered() {
                            self.help = help_text(help).into();
                        }
                    });

                    // Routing grid A1 A2 A3 / B1 B2 B3.
                    let (_, hov) = route_grid(ui, &mut sp.route, &BUS_NAMES, 3, inner);
                    if let Some(b) = hov {
                        self.help = help_text(if b < 3 { "STRIP_A1" } else { "STRIP_B1" }).into();
                    }

                    // Fader row: meter, fader, side column.
                    let (l, r) = stereo_levels(&self.disp_strips[s], strip_channels(s));
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        meter(ui, [l, r], INPUT_METER_H, sp.mute, Some(sp.limit_db));
                        let (fr, _) = fader(ui, &mut sp.gain_db, 0.0, Some(&mut sp.limit_db), INPUT_METER_H);
                        let fr = fr.on_hover_text(format!("{} dB  (Ctrl+drag: limiter {:.0} dB)", fmt_db(sp.gain_db), sp.limit_db));
                        if fr.hovered() {
                            self.help = help_text("STRIP_FADER").into();
                        }
                        let side_w = inner - 12.0 - 24.0 - 16.0;
                        ui.vertical(|ui| {
                            ui.set_width(side_w);
                            ui.spacing_mut().item_spacing.y = 5.0;
                            db_readout(ui, sp.gain_db);
                            let bs = Vec2::new(side_w, 28.0);
                            let r = flag_button(ui, bs, "Mono", sp.mono.then_some(AMBER));
                            if r.clicked() {
                                sp.mono = !sp.mono;
                            }
                            if r.hovered() {
                                self.help = help_text("STRIP_MONO").into();
                            }
                            let r = flag_button(ui, bs, "Solo", sp.solo.then_some(AMBER));
                            if r.clicked() {
                                sp.solo = !sp.solo;
                            }
                            if r.hovered() {
                                self.help = help_text("STRIP_SOLO").into();
                            }
                            let mut r = flag_button(ui, bs, "Mute", sp.mute.then_some(RED));
                            let hk = self.session.options.hotkey;
                            if hk.active() && hk.strip == s {
                                r = r.on_hover_text(format!("{} on {}", hk.mode.label(), hotkey::key_name(hk.vk)));
                            }
                            if r.clicked() {
                                sp.mute = !sp.mute;
                            }
                            if r.hovered() {
                                self.help = help_text("STRIP_MUTE").into();
                            }
                            if !virt {
                                let r = flag_button(ui, bs, "Denoise", sp.denoise_on.then_some(ACCENT_A))
                                    .on_hover_text(format!("Noise removal {:.0}% · right-click to change", sp.denoise * 100.0));
                                if r.clicked() {
                                    sp.denoise_on = !sp.denoise_on;
                                }
                                if r.hovered() {
                                    self.help = help_text("STRIP_DENOISE").into();
                                }
                                r.context_menu(|ui| {
                                    ui.label(RichText::new("Noise removal amount").strong());
                                    let mut pct = sp.denoise * 100.0;
                                    if ui.add(egui::Slider::new(&mut pct, 0.0..=100.0).suffix(" %").fixed_decimals(0)).changed() {
                                        sp.denoise = pct / 100.0;
                                    }
                                    ui.checkbox(&mut sp.denoise_on, "Enabled");
                                });
                            }
                            if s == NUM_HW_STRIPS {
                                let r = flag_button(ui, bs, "M.C.", sp.mute_center.then_some(AMBER));
                                if r.clicked() {
                                    sp.mute_center = !sp.mute_center;
                                }
                                if r.hovered() {
                                    self.help = help_text("STRIP_MC").into();
                                }
                            } else if s == NUM_HW_STRIPS + 1 {
                                let txt = if sp.karaoke == 0 { "K".to_string() } else { format!("K{}", sp.karaoke) };
                                let r = flag_button(ui, bs, &txt, (sp.karaoke > 0).then_some(AMBER));
                                if r.clicked() {
                                    sp.karaoke = (sp.karaoke + 1) % 5;
                                }
                                if r.secondary_clicked() {
                                    sp.karaoke = 0;
                                }
                                if r.hovered() {
                                    self.help = help_text("STRIP_KARAOKE").into();
                                }
                            }
                        });
                    });
                });
            });
            self.align_card_bottom(ui);
        });
    }

    fn strip_context_menu(&mut self, ui: &mut Ui, s: usize, p: &mut MixParams) {
        ui.label(RichText::new(format!("Strip #{}", s + 1)).strong());
        if ui.button("Edit Label...").clicked() {
            self.dlg.rename = Some((true, s, self.session.settings.labels.strip_display(s)));
            ui.close_menu();
        }
        ui.separator();
        if ui.button("Copy All").clicked() {
            self.clipboard_strip = Some(p.strips[s]);
            ui.close_menu();
        }
        if ui.add_enabled(self.clipboard_strip.is_some(), egui::Button::new("Paste")).clicked() {
            if let Some(c) = self.clipboard_strip {
                let route = p.strips[s].route;
                p.strips[s] = c;
                p.strips[s].route = route;
            }
            ui.close_menu();
        }
        ui.separator();
        let d = StripParams::default();
        if ui.button("Reset Strip").clicked() {
            p.strips[s] = StripParams { route: p.strips[s].route, ..d };
            ui.close_menu();
        }
        if ui.button("Reset EQUALIZER").clicked() {
            p.strips[s].eq_gain = [0.0; 3];
            p.strips[s].color_x = 0.0;
            p.strips[s].color_y = 0.0;
            ui.close_menu();
        }
        if ui.button("Reset Pan Pot").clicked() {
            p.strips[s].pan_x = 0.0;
            p.strips[s].pan_y = 0.0;
            ui.close_menu();
        }
        if ui.button("Reset INTELLIPAN").clicked() {
            let sp = &mut p.strips[s];
            (sp.pan_x, sp.pan_y, sp.color_x, sp.color_y, sp.fx_x, sp.fx_y) = (0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
            ui.close_menu();
        }
        if ui.button("Reset Compressor").clicked() {
            p.strips[s].comp = d.comp;
            ui.close_menu();
        }
        if ui.button("Reset Gate").clicked() {
            p.strips[s].gate = d.gate;
            ui.close_menu();
        }
    }
}
