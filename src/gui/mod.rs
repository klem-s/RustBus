//! Main window and application state.

mod desktop;
mod dialogs;
mod help;
mod theme;
mod widgets;

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use eframe::egui::{self, Color32, RichText, Rounding, Sense, Stroke, Ui, Vec2};

use rustbus::engine::mixer::Meters;
use rustbus::hotkey;
use rustbus::options::AppOptions;
use rustbus::params::*;
use rustbus::script::TapeCmd;
use rustbus::session::Session;
use rustbus::settings::{self, DeviceRef, DEV_ASIO};

use help::help_text;
use theme::*;
use widgets::*;

pub fn run() -> eframe::Result<()> {
    // A second copy would fight the first over the audio devices: show that one instead.
    if desktop::activate_existing_instance() {
        return Ok(());
    }
    // Carry settings over from the pre-rename "VoicemeeterRS" folder on first start.
    let _ = settings::migrate_legacy_config();
    let options = AppOptions::load();
    let mut vp = egui::ViewportBuilder::default()
        // Close to the fitted size; `fit_window` sets the exact size on the first frames.
        .with_inner_size([1912.0, 740.0])
        .with_resizable(false)
        .with_maximize_button(false)
        .with_title("RustBus");
    if let Some((x, y)) = options.win_pos {
        vp = vp.with_position([x, y]);
    }
    let native = eframe::NativeOptions { viewport: vp, ..Default::default() };
    eframe::run_native("RustBus", native, Box::new(move |cc| Ok(Box::new(App::new(cc, options)))))
}

#[derive(Default)]
pub(crate) struct Dialogs {
    pub system: bool,
    pub recorder: bool,
    pub midi: bool,
    pub vban: bool,
    pub presets: bool,
    pub console: bool,
    pub about: bool,
    pub bus_eq: Option<usize>,
    pub rename: Option<(bool, usize, String)>,
    pub message: Option<(String, String)>,
    pub confirm_reset: bool,
    pub hotkey: bool,
}

pub(crate) struct App {
    pub session: Session,
    pub dlg: Dialogs,
    disp_strips: [[f32; MAX_CH]; NUM_STRIPS],
    disp_buses: [[f32; MAX_CH]; NUM_BUSES],
    help: String,
    pub console_input: String,
    pub clipboard_strip: Option<StripParams>,
    pub preset_with_devices: bool,
    pub vban_text_dest: String,
    pub vban_text_body: String,
    pub preset_name: String,
    /// Bus EQ dialog channel selection (MAX_CH = all channels).
    pub eq_channel: usize,
    tray: Option<desktop::Tray>,
    /// "Press a key" capture in the hotkey dialog: keys already down when it started.
    pub hotkey_capture: Option<Vec<u16>>,
    /// Started by the Windows startup entry: hide to the tray on the first frame.
    start_hidden: bool,
    /// Shared bottom edge of all strip and bus cards (last frame / being measured).
    cards_bottom: f32,
    cards_bottom_next: f32,
    /// Window inner size last sent by `fit_window`.
    fitted_size: Option<Vec2>,
}

impl App {
    fn new(cc: &eframe::CreationContext, options: AppOptions) -> Self {
        theme::apply(&cc.egui_ctx);
        cc.egui_ctx.set_zoom_factor(options.zoom);
        let ctx = cc.egui_ctx.clone();
        let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(move || ctx.request_repaint());

        let (settings, startup_error) = Session::startup_settings(&options);
        let mut session = Session::new(settings, options, wake);
        session.refresh_devices();
        session.refresh_midi_ports();
        let mut app = App {
            session,
            dlg: Dialogs::default(),
            disp_strips: [[0.0; MAX_CH]; NUM_STRIPS],
            disp_buses: [[0.0; MAX_CH]; NUM_BUSES],
            help: String::new(),
            console_input: String::new(),
            clipboard_strip: None,
            preset_with_devices: false,
            vban_text_dest: "127.0.0.1:6980".into(),
            vban_text_body: "Strip[0].Mute = 1;".into(),
            preset_name: "Scene".into(),
            eq_channel: MAX_CH,
            tray: None,
            hotkey_capture: None,
            start_hidden: std::env::args().any(|a| a == desktop::TRAY_ARG),
            cards_bottom: 0.0,
            cards_bottom_next: 0.0,
            fitted_size: None,
        };
        if let Some(h) = desktop::hwnd_of(cc) {
            desktop::set_window(h);
        }
        app.tray = desktop::Tray::new(cc.egui_ctx.clone(), app.session.shared.params.clone(), app.session.mic_strip.clone());
        desktop::refresh_autostart();
        app.session.refresh_presets();
        app.session.restart_engine();
        app.session.restart_midi();
        app.session.restart_hotkey();
        if let Some(e) = startup_error {
            app.dlg.message = Some(("Load Error".into(), format!("Cannot Load settings from this file\n{e}")));
        }
        if app.session.options.show_on_top {
            cc.egui_ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(egui::WindowLevel::AlwaysOnTop));
        }
        app.dlg.presets = app.session.options.show_preset_scene;
        app
    }

    // ------------------------------------------------------------ engine / io

    /// Size the window to the mixer (top bar + panels + status line), clamped to the monitor.
    fn fit_window(&mut self, ctx: &egui::Context, content: Vec2, status_h: f32) {
        let mut want = Vec2::new(content.x + 48.0, 56.0 + 20.0 + content.y + 24.0 + status_h).round();
        if let Some(m) = ctx.input(|i| i.viewport().monitor_size) {
            // Leave room for the taskbar and the title bar.
            want = want.min(m - Vec2::new(0.0, 80.0));
        }
        if self.fitted_size.map_or(true, |s| (s - want).length() > 1.0) {
            self.fitted_size = Some(want);
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(want));
        }
    }

    /// Stretch a strip/bus card's content to the shared bottom line. Call last inside the card.
    fn align_card_bottom(&mut self, ui: &mut Ui) {
        let natural = ui.min_rect().bottom() + STRIP_PAD;
        self.cards_bottom_next = self.cards_bottom_next.max(natural);
        let h = self.cards_bottom - STRIP_PAD - ui.min_rect().top();
        if h > ui.min_rect().height() {
            ui.set_min_height(h);
        }
    }

    /// Close-to-tray, tray "Exit" and the hidden start from the Windows startup entry.
    fn tray_and_close(&mut self, ctx: &egui::Context) {
        if self.start_hidden && self.tray.is_some() {
            // eframe shows the window after its first frames; hide once that has happened.
            if ctx.cumulative_pass_nr() >= 2 {
                self.start_hidden = false;
                desktop::hide_window();
            } else {
                ctx.request_repaint();
            }
        }
        let close = ctx.input(|i| i.viewport().close_requested());
        if desktop::QUIT.load(Ordering::Relaxed) {
            if !close {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        } else if close && self.session.options.sys_tray && self.tray.is_some() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            // Frames stop while hidden, so save now rather than relying on the autosave.
            self.session.save_now();
            desktop::hide_window();
        }
    }

    pub fn load_settings_dialog(&mut self, as_startup: bool) {
        if let Some(p) = rfd::FileDialog::new().add_filter("XML", &["xml"]).set_title("Load Settings...").pick_file() {
            match self.session.load_settings(&p) {
                Ok(()) => {
                    if as_startup {
                        self.session.options.use_startup_file = true;
                        self.session.options.startup_file = p.to_string_lossy().into();
                        self.session.options.save();
                    }
                }
                Err(e) => self.dlg.message = Some(("Load Error".into(), format!("Cannot Load settings from this file\n{e}"))),
            }
        }
    }

    pub fn save_settings_dialog(&mut self) {
        if let Some(p) = rfd::FileDialog::new().add_filter("XML", &["xml"]).set_file_name("RustBus_Settings.xml").set_title("Save Current Settings...").save_file() {
            if let Err(e) = self.session.save_settings_to(&p) {
                self.dlg.message = Some(("Save Error".into(), e));
            }
        }
    }

    fn poll_background(&mut self, ctx: &egui::Context) {
        self.session.tick();
        self.take_session_notices();
        // Meters.
        let m = &self.session.shared.meters;
        for s in 0..NUM_STRIPS {
            for c in 0..MAX_CH {
                let v = Meters::take(&m.strips[s][c]);
                self.disp_strips[s][c] = v.max(self.disp_strips[s][c] * 0.82);
            }
        }
        for b in 0..NUM_BUSES {
            for c in 0..MAX_CH {
                let v = Meters::take(&m.buses[b][c]);
                self.disp_buses[b][c] = v.max(self.disp_buses[b][c] * 0.82);
            }
        }
        ctx.request_repaint_after(Duration::from_millis(33));
    }

    /// Show what the session asked the user about (an error message, the reset prompt).
    pub fn take_session_notices(&mut self) {
        if let Some(m) = self.session.message.take() {
            self.dlg.message = Some(m);
        }
        if std::mem::take(&mut self.session.reset_requested) {
            self.dlg.confirm_reset = true;
        }
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        use egui::Key;
        let (ctrl, keys) = ctx.input(|i| (i.modifiers.ctrl, i.keys_down.clone()));
        let pressed = |k: Key| ctx.input(|i| i.key_pressed(k));
        let _ = keys;
        if ctrl && pressed(Key::R) {
            self.session.restart_engine();
        }
        if ctrl && pressed(Key::Num1) {
            self.dlg.vban = !self.dlg.vban;
        }
        if ctrl && pressed(Key::Num3) {
            self.dlg.midi = !self.dlg.midi;
        }
        if ctrl && pressed(Key::Num4) {
            self.dlg.presets = !self.dlg.presets;
        }
        if ctrl && pressed(Key::Num5) {
            self.dlg.recorder = !self.dlg.recorder;
        }
        if ctrl && pressed(Key::Comma) {
            self.dlg.system = !self.dlg.system;
        }
        if pressed(Key::F5) {
            self.session.refresh_devices();
            self.session.refresh_midi_ports();
        }
    }

    // ------------------------------------------------------------ panels

    fn top_bar(&mut self, ui: &mut Ui, ctx: &egui::Context) {
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
        let st = self.session.shared.status.lock().clone();
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

    fn input_device_menu(&mut self, ui: &mut Ui, s: usize) {
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

    fn output_device_menu(&mut self, ui: &mut Ui, b: usize) {
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

    fn strip_ui(&mut self, ui: &mut Ui, s: usize, p: &mut MixParams) {
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

    fn bus_ui(&mut self, ui: &mut Ui, b: usize, p: &mut MixParams) {
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

    /// Player / recorder bar at the top of the Master section.
    fn player_ui(&mut self, ui: &mut Ui, p: &mut MixParams, width: f32) {
        let t = self.session.shared.tape.clone();
        card().inner_margin(egui::Margin::symmetric(14.0, 12.0)).show(ui, |ui| {
            ui.set_width(width - 28.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 16.0;
                let playing = t.playing.load(Ordering::Relaxed);
                let rec = t.recording.load(Ordering::Relaxed);

                // Transport.
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    let r = transport_button(ui, false, |ui, c, col| icon_skip(ui, c, col, false)).on_hover_text("Back 5 s");
                    if r.clicked() {
                        self.session.tape_cmd(TapeCmd::Rew);
                    }
                    self.hover_help(&r, "TAPE_REW");
                    let r = if playing {
                        transport_button(ui, true, |ui, c, col| icon_pause(ui, c, col)).on_hover_text("Pause")
                    } else {
                        transport_button(ui, true, |ui, c, col| icon_play(ui, c, col)).on_hover_text("Play")
                    };
                    if r.clicked() {
                        if playing {
                            t.playing.store(false, Ordering::Relaxed);
                        } else {
                            self.session.tape_cmd(TapeCmd::Play);
                        }
                    }
                    self.hover_help(&r, "TAPE_PLAY");
                    let r = transport_button(ui, false, |ui, c, col| icon_stop(ui, c, col)).on_hover_text("Stop (again: back to start)");
                    if r.clicked() {
                        self.session.tape_cmd(TapeCmd::Stop);
                    }
                    self.hover_help(&r, "TAPE_STOP");
                    let r = transport_button(ui, false, |ui, c, col| icon_skip(ui, c, col, true)).on_hover_text("Forward 5 s");
                    if r.clicked() {
                        self.session.tape_cmd(TapeCmd::Ff);
                    }
                    self.hover_help(&r, "TAPE_FF");
                    let paused = rec && t.rec_paused.load(Ordering::Relaxed);
                    let r = rec_button(ui, rec, if paused { "Paused" } else { "Rec" });
                    if r.clicked() {
                        self.session.tape_cmd(TapeCmd::Record);
                    }
                    self.hover_help(&r, "TAPE_REC");
                });

                // File / progress area (click: load a file, right-click: recorder options).
                let right_w = 228.0 + 12.0 + 40.0;
                let prog_w = (ui.available_width() - right_w - 16.0).max(200.0);
                let (rect, resp) = ui.allocate_exact_size(Vec2::new(prog_w, 36.0), Sense::click());
                let name = t.file_name.lock().clone();
                let status = t.status.lock().clone();
                let (label, label_col, time, frac) = if rec {
                    let secs = t.seconds(t.rec_frames.load(Ordering::Relaxed));
                    let what = if t.rec_paused.load(Ordering::Relaxed) { "Recording paused" } else { "Recording…" };
                    (what.to_string(), RED, clock(secs), 0.0)
                } else if !name.is_empty() {
                    let pos = t.pos.load(Ordering::Relaxed);
                    let len = t.len.load(Ordering::Relaxed).max(1);
                    (name, TEXT_2, format!("{} / {}", clock(t.seconds(pos)), clock(t.seconds(len))), pos as f32 / len as f32)
                } else if !status.is_empty() {
                    (status, TEXT_3, "00:00 / 00:00".to_string(), 0.0)
                } else {
                    ("Click to load a file, or press Rec".to_string(), TEXT_2, "00:00 / 00:00".to_string(), 0.0)
                };
                let time_w = ui.fonts(|f| f.layout_no_wrap(time.clone(), mono(12.0), TEXT).size().x);
                let row_y = rect.top() + 9.0;
                paint_text(ui, egui::pos2(rect.left(), row_y), egui::Align2::LEFT_CENTER, &label, regular(12.0), label_col, rect.width() - time_w - 12.0);
                ui.painter().text(egui::pos2(rect.right(), row_y), egui::Align2::RIGHT_CENTER, &time, mono(12.0), TEXT_3);
                let line = egui::Rect::from_min_size(egui::pos2(rect.left(), rect.bottom() - 10.0), Vec2::new(rect.width(), 4.0));
                ui.painter().rect_filled(line, 2.0, SUNKEN);
                if frac > 0.0 {
                    let fill = egui::Rect::from_min_size(line.min, Vec2::new(line.width() * frac.clamp(0.0, 1.0), 4.0));
                    ui.painter().rect_filled(fill, 2.0, ACCENT_A);
                }
                if resp.clicked() {
                    if let Some(f) = rfd::FileDialog::new().add_filter("Audio (wav, bwf)", &["wav", "bwf"]).set_title("Select Audio File").pick_file() {
                        let play = !self.session.settings.recorder.dont_play;
                        self.session.load_tape_file(f, play);
                    }
                }
                if resp.secondary_clicked() {
                    self.dlg.recorder = true;
                }
                self.hover_help(&resp, "TAPE");

                // Routing chips + gain knob.
                let (_, hov) = route_grid(ui, &mut p.tape.route, &BUS_NAMES, 6, 228.0);
                if hov.is_some() {
                    self.help = help_text("TAPE_A1").into();
                }
                let fmt = |v: f32| format!("{} dB", fmt_db(v));
                knob(ui, &mut p.tape.gain_db, (GAIN_MIN_DB, GAIN_MAX_DB), 0.0, "Gain", 36.0, 40.0, ACCENT_A, &fmt);
            });
        });
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.tray_and_close(ctx);
        self.poll_background(ctx);
        self.shortcuts(ctx);
        self.help.clear();

        let before = *self.session.shared.params.lock();
        let mut p = before;

        egui::TopBottomPanel::top("top")
            .exact_height(56.0)
            .frame(egui::Frame::none().fill(TOPBAR).inner_margin(egui::Margin::symmetric(24.0, 12.0)))
            .show(ctx, |ui| self.top_bar(ui, ctx));
        let status_h = egui::TopBottomPanel::bottom("status")
            .frame(egui::Frame::none().fill(TOPBAR).inner_margin(egui::Margin::symmetric(24.0, 6.0)))
            .show(ctx, |ui| {
                let help = if self.session.options.show_caption_help && !self.help.is_empty() { self.help.clone() } else { String::from(" ") };
                ui.label(text(help, regular(11.0), TEXT_3));
            })
            .response
            .rect
            .height();
        // Cards stretch to the tallest card's bottom from the last frame.
        self.cards_bottom = std::mem::take(&mut self.cards_bottom_next);
        let content = egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(BG).inner_margin(egui::Margin { left: 24.0, right: 24.0, top: 20.0, bottom: 24.0 }))
            .show(ctx, |ui| {
                egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = SECTION_GAP;
                        section().show(ui, |ui| {
                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing.y = 14.0;
                                section_head(ui, "Hardware inputs", &NUM_HW_STRIPS.to_string());
                                ui.horizontal_top(|ui| {
                                    ui.spacing_mut().item_spacing.x = STRIP_GAP;
                                    for s in 0..NUM_HW_STRIPS {
                                        self.strip_ui(ui, s, &mut p);
                                    }
                                });
                            });
                        });
                        section().show(ui, |ui| {
                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing.y = 14.0;
                                section_head(ui, "Virtual inputs", &(NUM_STRIPS - NUM_HW_STRIPS).to_string());
                                ui.horizontal_top(|ui| {
                                    ui.spacing_mut().item_spacing.x = STRIP_GAP;
                                    for s in NUM_HW_STRIPS..NUM_STRIPS {
                                        self.strip_ui(ui, s, &mut p);
                                    }
                                });
                            });
                        });
                        section().show(ui, |ui| {
                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing.y = 14.0;
                                section_head(ui, "Master", &format!("{NUM_BUSES} buses"));
                                let width = NUM_BUSES as f32 * BUS_W + (NUM_BUSES as f32 - 1.0) * STRIP_GAP;
                                self.player_ui(ui, &mut p, width);
                                ui.horizontal_top(|ui| {
                                    ui.spacing_mut().item_spacing.x = STRIP_GAP;
                                    for b in 0..NUM_BUSES {
                                        self.bus_ui(ui, b, &mut p);
                                    }
                                });
                            });
                        });
                    })
                    .response
                    .rect
                    .size()
                })
                .inner
            })
            .inner;
        if (self.cards_bottom_next - self.cards_bottom).abs() > 0.5 {
            ctx.request_repaint();
        }
        self.fit_window(ctx, content, status_h);

        dialogs::show(self, ctx, &mut p);

        // Write back only the fields edited this frame, so concurrent writes to the shared params
        // (MIDI, hotkey, tray, and in-frame preset recall / console / reset) are not clobbered.
        let p = {
            let mut shared = self.session.shared.params.lock();
            if p != before {
                rustbus::params::merge(&before, &p, &mut shared);
            }
            *shared
        };
        if let Some(t) = &self.tray {
            t.set_mic_muted(p.strips[self.session.options.hotkey.strip.min(NUM_STRIPS - 1)].mute);
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.session.shutdown();
    }
}


pub(crate) fn short(s: &str) -> String {
    let s = s.trim();
    if s.chars().count() > 26 {
        format!("{}…", s.chars().take(25).collect::<String>())
    } else {
        s.to_string()
    }
}

/// "Speakers (Scarlett 2i2 USB)" → "Speakers · Scarlett 2i2 USB".
pub(crate) fn pretty_device(name: &str) -> String {
    let n = name.trim();
    match n.rfind(" (") {
        Some(i) if n.ends_with(')') => format!("{} · {}", &n[..i], &n[i + 2..n.len() - 1]),
        _ => n.to_string(),
    }
}

/// Two meter bars from up to 8 channels: stereo as-is, multichannel as max(even) / max(odd).
fn stereo_levels(levels: &[f32; MAX_CH], nch: usize) -> (f32, f32) {
    if nch <= 2 {
        (levels[0], levels[1])
    } else {
        let l = levels[..nch].iter().step_by(2).fold(0.0f32, |a, x| a.max(*x));
        let r = levels[1..nch].iter().step_by(2).fold(0.0f32, |a, x| a.max(*x));
        (l, r)
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

/// 36 px transport button; `filled` = the white play/pause style.
fn transport_button(ui: &mut Ui, filled: bool, icon: impl FnOnce(&Ui, egui::Pos2, Color32)) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(36.0), Sense::click());
    let (bg, border, fg) = if filled {
        (TEXT, TEXT, BG)
    } else {
        (if resp.hovered() { GHOST_HOVER } else { WELL }, BORDER_CTL, TEXT_HOVER)
    };
    ui.painter().rect(rect, Rounding::same(9.0), bg, Stroke::new(1.0f32, border));
    icon(ui, rect.center(), fg);
    resp
}

/// Rec button: red outline at rest, filled red while recording.
fn rec_button(ui: &mut Ui, on: bool, label: &str) -> egui::Response {
    let font = semi(12.0);
    let tw = ui.fonts(|f| f.layout_no_wrap(label.to_owned(), font.clone(), TEXT).size().x);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(12.0 + 8.0 + 8.0 + tw + 12.0, 36.0), Sense::click());
    let (bg, border, fg) = if on { (RED, RED, ON_RED) } else { (REC_BG, REC_BORDER, REC_TEXT) };
    ui.painter().rect(rect, Rounding::same(9.0), bg, Stroke::new(1.0f32, border));
    ui.painter().circle_filled(egui::pos2(rect.left() + 16.0, rect.center().y), 4.0, fg);
    ui.painter().text(egui::pos2(rect.left() + 28.0, rect.center().y), egui::Align2::LEFT_CENTER, label, font, fg);
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_names_are_prettified() {
        assert_eq!(pretty_device("Speakers (Scarlett 2i2 USB)"), "Speakers · Scarlett 2i2 USB");
        assert_eq!(pretty_device("Line 1 (Virtual Audio Cable)"), "Line 1 · Virtual Audio Cable");
        assert_eq!(pretty_device("CABLE Input"), "CABLE Input");
    }

    #[test]
    fn multichannel_meter_folds_to_stereo() {
        let lv = [0.1, 0.2, 0.5, 0.1, 0.0, 0.7, 0.0, 0.0];
        assert_eq!(stereo_levels(&lv, 2), (0.1, 0.2));
        assert_eq!(stereo_levels(&lv, 8), (0.5, 0.7));
    }
}
