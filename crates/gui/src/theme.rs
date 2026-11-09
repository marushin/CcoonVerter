//! Colours, fonts sizes and the egui style (port of the Python `config.py` palette).

use eframe::egui::{
    self, Color32, CornerRadius, FontFamily, FontId, Margin, Stroke, TextStyle, Theme, Vec2,
};

/// Window background (`COLOR_BG`).
pub const BG: Color32 = Color32::from_rgb(0x1e, 0x1e, 0x1e);
/// Panel background (`COLOR_SURFACE`).
pub const PANEL: Color32 = Color32::from_rgb(0x2b, 0x2b, 0x2b);
/// Gold accent (`COLOR_ACCENT`).
pub const ACCENT: Color32 = Color32::from_rgb(0xD4, 0xAF, 0x37);
/// Slightly darker gold used on hover.
pub const ACCENT_HOVER: Color32 = Color32::from_rgb(0xb8, 0x96, 0x2e);
/// Normal text (`COLOR_TEXT`).
pub const TEXT: Color32 = Color32::from_rgb(0xe0, 0xe0, 0xe0);
/// Dimmed text (`COLOR_TEXT_DIM`).
pub const TEXT_DIM: Color32 = Color32::from_rgb(0xa0, 0xa0, 0xa0);
/// Subtle borders (`COLOR_BORDER`).
pub const BORDER: Color32 = Color32::from_rgb(0x3d, 0x3d, 0x3d);
/// Muted red for CANCEL / DELETE.
pub const DANGER: Color32 = Color32::from_rgb(0xe6, 0x4a, 0x19);
/// Slider rail (CustomTkinter's dark slider background).
pub const RAIL: Color32 = Color32::from_rgb(0x4a, 0x4d, 0x50);

pub const SLIDER_RED: Color32 = Color32::from_rgb(0xff, 0x52, 0x52);
pub const SLIDER_RED_HOVER: Color32 = Color32::from_rgb(0xff, 0x8a, 0x80);
pub const SLIDER_GREEN: Color32 = Color32::from_rgb(0x69, 0xf0, 0xae);
pub const SLIDER_GREEN_HOVER: Color32 = Color32::from_rgb(0xb9, 0xf6, 0xca);
pub const SLIDER_BLUE: Color32 = Color32::from_rgb(0x44, 0x8a, 0xff);
pub const SLIDER_BLUE_HOVER: Color32 = Color32::from_rgb(0x82, 0xb1, 0xff);
pub const SLIDER_WHITE: Color32 = Color32::from_rgb(0xff, 0xff, 0xff);
pub const SLIDER_WHITE_HOVER: Color32 = Color32::from_rgb(0xee, 0xee, 0xee);

/// Python paddings.
pub const PAD_LARGE: f32 = 15.0;
pub const PAD_MEDIUM: f32 = 6.0;

/// Name of the bold font family registered by [`crate::fonts`].
pub const BOLD_FAMILY: &str = "bold";

/// Python `FONT_HEADER` (Segoe UI 16 bold).
pub fn font_header() -> FontId {
    FontId::new(16.0, FontFamily::Name(BOLD_FAMILY.into()))
}

/// Python `FONT_BODY_BOLD` (Segoe UI 12 bold).
pub fn font_body_bold() -> FontId {
    FontId::new(13.0, FontFamily::Name(BOLD_FAMILY.into()))
}

/// Python `FONT_BODY` (Segoe UI 12).
pub fn font_body() -> FontId {
    FontId::new(13.0, FontFamily::Proportional)
}

/// Python `("Segoe UI", 10)` notes.
pub fn font_small() -> FontId {
    FontId::new(11.0, FontFamily::Proportional)
}

/// Python `("Segoe UI", 10, "bold")` (RESET button).
pub fn font_small_bold() -> FontId {
    FontId::new(11.0, FontFamily::Name(BOLD_FAMILY.into()))
}

/// Apply the dark/gold, square-cornered look to every egui style.
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
    // Vertical spacing is tighter than Python's grid so the whole window fits
    // the 600x820 default size without scrolling.
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
    v.window_corner_radius = CornerRadius::ZERO;
    v.menu_corner_radius = CornerRadius::ZERO;
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
    v.hyperlink_color = ACCENT;
    v.selection.bg_fill = Color32::from_rgb(0x6b, 0x58, 0x1c);
    v.selection.stroke = Stroke::new(1.0, TEXT);
    v.text_cursor.stroke = Stroke::new(2.0, ACCENT);
    v.slider_trailing_fill = true;

    let w = &mut v.widgets;
    for wv in [
        &mut w.noninteractive,
        &mut w.inactive,
        &mut w.hovered,
        &mut w.active,
        &mut w.open,
    ] {
        wv.corner_radius = CornerRadius::ZERO;
        wv.expansion = 0.0;
    }
    w.noninteractive.bg_fill = PANEL;
    w.noninteractive.weak_bg_fill = PANEL;
    w.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
    w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);

    w.inactive.bg_fill = BORDER;
    w.inactive.weak_bg_fill = BG;
    w.inactive.bg_stroke = Stroke::new(1.0, BORDER);
    w.inactive.fg_stroke = Stroke::new(1.0, TEXT);

    w.hovered.bg_fill = BORDER;
    w.hovered.weak_bg_fill = BORDER;
    w.hovered.bg_stroke = Stroke::new(1.0, ACCENT_HOVER);
    w.hovered.fg_stroke = Stroke::new(1.5, TEXT);

    w.active.bg_fill = BORDER;
    w.active.weak_bg_fill = BORDER;
    w.active.bg_stroke = Stroke::new(1.0, ACCENT);
    w.active.fg_stroke = Stroke::new(2.0, TEXT);

    w.open.bg_fill = PANEL;
    w.open.weak_bg_fill = PANEL;
    w.open.bg_stroke = Stroke::new(1.0, BORDER);
    w.open.fg_stroke = Stroke::new(1.0, TEXT);
}
