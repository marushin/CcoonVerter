//! Colours, font sizes, the radius scale and the egui style: "warm darkroom".
//!
//! Three rules. The area around the photo is strictly neutral grey, so colour
//! judgement is not biased ([`PHOTO_SURROUND`] and friends have R = G = B).
//! The chrome around it may be very slightly warm. The butter accent appears
//! only on the primary button and on the value being manipulated.

use eframe::egui::{
    self, Color32, CornerRadius, FontFamily, FontId, Margin, Stroke, TextStyle, Theme, Vec2,
};

/// Window background: very slightly warm.
pub const BG: Color32 = Color32::from_rgb(0x20, 0x1e, 0x1c);
/// Panel background.
pub const PANEL: Color32 = Color32::from_rgb(0x2b, 0x28, 0x25);
/// Inputs and buttons sitting on a panel.
pub const RAISED: Color32 = Color32::from_rgb(0x38, 0x34, 0x30);
/// Subtle borders.
pub const BORDER: Color32 = Color32::from_rgb(0x46, 0x41, 0x3b);
/// Normal text.
pub const TEXT: Color32 = Color32::from_rgb(0xee, 0xe8, 0xdf);
/// Dimmed text and small section titles.
pub const TEXT_DIM: Color32 = Color32::from_rgb(0xb5, 0xad, 0xa3);

/// Everything directly around the photo. R = G = B on purpose.
pub const PHOTO_SURROUND: Color32 = Color32::from_rgb(0x26, 0x26, 0x26);
/// Preview controls: neutral too, because they sit next to the photo.
pub const NEUTRAL_CONTROL: Color32 = Color32::from_rgb(0x3a, 0x3a, 0x3a);
pub const NEUTRAL_ACTIVE: Color32 = Color32::from_rgb(0x5c, 0x5c, 0x5c);
pub const NEUTRAL_TEXT: Color32 = Color32::from_rgb(0xe6, 0xe6, 0xe6);
pub const NEUTRAL_DIM: Color32 = Color32::from_rgb(0xb0, 0xb0, 0xb0);

/// Butter, the mean of the icon's cube face (`assets/icon_256.png`).
pub const ACCENT: Color32 = Color32::from_rgb(0xfc, 0xcb, 0x59);
/// The icon's lighter butter highlight, for hover.
pub const ACCENT_HOVER: Color32 = Color32::from_rgb(0xfc, 0xd5, 0x70);
/// Text on butter: the icon's brown outline (#853417), darkened for contrast.
pub const ON_ACCENT: Color32 = Color32::from_rgb(0x4a, 0x24, 0x10);

/// Errors and destructive actions.
pub const DANGER: Color32 = Color32::from_rgb(0xff, 0x8f, 0x70);
/// Slider rail.
pub const RAIL: Color32 = Color32::from_rgb(0x4e, 0x49, 0x43);
/// Slider fill while not being moved.
pub const FILL_IDLE: Color32 = Color32::from_rgb(0x9a, 0x93, 0x89);

/// Radius scale: small marks, controls, panels and cards.
pub const RADIUS_SMALL: u8 = 4;
pub const RADIUS_MEDIUM: u8 = 8;
pub const RADIUS_LARGE: u8 = 12;

pub const PAD_LARGE: f32 = 14.0;
pub const PAD_MEDIUM: f32 = 6.0;

/// Name of the bold font family registered by [`crate::fonts`].
pub const BOLD_FAMILY: &str = "bold";

/// Primary button text and card titles.
pub fn font_header() -> FontId {
    FontId::new(16.0, FontFamily::Name(BOLD_FAMILY.into()))
}

pub fn font_body_bold() -> FontId {
    FontId::new(13.0, FontFamily::Name(BOLD_FAMILY.into()))
}

pub fn font_body() -> FontId {
    FontId::new(13.0, FontFamily::Proportional)
}

/// Notes, section titles and style names.
pub fn font_small() -> FontId {
    FontId::new(11.5, FontFamily::Proportional)
}

pub fn font_small_bold() -> FontId {
    FontId::new(11.5, FontFamily::Name(BOLD_FAMILY.into()))
}

/// Apply the dark, rounded look to every egui style.
pub fn apply(ctx: &egui::Context) {
    ctx.set_theme(Theme::Dark);
    ctx.style_mut_of(Theme::Dark, configure_style);
}

fn configure_style(style: &mut egui::Style) {
    use TextStyle::*;
    style.text_styles = [
        (Small, font_small()),
        (Body, font_body()),
        (Button, font_body()),
        (Heading, font_header()),
        (Monospace, FontId::new(12.0, FontFamily::Monospace)),
    ]
    .into();

    let s = &mut style.spacing;
    s.item_spacing = Vec2::new(PAD_MEDIUM, 4.0);
    s.button_padding = Vec2::new(10.0, 4.0);
    s.interact_size = Vec2::new(40.0, 26.0);
    s.combo_height = 300.0;
    s.window_margin = Margin::same(14);
    s.menu_margin = Margin::same(4);

    let v = &mut style.visuals;
    v.dark_mode = true;
    v.panel_fill = BG;
    v.window_fill = PANEL;
    v.window_stroke = Stroke::new(1.0, BORDER);
    v.window_corner_radius = CornerRadius::same(RADIUS_LARGE);
    v.menu_corner_radius = CornerRadius::same(RADIUS_MEDIUM);
    v.window_shadow = egui::epaint::Shadow {
        offset: [0, 6],
        blur: 16,
        spread: 0,
        color: Color32::from_black_alpha(120),
    };
    v.popup_shadow = egui::epaint::Shadow {
        offset: [0, 4],
        blur: 8,
        spread: 0,
        color: Color32::from_black_alpha(100),
    };
    v.extreme_bg_color = BG;
    v.text_edit_bg_color = Some(BG);
    v.faint_bg_color = PANEL;
    v.hyperlink_color = TEXT;
    v.selection.bg_fill = Color32::from_rgb(0x6e, 0x58, 0x28);
    v.selection.stroke = Stroke::new(1.0, TEXT);
    v.text_cursor.stroke = Stroke::new(2.0, TEXT);
    v.slider_trailing_fill = true;

    let w = &mut v.widgets;
    for wv in [
        &mut w.noninteractive,
        &mut w.inactive,
        &mut w.hovered,
        &mut w.active,
        &mut w.open,
    ] {
        wv.corner_radius = CornerRadius::same(RADIUS_MEDIUM);
        wv.expansion = 0.0;
    }
    w.noninteractive.bg_fill = PANEL;
    w.noninteractive.weak_bg_fill = PANEL;
    w.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
    w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);

    w.inactive.bg_fill = RAISED;
    w.inactive.weak_bg_fill = RAISED;
    w.inactive.bg_stroke = Stroke::new(1.0, BORDER);
    w.inactive.fg_stroke = Stroke::new(1.0, TEXT);

    w.hovered.bg_fill = BORDER;
    w.hovered.weak_bg_fill = BORDER;
    w.hovered.bg_stroke = Stroke::new(1.0, FILL_IDLE);
    w.hovered.fg_stroke = Stroke::new(1.5, TEXT);

    w.active.bg_fill = BORDER;
    w.active.weak_bg_fill = BORDER;
    w.active.bg_stroke = Stroke::new(1.0, TEXT_DIM);
    w.active.fg_stroke = Stroke::new(2.0, TEXT);

    w.open.bg_fill = RAISED;
    w.open.weak_bg_fill = RAISED;
    w.open.bg_stroke = Stroke::new(1.0, BORDER);
    w.open.fg_stroke = Stroke::new(1.0, TEXT);
}

#[cfg(test)]
pub mod tests {
    use super::*;

    fn linear(c: u8) -> f64 {
        let v = f64::from(c) / 255.0;
        if v <= 0.03928 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    }

    fn luminance(c: Color32) -> f64 {
        0.2126 * linear(c.r()) + 0.7152 * linear(c.g()) + 0.0722 * linear(c.b())
    }

    /// WCAG 2.x contrast ratio.
    pub fn contrast(a: Color32, b: Color32) -> f64 {
        let (la, lb) = (luminance(a), luminance(b));
        let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
        (hi + 0.05) / (lo + 0.05)
    }

    #[test]
    fn text_contrast_meets_wcag_aa() {
        let pairs = [
            ("primary button text", ON_ACCENT, ACCENT),
            ("primary button text, hovered", ON_ACCENT, ACCENT_HOVER),
            ("text on background", TEXT, BG),
            ("text on panel", TEXT, PANEL),
            ("text on control", TEXT, RAISED),
            ("dim text on background", TEXT_DIM, BG),
            ("section title on panel", TEXT_DIM, PANEL),
            ("dim text on control", TEXT_DIM, RAISED),
            ("active value on panel", ACCENT, PANEL),
            ("selected segment text", ON_ACCENT, ACCENT),
            ("danger on panel", DANGER, PANEL),
            ("danger on background", DANGER, BG),
            ("preview control text", NEUTRAL_TEXT, NEUTRAL_CONTROL),
            ("preview active text", NEUTRAL_TEXT, NEUTRAL_ACTIVE),
            ("preview status text", NEUTRAL_DIM, PHOTO_SURROUND),
            ("preview text", NEUTRAL_TEXT, PHOTO_SURROUND),
        ];
        for (name, fg, bg) in pairs {
            let ratio = contrast(fg, bg);
            println!("{name}: {ratio:.2}:1");
            assert!(ratio >= 4.5, "{name}: {ratio:.2}");
        }
    }

    #[test]
    fn photo_surround_is_strictly_neutral() {
        for c in [
            PHOTO_SURROUND,
            NEUTRAL_CONTROL,
            NEUTRAL_ACTIVE,
            NEUTRAL_TEXT,
            NEUTRAL_DIM,
        ] {
            assert!(c.r() == c.g() && c.g() == c.b(), "{c:?}");
        }
    }

    #[test]
    fn chrome_is_only_slightly_warm() {
        for c in [BG, PANEL, RAISED, BORDER] {
            let warmth = i32::from(c.r()) - i32::from(c.b());
            assert!((1..=12).contains(&warmth), "{c:?}: {warmth}");
        }
    }
}
