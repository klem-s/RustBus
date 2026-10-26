//! Main window and application state.

mod bus;
mod desktop;
mod dialogs;
mod help;
mod player;
mod strip;
mod theme;
mod topbar;
mod widgets;

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use eframe::egui::{self, Ui, Vec2};

use rustbus::engine::mixer::Meters;
use rustbus::options::AppOptions;
use rustbus::params::*;
use rustbus::session::Session;
use rustbus::settings;

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
    pub latency: bool,
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
