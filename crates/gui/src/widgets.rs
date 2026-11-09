//! Small custom widgets for the "warm darkroom" look. Painting respects
//! `ui.is_enabled()`: a disabled `Ui` already fades its painter, and the
//! widgets do not react to input. Every widget reports its name to egui's
//! accessibility tree, which screen readers and the in-process UI tests use.

use std::ops::RangeInclusive;

use eframe::egui::{
    self, pos2, vec2, Align, Align2, Color32, CornerRadius, CursorIcon, FontId, Layout, Rect,
    Response, RichText, Role, Sense, Stroke, StrokeKind, TextureId, Ui, Vec2, WidgetInfo,
};

use crate::theme::{
    self, ACCENT, ACCENT_HOVER, BORDER, DANGER, FILL_IDLE, NEUTRAL_ACTIVE, NEUTRAL_CONTROL,
    NEUTRAL_TEXT, ON_ACCENT, RADIUS_MEDIUM, RADIUS_SMALL, RAIL, RAISED, TEXT, TEXT_DIM,
};

fn pointer_cursor(ui: &Ui, resp: &Response) {
    if resp.hovered() && ui.is_enabled() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
}

fn name_button(ui: &Ui, resp: &Response, text: &str) {
    let enabled = ui.is_enabled();
    resp.widget_info(|| WidgetInfo::labeled(Role::Button, enabled, text));
}

/// Tooltip shown whether the widget is enabled or disabled.
pub fn tip(resp: Response, text: &str) -> Response {
    resp.on_hover_text(text).on_disabled_hover_text(text)
}

/// The one butter-coloured button with dark brown text.
pub fn primary_button(ui: &mut Ui, text: &str, size: Vec2, font: FontId) -> Response {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    if ui.is_rect_visible(rect) {
        let fill = if resp.hovered() && ui.is_enabled() {
            ACCENT_HOVER
        } else {
            ACCENT
        };
        let p = ui.painter();
        p.rect_filled(rect, RADIUS_MEDIUM, fill);
        p.text(rect.center(), Align2::CENTER_CENTER, text, font, ON_ACCENT);
    }
    name_button(ui, &resp, text);
    pointer_cursor(ui, &resp);
    resp
}

/// Quiet raised button for everything that is not the primary action.
pub fn quiet_button(ui: &mut Ui, text: &str, size: Vec2, font: FontId) -> Response {
    outlined(ui, text, size, font, TEXT, RAISED, BORDER)
}

/// Quiet button for destructive actions (delete, cancel).
pub fn danger_button(ui: &mut Ui, text: &str, size: Vec2, font: FontId) -> Response {
    outlined(ui, text, size, font, DANGER, Color32::TRANSPARENT, RAISED)
}

/// Quiet button next to the photo: strictly neutral colours.
pub fn neutral_button(ui: &mut Ui, text: &str, size: Vec2, font: FontId, on: bool) -> Response {
    let fill = if on { NEUTRAL_ACTIVE } else { NEUTRAL_CONTROL };
    outlined(ui, text, size, font, NEUTRAL_TEXT, fill, NEUTRAL_ACTIVE)
}

fn outlined(
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
        p.rect_filled(rect, RADIUS_MEDIUM, bg);
        p.text(rect.center(), Align2::CENTER_CENTER, text, font, color);
    }
    name_button(ui, &resp, text);
    pointer_cursor(ui, &resp);
    resp
}

/// Colours of a [`segmented`] control.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegmentStyle {
    /// Butter on the selected segment (the mode of the adjustments).
    Accent,
    /// Neutral grey, for controls next to the photo.
    Neutral,
}

/// Two or more options, one selected. Returns the index that was clicked.
pub fn segmented(
    ui: &mut Ui,
    options: &[&str],
    selected: usize,
    size: Vec2,
    style: SegmentStyle,
) -> Option<usize> {
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let (track, on_fill, on_text, off_text) = match style {
        SegmentStyle::Accent => (RAISED, ACCENT, ON_ACCENT, TEXT),
        SegmentStyle::Neutral => (
            NEUTRAL_CONTROL,
            NEUTRAL_ACTIVE,
            NEUTRAL_TEXT,
            theme::NEUTRAL_DIM,
        ),
    };
    let painter = ui.painter().clone();
    painter.rect_filled(rect, RADIUS_MEDIUM, track);
    let w = rect.width() / options.len().max(1) as f32;
    let mut clicked = None;
    let enabled = ui.is_enabled();
    for (i, label) in options.iter().enumerate() {
        let seg = Rect::from_min_size(rect.min + vec2(w * i as f32, 0.0), vec2(w, rect.height()));
        let resp = ui.interact(seg, ui.scope_id().with(("segment", *label)), Sense::click());
        let on = i == selected;
        if on {
            painter.rect_filled(seg.shrink(2.0), RADIUS_MEDIUM - 2, on_fill);
        } else if resp.hovered() && enabled {
            painter.rect_filled(seg.shrink(2.0), RADIUS_MEDIUM - 2, BORDER);
        }
        let font = if on {
            theme::font_body_bold()
        } else {
            theme::font_body()
        };
        let color = if on { on_text } else { off_text };
        painter.text(seg.center(), Align2::CENTER_CENTER, *label, font, color);
        resp.widget_info(|| WidgetInfo::selected(Role::RadioButton, enabled, on, *label));
        pointer_cursor(ui, &resp);
        if resp.clicked() {
            clicked = Some(i);
        }
    }
    clicked
}

/// Rounded check box whose label is part of the click target.
pub fn checkbox(ui: &mut Ui, checked: &mut bool, label: &str, font: FontId) -> Response {
    let box_size = 18.0;
    let galley = ui.painter().layout_no_wrap(label.to_owned(), font, TEXT);
    let size = vec2(
        box_size + 8.0 + galley.size().x,
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
            p.rect_filled(b, RADIUS_SMALL, TEXT);
            let pts = [
                pos2(b.left() + 4.0, b.center().y),
                pos2(b.left() + 7.5, b.bottom() - 4.5),
                pos2(b.right() - 3.5, b.top() + 4.5),
            ];
            let stroke = Stroke::new(2.0, theme::PANEL);
            p.line_segment([pts[0], pts[1]], stroke);
            p.line_segment([pts[1], pts[2]], stroke);
        } else {
            p.rect_filled(b, RADIUS_SMALL, theme::BG);
            p.rect_stroke(
                b,
                RADIUS_SMALL,
                Stroke::new(1.5, if hovered { TEXT } else { TEXT_DIM }),
                StrokeKind::Inside,
            );
        }
        let pos = pos2(b.right() + 8.0, rect.center().y - galley.size().y / 2.0);
        p.galley(pos, galley, TEXT);
    }
    let enabled = ui.is_enabled();
    let on = *checked;
    resp.widget_info(|| WidgetInfo::selected(Role::CheckBox, enabled, on, label));
    pointer_cursor(ui, &resp);
    resp
}

/// What a [`slider`] edits.
#[derive(Debug, Clone)]
pub struct SliderSpec<'a> {
    pub name: &'a str,
    pub range: RangeInclusive<f32>,
    /// The fill is drawn from here to the value: centre for bipolar values,
    /// one end for a plain amount.
    pub origin: f32,
    /// Value restored by a double click.
    pub default: f32,
    pub step: f32,
}

/// Result of a [`slider`] this frame.
pub struct SliderResult {
    pub response: Response,
    /// The value is being dragged or nudged right now.
    pub active: bool,
}

/// Colours of a slider.
struct SliderPalette {
    rail: egui::Color32,
    fill: egui::Color32,
    handle: egui::Color32,
    hover: egui::Color32,
    active_fill: egui::Color32,
    active_handle: egui::Color32,
}

/// Horizontal slider, filled from `spec.origin` to the value. Grey until it
/// is being moved; double click returns it to `spec.default`.
pub fn slider(ui: &mut Ui, value: &mut f32, spec: &SliderSpec<'_>, width: f32) -> SliderResult {
    let palette = SliderPalette {
        rail: RAIL,
        fill: FILL_IDLE,
        handle: TEXT_DIM,
        hover: TEXT,
        active_fill: ACCENT,
        active_handle: ACCENT_HOVER,
    };
    slider_with(ui, value, spec, width, &palette)
}

/// A [`slider`] that stays strictly grey, for use right beside the photos.
pub fn neutral_slider(
    ui: &mut Ui,
    value: &mut f32,
    spec: &SliderSpec<'_>,
    width: f32,
) -> SliderResult {
    let palette = SliderPalette {
        rail: theme::NEUTRAL_CONTROL,
        fill: theme::NEUTRAL_ACTIVE,
        handle: theme::NEUTRAL_DIM,
        hover: theme::NEUTRAL_TEXT,
        active_fill: theme::NEUTRAL_ACTIVE,
        active_handle: theme::NEUTRAL_TEXT,
    };
    slider_with(ui, value, spec, width, &palette)
}

fn slider_with(
    ui: &mut Ui,
    value: &mut f32,
    spec: &SliderSpec<'_>,
    width: f32,
    palette: &SliderPalette,
) -> SliderResult {
    let height = 20.0;
    let (min, max) = (*spec.range.start(), *spec.range.end());
    let (rect, mut resp) = ui.allocate_exact_size(vec2(width, height), Sense::click_and_drag());
    let r = 8.0;
    let left = rect.left() + r;
    let right = (rect.right() - r).max(left + 1.0);

    if let Some(pos) = resp.interact_pointer_pos() {
        if resp.dragged() || resp.is_pointer_button_down_on() {
            let t = ((pos.x - left) / (right - left)).clamp(0.0, 1.0);
            let snapped = min + ((t * (max - min)) / spec.step).round() * spec.step;
            let v = snapped.clamp(min, max);
            if v != *value {
                *value = v;
                resp.mark_changed();
            }
        }
    }
    let mut nudged = false;
    if resp.has_focus() {
        let delta = ui.input(|i| {
            let mut d = 0.0;
            if i.key_pressed(egui::Key::ArrowLeft) || i.key_pressed(egui::Key::ArrowDown) {
                d -= spec.step;
            }
            if i.key_pressed(egui::Key::ArrowRight) || i.key_pressed(egui::Key::ArrowUp) {
                d += spec.step;
            }
            d
        });
        if delta != 0.0 {
            *value = (*value + delta).clamp(min, max);
            resp.mark_changed();
            nudged = true;
        }
    }
    if resp.double_clicked() && *value != spec.default {
        *value = spec.default;
        resp.mark_changed();
    }
    let active = resp.is_pointer_button_down_on() || resp.dragged() || nudged;

    if ui.is_rect_visible(rect) {
        let x_of = |v: f32| {
            let t = if max > min {
                ((v - min) / (max - min)).clamp(0.0, 1.0)
            } else {
                0.0
            };
            left + t * (right - left)
        };
        let (x, origin) = (x_of(*value), x_of(spec.origin));
        let cy = rect.center().y;
        let p = ui.painter();
        p.rect_filled(
            Rect::from_min_max(pos2(left, cy - 2.0), pos2(right, cy + 2.0)),
            2.0,
            palette.rail,
        );
        if origin > left + 0.5 && origin < right - 0.5 {
            // Centre mark of a bipolar value.
            p.rect_filled(
                Rect::from_center_size(pos2(origin, cy), vec2(2.0, 10.0)),
                1.0,
                palette.fill,
            );
        }
        let (color, handle) = if active {
            (palette.active_fill, palette.active_handle)
        } else if resp.hovered() {
            (palette.fill, palette.hover)
        } else {
            (palette.fill, palette.handle)
        };
        let (a, b) = (origin.min(x), origin.max(x));
        if b - a > 0.5 {
            p.rect_filled(
                Rect::from_min_max(pos2(a, cy - 2.0), pos2(b, cy + 2.0)),
                2.0,
                color,
            );
        }
        p.circle_filled(pos2(x, cy), r, handle);
    }
    let enabled = ui.is_enabled();
    let shown = f64::from(*value);
    resp.widget_info(|| WidgetInfo::slider(enabled, shown, spec.name));
    if resp.hovered() && ui.is_enabled() {
        ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
    }
    SliderResult {
        response: resp,
        active,
    }
}

/// Thin progress bar. Neutral: the accent stays on the button that started it.
pub fn thin_progress(ui: &mut Ui, fraction: f32, height: f32) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, height / 2.0, RAIL);
    let f = fraction.clamp(0.0, 1.0);
    if f > 0.0 {
        let mut fill = rect;
        fill.max.x = rect.left() + rect.width() * f;
        p.rect_filled(fill, height / 2.0, TEXT);
    }
}

/// Small grey section title.
pub fn section_title(ui: &mut Ui, text: &str) -> Response {
    ui.label(
        RichText::new(text)
            .font(theme::font_small_bold())
            .color(TEXT_DIM),
    )
}

/// Dim body label.
pub fn dim_label(ui: &mut Ui, text: &str) -> Response {
    ui.label(RichText::new(text).font(theme::font_body()).color(TEXT_DIM))
}

/// Small note, wrapped to the available width.
pub fn note(ui: &mut Ui, text: &str) {
    ui.label(
        RichText::new(text)
            .font(theme::font_small())
            .color(TEXT_DIM),
    );
}

/// Value beside a slider; butter while that slider is being moved.
pub fn value_label(ui: &mut Ui, text: &str, active: bool) -> Response {
    let color = if active { ACCENT } else { TEXT };
    ui.label(RichText::new(text).font(theme::font_body()).color(color))
}

/// One style in the style picker: picture on top, name below.
pub fn style_tile(
    ui: &mut Ui,
    texture: TextureId,
    uv: Rect,
    name: &str,
    selected: bool,
    size: Vec2,
) -> Response {
    let name_h = 16.0;
    let (rect, resp) = ui.allocate_exact_size(size + vec2(0.0, name_h), Sense::click());
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        let img = Rect::from_min_size(rect.min, size);
        egui::Image::from_texture((texture, size))
            .uv(uv)
            .corner_radius(CornerRadius::same(RADIUS_SMALL + 2))
            .paint_at(ui, img);
        if selected {
            p.rect_stroke(
                img,
                RADIUS_SMALL + 2,
                Stroke::new(2.0, ACCENT),
                StrokeKind::Inside,
            );
        } else if resp.hovered() && ui.is_enabled() {
            p.rect_stroke(
                img,
                RADIUS_SMALL + 2,
                Stroke::new(1.0, TEXT_DIM),
                StrokeKind::Inside,
            );
        }
        let color = if selected { ACCENT } else { TEXT_DIM };
        p.text(
            pos2(rect.center().x, img.bottom() + name_h / 2.0 + 1.0),
            Align2::CENTER_CENTER,
            name,
            theme::font_small(),
            color,
        );
    }
    let enabled = ui.is_enabled();
    resp.widget_info(|| WidgetInfo::selected(Role::RadioButton, enabled, selected, name));
    pointer_cursor(ui, &resp);
    resp
}

/// Column widths of a settings grid (label | control | value).
#[derive(Debug, Clone, Copy)]
pub struct Columns {
    pub label: f32,
    pub value: f32,
    pub row_height: f32,
}

impl Columns {
    pub const SETTINGS: Columns = Columns {
        label: 96.0,
        value: 50.0,
        row_height: 26.0,
    };

    /// One grid row: `label` cell, an expanding control cell (receives its
    /// width and returns a value), and a `value` cell that receives the
    /// control's result. Cells are vertically centred.
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
            cell(ui, self.value, h, Align::Max, |ui| value(ui, &r));
            r
        })
        .inner
    }

    /// Row whose control spans the control and value columns.
    pub fn wide_row(
        &self,
        ui: &mut Ui,
        label: impl FnOnce(&mut Ui),
        control: impl FnOnce(&mut Ui, f32),
    ) {
        ui.horizontal(|ui| {
            cell(ui, self.label, self.row_height, Align::Min, label);
            let w = (ui.available_width() - 4.0).max(80.0);
            cell(ui, w, self.row_height, Align::Min, |ui| control(ui, w));
        });
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

/// Rounded text field on the dark background.
pub fn text_field(ui: &mut Ui, text: &mut String, hint: &str, width: f32) -> Response {
    let mut edit = egui::TextEdit::singleline(text)
        .desired_width(width)
        .font(theme::font_body())
        .text_color(TEXT)
        .background_color(theme::BG)
        .margin(egui::Margin::symmetric(8, 6));
    if !hint.is_empty() {
        edit = edit.hint_text(RichText::new(hint.to_owned()).color(TEXT_DIM));
    }
    ui.add(edit)
}
