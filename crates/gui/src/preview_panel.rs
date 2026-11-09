//! The preview panel: state and drawing on the UI thread. All heavy work is
//! in [`crate::preview`]; this module only posts requests and shows results.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use ccoonverter_core::ConvertSettings;
use eframe::egui::{
    self, pos2, vec2, Align, Align2, Color32, ColorImage, Rect, RichText, Sense, TextureFilter,
    TextureHandle, TextureOptions, Ui,
};

use crate::controller::DropSelection;
use crate::preview::{desired_request, short_message, PreviewEvent, PreviewWorker, RenderRequest};
use crate::theme::{self, ACCENT, BG, BORDER, PANEL, TEXT, TEXT_DIM};
use crate::widgets;

const TEX_OPTIONS: TextureOptions =
    TextureOptions::LINEAR.with_mipmap_mode(Some(TextureFilter::Linear));

/// Wait this long after the source text stops changing before scanning.
const SCAN_DEBOUNCE: Duration = Duration::from_millis(350);

struct Frame {
    texture: TextureHandle,
    size: [usize; 2],
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
    show_before: bool,
    /// The worker has reported whether the current file has a camera JPEG.
    camera_checked: bool,
    /// See `RenderRequest::session`.
    session: u64,
    /// A batch is running: no new preview work is requested.
    paused: bool,
    /// Short message shown instead of an image (no files, failure, ...).
    message: Option<String>,
    status: String,
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
            show_before: false,
            camera_checked: false,
            session: 0,
            paused: false,
            message: None,
            status: String::new(),
        }
    }

    fn current(&self) -> Option<&PathBuf> {
        self.files.get(self.index)
    }

    fn clear_images(&mut self) {
        self.developed = None;
        self.embedded = None;
        self.camera_checked = false;
        self.session += 1;
    }

    fn set_files(&mut self, files: Vec<PathBuf>) {
        self.files = files;
        self.index = 0;
        self.clear_images();
        self.last_sent = None;
        self.message = self.files.is_empty().then(|| "No RAW files here.".into());
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
        let Some(worker) = self.worker.as_ref() else {
            return;
        };
        let events = worker.drain();
        for e in events {
            self.apply(ctx, e);
        }
        self.paused = converting;

        let key = selection
            .map(|s| format!("{}\u{1}{}", s.root.display(), s.files.len()))
            .unwrap_or_else(|| source_text.trim().to_owned());
        if self.source_key.as_deref() != Some(key.as_str()) {
            self.source_key = Some(key.clone());
            self.scan_due = None;
            self.set_files(Vec::new());
            if let Some(s) = selection {
                self.set_files(s.files.clone());
            } else if key.is_empty() {
                self.message = Some("Select a folder or drop RAW files.".into());
            } else {
                self.message = Some("Looking for RAW files...".into());
                self.scan_due = Some(Instant::now() + SCAN_DEBOUNCE);
            }
        }
        if visible {
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
            if !converting {
                let desired = desired_request(&self.files, self.index, settings).map(|mut r| {
                    r.session = self.session;
                    r
                });
                if desired.is_some() && desired != self.last_sent {
                    if let (Some(w), Some(req)) = (self.worker.as_mut(), desired.clone()) {
                        w.request_render(req);
                        self.last_sent = desired;
                        self.status = "Updating...".into();
                    }
                }
            }
        }
    }

    fn apply(&mut self, ctx: &egui::Context, event: PreviewEvent) {
        let is_current = |p: &PathBuf, cur: Option<&PathBuf>| cur == Some(p);
        match event {
            PreviewEvent::Scanned { root, files, error } => {
                if let Some(e) = error {
                    log::warn!("Preview scan of {} failed: {e}", root.display());
                }
                self.set_files(files);
                if self.files.is_empty() {
                    self.message = Some("No RAW files in this folder.".into());
                } else {
                    self.message = None;
                }
                self.status = format!("{} RAW file(s)", self.files.len());
            }
            PreviewEvent::Developing { file } => {
                if is_current(&file, self.current()) {
                    self.status = "Developing...".into();
                    self.message = None;
                }
            }
            PreviewEvent::Embedded { file, image } => {
                if is_current(&file, self.current()) {
                    self.camera_checked = true;
                    self.embedded =
                        image.map(|i| upload(ctx, "preview_camera", (*i).clone(), None));
                }
            }
            PreviewEvent::Developed {
                file,
                image,
                millis,
                ..
            } => {
                if is_current(&file, self.current()) {
                    let size = image.size;
                    self.developed = Some(upload(ctx, "preview_dev", image, self.developed.take()));
                    self.status = format!("{}x{}  ·  {millis:.0} ms", size[0], size[1]);
                    self.message = None;
                }
            }
            PreviewEvent::Failed { file, message } => {
                if is_current(&file, self.current()) {
                    self.developed = None;
                    self.status.clear();
                    let what = short_message(&message);
                    self.message = Some(if self.embedded.is_some() {
                        format!("Can't develop this file ({what}). Camera JPEG shown.")
                    } else {
                        format!("Preview unavailable: {what}")
                    });
                }
            }
        }
    }

    /// Draw the panel: header row, image area of `image_height`, status line.
    pub fn draw(&mut self, ui: &mut Ui, image_height: f32, embedded_mode: bool) {
        let n = self.files.len();
        let mut step = 0isize;
        ui.horizontal(|ui| {
            widgets::header(ui, "PREVIEW");
            let nav = n > 1;
            ui.add_enabled_ui(nav, |ui| {
                if nav_button(ui, "<") {
                    step = -1;
                }
                if nav_button(ui, ">") {
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
            let right_w = 120.0;
            let name_w = (ui.available_width() - right_w - 8.0).max(40.0);
            widgets::cell(ui, name_w, 26.0, Align::Min, |ui| {
                ui.add(
                    egui::Label::new(
                        RichText::new(label)
                            .font(theme::font_body())
                            .color(TEXT_DIM),
                    )
                    .truncate(),
                );
            });
            let has_camera = self.embedded.is_some();
            if embedded_mode {
                widgets::cell(ui, right_w, 26.0, Align::Max, |ui| {
                    widgets::dim_label(ui, "Camera JPEG");
                });
            } else {
                let text = if self.show_before && has_camera {
                    "SHOW AFTER"
                } else {
                    "SHOW BEFORE"
                };
                ui.add_enabled_ui(has_camera, |ui| {
                    if widgets::outline_button(
                        ui,
                        text,
                        vec2(right_w, 24.0),
                        theme::font_small_bold(),
                        ACCENT,
                        PANEL,
                        BORDER,
                    )
                    .on_hover_text("Compare with the camera's embedded JPEG")
                    .clicked()
                    {
                        self.show_before = !self.show_before;
                    }
                });
            }
        });
        if step != 0 {
            self.step(step);
        }

        let showing_camera = embedded_mode || (self.show_before && self.embedded.is_some());
        let frame = if showing_camera {
            self.embedded.as_ref()
        } else {
            self.developed.as_ref().or(self.embedded.as_ref())
        };
        let is_camera_frame = frame.is_some()
            && (showing_camera || self.developed.is_none())
            && self.embedded.is_some();

        let width = ui.available_width();
        let (rect, _) = ui.allocate_exact_size(vec2(width, image_height.max(80.0)), Sense::hover());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, BG);
        match frame {
            Some(f) => {
                let scale = (rect.width() / f.size[0] as f32)
                    .min(rect.height() / f.size[1] as f32)
                    .min(1.0);
                let draw = Rect::from_center_size(
                    rect.center(),
                    vec2(f.size[0] as f32 * scale, f.size[1] as f32 * scale),
                );
                let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
                painter.image(f.texture.id(), draw, uv, Color32::WHITE);
                if is_camera_frame {
                    let tag = Rect::from_min_size(draw.left_top(), vec2(92.0, 20.0));
                    painter.rect_filled(tag, 0.0, Color32::from_black_alpha(170));
                    painter.text(
                        tag.center(),
                        Align2::CENTER_CENTER,
                        "CAMERA JPEG",
                        theme::font_small_bold(),
                        ACCENT,
                    );
                }
            }
            None => {
                let text = if let Some(m) = self.message.as_deref() {
                    m
                } else if self.paused {
                    "Paused while converting."
                } else if embedded_mode && self.camera_checked {
                    "No embedded JPEG in this file."
                } else if n > 0 {
                    "Loading preview..."
                } else {
                    "No preview yet."
                };
                painter.text(
                    rect.center(),
                    Align2::CENTER_CENTER,
                    text,
                    theme::font_body(),
                    TEXT_DIM,
                );
            }
        }

        let status = if self.paused {
            "Preview paused while converting.".to_string()
        } else if embedded_mode && self.camera_checked && self.embedded.is_none() {
            "No embedded JPEG in this file; conversion will develop the RAW.".to_string()
        } else if frame.is_some() {
            self.message.clone().unwrap_or_else(|| self.status.clone())
        } else {
            self.status.clone()
        };
        ui.horizontal(|ui| {
            ui.add(
                egui::Label::new(RichText::new(status).font(theme::font_small()).color(TEXT))
                    .truncate(),
            );
        });
    }
}

fn nav_button(ui: &mut Ui, text: &str) -> bool {
    widgets::outline_button(
        ui,
        text,
        vec2(26.0, 24.0),
        theme::font_body_bold(),
        ACCENT,
        PANEL,
        BORDER,
    )
    .clicked()
}

fn upload(ctx: &egui::Context, name: &str, image: ColorImage, previous: Option<Frame>) -> Frame {
    let size = image.size;
    match previous {
        Some(mut f) => {
            f.texture.set(image, TEX_OPTIONS);
            Frame { size, ..f }
        }
        None => Frame {
            texture: ctx.load_texture(name, image, TEX_OPTIONS),
            size,
        },
    }
}
