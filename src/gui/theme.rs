//! Design tokens and fonts from `design/DESIGN_HANDOFF.md` / `design/rustbus-mixer.html`.

use eframe::egui::{self, Color32, FontData, FontDefinitions, FontFamily, FontId, RichText, Rounding, Stroke};

const fn hex(rgb: u32) -> Color32 {
    Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

// Surfaces
pub const BG: Color32 = hex(0x0A0C0F);
pub const TOPBAR: Color32 = hex(0x0D0F13);
pub const DIVIDER: Color32 = hex(0x1A1E24);
pub const SECTION: Color32 = hex(0x111419);
pub const SECTION_BORDER: Color32 = hex(0x1E232A);
pub const CARD: Color32 = hex(0x171B21);
pub const CARD_BORDER: Color32 = hex(0x232830);
pub const WELL: Color32 = hex(0x12151A);
pub const SUNKEN: Color32 = hex(0x0F1216);
pub const BORDER_CTL: Color32 = hex(0x262B33);
pub const BORDER_HOVER: Color32 = hex(0x3A424D);
pub const GHOST: Color32 = hex(0x14171C);
pub const GHOST_HOVER: Color32 = hex(0x1B1F26);
pub const GHOST_TEXT: Color32 = hex(0xD5DAE1);
pub const KNOB_DISC: Color32 = hex(0x222830);
pub const PAD_LINE: Color32 = hex(0x1F242B);
pub const COUNT_BG: Color32 = hex(0x1A1E24);
pub const IDLE_DOT: Color32 = hex(0x3A424D);

// Text
pub const TEXT: Color32 = hex(0xEEF1F4);
pub const TEXT_2: Color32 = hex(0xAEB6C2);
pub const TEXT_3: Color32 = hex(0x8B93A1);
pub const TEXT_HOVER: Color32 = hex(0xC9D0D9);
pub const TEXT_FAINT: Color32 = hex(0x5C6573);

// Accents (and the text colour to use on top of each)
pub const ACCENT_A: Color32 = hex(0x5EEAD4);
pub const ON_A: Color32 = hex(0x06201C);
pub const ACCENT_B: Color32 = hex(0xA78BFA);
pub const ON_B: Color32 = hex(0x170C33);
pub const PUCK_B: Color32 = hex(0xC4B5FD);
pub const AMBER: Color32 = hex(0xFBBF24);
pub const ON_AMBER: Color32 = hex(0x2A1B00);
pub const RED: Color32 = hex(0xF87171);
pub const ON_RED: Color32 = hex(0x2B0606);

// Status pill
pub const STATUS_BG: Color32 = hex(0x0F1A18);
pub const STATUS_BORDER: Color32 = hex(0x16332E);
pub const WARN_BG: Color32 = hex(0x1F1A0C);
pub const WARN_BORDER: Color32 = hex(0x3A2F12);

// Rec button at rest
pub const REC_BG: Color32 = hex(0x1C1416);
pub const REC_BORDER: Color32 = hex(0x3A2326);
pub const REC_TEXT: Color32 = hex(0xFCA5A5);

// Geometry (4 px grid)
pub const STRIP_W: f32 = 148.0;
pub const BUS_W: f32 = 124.0;
pub const STRIP_PAD: f32 = 12.0;
pub const STRIP_GAP: f32 = 10.0;
pub const SECTION_PAD: f32 = 16.0;
pub const SECTION_GAP: f32 = 16.0;
pub const INPUT_METER_H: f32 = 180.0;
pub const BUS_METER_H: f32 = 236.0;

// Font families (egui has no weights: each weight is its own family).
pub const F_MEDIUM: &str = "inter-500";
pub const F_SEMI: &str = "inter-600";
pub const F_BOLD: &str = "inter-700";
pub const F_MONO_SEMI: &str = "jbm-600";

pub fn regular(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}
pub fn medium(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(F_MEDIUM.into()))
}
pub fn semi(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(F_SEMI.into()))
}
pub fn bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(F_BOLD.into()))
}
pub fn mono(size: f32) -> FontId {
    FontId::new(size, FontFamily::Monospace)
}
pub fn mono_semi(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(F_MONO_SEMI.into()))
}

pub fn text(s: impl Into<String>, font: FontId, color: Color32) -> RichText {
    RichText::new(s).font(font).color(color)
}

/// Section / mini titles: uppercase with letter spacing.
pub fn caps(s: &str, font: FontId, color: Color32, tracking: f32) -> RichText {
    RichText::new(s.to_uppercase()).font(font).color(color).extra_letter_spacing(tracking)
}

pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let add = |f: &mut FontDefinitions, name: &str, bytes: &'static [u8]| {
        f.font_data.insert(name.to_owned(), FontData::from_static(bytes));
    };
    add(&mut fonts, "inter-400", include_bytes!("../../assets/fonts/Inter-400.ttf"));
    add(&mut fonts, F_MEDIUM, include_bytes!("../../assets/fonts/Inter-500.ttf"));
    add(&mut fonts, F_SEMI, include_bytes!("../../assets/fonts/Inter-600.ttf"));
    add(&mut fonts, F_BOLD, include_bytes!("../../assets/fonts/Inter-700.ttf"));
    add(&mut fonts, "jbm-500", include_bytes!("../../assets/fonts/JetBrainsMono-500.ttf"));
    add(&mut fonts, F_MONO_SEMI, include_bytes!("../../assets/fonts/JetBrainsMono-600.ttf"));

    // egui's bundled fonts stay as fallbacks for glyphs outside the Latin subset (icons, emoji).
    let fallback_prop = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let fallback_mono = fonts.families.get(&FontFamily::Monospace).cloned().unwrap_or_default();
    let with_fallback = |first: &str, rest: &[String]| {
        let mut v = vec![first.to_owned()];
        v.extend(rest.iter().cloned());
        v
    };
    fonts.families.insert(FontFamily::Proportional, with_fallback("inter-400", &fallback_prop));
    fonts.families.insert(FontFamily::Monospace, with_fallback("jbm-500", &fallback_mono));
    for name in [F_MEDIUM, F_SEMI, F_BOLD] {
        fonts.families.insert(FontFamily::Name(name.into()), with_fallback(name, &fallback_prop));
    }
    fonts.families.insert(FontFamily::Name(F_MONO_SEMI.into()), with_fallback(F_MONO_SEMI, &fallback_mono));
    ctx.set_fonts(fonts);
}

/// Global egui visuals (dialogs, menus, built-in widgets) mapped to the tokens.
pub fn apply(ctx: &egui::Context) {
    install_fonts(ctx);
    let mut style = (*ctx.style()).clone();
    let v = &mut style.visuals;
    *v = egui::Visuals::dark();
    v.panel_fill = BG;
    v.window_fill = SECTION;
    v.window_stroke = Stroke::new(1.0f32, SECTION_BORDER);
    v.window_rounding = Rounding::same(12.0);
    v.menu_rounding = Rounding::same(10.0);
    v.extreme_bg_color = SUNKEN;
    v.faint_bg_color = CARD;
    v.hyperlink_color = ACCENT_A;
    v.selection.bg_fill = ACCENT_A.gamma_multiply(0.35);
    v.selection.stroke = Stroke::new(2.0f32, ACCENT_A);
    v.widgets.noninteractive.bg_fill = CARD;
    v.widgets.noninteractive.weak_bg_fill = CARD;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0f32, DIVIDER);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0f32, TEXT_2);
    for (w, border, fg) in [
        (&mut v.widgets.inactive, BORDER_CTL, TEXT_2),
        (&mut v.widgets.hovered, BORDER_HOVER, TEXT),
        (&mut v.widgets.active, ACCENT_A, TEXT),
        (&mut v.widgets.open, BORDER_HOVER, TEXT),
    ] {
        w.bg_fill = WELL;
        w.weak_bg_fill = WELL;
        w.bg_stroke = Stroke::new(1.0f32, border);
        w.fg_stroke = Stroke::new(1.0f32, fg);
        w.rounding = Rounding::same(7.0);
        w.expansion = 0.0;
    }
    v.widgets.hovered.weak_bg_fill = GHOST_HOVER;
    style.spacing.interact_size.y = 26.0;
    style.spacing.button_padding = egui::vec2(10.0, 4.0);
    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    ctx.set_style(style);
}

/// "48 kHz", "44.1 kHz".
pub fn khz(sr: u32) -> String {
    if sr % 1000 == 0 {
        format!("{} kHz", sr / 1000)
    } else {
        format!("{:.1} kHz", sr as f32 / 1000.0)
    }
}

/// dB readout with a true minus sign: "+0.0", "−6.0".
pub fn fmt_db(db: f32) -> String {
    // `+ 0.0` turns −0.0 into 0.0 so tiny negative values read "+0.0", not "+-0.0".
    let v = (db * 10.0).round() / 10.0 + 0.0;
    if v < 0.0 {
        format!("\u{2212}{:.1}", -v)
    } else {
        format!("+{:.1}", v)
    }
}

/// "mm:ss", or "h:mm:ss" past an hour.
pub fn clock(secs: f64) -> String {
    let t = secs.max(0.0) as u64;
    if t >= 3600 {
        format!("{}:{:02}:{:02}", t / 3600, (t / 60) % 60, t % 60)
    } else {
        format!("{:02}:{:02}", t / 60, t % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        assert_eq!(fmt_db(0.0), "+0.0");
        assert_eq!(fmt_db(-6.04), "\u{2212}6.0");
        assert_eq!(fmt_db(-0.04), "+0.0");
        assert_eq!(khz(48000), "48 kHz");
        assert_eq!(khz(44100), "44.1 kHz");
        assert_eq!(clock(75.0), "01:15");
        assert_eq!(clock(3725.0), "1:02:05");
    }
}
