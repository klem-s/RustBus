//! Custom-painted widgets matching `design/rustbus-mixer.html`.

use std::hash::Hash;

use eframe::egui::{
    self, epaint::Mesh, text::LayoutJob, Align2, Color32, FontId, Pos2, Rect, Response, Rounding, Sense, Shape, Stroke, TextFormat, Ui, Vec2,
};

use super::theme::*;

// Aliases used by the dialogs (kept so dialog code doesn't need to change).
pub const ACCENT: Color32 = ACCENT_A;
pub const TEXT_DIM: Color32 = TEXT_3;
pub const ON_BLUE: Color32 = ACCENT_B;
pub const ON_YELLOW: Color32 = AMBER;

/// Text colour to use on a filled accent.
pub fn on_color(fill: Color32) -> Color32 {
    match fill {
        f if f == ACCENT_A => ON_A,
        f if f == ACCENT_B => ON_B,
        f if f == AMBER => ON_AMBER,
        f if f == RED => ON_RED,
        _ => Color32::BLACK,
    }
}

pub fn lin_to_db(x: f32) -> f32 {
    if x <= 1e-6 {
        -120.0
    } else {
        20.0 * x.log10()
    }
}

/// Fader scale: −60 … +12 dB, so 0 dB sits at 83.3 %.
pub fn db_to_frac(db: f32) -> f32 {
    ((db - vmin()) / (vmax() - vmin())).clamp(0.0, 1.0)
}
fn vmin() -> f32 {
    rustbus::params::GAIN_MIN_DB
}
fn vmax() -> f32 {
    rustbus::params::GAIN_MAX_DB
}

/// Single-line text, truncated with "…" to `max_w`.
pub fn paint_text(ui: &Ui, pos: Pos2, align: Align2, s: &str, font: FontId, color: Color32, max_w: f32) -> Rect {
    let mut job = LayoutJob::single_section(s.to_owned(), TextFormat::simple(font, color));
    job.wrap.max_width = max_w.max(1.0);
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    job.wrap.overflow_character = Some('…');
    let galley = ui.fonts(|f| f.layout_job(job));
    let rect = align.anchor_size(pos, galley.size());
    ui.painter().galley(rect.min, galley, color);
    rect
}

/// Generic rounded control surface used by buttons, chips and selects.
fn surface(ui: &Ui, rect: Rect, r: f32, on: Option<Color32>, hovered: bool) -> Color32 {
    let p = ui.painter();
    match on {
        Some(fill) => {
            p.rect(rect, Rounding::same(r), fill, Stroke::new(1.0f32, fill));
            on_color(fill)
        }
        None => {
            p.rect(rect, Rounding::same(r), WELL, Stroke::new(1.0f32, if hovered { BORDER_HOVER } else { BORDER_CTL }));
            if hovered {
                TEXT
            } else {
                TEXT_2
            }
        }
    }
}

/// 28 px state button (Mono / Solo / Mute / EQ …). `on` = fill colour when active.
pub fn flag_button(ui: &mut Ui, size: Vec2, label: &str, on: Option<Color32>) -> Response {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let fg = surface(ui, rect, 7.0, on, resp.hovered());
    paint_text(ui, rect.center(), Align2::CENTER_CENTER, label, semi(11.0), fg, rect.width() - 4.0);
    resp
}

/// Bus mute button with a speaker-cross icon.
pub fn mute_icon_button(ui: &mut Ui, size: Vec2, on: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let fg = surface(ui, rect, 7.0, on.then_some(RED), resp.hovered());
    icon_speaker_x(ui, rect.center(), 14.0, fg);
    resp
}

/// 26 px routing chip (A1..B3) in JetBrains Mono.
pub fn chip(ui: &mut Ui, size: Vec2, label: &str, on: bool, accent: Color32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let fg = if on {
        surface(ui, rect, 6.0, Some(accent), false)
    } else {
        let c = surface(ui, rect, 6.0, None, resp.hovered());
        if resp.hovered() {
            TEXT_HOVER
        } else {
            let _ = c;
            TEXT_3
        }
    };
    paint_text(ui, rect.center(), Align2::CENTER_CENTER, label, mono_semi(11.0), fg, rect.width());
    resp
}

/// Grid of routing chips, `cols` per row. Returns true if anything changed.
pub fn route_grid(ui: &mut Ui, routes: &mut [bool], names: &[&str], cols: usize, width: f32) -> (bool, Option<usize>) {
    let gap = 4.0;
    let w = (width - gap * (cols as f32 - 1.0)) / cols as f32;
    let mut changed = false;
    let mut hovered = None;
    let rows = routes.len().div_ceil(cols);
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing = Vec2::splat(gap);
        for r in 0..rows {
            ui.horizontal(|ui| {
                for c in 0..cols {
                    let i = r * cols + c;
                    if i >= routes.len() {
                        break;
                    }
                    let accent = if names[i].starts_with('A') { ACCENT_A } else { ACCENT_B };
                    let resp = chip(ui, Vec2::new(w, 26.0), names[i], routes[i], accent);
                    if resp.clicked() {
                        routes[i] = !routes[i];
                        changed = true;
                    }
                    if resp.hovered() {
                        hovered = Some(i);
                    }
                }
            });
        }
    });
    (changed, hovered)
}

/// Dropdown-looking button (30 px, chevron). Click toggles a popup with `contents`.
pub fn select(ui: &mut Ui, id: impl Hash, label: &str, width: f32, contents: impl FnOnce(&mut Ui)) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, 30.0), Sense::click());
    let popup_id = ui.make_persistent_id(id);
    let open = ui.memory(|m| m.is_popup_open(popup_id));
    let hover = resp.hovered() || open;
    ui.painter().rect(rect, Rounding::same(8.0), WELL, Stroke::new(1.0f32, if hover { hex_353c46() } else { BORDER_CTL }));
    paint_text(ui, Pos2::new(rect.left() + 8.0, rect.center().y), Align2::LEFT_CENTER, label, regular(11.0), TEXT_2, rect.width() - 24.0);
    icon_chevron(ui, Pos2::new(rect.right() - 12.0, rect.center().y), TEXT_3);
    if resp.clicked() {
        ui.memory_mut(|m| m.toggle_popup(popup_id));
    }
    egui::popup::popup_below_widget(ui, popup_id, &resp, egui::PopupCloseBehavior::CloseOnClickOutside, |ui| {
        ui.set_min_width(width.max(240.0));
        contents(ui);
    });
    resp
}

fn hex_353c46() -> Color32 {
    Color32::from_rgb(0x35, 0x3C, 0x46)
}

/// Close whichever popup/menu the caller is in.
pub fn close_popup(ui: &mut Ui) {
    ui.memory_mut(|m| m.close_popup());
    ui.close_menu();
}

/// 32 px "ghost" button in the top bar.
pub fn ghost_button(ui: &mut Ui, label: &str, icon: Option<fn(&Ui, Pos2, Color32)>, lit: bool) -> Response {
    let font = medium(13.0);
    let tw = ui.fonts(|f| f.layout_no_wrap(label.to_owned(), font.clone(), TEXT).size().x);
    let w = 24.0 + tw + if icon.is_some() { 22.0 } else { 0.0 };
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, 32.0), Sense::click());
    ui.painter().rect(rect, Rounding::same(8.0), if resp.hovered() { GHOST_HOVER } else { GHOST }, Stroke::new(1.0f32, BORDER_CTL));
    let fg = if lit { ACCENT_A } else { GHOST_TEXT };
    let mut x = rect.left() + 12.0;
    if let Some(icon) = icon {
        icon(ui, Pos2::new(x + 7.0, rect.center().y), fg);
        x += 22.0;
    }
    paint_text(ui, Pos2::new(x, rect.center().y), Align2::LEFT_CENTER, label, font, fg, tw + 2.0);
    resp
}

/// Coloured 8 px status dot.
pub fn dot(ui: &mut Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(8.0), Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.0, color);
}

/// Rotary knob: 270° arc (135° → 405°), 3 px track, accent value arc, 20 px disc, white pointer.
pub fn knob(
    ui: &mut Ui,
    value: &mut f32,
    range: (f32, f32),
    default: f32,
    label: &str,
    size: f32,
    width: f32,
    accent: Color32,
    fmt: &dyn Fn(f32) -> String,
) -> (Response, bool) {
    let (min, max) = range;
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, size + 14.0), Sense::click_and_drag());
    let mut changed = false;
    if resp.double_clicked() {
        *value = default;
        changed = true;
    } else if resp.dragged() {
        let fine = ui.input(|i| i.modifiers.shift);
        *value = (*value - resp.drag_delta().y * (max - min) / if fine { 1000.0 } else { 150.0 }).clamp(min, max);
        changed = true;
    }
    if resp.hovered() {
        let s = ui.input(|i| i.raw_scroll_delta.y);
        if s != 0.0 {
            *value = (*value + s.signum() * (max - min) / 50.0).clamp(min, max);
            changed = true;
        }
    }
    let k = size / 40.0;
    let c = Pos2::new(rect.center().x, rect.top() + size / 2.0);
    let r = 15.0 * k;
    let t = ((*value - min) / (max - min)).clamp(0.0, 1.0);
    let a0 = 135f32.to_radians();
    let sweep = 270f32.to_radians();
    let p = ui.painter();
    let arc = |from: f32, to: f32, col: Color32| {
        let steps = ((to - from).abs() / 0.08).ceil().max(2.0) as usize;
        let pts: Vec<Pos2> = (0..=steps).map(|i| c + Vec2::angled(from + (to - from) * i as f32 / steps as f32) * r).collect();
        p.add(Shape::line(pts, Stroke::new(3.0 * k, col)));
        // Round caps.
        p.circle_filled(c + Vec2::angled(from) * r, 1.5 * k, col);
        p.circle_filled(c + Vec2::angled(to) * r, 1.5 * k, col);
    };
    arc(a0, a0 + sweep, BORDER_CTL);
    if t > 0.001 {
        arc(a0, a0 + sweep * t, accent);
    }
    p.circle_filled(c, 10.0 * k, KNOB_DISC);
    let ang = a0 + sweep * t;
    p.line_segment([c, c + Vec2::angled(ang) * 7.5 * k], Stroke::new(2.0 * k, TEXT));
    p.text(Pos2::new(rect.center().x, rect.bottom() - 6.0), Align2::CENTER_CENTER, label, regular(10.0), TEXT_3);
    let resp = resp.on_hover_text(format!("{label}: {}", fmt(*value)));
    (resp, changed)
}

/// Stereo peak meter: gradient fixed to the full height (accent-a → amber at 84 % →
/// red at 100 %), revealed from the bottom, so colour shows absolute level.
pub fn meter(ui: &mut Ui, levels: [f32; 2], height: f32, muted: bool, limit_db: Option<f32>) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(12.0, height), Sense::hover());
    let p = ui.painter();
    let alpha = if muted { 0.25 } else { 1.0 };
    let stops = [(0.0f32, ACCENT_A), (0.68, ACCENT_A), (0.84, AMBER), (1.0, RED)];
    for (i, lvl) in levels.iter().enumerate() {
        let x = rect.left() + i as f32 * 7.0;
        let bar = Rect::from_min_max(Pos2::new(x, rect.top()), Pos2::new(x + 5.0, rect.bottom()));
        p.rect_filled(bar, 3.0, SUNKEN);
        let f = db_to_frac(lin_to_db(*lvl));
        if f <= 0.0 {
            continue;
        }
        let y_of = |frac: f32| bar.bottom() - frac * bar.height();
        let mut mesh = Mesh::default();
        for w in stops.windows(2) {
            let (f0, c0) = w[0];
            let (f1, c1) = w[1];
            if f <= f0 {
                break;
            }
            let top = f.min(f1);
            let c_top = lerp_color(c0, c1, (top - f0) / (f1 - f0)).gamma_multiply(alpha);
            let c_bot = c0.gamma_multiply(alpha);
            let idx = mesh.vertices.len() as u32;
            mesh.colored_vertex(Pos2::new(bar.left(), y_of(f0)), c_bot);
            mesh.colored_vertex(Pos2::new(bar.right(), y_of(f0)), c_bot);
            mesh.colored_vertex(Pos2::new(bar.right(), y_of(top)), c_top);
            mesh.colored_vertex(Pos2::new(bar.left(), y_of(top)), c_top);
            mesh.add_triangle(idx, idx + 1, idx + 2);
            mesh.add_triangle(idx, idx + 2, idx + 3);
        }
        p.add(Shape::mesh(mesh));
    }
    if let Some(l) = limit_db.filter(|l| *l < rustbus::params::LIMIT_MAX_DB) {
        let y = rect.bottom() - db_to_frac(l) * rect.height();
        p.line_segment([Pos2::new(rect.left() - 1.0, y), Pos2::new(rect.right() + 1.0, y)], Stroke::new(2.0f32, RED));
    }
}

fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

/// Gain fader: 24 px wide, 4 px sunken track, 24×12 thumb. Double-click = `default`,
/// wheel = ±0.5 dB, Shift = fine, Ctrl+drag sets the limiter.
pub fn fader(ui: &mut Ui, value: &mut f32, default: f32, limit: Option<&mut f32>, height: f32) -> (Response, bool) {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(24.0, height), Sense::click_and_drag());
    let (min, max) = (vmin(), vmax());
    let top = rect.top() + 6.0;
    let bottom = rect.bottom() - 6.0;
    let to_y = |v: f32| bottom - db_to_frac(v) * (bottom - top);
    let from_y = |y: f32| min + (bottom - y) / (bottom - top) * (max - min);
    let mut changed = false;
    let ctrl = ui.input(|i| i.modifiers.ctrl);
    if let Some(lim) = limit {
        if resp.dragged() && ctrl {
            if let Some(pos) = resp.interact_pointer_pos() {
                *lim = from_y(pos.y).clamp(rustbus::params::LIMIT_MIN_DB, rustbus::params::LIMIT_MAX_DB).round();
                changed = true;
            }
        }
    }
    if resp.double_clicked() {
        *value = default;
        changed = true;
    } else if resp.dragged() && !ctrl {
        let fine = ui.input(|i| i.modifiers.shift);
        let scale = (max - min) / (bottom - top) * if fine { 0.1 } else { 1.0 };
        *value = (*value - resp.drag_delta().y * scale).clamp(min, max);
        changed = true;
    }
    if resp.hovered() {
        let s = ui.input(|i| i.raw_scroll_delta.y);
        if s != 0.0 {
            *value = (*value + s.signum() * 0.5).clamp(min, max);
            changed = true;
        }
    }
    let p = ui.painter();
    let track = Rect::from_center_size(Pos2::new(rect.center().x, (top + bottom) / 2.0), Vec2::new(4.0, bottom - top));
    p.rect_filled(track, 2.0, SUNKEN);
    let ty = to_y(*value);
    let thumb = Rect::from_center_size(Pos2::new(rect.center().x, ty), Vec2::new(24.0, 12.0));
    p.rect_filled(thumb.translate(Vec2::new(0.0, 2.0)).expand(1.0), 5.0, Color32::from_black_alpha(110));
    p.rect_filled(thumb, 4.0, if resp.dragged() { Color32::WHITE } else { Color32::from_rgb(0xE7, 0xEA, 0xEE) });
    p.rect_filled(Rect::from_min_max(Pos2::new(thumb.left() + 5.0, ty - 1.0), Pos2::new(thumb.right() - 5.0, ty + 1.0)), 1.0, BG);
    (resp, changed)
}

/// "+0.0 dB" readout (JetBrains Mono 13/600 + small dB).
pub fn db_readout(ui: &mut Ui, db: f32) {
    let mut job = LayoutJob::default();
    job.append(&fmt_db(db), 0.0, TextFormat::simple(mono_semi(13.0), TEXT));
    job.append(" dB", 0.0, TextFormat::simple(medium(10.0), TEXT_3));
    ui.label(job);
}

/// 2D pan pad: sunken, crosshair, 12 px puck with a halo.
pub fn pan_pad(ui: &mut Ui, x: &mut f32, y: &mut f32, xr: (f32, f32), yr: (f32, f32), width: f32, puck: Color32) -> (Response, bool) {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, 76.0), Sense::click_and_drag());
    let mut changed = false;
    if resp.double_clicked() {
        *x = 0.0f32.clamp(xr.0, xr.1);
        *y = 0.0f32.clamp(yr.0, yr.1);
        changed = true;
    } else if resp.dragged() || resp.clicked() {
        if let Some(pos) = resp.interact_pointer_pos() {
            let fx = ((pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
            let fy = ((pos.y - rect.top()) / rect.height()).clamp(0.0, 1.0);
            *x = xr.0 + fx * (xr.1 - xr.0);
            *y = yr.0 + fy * (yr.1 - yr.0);
            changed = true;
        }
    }
    let p = ui.painter();
    p.rect(rect, Rounding::same(8.0), SUNKEN, Stroke::new(1.0f32, CARD_BORDER));
    p.line_segment([Pos2::new(rect.left() + 1.0, rect.center().y), Pos2::new(rect.right() - 1.0, rect.center().y)], Stroke::new(1.0f32, PAD_LINE));
    p.line_segment([Pos2::new(rect.center().x, rect.top() + 1.0), Pos2::new(rect.center().x, rect.bottom() - 1.0)], Stroke::new(1.0f32, PAD_LINE));
    let px = rect.left() + (*x - xr.0) / (xr.1 - xr.0) * rect.width();
    let py = rect.top() + (*y - yr.0) / (yr.1 - yr.0) * rect.height();
    let c = Pos2::new(px.clamp(rect.left() + 6.0, rect.right() - 6.0), py.clamp(rect.top() + 6.0, rect.bottom() - 6.0));
    p.circle_filled(c, 10.0, Color32::from_rgba_unmultiplied(238, 241, 244, 30));
    p.circle_filled(c, 6.0, puck);
    (resp, changed)
}

/// Small segmented control (Color / 3D / FX). Returns the clicked index.
pub fn segmented(ui: &mut Ui, labels: &[&str], active: usize) -> Option<usize> {
    let font = semi(9.0);
    let widths: Vec<f32> = labels.iter().map(|l| ui.fonts(|f| f.layout_no_wrap((*l).to_owned(), font.clone(), TEXT).size().x) + 10.0).collect();
    let total = widths.iter().sum::<f32>() + 2.0 * (labels.len() as f32 - 1.0) + 4.0;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(total, 18.0), Sense::hover());
    ui.painter().rect(rect, Rounding::same(6.0), WELL, Stroke::new(1.0f32, CARD_BORDER));
    let mut x = rect.left() + 2.0;
    let mut clicked = None;
    for (i, (l, w)) in labels.iter().zip(widths).enumerate() {
        let r = Rect::from_min_size(Pos2::new(x, rect.top() + 2.0), Vec2::new(w, rect.height() - 4.0));
        let resp = ui.interact(r, ui.id().with(("seg", i, *l)), Sense::click());
        if i == active {
            ui.painter().rect_filled(r, 4.0, BORDER_CTL);
        }
        let col = if i == active { TEXT } else if resp.hovered() { TEXT_HOVER } else { TEXT_3 };
        ui.painter().text(r.center(), Align2::CENTER_CENTER, *l, font.clone(), col);
        if resp.clicked() {
            clicked = Some(i);
        }
        x += w + 2.0;
    }
    clicked
}

// ---------------------------------------------------------------- icons

pub fn icon_chevron(ui: &Ui, c: Pos2, col: Color32) {
    let s = Stroke::new(1.6f32, col);
    ui.painter().add(Shape::line(vec![c + Vec2::new(-3.5, -1.8), c + Vec2::new(0.0, 1.8), c + Vec2::new(3.5, -1.8)], s));
}

pub fn icon_play(ui: &Ui, c: Pos2, col: Color32) {
    ui.painter().add(Shape::convex_polygon(vec![c + Vec2::new(-3.5, -6.0), c + Vec2::new(6.5, 0.0), c + Vec2::new(-3.5, 6.0)], col, Stroke::NONE));
}

pub fn icon_pause(ui: &Ui, c: Pos2, col: Color32) {
    for dx in [-4.0, 4.0] {
        ui.painter().rect_filled(Rect::from_center_size(c + Vec2::new(dx, 0.0), Vec2::new(4.0, 13.0)), 1.0, col);
    }
}

pub fn icon_stop(ui: &Ui, c: Pos2, col: Color32) {
    ui.painter().rect_filled(Rect::from_center_size(c, Vec2::splat(11.0)), 2.0, col);
}

/// ⏮ / ⏭ : triangle plus a bar.
pub fn icon_skip(ui: &Ui, c: Pos2, col: Color32, forward: bool) {
    let d = if forward { 1.0 } else { -1.0 };
    let s = Stroke::new(1.8f32, col);
    let tri = vec![c + Vec2::new(-4.0 * d, -5.5), c + Vec2::new(4.0 * d, 0.0), c + Vec2::new(-4.0 * d, 5.5)];
    ui.painter().add(Shape::closed_line(tri, s));
    ui.painter().line_segment([c + Vec2::new(6.0 * d, -5.5), c + Vec2::new(6.0 * d, 5.5)], s);
}

pub fn icon_speaker_x(ui: &Ui, c: Pos2, size: f32, col: Color32) {
    let k = size / 24.0;
    let pt = |x: f32, y: f32| c + Vec2::new((x - 12.0) * k, (y - 12.0) * k);
    let s = Stroke::new(2.0 * k.max(0.6), col);
    ui.painter().add(Shape::closed_line(vec![pt(11.0, 5.0), pt(6.0, 9.0), pt(3.0, 9.0), pt(3.0, 15.0), pt(6.0, 15.0), pt(11.0, 19.0)], s));
    ui.painter().line_segment([pt(22.0, 9.0), pt(16.0, 15.0)], s);
    ui.painter().line_segment([pt(16.0, 9.0), pt(22.0, 15.0)], s);
}

/// RustBus logo: 28 px teal tile with four level bars.
pub fn logo(ui: &mut Ui) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(28.0), Sense::click());
    let p = ui.painter();
    p.rect_filled(rect, 8.0, ACCENT_A);
    let s = Stroke::new(2.2f32, ON_A);
    for (x, y0, y1) in [(8.0, 14.0, 18.0), (12.0, 8.0, 20.0), (16.0, 11.0, 17.0), (20.0, 11.0, 19.0)] {
        let a = rect.min + Vec2::new(x, y0);
        let b = rect.min + Vec2::new(x, y1);
        p.line_segment([a, b], s);
        p.circle_filled(a, 1.1, ON_A);
        p.circle_filled(b, 1.1, ON_A);
    }
    resp
}

// ---------------------------------------------------------------- dialog helpers (new style)

/// Toggle button used in dialogs.
pub fn toggle(ui: &mut Ui, on: &mut bool, text: &str, color: Color32, width: f32) -> Response {
    let r = flag_button(ui, Vec2::new(width, 26.0), text, on.then_some(color));
    if r.clicked() {
        *on = !*on;
    }
    r
}

/// Same look as `toggle` for a non-bool state.
pub fn lit_button(ui: &mut Ui, lit: bool, text: &str, color: Color32, width: f32) -> Response {
    flag_button(ui, Vec2::new(width, 26.0), text, lit.then_some(color))
}

/// Card frame (strips, player).
pub fn card() -> egui::Frame {
    egui::Frame::none().fill(CARD).stroke(Stroke::new(1.0f32, CARD_BORDER)).rounding(12.0).inner_margin(STRIP_PAD)
}

/// Section panel frame.
pub fn section() -> egui::Frame {
    egui::Frame::none().fill(SECTION).stroke(Stroke::new(1.0f32, SECTION_BORDER)).rounding(16.0).inner_margin(SECTION_PAD)
}

/// Section title + count chip.
pub fn section_head(ui: &mut Ui, title: &str, count: &str) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        ui.label(caps(title, semi(12.0), TEXT_2, 1.0));
        egui::Frame::none().fill(COUNT_BG).rounding(6.0).inner_margin(egui::Margin::symmetric(6.0, 2.0)).show(ui, |ui| {
            ui.label(text(count, mono(11.0), TEXT_3));
        });
    });
}

/// Small uppercase label (10/600, +0.06em).
pub fn mini(ui: &mut Ui, s: &str, color: Color32) -> Response {
    ui.label(caps(s, semi(10.0), color, 0.6))
}
