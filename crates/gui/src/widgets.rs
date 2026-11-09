//! Small custom widgets reproducing the CustomTkinter look of the Python app
//! (flat gold buttons, outlined buttons, coloured sliders, thin progress bar,
//! gold check boxes). Painting respects `ui.is_enabled()`: a disabled `Ui`
//! already fades its painter, and the widgets do not react to input.

use std::ops::RangeInclusive;

use eframe::egui::{
    self, pos2, vec2, Align, Align2, Color32, CursorIcon, FontId, Layout, Rect, Response, RichText,
    Sense, Stroke, StrokeKind, Ui, Vec2,
};

use crate::theme::{self, ACCENT, ACCENT_HOVER, BORDER, PANEL, RAIL, TEXT, TEXT_DIM};

fn pointer_cursor(ui: &Ui, resp: &Response) {
    if resp.hovered() && ui.is_enabled() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
}

/// Flat gold button with dark text (BROWSE, SAVE, "Let's Get Cooking!", DOWNLOAD).
pub fn gold_button(ui: &mut Ui, text: &str, size: Vec2, font: FontId) -> Response {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    if ui.is_rect_visible(rect) {
        let fill = if resp.is_pointer_button_down_on() || resp.hovered() {
            ACCENT_HOVER
        } else {
            ACCENT
        };
        let p = ui.painter();
        p.rect_filled(rect, 0.0, fill);
        p.text(rect.center(), Align2::CENTER_CENTER, text, font, PANEL);
    }
    pointer_cursor(ui, &resp);
    resp
}

/// Outlined button (CANCEL, DELETE, RESET, LATER).
pub fn outline_button(
    ui: &mut Ui,
    text: &str,
    size: Vec2,
    font: FontId,
    color: Color32,
    fill: Color32,
    hover_fill: Color32,
) -> Response {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    if ui.is_rect_visible(rect) {
        let bg = if resp.hovered() { hover_fill } else { fill };
        let p = ui.painter();
        p.rect_filled(rect, 0.0, bg);
        p.rect_stroke(rect, 0.0, Stroke::new(1.0, color), StrokeKind::Inside);
        p.text(rect.center(), Align2::CENTER_CENTER, text, font, color);
    }
    pointer_cursor(ui, &resp);
    resp
}

/// Gold square check box. The label (if any) is drawn to the right.
pub fn checkbox(ui: &mut Ui, checked: &mut bool, label: &str, font: FontId) -> Response {
    let box_size = 18.0;
    let galley =
        (!label.is_empty()).then(|| ui.painter().layout_no_wrap(label.to_owned(), font, TEXT));
    let text_w = galley.as_ref().map_or(0.0, |g| g.size().x + 8.0);
    let size = vec2(
        box_size + text_w,
        box_size.max(ui.spacing().interact_size.y),
    );
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click());
    if resp.clicked() {
        *checked = !*checked;
        resp.mark_changed();
    }
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        let b = Rect::from_center_size(
            pos2(rect.left() + box_size / 2.0, rect.center().y),
            vec2(box_size, box_size),
        );
        let hovered = resp.hovered();
        if *checked {
            p.rect_filled(b, 0.0, if hovered { ACCENT_HOVER } else { ACCENT });
            // Check mark.
            let pts = [
                pos2(b.left() + 4.0, b.center().y),
                pos2(b.left() + 7.5, b.bottom() - 4.5),
                pos2(b.right() - 3.5, b.top() + 4.5),
            ];
            let stroke = Stroke::new(2.0, PANEL);
            p.line_segment([pts[0], pts[1]], stroke);
            p.line_segment([pts[1], pts[2]], stroke);
        } else {
            p.rect_filled(b, 0.0, theme::BG);
            p.rect_stroke(
                b,
                0.0,
                Stroke::new(1.5, if hovered { ACCENT_HOVER } else { TEXT_DIM }),
                StrokeKind::Inside,
            );
        }
        if let Some(g) = galley {
            let pos = pos2(b.right() + 8.0, rect.center().y - g.size().y / 2.0);
            p.galley(pos, g, TEXT);
        }
    }
    pointer_cursor(ui, &resp);
    resp
}

/// Horizontal slider with a coloured trailing fill and a round handle
/// (CTkSlider look). `step` snaps the value (e.g. 1.0 for integer sliders).
#[allow(clippy::too_many_arguments)]
pub fn color_slider(
    ui: &mut Ui,
    value: &mut f32,
    range: RangeInclusive<f32>,
    step: Option<f32>,
    color: Color32,
    hover: Color32,
    width: f32,
) -> Response {
    let height = 20.0;
    let (min, max) = (*range.start(), *range.end());
    let (rect, mut resp) = ui.allocate_exact_size(vec2(width, height), Sense::click_and_drag());
    let r = 8.0;
    let left = rect.left() + r;
    let right = (rect.right() - r).max(left + 1.0);

    if let Some(pos) = resp.interact_pointer_pos() {
        if resp.dragged() || resp.is_pointer_button_down_on() {
            let t = ((pos.x - left) / (right - left)).clamp(0.0, 1.0);
            let mut v = min + t * (max - min);
            if let Some(s) = step.filter(|s| *s > 0.0) {
                v = min + ((v - min) / s).round() * s;
            }
            let v = v.clamp(min, max);
            if v != *value {
                *value = v;
                resp.mark_changed();
            }
        }
    }
    // Keyboard nudging when focused (accessibility nicety).
    if resp.has_focus() {
        let s = step.unwrap_or((max - min) / 100.0);
        let delta = ui.input(|i| {
            let mut d = 0.0;
            if i.key_pressed(egui::Key::ArrowLeft) || i.key_pressed(egui::Key::ArrowDown) {
                d -= s;
            }
            if i.key_pressed(egui::Key::ArrowRight) || i.key_pressed(egui::Key::ArrowUp) {
                d += s;
            }
            d
        });
        if delta != 0.0 {
            *value = (*value + delta).clamp(min, max);
            resp.mark_changed();
        }
    }

    if ui.is_rect_visible(rect) {
        let t = if max > min {
            ((*value - min) / (max - min)).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let x = left + t * (right - left);
        let cy = rect.center().y;
        let p = ui.painter();
        let rail = Rect::from_min_max(pos2(left, cy - 2.0), pos2(right, cy + 2.0));
        p.rect_filled(rail, 2.0, RAIL);
        p.rect_filled(
            Rect::from_min_max(rail.min, pos2(x, rail.max.y)),
            2.0,
            color,
        );
        let active = resp.hovered() || resp.dragged();
        p.circle_filled(pos2(x, cy), r, if active { hover } else { color });
    }
    pointer_cursor(ui, &resp);
    resp
}

/// Thin flat progress bar (4 px, gold).
pub fn thin_progress(ui: &mut Ui, fraction: f32, height: f32) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 0.0, RAIL);
    let f = fraction.clamp(0.0, 1.0);
    if f > 0.0 {
        let mut fill = rect;
        fill.max.x = rect.left() + rect.width() * f;
        p.rect_filled(fill, 0.0, ACCENT);
    }
}

/// Gold header text (Python `FONT_HEADER` + `COLOR_ACCENT`); dimmed when the `Ui` is disabled.
pub fn header(ui: &mut Ui, text: &str) -> Response {
    let color = if ui.is_enabled() { ACCENT } else { TEXT_DIM };
    ui.label(RichText::new(text).font(theme::font_header()).color(color))
}

/// Dim body label.
pub fn dim_label(ui: &mut Ui, text: &str) -> Response {
    ui.label(RichText::new(text).font(theme::font_body()).color(TEXT_DIM))
}

/// Normal value label.
pub fn value_label(ui: &mut Ui, text: &str) -> Response {
    ui.label(RichText::new(text).font(theme::font_body()).color(TEXT))
}

/// Column widths of the Python three-column grid (label | control | value).
#[derive(Debug, Clone, Copy)]
pub struct Columns {
    pub label: f32,
    pub value: f32,
    pub row_height: f32,
}

impl Columns {
    pub const SETTINGS: Columns = Columns {
        label: 170.0,
        value: 64.0,
        row_height: 25.0,
    };

    /// One grid row: `label` cell, an expanding control cell (receives its
    /// width and returns a value), and a `value` cell that receives the
    /// control's result (e.g. the slider value to print). Cells are vertically centred.
    pub fn row<R>(
        &self,
        ui: &mut Ui,
        label: impl FnOnce(&mut Ui),
        control: impl FnOnce(&mut Ui, f32) -> R,
        value: impl FnOnce(&mut Ui, &R),
    ) -> R {
        let spacing = ui.spacing().item_spacing.x;
        ui.horizontal(|ui| {
            let h = self.row_height;
            cell(ui, self.label, h, Align::Min, label);
            let ctrl_w = (ui.available_width() - self.value - spacing).max(60.0);
            let r = cell(ui, ctrl_w, h, Align::Min, |ui| control(ui, ctrl_w));
            cell(ui, self.value, h, Align::Center, |ui| value(ui, &r));
            r
        })
        .inner
    }
}

/// Fixed-size cell laid out left-to-right (or centred) with vertical centring.
pub fn cell<R>(
    ui: &mut Ui,
    width: f32,
    height: f32,
    align: Align,
    add: impl FnOnce(&mut Ui) -> R,
) -> R {
    let layout = match align {
        Align::Center => Layout::centered_and_justified(egui::Direction::LeftToRight),
        Align::Max => Layout::right_to_left(Align::Center),
        Align::Min => Layout::left_to_right(Align::Center),
    };
    ui.allocate_ui_with_layout(vec2(width, height), layout, |ui| {
        ui.set_min_size(vec2(width, height));
        ui.set_max_width(width);
        add(ui)
    })
    .inner
}

/// Text field with the Python entry look (dark background, square border).
pub fn text_field(ui: &mut Ui, text: &mut String, hint: &str, width: f32) -> Response {
    let mut edit = egui::TextEdit::singleline(text)
        .desired_width(width)
        .font(theme::font_body())
        .text_color(TEXT)
        .background_color(theme::BG)
        .margin(egui::Margin::symmetric(6, 5));
    if !hint.is_empty() {
        edit = edit.hint_text(RichText::new(hint.to_owned()).color(Color32::from_gray(0x70)));
    }
    ui.scope(|ui| {
        let w = &mut ui.visuals_mut().widgets;
        w.inactive.bg_stroke = Stroke::new(1.0, BORDER);
        ui.add(edit)
    })
    .inner
}
