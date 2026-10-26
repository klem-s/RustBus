//! Player / recorder bar at the top of the Master section.

use std::sync::atomic::Ordering;

use eframe::egui::{self, Color32, Rounding, Sense, Stroke, Ui, Vec2};

use rustbus::params::*;
use rustbus::script::TapeCmd;

use super::help::help_text;
use super::theme::*;
use super::widgets::*;
use super::App;

impl App {
    /// Player / recorder bar at the top of the Master section.
    pub(super) fn player_ui(&mut self, ui: &mut Ui, p: &mut MixParams, width: f32) {
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
