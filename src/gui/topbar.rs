//! Top bar: brand, main menu, VBAN button, status pill, hardware-out pills and the device menus.

use eframe::egui::{self, Color32, RichText, Rounding, Sense, Stroke, Ui, Vec2};

use rustbus::params::*;
use rustbus::settings::{DeviceRef, DEV_ASIO};

use super::theme::*;
use super::widgets::*;
use super::{desktop, pretty_device, short, App};

impl App {
    pub(super) fn top_bar(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 16.0;
            // Brand.
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                let r = logo(ui).on_hover_text("About RustBus");
                if r.clicked() {
                    self.dlg.about = true;
                }
                self.hover_help(&r, "BUTNAME");
                ui.label(text("RustBus", bold(17.0), TEXT));
                ui.label(text("Virtual audio mixer", regular(12.0), TEXT_3));
            });
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                // The main menu keeps egui's menu (submenus, checkboxes), styled as a ghost button.
                ui.scope(|ui| {
                    let w = &mut ui.visuals_mut().widgets;
                    for v in [&mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
                        v.rounding = Rounding::same(8.0);
                        v.bg_stroke = Stroke::new(1.0f32, BORDER_CTL);
                        v.fg_stroke.color = GHOST_TEXT;
                        v.weak_bg_fill = GHOST_HOVER;
                    }
                    w.inactive.weak_bg_fill = GHOST;
                    ui.spacing_mut().interact_size.y = 32.0;
                    ui.spacing_mut().button_padding = egui::vec2(12.0, 6.0);
                    let menu = ui.menu_button(text("☰  Menu", medium(13.0), GHOST_TEXT), |ui| self.main_menu(ui, ctx));
                    self.hover_help(&menu.response, "MENU");
                });
                let v = ghost_button(ui, "VBAN", None, self.session.settings.vban.enabled);
                if v.clicked() {
                    self.dlg.vban = !self.dlg.vban;
                }
                self.hover_help(&v, "BUTVBAN");
            });
            self.status_pill(ui);
            if self.session.options.lock_gui {
                let r = ui.label(text("Locked", semi(12.0), RED));
                self.hover_help(&r, "LOCKICON");
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                for b in (0..3).rev() {
                    self.hw_out_pill(ui, b);
                }
                ui.add_space(4.0);
                mini(ui, "Hardware out", TEXT_3);
            });
        });
    }

    /// Status pill: dot, sample rate, clock source. Amber when the engine reports problems.
    fn status_pill(&mut self, ui: &mut Ui) {
        let st = self.session.status();
        let warn = !st.errors.is_empty() || !st.running;
        let (bg, border, dotc) = if warn { (WARN_BG, WARN_BORDER, AMBER) } else { (STATUS_BG, STATUS_BORDER, ACCENT_A) };
        let source = if st.running { format!("Clock · {}", st.master.replacen(": ", " ", 1)) } else { "Audio engine stopped".into() };
        let resp = egui::Frame::none()
            .fill(bg)
            .stroke(Stroke::new(1.0f32, border))
            .rounding(8.0)
            .inner_margin(egui::Margin::symmetric(12.0, 0.0))
            .show(ui, |ui| {
                ui.set_height(32.0);
                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(7.0), Sense::hover());
                    ui.painter().circle_filled(r.center(), 6.5, dotc.gamma_multiply(0.18));
                    ui.painter().circle_filled(r.center(), 3.5, dotc);
                    ui.label(text(khz(st.sample_rate), mono_semi(12.0), dotc));
                    ui.label(text("|", regular(12.0), BORDER_HOVER));
                    ui.label(text(source, regular(12.0), TEXT_2));
                });
            })
            .response;
        if !st.errors.is_empty() {
            resp.on_hover_text(st.errors.join("\n"));
        }
    }

    /// "A1 · device" pill in the top bar; teal badge when assigned. Click opens the device list.
    fn hw_out_pill(&mut self, ui: &mut Ui, b: usize) {
        let name = self.session.settings.device.outputs[b].as_ref().map(|d| pretty_device(&d.name));
        let assigned = name.is_some();
        let label = name.unwrap_or_else(|| "Not assigned".into());
        let font = regular(12.0);
        let tw = ui.fonts(|f| f.layout_no_wrap(label.clone(), font.clone(), TEXT).size().x).min(190.0);
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(8.0 + 26.0 + 8.0 + tw + 10.0, 32.0), Sense::click());
        let popup_id = ui.make_persistent_id(("hw-out", b));
        let open = ui.memory(|m| m.is_popup_open(popup_id));
        ui.painter().rect(rect, Rounding::same(8.0), if resp.hovered() || open { GHOST_HOVER } else { GHOST }, Stroke::new(1.0f32, BORDER_CTL));
        let badge = egui::Rect::from_min_size(egui::pos2(rect.left() + 8.0, rect.center().y - 9.0), Vec2::new(26.0, 18.0));
        ui.painter().rect_filled(badge, 5.0, if assigned { ACCENT_A } else { BORDER_CTL });
        ui.painter().text(badge.center(), egui::Align2::CENTER_CENTER, BUS_NAMES[b], mono_semi(11.0), if assigned { BG } else { Color32::from_rgb(0x9A, 0xA3, 0xB0) });
        paint_text(ui, egui::pos2(badge.right() + 8.0, rect.center().y), egui::Align2::LEFT_CENTER, &label, font, GHOST_TEXT, tw + 1.0);
        if resp.clicked() {
            ui.memory_mut(|m| m.toggle_popup(popup_id));
        }
        egui::popup::popup_below_widget(ui, popup_id, &resp, egui::PopupCloseBehavior::CloseOnClickOutside, |ui| {
            ui.set_min_width(260.0);
            self.output_device_menu(ui, b);
        });
        self.hover_help(&resp, ["BUSA1", "BUSA2", "BUSA3"][b]);
    }

    fn main_menu(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        if ui.button("Restart Audio Engine\t[CTRL+R]").clicked() {
            self.session.restart_engine();
            ui.close_menu();
        }
        ui.checkbox(&mut self.session.options.auto_restart, "Auto Restart Audio Engine (on device error)");
        ui.separator();
        if ui.button("Mic Hotkey (Push to Talk)...").clicked() {
            self.dlg.hotkey = true;
            ui.close_menu();
        }
        ui.checkbox(&mut self.session.options.sys_tray, "Close to System Tray (keep audio running)");
        let mut auto = desktop::autostart_enabled();
        if ui.checkbox(&mut auto, "Start with Windows (in tray)").changed() {
            if let Err(e) = desktop::set_autostart(auto) {
                self.dlg.message = Some(("Start with Windows".into(), e));
            } else if auto {
                // Starting hidden only makes sense if closing keeps it in the tray.
                self.session.options.sys_tray = true;
            }
        }
        if ui.button("Eject Cassette (release audio file)").clicked() {
            self.session.shared.tape.eject();
            ui.close_menu();
        }
        ui.separator();
        if ui.button("Load Settings...").clicked() {
            ui.close_menu();
            self.load_settings_dialog(false);
        }
        if ui.button("Load Settings and set as startup settings...").clicked() {
            ui.close_menu();
            self.load_settings_dialog(true);
        }
        ui.checkbox(&mut self.session.options.use_startup_file, format!("Load Settings on Startup: {}", if self.session.options.startup_file.is_empty() { "-".into() } else { short(&self.session.options.startup_file) }));
        if ui.button("Save Settings...").clicked() {
            ui.close_menu();
            self.save_settings_dialog();
        }
        if ui.button("Reset Settings (Re-Initialization)").clicked() {
            self.dlg.confirm_reset = true;
            ui.close_menu();
        }
        ui.separator();
        if ui.checkbox(&mut self.session.options.show_on_top, "Set as Always Visible").changed() {
            let lvl = if self.session.options.show_on_top { egui::WindowLevel::AlwaysOnTop } else { egui::WindowLevel::Normal };
            ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(lvl));
        }
        ui.checkbox(&mut self.session.options.lock_gui, "Lock Graphic User Interface");
        ui.checkbox(&mut self.session.options.limit_remote_gain, "Limit Remote Gain to 0 dBfs");
        ui.checkbox(&mut self.session.options.show_caption_help, "Show Contextual Help In Caption");
        ui.checkbox(&mut self.session.options.show_preset_scene, "Show Preset Scene On Startup");
        ui.menu_button("Zoom", |ui| {
            for z in [1.0f32, 0.9, 0.75, 0.6, 0.5] {
                if ui.radio(self.session.options.zoom == z, format!("Zoom {}%", (z * 100.0) as i32)).clicked() {
                    self.session.options.zoom = z;
                    ctx.set_zoom_factor(z);
                }
            }
        });
        ui.separator();
        for (label, flag) in [
            ("Preset Scene...\t[CTRL+4]", &mut self.dlg.presets),
            ("System Settings...\t[CTRL+',']", &mut self.dlg.system),
            ("Tape Recorder Options...\t[CTRL+5]", &mut self.dlg.recorder),
            ("M.I.D.I. Mapping...\t[CTRL+3]", &mut self.dlg.midi),
            ("VBAN Options... (VB-Audio Network)\t[CTRL+1]", &mut self.dlg.vban),
            ("Command Console (parameter scripts)...", &mut self.dlg.console),
            ("Latency Monitor...", &mut self.dlg.latency),
        ] {
            if ui.button(label).clicked() {
                *flag = true;
                ui.close_menu();
            }
        }
        ui.menu_button("Windows Audio Settings...", |ui| {
            for (label, cmd, arg) in [
                ("Windows Sound Settings...", "explorer.exe", "ms-settings:sound"),
                ("Windows Apps Volume and Device Preferences...", "explorer.exe", "ms-settings:apps-volume"),
                ("Windows Privacy for Microphone...", "explorer.exe", "ms-settings:privacy-microphone"),
                ("Windows Sounds Playback Devices...", "control.exe", "mmsys.cpl,,0"),
                ("Windows Sounds Recording Devices...", "control.exe", "mmsys.cpl,,1"),
            ] {
                if ui.button(label).clicked() {
                    let _ = std::process::Command::new(cmd).arg(arg).spawn();
                    ui.close_menu();
                }
            }
        });
        if ui.button("Refresh Device Lists\t[F5]").clicked() {
            self.session.refresh_devices();
            self.session.refresh_midi_ports();
            ui.close_menu();
        }
        ui.separator();
        if ui.button("About Box...").clicked() {
            self.dlg.about = true;
            ui.close_menu();
        }
        if ui.button("Shut Down").clicked() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    pub(super) fn input_device_menu(&mut self, ui: &mut Ui, s: usize) {
        ui.label(RichText::new(format!("Select Input Device #{}", s + 1)).strong());
        if ui.button("- Remove Selection -").clicked() {
            self.session.set_strip_device(s, None);
            close_popup(ui);
        }
        ui.separator();
        let cur = self.session.settings.device.inputs[s].clone();
        // ASIO inputs: channels of the driver running A1 (hardware strips only, like the original).
        if !is_virtual_strip(s) {
            let asio = self.session.shared.status.lock().asio.clone();
            let a1_is_asio = self.session.settings.device.outputs[0].as_ref().is_some_and(|d| d.kind == DEV_ASIO);
            if let Some(info) = asio {
                let patch = self.session.settings.device.asio_patch[s];
                let is_cur = |l: usize, r: usize| cur.as_ref().is_some_and(|d| d.kind == DEV_ASIO) && patch == [l as i32, r as i32];
                Self::menu_header(ui, &format!("ASIO · {}", info.driver));
                let n = info.inputs.len();
                let mut choices: Vec<(usize, usize)> = (0..n / 2).map(|k| (2 * k, 2 * k + 1)).collect();
                choices.extend((0..n).map(|i| (i, i)));
                for (l, r) in choices {
                    let label = if l == r { format!("ASIO: {} (mono)", info.inputs[l]) } else { format!("ASIO: {} + {}", info.inputs[l], info.inputs[r]) };
                    if ui.radio(is_cur(l, r), label).clicked() {
                        self.session.settings.device.asio_patch[s] = [l as i32, r as i32];
                        self.session.settings.device.inputs[s] = Some(DeviceRef { kind: DEV_ASIO, name: info.driver.clone() });
                        self.session.request_restart();
                        close_popup(ui);
                    }
                }
                ui.separator();
            } else if a1_is_asio {
                ui.label(RichText::new("ASIO inputs appear here once A1's ASIO driver is running.").size(11.0).color(TEXT_3));
                ui.separator();
            } else if !self.session.asio_drivers.is_empty() {
                ui.label(RichText::new("For ASIO inputs, set A1 to an ASIO driver first.").size(11.0).color(TEXT_3));
                ui.separator();
            }
        }
        Self::menu_header(ui, "Windows audio (WDM)");
        for name in self.session.devices.0.clone() {
            let on = cur.as_ref().is_some_and(|d| d.kind != DEV_ASIO && d.name == name);
            if ui.radio(on, format!("WDM: {name}")).clicked() {
                self.session.set_strip_device(s, Some(name));
                close_popup(ui);
            }
        }
    }

    pub(super) fn output_device_menu(&mut self, ui: &mut Ui, b: usize) {
        ui.label(RichText::new(format!("Select {} Output Device", BUS_NAMES[b])).strong());
        if ui.button("- Remove Selection -").clicked() {
            self.session.set_bus_device(b, None);
            close_popup(ui);
        }
        ui.separator();
        let cur = self.session.settings.device.outputs[b].clone();
        // ASIO drives the master clock, so (as in the original) only A1 can use it.
        if b == 0 && !self.session.asio_drivers.is_empty() {
            Self::menu_header(ui, "ASIO (lowest latency, A1 only)");
            for name in self.session.asio_drivers.clone() {
                let on = cur.as_ref().is_some_and(|d| d.kind == DEV_ASIO && d.name == name);
                if ui.radio(on, format!("ASIO: {name}")).clicked() {
                    self.session.settings.device.outputs[0] = Some(DeviceRef { kind: DEV_ASIO, name });
                    self.session.request_restart();
                    close_popup(ui);
                }
            }
            ui.separator();
        }
        Self::menu_header(ui, "Windows audio (WDM)");
        for name in self.session.devices.1.clone() {
            let on = cur.as_ref().is_some_and(|d| d.kind != DEV_ASIO && d.name == name);
            if ui.radio(on, format!("WDM: {name}")).clicked() {
                self.session.set_bus_device(b, Some(name));
                close_popup(ui);
            }
        }
    }

    /// Small uppercase section title inside device menus.
    fn menu_header(ui: &mut Ui, title: &str) {
        ui.label(caps(title, semi(10.0), TEXT_3, 0.6));
    }
}
