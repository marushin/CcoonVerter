//! The preview panel: state and drawing on the UI thread. All heavy work is
//! in [`crate::preview`]; this module only posts requests and shows results.
//!
//! Everything painted here is neutral grey on purpose (see [`crate::theme`]).

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ccoonverter_core::render::{Region, RenderParams};
use ccoonverter_core::{style, ConvertSettings, PictureStyle, RgbF32Image};
use eframe::egui::{
    self, pos2, vec2, Align, Align2, Color32, ColorImage, CursorIcon, Layout, Painter, Rect,
    Response, RichText, Role, Sense, Stroke, StrokeKind, TextureFilter, TextureHandle, TextureId,
    TextureOptions, Ui, WidgetInfo,
};

use crate::controller::DropSelection;
use crate::i18n::{t, tf, Key};
use crate::preview::{
    desired_request, short_message, view_origin, Histogram, PreviewEvent, PreviewWorker,
    RenderRequest, ThumbRequest, ZoomRequest, HISTOGRAM_BINS,
};
use crate::theme::{
    self, NEUTRAL_DIM, NEUTRAL_TEXT, PHOTO_SURROUND, RADIUS_LARGE, RADIUS_MEDIUM, RADIUS_SMALL,
};
use crate::widgets::{self, tip, SegmentStyle};

const TEX_OPTIONS: TextureOptions =
    TextureOptions::LINEAR.with_mipmap_mode(Some(TextureFilter::Linear));
/// Crops are drawn 1:1, so sampling must not interpolate.
const CROP_OPTIONS: TextureOptions = TextureOptions::NEAREST;
const THUMB_OPTIONS: TextureOptions = TextureOptions::LINEAR;

/// Wait this long after the source text stops changing before scanning.
const SCAN_DEBOUNCE: Duration = Duration::from_millis(350);
/// Style thumbnails are refreshed once the settings have been still this long.
const THUMB_DEBOUNCE: Duration = Duration::from_millis(250);
/// How often to ask again for thumbnails the worker gave up on.
const THUMB_RETRY: Duration = Duration::from_millis(500);
const FULL_UV: Rect = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));

struct Frame {
    texture: TextureHandle,
    size: [usize; 2],
}

/// Full-resolution pixels of the 100 % view.
struct Crop {
    file: PathBuf,
    region: Region,
    developed: Option<Frame>,
    camera: Option<Frame>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Zoom {
    Fit,
    Actual,
}

pub struct PreviewPanel {
    worker: Option<PreviewWorker>,
    /// Expanded state in the narrow layout (always visible when wide).
    pub open: bool,
    /// Source the file list was built from; empty until first use.
    source_key: Option<String>,
    scan_due: Option<Instant>,
    files: Vec<PathBuf>,
    index: usize,
    last_sent: Option<RenderRequest>,
    developed: Option<Frame>,
    embedded: Option<Frame>,
    histogram: Option<Histogram>,
    camera_histogram: Option<Histogram>,
    /// The worker has reported whether the current file has a camera JPEG.
    camera_checked: bool,
    /// See `RenderRequest::session`.
    session: u64,
    /// A batch is running: no new preview work is requested.
    paused: bool,
    /// Short message shown instead of an image (no files, failure, ...).
    message: Option<String>,
    status: String,

    zoom: Zoom,
    /// Centre of the 100 % view, 0..1 on the whole image.
    center: [f32; 2],
    /// Upright size of the full-resolution image, once known.
    full_size: Option<[u32; 2]>,
    /// View size in physical pixels, from the last draw.
    view_px: [u32; 2],
    /// From the GPU, via egui; 100 % crops stay within it.
    max_texture_side: u32,
    crop: Option<Crop>,
    zoom_sent: Option<ZoomRequest>,

    compare: bool,
    /// Divider position of the split compare, 0..1 across the picture.
    split: f32,
    dragging_split: bool,
    show_histogram: bool,

    icon: Option<TextureHandle>,
    placeholders: Vec<(PictureStyle, TextureHandle)>,
    thumbs: Vec<(PictureStyle, TextureHandle)>,
    thumbs_done: Option<ThumbRequest>,
    thumb_key: Option<ThumbRequest>,
    thumb_changed: Instant,
    thumb_sent: Option<Instant>,
    #[cfg(test)]
    injected: Option<(PathBuf, [u32; 2], Region, ColorImage)>,
}

impl PreviewPanel {
    pub fn new(worker: Option<PreviewWorker>, open: bool) -> Self {
        Self {
            worker,
            open,
            source_key: None,
            scan_due: None,
            files: Vec::new(),
            index: 0,
            last_sent: None,
            developed: None,
            embedded: None,
            histogram: None,
            camera_histogram: None,
            camera_checked: false,
            session: 0,
            paused: false,
            message: None,
            status: String::new(),
            zoom: Zoom::Fit,
            center: [0.5, 0.5],
            full_size: None,
            view_px: [1, 1],
            max_texture_side: 2048,
            crop: None,
            zoom_sent: None,
            compare: false,
            split: 0.5,
            dragging_split: false,
            show_histogram: true,
            icon: None,
            placeholders: Vec::new(),
            thumbs: Vec::new(),
            thumbs_done: None,
            thumb_key: None,
            thumb_changed: Instant::now(),
            thumb_sent: None,
            #[cfg(test)]
            injected: None,
        }
    }

    fn current(&self) -> Option<&PathBuf> {
        self.files.get(self.index)
    }

    /// No folder chosen yet: the drop target is the main thing on screen.
    pub fn is_empty(&self) -> bool {
        self.source_key.as_deref().is_none_or(str::is_empty)
            && self.files.is_empty()
            && self.developed.is_none()
            && self.embedded.is_none()
    }

    fn clear_images(&mut self) {
        self.developed = None;
        self.embedded = None;
        self.histogram = None;
        self.camera_histogram = None;
        self.camera_checked = false;
        self.session += 1;
        self.full_size = None;
        self.crop = None;
        self.zoom_sent = None;
        self.center = [0.5, 0.5];
        self.thumbs.clear();
        self.thumbs_done = None;
        self.thumb_key = None;
    }

    fn set_files(&mut self, files: Vec<PathBuf>) {
        // A new list has nothing to do with the photo being viewed at 100 %.
        self.set_zoom(Zoom::Fit);
        self.files = files;
        self.index = 0;
        self.clear_images();
        self.last_sent = None;
        self.message = self.files.is_empty().then(|| t(Key::NoRawHere).to_string());
    }

    fn step(&mut self, delta: isize) {
        let n = self.files.len();
        if n == 0 {
            return;
        }
        self.index = (self.index as isize + delta).rem_euclid(n as isize) as usize;
        self.clear_images();
        self.message = None;
    }

    fn set_zoom(&mut self, zoom: Zoom) {
        if self.zoom == zoom {
            return;
        }
        self.zoom = zoom;
        if zoom == Zoom::Fit {
            self.crop = None;
            self.zoom_sent = None;
            if let Some(w) = &self.worker {
                w.release_full();
            }
        }
    }

    /// Start or stop the worker's input for this frame and apply its results.
    pub fn tick(
        &mut self,
        ctx: &egui::Context,
        source_text: &str,
        selection: Option<&DropSelection>,
        settings: &ConvertSettings,
        converting: bool,
        visible: bool,
    ) {
        self.ensure_placeholders(ctx);
        self.max_texture_side = ctx.input(|i| i.max_texture_side).min(u32::MAX as usize) as u32;
        #[cfg(test)]
        self.apply_injected_crop(ctx);
        let Some(worker) = self.worker.as_ref() else {
            return;
        };
        let events = worker.drain();
        for e in events {
            self.apply(ctx, e);
        }
        if settings.extract_embedded {
            // Nothing is being developed in this mode, so no development
            // status may stay on the photo.
            self.status.clear();
        }
        if converting && !self.paused {
            // A batch needs the memory more than the 100 % view does.
            self.crop = None;
            self.zoom_sent = None;
            if let Some(w) = &self.worker {
                w.release_full();
            }
        }
        self.paused = converting;

        let key = selection
            .map(selection_key)
            .unwrap_or_else(|| source_text.trim().to_owned());
        if self.source_key.as_deref() != Some(key.as_str()) {
            self.source_key = Some(key.clone());
            self.scan_due = None;
            self.set_files(Vec::new());
            if let Some(s) = selection {
                self.set_files(s.files.clone());
            } else if key.is_empty() {
                self.message = None;
            } else {
                self.message = Some(t(Key::LookingForRaw).to_string());
                self.scan_due = Some(Instant::now() + SCAN_DEBOUNCE);
            }
        }
        if !visible {
            // Hidden: do not keep the full-resolution image for it.
            self.set_zoom(Zoom::Fit);
            return;
        }
        if let Some(due) = self.scan_due {
            if Instant::now() >= due {
                self.scan_due = None;
                if let Some(w) = &self.worker {
                    w.request_scan(PathBuf::from(source_text.trim()));
                }
            } else {
                ctx.request_repaint_after(due - Instant::now());
            }
        }
        if converting {
            return;
        }
        let desired = desired_request(&self.files, self.index, settings).map(|mut r| {
            r.session = self.session;
            r
        });
        if desired.is_some() && desired != self.last_sent {
            if let (Some(w), Some(req)) = (self.worker.as_mut(), desired.clone()) {
                w.request_render(req);
                self.last_sent = desired;
            }
        }
        self.request_thumbnails(ctx, settings);
        self.request_zoom(settings);
    }

    fn request_thumbnails(&mut self, ctx: &egui::Context, settings: &ConvertSettings) {
        if settings.extract_embedded || self.developed.is_none() {
            return;
        }
        let Some(file) = self.current().cloned() else {
            return;
        };
        let key = ThumbRequest::new(file, &RenderParams::for_display(settings));
        if self.thumb_key.as_ref() != Some(&key) {
            self.thumb_key = Some(key.clone());
            self.thumb_changed = Instant::now();
        }
        if self.thumbs_done.as_ref() == Some(&key) {
            return;
        }
        let since = self.thumb_changed.elapsed();
        if since < THUMB_DEBOUNCE {
            ctx.request_repaint_after(THUMB_DEBOUNCE - since);
            return;
        }
        if self.thumb_sent.is_some_and(|t| t.elapsed() < THUMB_RETRY) {
            ctx.request_repaint_after(THUMB_RETRY);
            return;
        }
        if let Some(w) = &self.worker {
            w.request_thumbnails(key);
            self.thumb_sent = Some(Instant::now());
            ctx.request_repaint_after(THUMB_RETRY);
        }
    }

    fn request_zoom(&mut self, settings: &ConvertSettings) {
        if self.zoom != Zoom::Actual {
            return;
        }
        let Some(file) = self.current().cloned() else {
            return;
        };
        let params = (!settings.extract_embedded).then(|| RenderParams::for_display(settings));
        let want_camera = self.compare || settings.extract_embedded;
        let same_settings = self
            .zoom_sent
            .as_ref()
            .is_some_and(|s| s.file == file && s.params == params && s.want_camera == want_camera);
        if same_settings && self.crop_covers_view(&file) {
            return;
        }
        let req = ZoomRequest {
            file,
            params,
            center: self.center,
            view_px: self.view_px,
            want_camera,
            max_side: self.max_texture_side,
        };
        if self.zoom_sent.as_ref() == Some(&req) {
            return;
        }
        if let Some(w) = self.worker.as_mut() {
            w.request_zoom(req.clone());
            self.zoom_sent = Some(req);
        }
    }

    /// The current crop already holds every pixel of the current view.
    fn crop_covers_view(&self, file: &PathBuf) -> bool {
        let (Some(crop), Some(full)) = (&self.crop, self.full_size) else {
            return false;
        };
        if crop.file != *file {
            return false;
        }
        let origin = view_origin(full, self.center, self.view_px);
        let inside = |i: usize, start: u32, len: u32| {
            let a = origin[i].max(0);
            let b = (origin[i] + i64::from(self.view_px[i])).min(i64::from(full[i]));
            a >= i64::from(start) && b <= i64::from(start + len)
        };
        inside(0, crop.region.x, crop.region.w) && inside(1, crop.region.y, crop.region.h)
    }

    fn apply(&mut self, ctx: &egui::Context, event: PreviewEvent) {
        match event {
            PreviewEvent::Scanned { root, files, error } => {
                // A scan that was still running when the folder changed, or
                // when files were dropped, says nothing about what is shown.
                if self.source_key.as_deref().map(Path::new) != Some(root.as_path()) {
                    log::debug!("Ignoring the scan of {}", root.display());
                    return;
                }
                self.set_files(files);
                self.message = match &error {
                    Some(e) => {
                        log::warn!("Preview scan of {} failed: {e}", root.display());
                        Some(tf(Key::FileSystemError, &[e]))
                    }
                    None if self.files.is_empty() => Some(t(Key::NoRawInFolder).to_string()),
                    None => None,
                };
                self.status = tf(Key::RawCount, &[&self.files.len()]);
            }
            PreviewEvent::Developing { file } => {
                if self.current() == Some(&file) {
                    self.status = t(Key::PreviewDeveloping).to_string();
                    self.message = None;
                }
            }
            PreviewEvent::Embedded {
                file,
                image,
                histogram,
            } => {
                if self.current() == Some(&file) {
                    self.camera_checked = true;
                    self.camera_histogram = histogram;
                    self.embedded = image.map(|i| upload(ctx, "preview_camera", (*i).clone()));
                    if self.embedded.is_some() && self.status.is_empty() {
                        self.status = t(Key::PreviewDeveloping).to_string();
                    }
                }
            }
            PreviewEvent::Developed {
                file,
                image,
                histogram,
                full_size,
                ..
            } => {
                if self.current() == Some(&file) {
                    match self.developed.take() {
                        Some(mut frame) => {
                            frame.size = image.size;
                            frame.texture.set(image, TEX_OPTIONS);
                            self.developed = Some(frame);
                        }
                        None => self.developed = Some(upload(ctx, "preview_dev", image)),
                    }
                    self.histogram = Some(histogram);
                    self.full_size = Some(full_size);
                    self.status.clear();
                    self.message = None;
                }
            }
            PreviewEvent::Zoomed {
                file,
                full_size,
                region,
                developed,
                camera,
            } => {
                if self.current() == Some(&file) && self.zoom == Zoom::Actual && !self.paused {
                    self.full_size = Some(full_size);
                    let frame = |name, image, options| Frame {
                        size: [region.w as usize, region.h as usize],
                        texture: ctx.load_texture(name, image, options),
                    };
                    self.crop = Some(Crop {
                        file,
                        region,
                        developed: developed.map(|i| frame("zoom_dev", i, CROP_OPTIONS)),
                        camera: camera.map(|i| {
                            let options =
                                if [i.size[0] as u32, i.size[1] as u32] == [region.w, region.h] {
                                    CROP_OPTIONS
                                } else {
                                    TextureOptions::LINEAR
                                };
                            frame("zoom_camera", i, options)
                        }),
                    });
                }
            }
            PreviewEvent::ZoomFailed { file, message } => {
                if self.current() == Some(&file) && self.zoom == Zoom::Actual {
                    self.set_zoom(Zoom::Fit);
                    let has_image = self.developed.is_some() || self.embedded.is_some();
                    if has_image && self.message.is_none() {
                        self.message = Some(tf(Key::ZoomFailed, &[&short_message(&message)]));
                    }
                }
            }
            PreviewEvent::Thumbnails { request, images } => {
                if self.current() == Some(&request.file) {
                    self.thumbs = images
                        .iter()
                        .map(|(s, img)| {
                            let name = format!("style_thumb_{s:?}");
                            (*s, ctx.load_texture(name, (**img).clone(), THUMB_OPTIONS))
                        })
                        .collect();
                    self.thumbs_done = Some(request);
                }
            }
            PreviewEvent::Failed { file, message } => {
                if self.current() == Some(&file) {
                    self.developed = None;
                    self.status.clear();
                    let what = short_message(&message);
                    self.message = Some(if self.embedded.is_some() {
                        tf(Key::PreviewCantDevelop, &[&what])
                    } else {
                        tf(Key::PreviewUnavailable, &[&what])
                    });
                }
            }
        }
    }

    fn ensure_placeholders(&mut self, ctx: &egui::Context) {
        if !self.placeholders.is_empty() {
            return;
        }
        for s in PictureStyle::ALL {
            let name = format!("style_placeholder_{s:?}");
            let texture = ctx.load_texture(name, placeholder_image(s), THUMB_OPTIONS);
            self.placeholders.push((s, texture));
        }
    }

    /// Picture for a style tile: its real thumbnail, or the placeholder.
    pub fn style_thumbnail(&self, style: PictureStyle) -> Option<(TextureId, [usize; 2])> {
        let find = |v: &[(PictureStyle, TextureHandle)]| {
            v.iter()
                .find(|(s, _)| *s == style)
                .map(|(_, tex)| (tex.id(), tex.size()))
        };
        find(&self.thumbs).or_else(|| find(&self.placeholders))
    }

    /// The application icon as a texture.
    pub fn icon(&mut self, ctx: &egui::Context) -> Option<TextureHandle> {
        if self.icon.is_none() {
            match image::load_from_memory_with_format(crate::ICON_PNG, image::ImageFormat::Png) {
                Ok(img) => {
                    let rgba = img.into_rgba8();
                    let size = [rgba.width() as usize, rgba.height() as usize];
                    let image = ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
                    self.icon = Some(ctx.load_texture("app_icon", image, TEX_OPTIONS));
                }
                Err(e) => log::warn!("Could not decode the icon: {e}"),
            }
        }
        self.icon.clone()
    }

    fn draw_toolbar(&mut self, ui: &mut Ui, embedded_mode: bool) {
        let n = self.files.len();
        let mut step = 0isize;
        let font = theme::font_small_bold();
        let has_image = self.developed.is_some() || self.embedded.is_some();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.add_enabled_ui(n > 1, |ui| {
                let nav = vec2(26.0, 24.0);
                if widgets::neutral_button(ui, "<", nav, font.clone(), false).clicked() {
                    step = -1;
                }
                if widgets::neutral_button(ui, ">", nav, font.clone(), false).clicked() {
                    step = 1;
                }
            });
            let name = self
                .current()
                .and_then(|p| p.file_name())
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            let label = if n > 0 {
                format!("{}/{n}  {name}", self.index + 1)
            } else {
                String::new()
            };
            let controls_w = 124.0 + 4.0 + 72.0 + 4.0 + 84.0;
            let name_w = (ui.available_width() - controls_w - 8.0).max(40.0);
            widgets::cell(ui, name_w, 26.0, Align::Min, |ui| {
                ui.add(
                    egui::Label::new(
                        RichText::new(label)
                            .font(theme::font_body())
                            .color(NEUTRAL_DIM),
                    )
                    .truncate(),
                );
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                let hist = widgets::neutral_button(
                    ui,
                    t(Key::Histogram),
                    vec2(84.0, 24.0),
                    font.clone(),
                    self.show_histogram,
                );
                if tip(hist, t(Key::TipHistogram)).clicked() {
                    self.show_histogram = !self.show_histogram;
                }
                let can_compare = !embedded_mode && self.embedded.is_some();
                ui.add_enabled_ui(can_compare, |ui| {
                    let on = self.compare && can_compare;
                    let b = widgets::neutral_button(
                        ui,
                        t(Key::Compare),
                        vec2(72.0, 24.0),
                        font.clone(),
                        on,
                    );
                    let text = if embedded_mode || self.embedded.is_some() || !self.camera_checked {
                        t(Key::CompareTip)
                    } else {
                        t(Key::TipCompareNeedsCamera)
                    };
                    if tip(b, text).clicked() {
                        self.compare = !self.compare;
                        self.zoom_sent = None;
                    }
                });
                ui.add_enabled_ui(has_image, |ui| {
                    let selected = usize::from(self.zoom == Zoom::Actual);
                    let chosen = widgets::segmented(
                        ui,
                        &[t(Key::ZoomFit), t(Key::ZoomActual)],
                        selected,
                        vec2(124.0, 24.0),
                        SegmentStyle::Neutral,
                    );
                    match chosen {
                        Some(0) => self.set_zoom(Zoom::Fit),
                        Some(_) => self.set_zoom(Zoom::Actual),
                        None => {}
                    }
                });
            });
        });
        if step != 0 {
            self.step(step);
        }
    }

    fn draw_empty(&mut self, ui: &mut Ui, painter: &Painter, rect: Rect, resp: &Response) {
        resp.widget_info(|| WidgetInfo::labeled(Role::Label, true, t(Key::DropRawFolderHere)));
        let icon_size = (rect.height() * 0.42).clamp(56.0, 120.0);
        let top = rect.center().y - icon_size / 2.0 - 28.0;
        if let Some(icon) = self.icon(ui.ctx()) {
            let icon_rect = Rect::from_center_size(
                pos2(rect.center().x, top + icon_size / 2.0),
                vec2(icon_size, icon_size),
            );
            painter.image(icon.id(), icon_rect, FULL_UV, Color32::WHITE);
        }
        painter.text(
            pos2(rect.center().x, top + icon_size + 18.0),
            Align2::CENTER_CENTER,
            t(Key::DropRawFolderHere),
            theme::font_header(),
            NEUTRAL_TEXT,
        );
        painter.text(
            pos2(rect.center().x, top + icon_size + 42.0),
            Align2::CENTER_CENTER,
            t(Key::EmptyHint),
            theme::font_body(),
            NEUTRAL_DIM,
        );
        // The outline of the drop target.
        painter.rect_stroke(
            rect.shrink(10.0),
            RADIUS_LARGE,
            Stroke::new(1.5, theme::NEUTRAL_ACTIVE),
            StrokeKind::Inside,
        );
    }

    /// Note on the photo while full-resolution pixels are on their way. None
    /// while a batch runs: nothing is being loaded then, and the paused
    /// status is shown in this spot instead.
    fn zoom_loading_text(&self) -> Option<&'static str> {
        (!self.paused).then(|| t(Key::ZoomLoading))
    }

    /// Whether the split compare can be drawn in this mode.
    fn compare_active(&self, embedded_mode: bool) -> bool {
        self.compare && !embedded_mode && self.embedded.is_some() && self.developed.is_some()
    }

    fn draw_fit(
        &mut self,
        ui: &mut Ui,
        painter: &Painter,
        rect: Rect,
        resp: &Response,
        embedded_mode: bool,
    ) {
        let compare = self.compare_active(embedded_mode);
        let frame = if embedded_mode {
            self.embedded.as_ref()
        } else {
            self.developed.as_ref().or(self.embedded.as_ref())
        };
        let Some(f) = frame else {
            return;
        };
        let (texture, size) = (f.texture.id(), f.size);
        let shows_camera = embedded_mode || self.developed.is_none();
        let draw = fit_rect(rect, size);
        painter.image(texture, draw, FULL_UV, Color32::WHITE);
        if compare {
            if let Some(cam) = &self.embedded {
                let split_x = draw.left() + self.split * draw.width();
                let mut left = draw;
                left.max.x = split_x;
                painter
                    .with_clip_rect(left.intersect(painter.clip_rect()))
                    .image(cam.texture.id(), draw, FULL_UV, Color32::WHITE);
            }
            self.draw_split_overlay(ui, painter, draw, resp);
        } else if shows_camera {
            tag(
                painter,
                draw.left_top() + vec2(8.0, 8.0),
                t(Key::CameraJpegTag),
            );
        }
        if resp.double_clicked() {
            if let Some(p) = resp.interact_pointer_pos() {
                self.center = [
                    ((p.x - draw.left()) / draw.width()).clamp(0.0, 1.0),
                    ((p.y - draw.top()) / draw.height()).clamp(0.0, 1.0),
                ];
                self.set_zoom(Zoom::Actual);
            }
        }
    }

    fn draw_actual(
        &mut self,
        ui: &mut Ui,
        painter: &Painter,
        rect: Rect,
        resp: &Response,
        embedded_mode: bool,
    ) {
        let ppp = ui.ctx().pixels_per_point();
        self.view_px = [
            ((rect.width() * ppp).floor() as u32).max(1),
            ((rect.height() * ppp).floor() as u32).max(1),
        ];
        let compare = self.compare_active(embedded_mode);
        let underlay = if embedded_mode {
            self.embedded.as_ref()
        } else {
            self.developed.as_ref().or(self.embedded.as_ref())
        };
        let Some(full) = self.full_size else {
            // First request still running: show the reduced picture meanwhile.
            if let Some(f) = underlay {
                painter.image(
                    f.texture.id(),
                    fit_rect(rect, f.size),
                    FULL_UV,
                    Color32::WHITE,
                );
            }
            if let Some(text) = self.zoom_loading_text() {
                tag(painter, rect.left_bottom() + vec2(8.0, -28.0), text);
            }
            if resp.double_clicked() {
                self.set_zoom(Zoom::Fit);
            }
            return;
        };
        let origin = view_origin(full, self.center, self.view_px);
        let map = ScreenMap::new(rect, origin, ppp);
        let image_rect = map.rect(0, 0, full[0], full[1]);
        if let Some(f) = underlay {
            painter.image(f.texture.id(), image_rect, FULL_UV, Color32::WHITE);
        }
        let area = image_rect.intersect(rect);
        match &self.crop {
            Some(crop) => {
                let r = map.rect(crop.region.x, crop.region.y, crop.region.w, crop.region.h);
                let developed = crop.developed.as_ref().filter(|_| !embedded_mode);
                let main = developed.or(crop.camera.as_ref());
                if let Some(f) = main {
                    painter.image(f.texture.id(), r, FULL_UV, Color32::WHITE);
                }
                if compare {
                    if let Some(cam) = &crop.camera {
                        let split_x = area.left() + self.split * area.width();
                        let mut left = area;
                        left.max.x = split_x;
                        painter
                            .with_clip_rect(left.intersect(painter.clip_rect()))
                            .image(cam.texture.id(), r, FULL_UV, Color32::WHITE);
                    }
                }
            }
            None => {
                if let Some(text) = self.zoom_loading_text() {
                    tag(painter, rect.left_bottom() + vec2(8.0, -28.0), text);
                }
            }
        }
        if compare {
            self.draw_split_overlay(ui, painter, area, resp);
        } else if embedded_mode {
            tag(
                painter,
                area.left_top() + vec2(8.0, 8.0),
                t(Key::CameraJpegTag),
            );
        }

        if resp.double_clicked() {
            self.set_zoom(Zoom::Fit);
            return;
        }
        if resp.hovered() && !self.dragging_split {
            let icon = if resp.dragged() {
                CursorIcon::Grabbing
            } else {
                CursorIcon::Grab
            };
            ui.ctx().set_cursor_icon(icon);
        }
        if resp.dragged() && !self.dragging_split {
            let delta = resp.drag_delta() * ppp;
            for (i, d) in [delta.x, delta.y].into_iter().enumerate() {
                let (f, v) = (full[i] as f32, self.view_px[i] as f32);
                if f > v {
                    let half = v / f / 2.0;
                    self.center[i] = (self.center[i] - d / f).clamp(half, 1.0 - half);
                }
            }
        }
    }

    /// Divider, its handle and the two labels; moves the divider when dragged.
    fn draw_split_overlay(&mut self, ui: &Ui, painter: &Painter, area: Rect, resp: &Response) {
        if resp.drag_started() {
            if let Some(p) = resp.interact_pointer_pos() {
                let x = area.left() + self.split * area.width();
                self.dragging_split = (p.x - x).abs() <= 12.0;
            }
        }
        if !resp.dragged() && !resp.drag_started() {
            self.dragging_split = false;
        }
        if self.dragging_split {
            if let Some(p) = resp.interact_pointer_pos() {
                self.split = ((p.x - area.left()) / area.width()).clamp(0.0, 1.0);
            }
        }
        let x = area.left() + self.split * area.width();
        painter.vline(x, area.y_range(), Stroke::new(2.0, Color32::from_gray(235)));
        let handle = pos2(x, area.center().y);
        painter.circle_filled(handle, 11.0, Color32::from_gray(235));
        painter.circle_filled(handle, 8.0, Color32::from_gray(90));
        tag(
            painter,
            area.left_top() + vec2(8.0, 8.0),
            t(Key::CameraJpegTag),
        );
        let label = t(Key::CompareDeveloped);
        // Right of the divider, or left of it when the divider is at the far right.
        let w = tag_width(painter, label);
        let tag_x = if x + 8.0 + w <= area.right() - 8.0 {
            x + 8.0
        } else {
            x - 8.0 - w
        };
        tag(painter, pos2(tag_x, area.top() + 8.0), label);
        if resp.hovered() {
            if let Some(p) = ui.ctx().pointer_hover_pos() {
                if (p.x - x).abs() <= 12.0 {
                    ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
                }
            }
        }
    }

    fn draw_histogram(&self, painter: &Painter, rect: Rect, embedded_mode: bool) {
        if !self.show_histogram {
            return;
        }
        let hist = if embedded_mode {
            self.camera_histogram.as_ref()
        } else {
            self.histogram.as_ref().or(self.camera_histogram.as_ref())
        };
        let Some(hist) = hist else {
            return;
        };
        let w = (rect.width() * 0.34).clamp(140.0, 220.0);
        let h = 78.0;
        let area = Rect::from_min_size(rect.right_top() + vec2(-w - 10.0, 10.0), vec2(w, h));
        painter.rect_filled(area, RADIUS_SMALL, Color32::from_black_alpha(175));
        let inner = area.shrink(6.0);
        // The clipped end bins are shown but do not set the scale.
        let peak = hist
            .channels
            .iter()
            .flat_map(|c| c[1..HISTOGRAM_BINS - 1].iter())
            .copied()
            .max()
            .unwrap_or(1)
            .max(1) as f32;
        let bin_w = inner.width() / HISTOGRAM_BINS as f32;
        let colors = [
            Color32::from_rgba_unmultiplied(235, 90, 90, 130),
            Color32::from_rgba_unmultiplied(100, 215, 120, 130),
            Color32::from_rgba_unmultiplied(100, 145, 245, 130),
        ];
        for (channel, color) in hist.channels.iter().zip(colors) {
            for (i, &count) in channel.iter().enumerate() {
                let v = (count as f32 / peak).sqrt().min(1.0);
                if v <= 0.0 {
                    continue;
                }
                let x = inner.left() + i as f32 * bin_w;
                let bar = Rect::from_min_max(
                    pos2(x, inner.bottom() - v * inner.height()),
                    pos2(x + bin_w, inner.bottom()),
                );
                painter.rect_filled(bar, 0.0, color);
            }
        }
    }

    fn status_text(&self) -> String {
        if self.paused {
            t(Key::PreviewPausedStatus).to_string()
        } else if let Some(m) = &self.message {
            m.clone()
        } else {
            self.status.clone()
        }
    }

    /// Draw the panel: toolbar, then the image area of `image_height`.
    /// Short notes sit on the image, so no room is reserved for a status line.
    pub fn draw(&mut self, ui: &mut Ui, image_height: f32, embedded_mode: bool) {
        if !self.is_empty() {
            self.draw_toolbar(ui, embedded_mode);
        }
        let width = ui.available_width();
        let (rect, resp) =
            ui.allocate_exact_size(vec2(width, image_height.max(80.0)), Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, RADIUS_MEDIUM, PHOTO_SURROUND);

        // In camera JPEG mode an image exists only if the file has a camera JPEG.
        let has_image = self.embedded.is_some() || (!embedded_mode && self.developed.is_some());
        if self.is_empty() {
            self.draw_empty(ui, &painter, rect, &resp);
        } else if !has_image {
            let text = if self.paused {
                t(Key::PausedWhileConverting)
            } else if let Some(m) = self.message.as_deref() {
                m
            } else if embedded_mode && self.camera_checked {
                t(Key::NoEmbeddedDevelopStatus)
            } else {
                t(Key::LoadingPreview)
            };
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                text,
                theme::font_body(),
                NEUTRAL_DIM,
            );
        } else {
            match self.zoom {
                Zoom::Fit => self.draw_fit(ui, &painter, rect, &resp, embedded_mode),
                Zoom::Actual => self.draw_actual(ui, &painter, rect, &resp, embedded_mode),
            }
            self.draw_histogram(&painter, rect, embedded_mode);
            let status = self.status_text();
            if !status.is_empty() {
                tag(&painter, rect.left_bottom() + vec2(8.0, -28.0), &status);
            }
        }
    }
}

/// Identifies a set of dropped files: two drops of the same size are still
/// different selections.
fn selection_key(selection: &DropSelection) -> String {
    let mut hasher = DefaultHasher::new();
    selection.files.hash(&mut hasher);
    format!("{}\u{1}{:x}", selection.root.display(), hasher.finish())
}

/// Maps full-resolution image pixels to screen points so that one image pixel
/// lands exactly on one physical screen pixel, whatever the HiDPI scale.
#[derive(Debug, Clone, Copy)]
struct ScreenMap {
    /// Physical position of the view's top-left corner, on the pixel grid.
    screen_px: [f64; 2],
    origin: [i64; 2],
    ppp: f64,
}

impl ScreenMap {
    fn new(view: Rect, origin: [i64; 2], ppp: f32) -> Self {
        let ppp = f64::from(ppp);
        Self {
            screen_px: [
                (f64::from(view.left()) * ppp).round(),
                (f64::from(view.top()) * ppp).round(),
            ],
            origin,
            ppp,
        }
    }

    fn point(&self, x: u32, y: u32) -> egui::Pos2 {
        pos2(
            ((self.screen_px[0] + (i64::from(x) - self.origin[0]) as f64) / self.ppp) as f32,
            ((self.screen_px[1] + (i64::from(y) - self.origin[1]) as f64) / self.ppp) as f32,
        )
    }

    fn rect(&self, x: u32, y: u32, w: u32, h: u32) -> Rect {
        Rect::from_min_max(self.point(x, y), self.point(x + w, y + h))
    }
}

/// `size` scaled to fit in `area`, centred.
fn fit_rect(area: Rect, size: [usize; 2]) -> Rect {
    let scale = (area.width() / size[0] as f32).min(area.height() / size[1] as f32);
    Rect::from_center_size(
        area.center(),
        vec2(size[0] as f32 * scale, size[1] as f32 * scale),
    )
}

fn tag_width(painter: &Painter, text: &str) -> f32 {
    painter
        .layout_no_wrap(text.to_owned(), theme::font_small_bold(), NEUTRAL_TEXT)
        .size()
        .x
        + 14.0
}

/// A small neutral label placed at `at` on the photo.
fn tag(painter: &Painter, at: egui::Pos2, text: &str) {
    let w = tag_width(painter, text);
    let r = Rect::from_min_size(at, vec2(w, 20.0));
    painter.rect_filled(r, RADIUS_SMALL, Color32::from_black_alpha(170));
    painter.text(
        r.center(),
        Align2::CENTER_CENTER,
        text,
        theme::font_small_bold(),
        NEUTRAL_TEXT,
    );
}

fn upload(ctx: &egui::Context, name: &str, image: ColorImage) -> Frame {
    let size = image.size;
    Frame {
        texture: ctx.load_texture(name, image, TEX_OPTIONS),
        size,
    }
}

/// A small synthetic scene through the real style code, shown until a photo
/// is loaded: sky above, warm ground below, a grey ramp along the bottom.
fn placeholder_image(style_choice: PictureStyle) -> ColorImage {
    let (w, h) = (48u32, 32u32);
    let mut img = RgbF32Image::from_fn(w, h, |x, y| {
        let (fx, fy) = (x as f32 / (w - 1) as f32, y as f32 / (h - 1) as f32);
        if fy < 0.55 {
            image::Rgb([0.42 + 0.25 * fy, 0.62 + 0.15 * fy, 0.85 - 0.1 * fy])
        } else if fy < 0.85 {
            image::Rgb([0.55 + 0.2 * fx, 0.40 + 0.1 * fx, 0.22])
        } else {
            image::Rgb([fx, fx, fx])
        }
    });
    style::apply(&mut img, style_choice);
    let rgb = ccoonverter_core::resize::quantize(&img);
    ColorImage::from_rgb([w as usize, h as usize], rgb.as_raw())
}

#[cfg(feature = "capture")]
impl PreviewPanel {
    /// Move to file `index` of the folder once the folder has been scanned.
    pub fn capture_select(&mut self, index: usize) {
        if self.index != index && index < self.files.len() {
            self.index = index;
            self.clear_images();
            self.message = None;
        }
    }

    /// The photo and all its style thumbnails are on screen.
    pub fn capture_ready(&self, index: usize) -> bool {
        self.index == index
            && self.developed.is_some()
            && self.thumbs.len() == PictureStyle::ALL.len()
    }
}

#[cfg(test)]
impl PreviewPanel {
    pub fn zoom(&self) -> Zoom {
        self.zoom
    }

    pub fn has_developed(&self) -> bool {
        self.developed.is_some()
    }

    pub fn has_camera(&self) -> bool {
        self.embedded.is_some()
    }

    pub fn has_crop(&self) -> bool {
        self.crop.is_some()
    }

    pub fn has_real_thumbnails(&self) -> bool {
        self.thumbs.len() == PictureStyle::ALL.len()
    }

    pub fn current_file(&self) -> Option<&PathBuf> {
        self.current()
    }

    pub fn shown_status(&self) -> String {
        self.status_text()
    }

    /// Full-resolution releases the worker has carried out.
    pub fn full_releases(&self) -> usize {
        self.worker.as_ref().map_or(0, PreviewWorker::full_releases)
    }

    pub fn compare_on(&self) -> bool {
        self.compare
    }

    pub fn file_count(&self) -> usize {
        self.files.len()
    }

    pub fn current_index(&self) -> usize {
        self.index
    }

    pub fn view_px(&self) -> [u32; 2] {
        self.view_px
    }

    pub fn crop_region(&self) -> Option<Region> {
        self.crop.as_ref().map(|c| c.region)
    }

    pub fn set_compare(&mut self, on: bool) {
        self.compare = on;
        self.zoom_sent = None;
    }

    pub fn center(&self) -> [f32; 2] {
        self.center
    }

    /// Stop asking the worker for anything (tests that inject pixels).
    pub fn detach_worker(&mut self) {
        self.worker = None;
    }

    /// Inject a 100 % crop without a RAW file (the 1:1 rendering test).
    pub fn inject_crop(
        &mut self,
        file: PathBuf,
        full_size: [u32; 2],
        region: Region,
        image: ColorImage,
    ) {
        self.injected = Some((file, full_size, region, image));
    }

    fn apply_injected_crop(&mut self, ctx: &egui::Context) {
        let Some((file, full_size, region, image)) = self.injected.take() else {
            return;
        };
        self.files = vec![file.clone()];
        self.index = 0;
        self.full_size = Some(full_size);
        self.zoom = Zoom::Actual;
        self.developed = Some(upload(ctx, "test_dev", image.clone()));
        self.crop = Some(Crop {
            file,
            region,
            developed: Some(Frame {
                size: [region.w as usize, region.h as usize],
                texture: ctx.load_texture("test_crop", image, CROP_OPTIONS),
            }),
            camera: None,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::Pos2;

    #[test]
    fn screen_map_is_one_image_pixel_per_physical_pixel() {
        // A view starting at a fractional physical position at 150 %.
        let view = Rect::from_min_size(Pos2::new(10.3, 20.7), vec2(400.0, 300.0));
        for ppp in [1.0f32, 1.25, 1.5, 2.0] {
            let map = ScreenMap::new(view, [1000, 500], ppp);
            let a = map.point(1000, 500);
            let b = map.point(1001, 501);
            for (p, q) in [(a.x, b.x), (a.y, b.y)] {
                // Whole physical pixels, one apart.
                let (pp, qq) = (f64::from(p) * f64::from(ppp), f64::from(q) * f64::from(ppp));
                assert!((pp - pp.round()).abs() < 1e-3, "{ppp}: {pp}");
                assert!(((qq - pp) - 1.0).abs() < 1e-3, "{ppp}: {pp} {qq}");
            }
        }
    }

    fn with_source(key: &str) -> PreviewPanel {
        let mut panel = PreviewPanel::new(None, true);
        panel.source_key = Some(key.to_owned());
        panel
    }

    fn scanned(root: &str, error: Option<&str>) -> PreviewEvent {
        PreviewEvent::Scanned {
            root: PathBuf::from(root),
            files: Vec::new(),
            error: error.map(str::to_owned),
        }
    }

    /// Regression: a scan finishing after the folder changed replaced the file
    /// list of the new folder, or of dropped files.
    #[test]
    fn a_late_scan_of_another_folder_is_ignored() {
        let ctx = egui::Context::default();
        let mut panel = with_source("D:/new");
        panel.files = vec![PathBuf::from("D:/new/a.DNG")];
        panel.apply(&ctx, scanned("D:/old", None));
        assert_eq!(panel.files.len(), 1);
        panel.apply(&ctx, scanned("D:/new", None));
        assert!(panel.files.is_empty());
    }

    /// Regression: an unreadable folder was reported as a folder without RAW files.
    #[test]
    fn an_unreadable_folder_shows_the_reason() {
        crate::i18n::set_language(crate::i18n::Lang::English);
        let ctx = egui::Context::default();
        let mut panel = with_source("D:/locked");
        panel.apply(&ctx, scanned("D:/locked", Some("Access is denied")));
        let message = panel.message.clone().unwrap();
        assert!(message.contains("Access is denied"), "{message}");
        panel.apply(&ctx, scanned("D:/locked", None));
        assert_eq!(
            panel.message.as_deref(),
            Some(t(Key::NoRawInFolder)),
            "a readable empty folder keeps its own message"
        );
    }

    /// Regression: two drops with the same parent and file count were the same
    /// key, so the preview kept showing the first drop.
    #[test]
    fn selections_of_the_same_size_have_different_keys() {
        let selection = |name: &str| DropSelection {
            root: PathBuf::from("D:/shoot"),
            files: vec![PathBuf::from("D:/shoot").join(name)],
        };
        assert_ne!(
            selection_key(&selection("a.DNG")),
            selection_key(&selection("b.DNG"))
        );
        assert_eq!(
            selection_key(&selection("a.DNG")),
            selection_key(&selection("a.DNG"))
        );
    }

    /// Regression: while a batch ran, "Developing at full resolution..." was
    /// drawn over the paused status, and a late result was uploaded anyway.
    #[test]
    fn a_batch_pauses_the_100_percent_view() {
        let ctx = egui::Context::default();
        let file = PathBuf::from("D:/a.DNG");
        let zoomed = || PreviewEvent::Zoomed {
            file: file.clone(),
            full_size: [2, 2],
            region: Region {
                x: 0,
                y: 0,
                w: 2,
                h: 2,
            },
            developed: Some(ColorImage::from_rgb([2, 2], &[0u8; 12])),
            camera: None,
        };
        let mut panel = with_source("D:/");
        panel.files = vec![file.clone()];
        panel.zoom = Zoom::Actual;
        assert!(panel.zoom_loading_text().is_some());

        panel.paused = true;
        assert!(panel.zoom_loading_text().is_none());
        panel.apply(&ctx, zoomed());
        assert!(
            panel.crop.is_none(),
            "kept while the batch needs the memory"
        );

        panel.paused = false;
        panel.apply(&ctx, zoomed());
        assert!(panel.crop.is_some());
    }

    #[test]
    fn placeholder_uses_the_real_style_code() {
        let standard = placeholder_image(PictureStyle::Standard);
        let mono = placeholder_image(PictureStyle::BlackAndWhite);
        assert_ne!(standard.pixels, mono.pixels);
        let [r, g, b] = {
            let p = mono.pixels[0];
            [p.r(), p.g(), p.b()]
        };
        assert_eq!((r, g), (g, b), "black and white has no colour");
    }
}
