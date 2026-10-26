//! Output bus cards of the Master section.

use eframe::egui::{self, Sense, Ui, Vec2};

use rustbus::params::*;

use super::help::help_text;
use super::theme::*;
use super::widgets::*;
use super::{pretty_device, stereo_levels, App};

impl App {
    pub(super) fn bus_ui(&mut self, ui: &mut Ui, b: usize, p: &mut MixParams) {
        let locked = self.session.options.lock_gui;
        let virt = b >= 3;
        let accent = if virt { ACCENT_B } else { ACCENT_A };
        let idle = self.session.settings.device.outputs[b].is_none();
        let inner = BUS_W - 2.0 * STRIP_PAD;
        card().show(ui, |ui| {
            ui.set_width(inner);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing = Vec2::new(8.0, 12.0);
                let bus_title = self.session.settings.labels.bus_display(b);
                let r = ui
                    .horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        ui.set_min_height(20.0);
                        dot(ui, if idle { IDLE_DOT } else { accent });
                        ui.add(egui::Label::new(text(&bus_title, semi(14.0), if idle { TEXT_3 } else { TEXT })).truncate().sense(Sense::click()))
                    })
                    .inner
                    .on_hover_text("Double-click to rename · Right-click for bus menu");
                if r.double_clicked() {
                    self.dlg.rename = Some((false, b, bus_title.clone()));
                }
                r.context_menu(|ui| {
                    if ui.button("Edit Label...").clicked() {
                        self.dlg.rename = Some((false, b, bus_title.clone()));
                        ui.close_menu();
                    }
                    if ui.button("Bus EQ...").clicked() {
                        self.dlg.bus_eq = Some(b);
                        ui.close_menu();
                    }
                });
                self.hover_help(&r, &format!("BUS{}", BUS_NAMES[b]));
                ui.add_space(-6.0);
                mini(ui, if virt { "Virtual" } else { "Physical" }, TEXT_3);

                let dev = self.session.settings.device.outputs[b].as_ref().map(|d| pretty_device(&d.name)).unwrap_or_else(|| "Not assigned".into());
                let r = select(ui, ("out-dev", b), &dev, inner, |ui| self.output_device_menu(ui, b));
                self.hover_help(&r, &format!("BUS{}", BUS_NAMES[b]));

                ui.add_enabled_ui(!locked, |ui| {
                    let bp = &mut p.buses[b];
                    let label = format!("Mode · {}", mode_title(bp.mode));
                    let r = select(ui, ("bus-mode", b), &label, inner, |ui| {
                        for m in BusMode::ALL {
                            if ui.radio(bp.mode == m, mode_title(m)).clicked() {
                                bp.mode = m;
                                close_popup(ui);
                            }
                        }
                    });
                    self.hover_help(&r, "BUS_MIXNORMAL");

                    let (l, rr) = stereo_levels(&self.disp_buses[b], MAX_CH);
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        meter(ui, [l, rr], BUS_METER_H, bp.mute, Some(bp.limit_db));
                        let (fr, _) = fader(ui, &mut bp.gain_db, 0.0, Some(&mut bp.limit_db), BUS_METER_H);
                        let fr = fr.on_hover_text(format!("{} dB  (Ctrl+drag: limiter {:.0} dB)", fmt_db(bp.gain_db), bp.limit_db));
                        if fr.hovered() {
                            self.help = help_text("BUS_FADER").into();
                        }
                    });
                    db_readout(ui, bp.gain_db);

                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        let bs = Vec2::new((inner - 8.0) / 3.0, 28.0);
                        let mono_txt = if bp.mono == 2 { "LR" } else { "M" };
                        let r = flag_button(ui, bs, mono_txt, (bp.mono > 0).then_some(AMBER)).on_hover_text(match bp.mono {
                            1 => "Mono (click again to swap L/R)",
                            2 => "L/R swapped (click to turn off)",
                            _ => "Mono",
                        });
                        if r.clicked() {
                            bp.mono = (bp.mono + 1) % 3;
                        }
                        if r.hovered() {
                            self.help = help_text("BUS_MONO").into();
                        }
                        let r = flag_button(ui, bs, "EQ", bp.eq_on.then_some(ACCENT_A)).on_hover_text("Click: EQ on/off · Right-click: edit EQ");
                        if r.clicked() {
                            bp.eq_on = !bp.eq_on;
                        }
                        if r.secondary_clicked() {
                            self.dlg.bus_eq = Some(b);
                        }
                        if r.hovered() {
                            self.help = help_text("BUS_EQ").into();
                        }
                        let r = mute_icon_button(ui, bs, bp.mute).on_hover_text("Mute");
                        if r.clicked() {
                            bp.mute = !bp.mute;
                        }
                        if r.hovered() {
                            self.help = help_text("BUS_MUTE").into();
                        }
                    });
                });
            });
            self.align_card_bottom(ui);
        });
    }
}

/// Title-case bus mode names for the "Mode · …" select.
fn mode_title(m: BusMode) -> &'static str {
    match m {
        BusMode::Normal => "Normal",
        BusMode::MixDownA => "Mix-down A",
        BusMode::MixDownB => "Mix-down B",
        BusMode::Repeat => "Stereo repeat",
        BusMode::Composite => "Composite",
        BusMode::TvMix => "TV mix",
        BusMode::UpMix21 => "Up-mix 2.1",
        BusMode::UpMix41 => "Up-mix 4.1",
        BusMode::UpMix61 => "Up-mix 6.1",
        BusMode::LeftOnly => "Left only",
        BusMode::RightOnly => "Right only",
        BusMode::CenterOnly => "Center only",
        BusMode::LfeOnly => "LFE only",
        BusMode::RearOnly => "Rear only",
    }
}
