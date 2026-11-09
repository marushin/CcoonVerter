//! The main window: folders on top, adjustments left, photo in the middle,
//! and a run bar fixed at the bottom outside every scroll area.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use ccoonverter_core::config;
use ccoonverter_core::paths::AppPaths;
use ccoonverter_core::presets::PresetStore;
use ccoonverter_core::settings::UiLanguage;
use ccoonverter_core::update::{self, ReleaseInfo};
use ccoonverter_core::{AppSettings, ColorSpace, OutputFormat, PictureStyle, WhiteBalance};
use eframe::egui::{
    self, pos2, vec2, Align, Align2, Color32, ComboBox, CornerRadius, Frame, Id, Label, LayerId,
    Layout, Margin, Order, Panel, Rect, RichText, ScrollArea, Stroke, StrokeKind, Ui, Vec2,
    ViewportCommand,
};

use crate::controller::{
    color_note, conflict_label, format_note, plan_drop, prepare_batch, prepare_selection,
    style_name, wb_name, BatchOutcome, BatchReport, DropPlan, DropSelection, ProgressView,
    ReportKind, RunningBatch, UiSettings, CONFLICT_CHOICES,
};
use crate::dialogs;
use crate::i18n::{self, t, tf, Key};
use crate::preview::PreviewWorker;
use crate::preview_panel::PreviewPanel;
use crate::theme::{
    self, BG, BORDER, DANGER, PAD_LARGE, PAD_MEDIUM, PANEL, PHOTO_SURROUND, RADIUS_LARGE,
    RADIUS_MEDIUM, TEXT, TEXT_DIM,
};
use crate::widgets::{self, tip, Columns, SegmentStyle, SliderSpec};

/// Window size on first launch, shrunk to fit a smaller monitor.
pub const DEFAULT_WINDOW: [f32; 2] = [1240.0, 800.0];
pub const MIN_WINDOW: [f32; 2] = [520.0, 700.0];

/// From this window width on, adjustments and photo sit side by side.
const WIDE_LAYOUT_WIDTH: f32 = 900.0;
/// Width of the adjustments column in the wide layout.
const ADJUSTMENTS_WIDTH: f32 = 350.0;
/// Height of the bottom bar: fixed, never part of a scroll area.
const RUN_BAR_HEIGHT: f32 = 76.0;
/// Label column of the output options dialog.
const MODAL_LABEL_WIDTH: f32 = 110.0;
/// Label column of the folder rows.
const PATH_LABEL_WIDTH: f32 = 112.0;
/// Height of one row in the list of files that need attention.
const ISSUE_ROW_HEIGHT: f32 = 54.0;
/// Height of the photo area when the preview is expanded in the narrow layout.
const NARROW_PREVIEW_HEIGHT: f32 = 300.0;
/// Height of the drop target in the narrow layout.
const NARROW_EMPTY_HEIGHT: f32 = 220.0;
/// How long a notice stays visible.
const NOTICE_TIME: Duration = Duration::from_secs(6);
/// Width of a style tile's picture at most.
const STYLE_TILE_MAX_WIDTH: f32 = 84.0;
const STYLE_TILE_GAP: f32 = 6.0;

/// Deferred UI actions, executed after the frame's widgets are laid out
/// (native dialogs block, so they never run in the middle of drawing).
#[derive(Debug, Clone, PartialEq)]
enum Action {
    BrowseSource,
    BrowseOutput,
    Convert,
    Cancel,
    ResetAdjustments,
    OpenSavePreset,
    SavePreset(String),
    DeletePreset,
    ApplyPreset(String),
    OpenUrl(String),
    OpenResultFolder,
    CloseResult,
}

pub struct CcooNverterApp {
    paths: AppPaths,
    source: String,
    output: String,
    settings: UiSettings,
    check_updates: bool,
    language: UiLanguage,

    presets: PresetStore,
    /// Name shown in the preset combo (`None` = the "choose a preset" text).
    selected_preset: Option<String>,
    /// `Some(name being typed)` while the save-preset modal is open.
    preset_name_input: Option<String>,

    job: Option<RunningBatch>,
    progress: ProgressView,

    update_rx: Option<Receiver<ReleaseInfo>>,
    update_info: Option<ReleaseInfo>,

    /// Close already confirmed (conversion running) — let the next close request through.
    close_confirmed: bool,
    saved_on_exit: bool,
    /// Native title bar colours and window size were applied (first frame).
    window_prepared: bool,

    preview: PreviewPanel,
    /// Files staged by a drop: converted instead of scanning the source folder.
    selection: Option<DropSelection>,
    /// Transient message.
    notice: Option<(String, Instant)>,
    /// Output options modal is open.
    output_options_open: bool,
    /// Result card of the last batch, until the user closes it.
    report: Option<BatchReport>,
    /// Brightness slider is being dragged (keeps it on screen at 1.0).
    brightness_active: bool,
}

/// `desired` reduced to fit the screen, leaving room for the task bar.
fn fitted_window_size(desired: [f32; 2], monitor: Vec2) -> [f32; 2] {
    [
        desired[0].min((monitor.x * 0.92).max(MIN_WINDOW[0])),
        desired[1].min((monitor.y * 0.88).max(MIN_WINDOW[1])),
    ]
}

impl CcooNverterApp {
    pub fn new(
        ctx: egui::Context,
        paths: AppPaths,
        app_settings: AppSettings,
        presets: PresetStore,
    ) -> Self {
        let worker = match PreviewWorker::spawn(Box::new(move || ctx.request_repaint())) {
            Ok(w) => Some(w),
            Err(e) => {
                log::warn!("Preview disabled, could not start its worker: {e}");
                None
            }
        };
        let source = app_settings
            .last_source
            .as_ref()
            .filter(|p| p.is_dir())
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let output = app_settings
            .output_folder
            .as_ref()
            .map(|p| {
                log::info!("Loaded output folder preference: {}", p.display());
                p.to_string_lossy().into_owned()
            })
            .unwrap_or_default();

        let update_rx = app_settings
            .check_updates
            .then(|| spawn_update_check(paths.update_cache_file()));

        Self {
            source,
            output,
            settings: UiSettings::from_convert(&app_settings.convert),
            check_updates: app_settings.check_updates,
            language: app_settings.language,
            presets,
            selected_preset: None,
            preset_name_input: None,
            job: None,
            progress: ProgressView::default(),
            update_rx,
            update_info: None,
            close_confirmed: false,
            saved_on_exit: false,
            window_prepared: false,
            preview: PreviewPanel::new(worker, app_settings.preview_open),
            selection: None,
            notice: None,
            output_options_open: false,
            report: None,
            brightness_active: false,
            paths,
        }
    }

    fn is_converting(&self) -> bool {
        self.job.is_some()
    }

    fn app_settings(&self) -> AppSettings {
        let non_empty = |s: &str| {
            let t = s.trim();
            (!t.is_empty()).then(|| PathBuf::from(t))
        };
        AppSettings {
            convert: self.settings.to_convert(),
            output_folder: non_empty(&self.output),
            last_source: non_empty(&self.source),
            check_updates: self.check_updates,
            preview_open: self.preview.open,
            language: self.language,
        }
    }

    fn save_settings(&self) {
        let path = self.paths.settings_file();
        match self.app_settings().save(&path) {
            Ok(()) => log::info!("Settings saved to {}", path.display()),
            Err(e) => log::error!("Failed to save settings to {}: {e}", path.display()),
        }
    }

    fn save_presets(&self, frame: Option<&eframe::Frame>) -> bool {
        let path = self.paths.presets_file();
        match self.presets.save(&path) {
            Ok(()) => true,
            Err(e) => {
                log::error!("Failed to save presets to {}: {e}", path.display());
                dialogs::error(frame, t(Key::TitleError), &tf(Key::SavePresetFailed, &[&e]));
                false
            }
        }
    }

    // ------------------------------------------------------------------
    // Per-frame background work
    // ------------------------------------------------------------------

    fn poll_job(&mut self, ctx: &egui::Context) {
        let Some(job) = self.job.as_mut() else {
            return;
        };
        let cancelling = job.is_cancel_requested();
        for p in job.drain_progress() {
            if !cancelling {
                self.progress.apply(&p);
            }
        }
        if let Some(outcome) = job.try_finish() {
            let output_dir = job.output_dir.clone();
            self.job = None;
            self.progress.reset();
            self.on_batch_finished(outcome, &output_dir);
        } else {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    fn on_batch_finished(&mut self, outcome: BatchOutcome, output_dir: &Path) {
        match &outcome {
            BatchOutcome::Finished(s) => log::info!(
                "Conversion finished: {} converted, {} skipped, {} failed, cancelled={}",
                s.converted,
                s.skipped,
                s.failed,
                s.was_cancelled
            ),
            BatchOutcome::Error(e) => log::error!("Conversion error: {e}"),
        }
        self.report = Some(BatchReport::new(
            &outcome,
            output_dir,
            &self.paths.log_file(),
        ));
    }

    fn poll_update(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.update_rx else {
            return;
        };
        match rx.try_recv() {
            Ok(info) => {
                log::info!("Update available: {}", info.version);
                self.update_info = Some(info);
                self.update_rx = None;
            }
            Err(TryRecvError::Disconnected) => self.update_rx = None,
            Err(TryRecvError::Empty) => ctx.request_repaint_after(Duration::from_millis(500)),
        }
    }

    fn handle_close_request(&mut self, ctx: &egui::Context, frame: Option<&eframe::Frame>) {
        if !ctx.input(|i| i.viewport().close_requested()) {
            return;
        }
        if self.is_converting() && !self.close_confirmed {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            if dialogs::confirm(frame, t(Key::TitleQuit), t(Key::QuitWhileConverting)) {
                if let Some(job) = &self.job {
                    job.cancel();
                }
                self.close_confirmed = true;
                self.save_settings();
                self.saved_on_exit = true;
                ctx.send_viewport_cmd(ViewportCommand::Close);
            }
            return;
        }
        if !self.saved_on_exit {
            log::info!("Saving settings before exit...");
            self.save_settings();
            self.saved_on_exit = true;
        }
    }

    /// Called from `App::on_exit`: final save and a bounded wait for the worker.
    fn shutdown(&mut self) {
        if !self.saved_on_exit {
            self.save_settings();
            self.saved_on_exit = true;
        }
        self.preview = PreviewPanel::new(None, self.preview.open);
        if let Some(mut job) = self.job.take() {
            job.cancel();
            log::info!("Waiting for the conversion worker to stop...");
            if job.wait_timeout(Duration::from_secs(10)).is_none() {
                log::warn!("Conversion worker did not stop within 10 s; exiting anyway");
            }
        }
        log::info!("{} closing", config::APP_NAME);
    }

    // ------------------------------------------------------------------
    // Actions
    // ------------------------------------------------------------------

    fn run_action(&mut self, action: Action, ctx: &egui::Context, frame: Option<&eframe::Frame>) {
        match action {
            Action::BrowseSource => {
                if let Some(p) = pick_folder(&self.source, t(Key::BrowseSourceTitle)) {
                    log::info!("Selected folder: {}", p.display());
                    self.source = p.to_string_lossy().into_owned();
                }
            }
            Action::BrowseOutput => {
                if let Some(p) = pick_folder(&self.output, t(Key::BrowseOutputTitle)) {
                    log::info!("Selected output folder: {}", p.display());
                    self.output = p.to_string_lossy().into_owned();
                }
            }
            Action::Convert => self.start_conversion(ctx, frame),
            Action::Cancel => {
                if let Some(job) = &self.job {
                    if !job.is_cancel_requested()
                        && dialogs::confirm(
                            frame,
                            t(Key::TitleCancel),
                            t(Key::ConfirmCancelConversion),
                        )
                    {
                        if let Some(job) = &self.job {
                            job.cancel();
                        }
                        log::info!("Cancellation requested by user");
                        self.progress.reset();
                        self.progress.status = t(Key::Cancelling).to_string();
                    }
                }
            }
            Action::ResetAdjustments => {
                self.settings.reset_raw_adjustments();
                log::info!("RAW adjustments reset to defaults");
                self.set_notice(t(Key::ResetDone).to_string());
            }
            Action::OpenSavePreset => self.preset_name_input = Some(String::new()),
            Action::SavePreset(name) => {
                self.preset_name_input = None;
                let name = name.trim().to_owned();
                if name.is_empty() {
                    return;
                }
                self.presets
                    .insert(name.clone(), self.settings.to_convert());
                if self.save_presets(frame) {
                    log::info!("Saved preset '{name}'");
                    self.selected_preset = Some(name.clone());
                    self.set_notice(tf(Key::PresetSaved, &[&name]));
                }
            }
            Action::DeletePreset => {
                let Some(name) = self.selected_preset.clone() else {
                    return;
                };
                if dialogs::confirm(
                    frame,
                    t(Key::TitleConfirm),
                    &tf(Key::ConfirmDeletePreset, &[&name]),
                ) && self.presets.remove(&name)
                {
                    self.selected_preset = None;
                    if self.save_presets(frame) {
                        log::info!("Deleted preset '{name}'");
                        self.set_notice(tf(Key::PresetDeleted, &[&name]));
                    }
                }
            }
            Action::ApplyPreset(name) => {
                if let Some(p) = self.presets.get(&name).cloned() {
                    self.settings.apply_preset(&p);
                    self.selected_preset = Some(name.clone());
                    log::info!("Applied preset: {name}");
                }
            }
            Action::OpenUrl(url) => open_external(Path::new(&url)),
            Action::OpenResultFolder => {
                if let Some(r) = &self.report {
                    open_external(&r.output_dir);
                }
            }
            Action::CloseResult => self.report = None,
        }
    }

    fn start_conversion(&mut self, ctx: &egui::Context, frame: Option<&eframe::Frame>) {
        if self.is_converting() {
            log::warn!("Conversion already in progress");
            return;
        }
        // Dropped files (the selection) take the place of a folder scan.
        let batch = match &self.selection {
            Some(selection) => prepare_selection(selection, &self.output),
            None => match prepare_batch(&self.source, &self.output) {
                Ok(b) => b,
                Err(e) => {
                    self.set_notice(e.message());
                    return;
                }
            },
        };
        let settings = self.settings.to_convert();
        match RunningBatch::spawn(batch, settings, None) {
            Ok(job) => {
                self.report = None;
                self.progress = ProgressView::starting(job.total);
                self.job = Some(job);
                ctx.request_repaint();
            }
            Err(e) => {
                log::error!("Could not start worker thread: {e}");
                dialogs::error(frame, t(Key::TitleError), &tf(Key::CouldNotStart, &[&e]));
            }
        }
    }

    // ------------------------------------------------------------------
    // Drawing
    // ------------------------------------------------------------------

    /// Handle files dropped on the window: a folder becomes the source,
    /// RAW files are staged for conversion, the rest get a message.
    fn handle_drops(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .filter(|p| !p.as_os_str().is_empty())
                .collect()
        });
        if dropped.is_empty() {
            return;
        }
        if self.is_converting() {
            self.set_notice(t(Key::DropBusy).to_string());
            return;
        }
        log::info!("Dropped {} item(s)", dropped.len());
        match plan_drop(&dropped, |p| p.is_dir()) {
            DropPlan::SetSource { folder, notice } => {
                log::info!("Source folder from drop: {}", folder.display());
                self.source = folder.to_string_lossy().into_owned();
                self.selection = None;
                if let Some(n) = notice {
                    self.set_notice(n);
                }
            }
            DropPlan::Files { selection, notice } => {
                log::info!(
                    "Dropped {} RAW file(s) from {}",
                    selection.files.len(),
                    selection.root.display()
                );
                self.source = selection.root.to_string_lossy().into_owned();
                let msg = notice.unwrap_or_else(|| tf(Key::DropReady, &[&selection.files.len()]));
                self.selection = Some(selection);
                self.set_notice(msg);
            }
            DropPlan::Ignored { notice } => self.set_notice(notice),
        }
    }

    fn set_notice(&mut self, text: String) {
        log::info!("{text}");
        self.notice = Some((text, Instant::now()));
    }

    /// Hover state while files are dragged over the window, and the notice.
    fn draw_overlays(&mut self, ctx: &egui::Context) {
        let rect = ctx.content_rect();
        let hovering = ctx.input(|i| !i.raw.hovered_files.is_empty());
        if hovering {
            let painter =
                ctx.layer_painter(LayerId::new(Order::Foreground, Id::unique("drop_hover")));
            painter.rect_filled(rect, 0.0, Color32::from_black_alpha(190));
            painter.rect_stroke(
                rect.shrink(10.0),
                RADIUS_LARGE,
                Stroke::new(3.0, TEXT),
                StrokeKind::Inside,
            );
            painter.text(
                rect.center() - vec2(0.0, 14.0),
                Align2::CENTER_CENTER,
                t(Key::DropHoverTitle),
                theme::font_header(),
                TEXT,
            );
            painter.text(
                rect.center() + vec2(0.0, 14.0),
                Align2::CENTER_CENTER,
                t(Key::DropHoverHint),
                theme::font_body(),
                TEXT,
            );
        }
        let expired = self
            .notice
            .as_ref()
            .is_some_and(|(_, t)| t.elapsed() >= NOTICE_TIME);
        if expired {
            self.notice = None;
        }
        if let Some((text, shown)) = &self.notice {
            ctx.request_repaint_after(NOTICE_TIME.saturating_sub(shown.elapsed()));
            let painter = ctx.layer_painter(LayerId::new(Order::Foreground, Id::unique("notice")));
            let galley = painter.layout_no_wrap(text.clone(), theme::font_body_bold(), TEXT);
            let w = (galley.size().x + 28.0).min(rect.width() - 16.0);
            let h = galley.size().y + 14.0;
            let at = Rect::from_center_size(
                pos2(
                    rect.center().x,
                    rect.bottom() - RUN_BAR_HEIGHT - h / 2.0 - 8.0,
                ),
                vec2(w, h),
            );
            painter.rect_filled(at, RADIUS_MEDIUM, PANEL);
            painter.rect_stroke(
                at,
                RADIUS_MEDIUM,
                Stroke::new(1.0, BORDER),
                StrokeKind::Inside,
            );
            painter.galley(at.center() - galley.size() / 2.0, galley.clone(), TEXT);
        }
    }

    fn draw_paths(&mut self, ui: &mut Ui, actions: &mut Vec<Action>) {
        let row_h = 30.0;
        let folder_row =
            |ui: &mut Ui, title: &str, text: &mut String, hint: &str, tooltip: &str| -> bool {
                let mut browse = false;
                ui.horizontal(|ui| {
                    widgets::cell(ui, PATH_LABEL_WIDTH, row_h, Align::Min, |ui| {
                        tip(widgets::dim_label(ui, title), tooltip);
                    });
                    let field_w = (ui.available_width() - 82.0).max(100.0);
                    tip(widgets::text_field(ui, text, hint, field_w), tooltip);
                    let button = widgets::quiet_button(
                        ui,
                        t(Key::Browse),
                        vec2(76.0, row_h),
                        theme::font_body(),
                    );
                    browse = tip(button, t(Key::TipBrowse)).clicked();
                });
                browse
            };
        ui.horizontal(|ui| {
            if let Some(icon) = self.preview.icon(ui.ctx()) {
                ui.add(egui::Image::from_texture((icon.id(), vec2(52.0, 52.0))));
            }
            ui.vertical(|ui| {
                if folder_row(
                    ui,
                    t(Key::SourceFolder),
                    &mut self.source,
                    "",
                    t(Key::TipSource),
                ) {
                    actions.push(Action::BrowseSource);
                }
                if let Some(selection) = &self.selection {
                    let count = selection.files.len();
                    let mut clear = false;
                    ui.horizontal(|ui| {
                        ui.add_space(PATH_LABEL_WIDTH + PAD_MEDIUM);
                        ui.label(
                            RichText::new(tf(Key::DroppedSelected, &[&count]))
                                .font(theme::font_small())
                                .color(TEXT_DIM),
                        );
                        if widgets::quiet_button(
                            ui,
                            t(Key::Clear),
                            vec2(48.0, 20.0),
                            theme::font_small(),
                        )
                        .clicked()
                        {
                            clear = true;
                        }
                    });
                    if clear {
                        self.selection = None;
                    }
                }
                if folder_row(
                    ui,
                    t(Key::OutputFolderOptional),
                    &mut self.output,
                    t(Key::OutputHint),
                    t(Key::TipOutput),
                ) {
                    actions.push(Action::BrowseOutput);
                }
            });
        });
    }

    fn draw_adjustments(&mut self, ui: &mut Ui, actions: &mut Vec<Action>) {
        let width = ui.available_width();
        // Mode switch: the first control of the column.
        let selected = usize::from(self.settings.extract_embedded);
        if let Some(i) = widgets::segmented(
            ui,
            &[t(Key::ModeRaw), t(Key::ModeCamera)],
            selected,
            vec2(width, 32.0),
            SegmentStyle::Accent,
        ) {
            self.settings.extract_embedded = i == 1;
        }
        widgets::note(
            ui,
            t(if self.settings.extract_embedded {
                Key::ModeCameraNote
            } else {
                Key::ModeRawNote
            }),
        );
        ui.add_space(4.0);

        if !self.settings.extract_embedded {
            self.draw_raw_adjustments(ui, actions);
            ui.add_space(PAD_MEDIUM);
            self.draw_style_picker(ui);
            ui.add_space(PAD_MEDIUM);
            self.draw_advanced(ui);
            ui.add_space(PAD_MEDIUM);
        }
        self.draw_output(ui);
        ui.add_space(PAD_MEDIUM);
        self.draw_presets(ui, actions);
        ui.add_space(PAD_MEDIUM);
        self.draw_app_section(ui);
    }

    fn draw_raw_adjustments(&mut self, ui: &mut Ui, actions: &mut Vec<Action>) {
        let cols = Columns::SETTINGS;
        let s = &mut self.settings;
        slider_row(
            ui,
            cols,
            t(Key::ExposureLabel),
            t(Key::TipExposure),
            &mut s.exposure_ev,
            &SliderSpec {
                name: t(Key::ExposureLabel),
                range: config::MIN_EXPOSURE_EV..=config::MAX_EXPOSURE_EV,
                origin: 0.0,
                default: 0.0,
                step: 0.05,
            },
            |v| format!("{v:+.2}"),
        );
        cols.wide_row(
            ui,
            |ui| {
                tip(
                    widgets::dim_label(ui, t(Key::WhiteBalanceLabel)),
                    t(Key::TipWhiteBalance),
                );
            },
            |ui, w| {
                let combo = ComboBox::from_id_salt("white_balance")
                    .width(w)
                    .selected_text(t(wb_name(s.white_balance)))
                    .show_ui(ui, |ui| {
                        for wb in WhiteBalance::ALL {
                            ui.selectable_value(&mut s.white_balance, wb, t(wb_name(wb)));
                        }
                    });
                tip(combo.response, t(Key::TipWhiteBalance));
            },
        );
        ui.horizontal(|ui| {
            let curve =
                widgets::checkbox(ui, &mut s.tone_curve, t(Key::ToneCurve), theme::font_body());
            tip(curve, t(Key::TipToneCurve));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let reset =
                    widgets::quiet_button(ui, t(Key::Reset), vec2(68.0, 24.0), theme::font_small());
                if tip(reset, t(Key::TipReset)).clicked() {
                    actions.push(Action::ResetAdjustments);
                }
            });
        });
    }

    fn draw_style_picker(&mut self, ui: &mut Ui) {
        tip(
            widgets::section_title(ui, t(Key::SectionStyle)),
            t(Key::TipStyle),
        );
        let width = ui.available_width();
        let columns = ((width + STYLE_TILE_GAP) / (STYLE_TILE_MAX_WIDTH + STYLE_TILE_GAP))
            .ceil()
            .clamp(4.0, 6.0) as usize;
        let tile_w = (width - STYLE_TILE_GAP * (columns as f32 - 1.0)) / columns as f32;
        let tile_h = (tile_w * 2.0 / 3.0).round();
        for row in PictureStyle::ALL.chunks(columns) {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = STYLE_TILE_GAP;
                for &style in row {
                    if let Some((texture, size)) = self.preview.style_thumbnail(style) {
                        let uv = cover_uv(size, tile_w / tile_h);
                        let resp = widgets::style_tile(
                            ui,
                            texture,
                            uv,
                            t(style_name(style)),
                            self.settings.style == style,
                            vec2(tile_w, tile_h),
                        );
                        if tip(resp, t(Key::TipStyle)).clicked() {
                            self.settings.style = style;
                        }
                    }
                }
            });
        }
    }

    fn draw_advanced(&mut self, ui: &mut Ui) {
        let cols = Columns::SETTINGS;
        let s = &mut self.settings;
        let brightness_set = (s.brightness - 1.0).abs() > 1e-3;
        let header = if brightness_set {
            format!(
                "{}  ·  {}",
                t(Key::Advanced),
                tf(Key::AdvancedActive, &[&format!("{:.2}", s.brightness)])
            )
        } else {
            t(Key::Advanced).to_string()
        };
        let mut brightness_active = self.brightness_active;
        egui::CollapsingHeader::new(
            RichText::new(header)
                .font(theme::font_small_bold())
                .color(TEXT_DIM),
        )
        .id_salt("advanced")
        .default_open(false)
        .show(ui, |ui| {
            if brightness_set || brightness_active {
                brightness_active = slider_row(
                    ui,
                    cols,
                    t(Key::BrightnessLabel),
                    t(Key::TipBrightness),
                    &mut s.brightness,
                    &SliderSpec {
                        name: t(Key::BrightnessLabel),
                        range: config::MIN_BRIGHTNESS..=config::MAX_BRIGHTNESS,
                        origin: 1.0,
                        default: 1.0,
                        step: 0.01,
                    },
                    |v| format!("{v:.2}"),
                );
                ui.horizontal(|ui| {
                    widgets::note(ui, t(Key::BrightnessNote));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if widgets::quiet_button(
                            ui,
                            t(Key::Reset),
                            vec2(60.0, 20.0),
                            theme::font_small(),
                        )
                        .clicked()
                        {
                            s.brightness = 1.0;
                            brightness_active = false;
                        }
                    });
                });
                ui.add_space(4.0);
            }
            let rgb_title = t(Key::RgbBalanceLabel);
            for (i, channel) in ["R", "G", "B"].into_iter().enumerate() {
                let label = format!("{rgb_title} {channel}");
                slider_row(
                    ui,
                    cols,
                    &label,
                    t(Key::TipRgbBalance),
                    &mut s.rgb[i],
                    &SliderSpec {
                        name: &label,
                        range: config::MIN_RGB_VALUE..=config::MAX_RGB_VALUE,
                        origin: 1.0,
                        default: 1.0,
                        step: 0.01,
                    },
                    |v| format!("{v:.2}"),
                );
            }
            let black = t(Key::LevelsBlack);
            let white = t(Key::LevelsWhite);
            slider_row(
                ui,
                cols,
                black,
                t(Key::TipLevels),
                &mut s.black_point,
                &SliderSpec {
                    name: black,
                    range: f32::from(config::MIN_BLACK_POINT)..=f32::from(config::MAX_BLACK_POINT),
                    origin: f32::from(config::MIN_BLACK_POINT),
                    default: f32::from(config::MIN_BLACK_POINT),
                    step: 1.0,
                },
                |v| format!("{}", v.round() as i32),
            );
            slider_row(
                ui,
                cols,
                white,
                t(Key::TipLevels),
                &mut s.white_point,
                &SliderSpec {
                    name: white,
                    range: f32::from(config::MIN_WHITE_POINT)..=f32::from(config::MAX_WHITE_POINT),
                    origin: f32::from(config::MAX_WHITE_POINT),
                    default: f32::from(config::MAX_WHITE_POINT),
                    step: 1.0,
                },
                |v| format!("{}", v.round() as i32),
            );
        });
        self.brightness_active = brightness_active;
    }

    fn draw_output(&mut self, ui: &mut Ui) {
        let cols = Columns::SETTINGS;
        widgets::section_title(ui, t(Key::SectionOutput));
        let s = &mut self.settings;
        cols.wide_row(
            ui,
            |ui| {
                tip(
                    widgets::dim_label(ui, t(Key::ResizeLabel)),
                    t(Key::TipResize),
                );
            },
            |ui, w| {
                tip(
                    widgets::text_field(ui, &mut s.resize_text, t(Key::ResizeHint), w),
                    t(Key::TipResize),
                );
            },
        );
        // In camera JPEG mode a file is copied byte for byte unless it is resized.
        let quality_used = s.effective_format().uses_quality()
            && (!s.extract_embedded || crate::controller::parse_resize(&s.resize_text) > 0);
        let tooltip = if quality_used {
            t(Key::TipQuality)
        } else if s.extract_embedded {
            t(Key::TipQualityUnused)
        } else {
            t(format_note(s.output_format))
        };
        ui.add_enabled_ui(quality_used, |ui| {
            let name = t(Key::QualityLabel);
            slider_row(
                ui,
                cols,
                name,
                tooltip,
                &mut s.quality,
                &SliderSpec {
                    name,
                    range: f32::from(config::MIN_JPG_QUALITY)..=f32::from(config::MAX_JPG_QUALITY),
                    origin: f32::from(config::MIN_JPG_QUALITY),
                    default: f32::from(config::DEFAULT_JPG_QUALITY),
                    step: 1.0,
                },
                |v| format!("{}", v.round() as i32),
            );
        });
        let exif = widgets::checkbox(
            ui,
            &mut s.preserve_exif,
            t(Key::ExifKeep),
            theme::font_body(),
        );
        tip(exif, t(Key::TipExif));
        let keep = widgets::checkbox(
            ui,
            &mut s.keep_source_date,
            t(Key::KeepFileDate),
            theme::font_body(),
        );
        tip(keep, t(Key::TipKeepDate));
        let on_conflict = &mut s.on_conflict;
        cols.wide_row(
            ui,
            |ui| {
                tip(
                    widgets::dim_label(ui, t(Key::IfOutputExists)),
                    t(Key::TipConflict),
                );
            },
            |ui, w| {
                let combo = ComboBox::from_id_salt("on_conflict")
                    .width(w)
                    .selected_text(conflict_label(*on_conflict))
                    .show_ui(ui, |ui| {
                        for (p, label) in CONFLICT_CHOICES {
                            ui.selectable_value(on_conflict, p, t(label));
                        }
                    });
                tip(combo.response, t(Key::TipConflict));
            },
        );
        let summary = self.settings.output_summary();
        let button = widgets::quiet_button(
            ui,
            &tf(Key::OutputButton, &[&summary]),
            vec2(ui.available_width(), 28.0),
            theme::font_body(),
        );
        if tip(button, t(Key::TipOutputOptions)).clicked() {
            self.output_options_open = true;
        }
    }

    fn draw_presets(&mut self, ui: &mut Ui, actions: &mut Vec<Action>) {
        tip(
            widgets::section_title(ui, t(Key::SectionPresets)),
            t(Key::TipPreset),
        );
        let selected_preset = self.selected_preset.clone();
        let preset_names = self.presets.names();
        ui.horizontal(|ui| {
            let btn = vec2(58.0, 28.0);
            let gap = ui.spacing().item_spacing.x;
            let combo_w = (ui.available_width() - 2.0 * btn.x - 2.0 * gap - 8.0).max(100.0);
            let select_preset = t(Key::SelectPreset);
            let shown = selected_preset.as_deref().unwrap_or(select_preset);
            let combo = ComboBox::from_id_salt("presets")
                .width(combo_w)
                .selected_text(shown)
                .show_ui(ui, |ui| {
                    let _ = ui.selectable_label(selected_preset.is_none(), select_preset);
                    for name in &preset_names {
                        if ui
                            .selectable_label(selected_preset.as_deref() == Some(name), name)
                            .clicked()
                        {
                            actions.push(Action::ApplyPreset(name.clone()));
                        }
                    }
                });
            tip(combo.response, t(Key::TipPreset));
            let save = widgets::quiet_button(ui, t(Key::Save), btn, theme::font_body());
            if tip(save, t(Key::TipPresetSave)).clicked() {
                actions.push(Action::OpenSavePreset);
            }
            let delete = widgets::danger_button(ui, t(Key::Delete), btn, theme::font_body());
            if tip(delete, t(Key::TipPresetDelete)).clicked() {
                actions.push(Action::DeletePreset);
            }
        });
    }

    fn draw_app_section(&mut self, ui: &mut Ui) {
        widgets::section_title(ui, t(Key::SectionApp));
        let resp = widgets::checkbox(
            ui,
            &mut self.check_updates,
            t(Key::CheckUpdates),
            theme::font_body(),
        );
        if tip(resp, t(Key::TipCheckUpdates)).changed() {
            log::info!("Update check on startup: {}", self.check_updates);
        }
        let current = self.language;
        let combo = ComboBox::from_id_salt("language")
            .width(120.0)
            .selected_text(language_label(self.language))
            .show_ui(ui, |ui| {
                for choice in [UiLanguage::Auto, UiLanguage::Korean, UiLanguage::English] {
                    ui.selectable_value(&mut self.language, choice, language_label(choice));
                }
            });
        tip(combo.response, t(Key::TipLanguage));
        if self.language != current {
            i18n::apply(self.language);
            log::info!("Language: {}", self.language.as_str());
        }
        ui.label(
            RichText::new(tf(Key::FooterCredit, &[&config::APP_VERSION]))
                .font(theme::font_small())
                .color(TEXT_DIM),
        );
    }

    /// Always visible: status, progress and the primary button.
    fn draw_run_bar(&mut self, ui: &mut Ui, actions: &mut Vec<Action>) {
        let converting = self.is_converting();
        let total = ui.available_width();
        let gap = PAD_LARGE;
        let button_w = (total * 0.34).clamp(190.0, 280.0);
        let button_h = 48.0;
        let cancel_w = if converting { 80.0 + gap } else { 0.0 };
        let status_w = (total - button_w - cancel_w - gap).max(120.0);
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            ui.allocate_ui_with_layout(
                vec2(status_w, button_h),
                Layout::top_down(Align::Min),
                |ui| {
                    ui.set_width(status_w);
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        let eta_w = 124.0;
                        let text_w = (ui.available_width() - eta_w - 6.0).max(60.0);
                        widgets::cell(ui, text_w, 20.0, Align::Min, |ui| {
                            ui.add(
                                Label::new(
                                    RichText::new(&self.progress.status)
                                        .font(theme::font_body())
                                        .color(TEXT),
                                )
                                .truncate(),
                            );
                        });
                        widgets::cell(ui, eta_w, 20.0, Align::Max, |ui| {
                            if converting {
                                widgets::dim_label(ui, &self.progress.eta);
                            }
                        });
                    });
                    ui.add_space(4.0);
                    widgets::thin_progress(ui, self.progress.fraction, 6.0);
                },
            );
            if converting {
                let cancel = widgets::danger_button(
                    ui,
                    t(Key::CancelButton),
                    vec2(80.0, button_h),
                    theme::font_body_bold(),
                );
                if cancel.clicked() {
                    actions.push(Action::Cancel);
                }
            }
            let text = if converting {
                t(Key::Converting)
            } else {
                t(Key::StartButton)
            };
            let main = ui
                .add_enabled_ui(!converting, |ui| {
                    widgets::primary_button(
                        ui,
                        text,
                        vec2(button_w, button_h),
                        theme::font_header(),
                    )
                })
                .inner;
            if main.clicked() {
                actions.push(Action::Convert);
            }
        });
    }

    /// The batch result, shown in the photo area until it is closed.
    fn draw_result_card(&mut self, ui: &mut Ui, actions: &mut Vec<Action>, card_height: f32) {
        let Some(report) = &self.report else {
            return;
        };
        let width = (ui.available_width() - 2.0 * PAD_LARGE).clamp(280.0, 560.0);
        ui.vertical_centered(|ui| {
            Frame::new()
                .fill(PANEL)
                .corner_radius(CornerRadius::same(RADIUS_LARGE))
                .inner_margin(Margin::same(20))
                .show(ui, |ui| {
                    ui.set_width(width);
                    ui.with_layout(Layout::top_down(Align::Min), |ui| {
                        ui.label(
                            RichText::new(&report.title)
                                .font(theme::font_header())
                                .color(TEXT),
                        );
                        ui.add_space(10.0);
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 24.0;
                            count_tile(ui, report.converted, t(Key::ResultConverted), TEXT);
                            if report.skipped > 0 {
                                count_tile(ui, report.skipped, t(Key::ResultSkipped), TEXT_DIM);
                            }
                            let failed_color = if report.failed > 0 { DANGER } else { TEXT_DIM };
                            count_tile(ui, report.failed, t(Key::ResultFailed), failed_color);
                        });
                        ui.add_space(8.0);
                        if !report.elapsed.is_empty() {
                            widgets::note(ui, &report.elapsed);
                        }
                        if report.can_open {
                            widgets::note(
                                ui,
                                &tf(Key::ResultSavedTo, &[&report.output_dir.display()]),
                            );
                        }
                        if let Some(error) = &report.error {
                            ui.add_space(6.0);
                            ui.label(RichText::new(error).font(theme::font_body()).color(DANGER));
                        }
                        if report.kind == ReportKind::Partial || report.kind == ReportKind::Failed {
                            widgets::note(
                                ui,
                                &tf(Key::ResultLogHint, &[&report.log_file.display()]),
                            );
                        }
                        if !report.issues.is_empty() {
                            ui.add_space(10.0);
                            widgets::section_title(
                                ui,
                                &tf(Key::IssuesTitle, &[&report.issues.len()]),
                            );
                            let max = (card_height - 240.0).clamp(ISSUE_ROW_HEIGHT * 2.0, 320.0);
                            ScrollArea::vertical()
                                .id_salt("result_issues")
                                .max_height(max)
                                .auto_shrink([false, true])
                                .show_rows(
                                    ui,
                                    ISSUE_ROW_HEIGHT,
                                    report.issues.len(),
                                    |ui, range| {
                                        for issue in &report.issues[range] {
                                            ui.allocate_ui(
                                                vec2(width - 12.0, ISSUE_ROW_HEIGHT),
                                                |ui| {
                                                    ui.set_min_height(ISSUE_ROW_HEIGHT);
                                                    ui.add(
                                                        Label::new(
                                                            RichText::new(&issue.file)
                                                                .font(theme::font_body_bold())
                                                                .color(TEXT),
                                                        )
                                                        .truncate(),
                                                    );
                                                    widgets::note(ui, &issue.reason);
                                                },
                                            );
                                        }
                                    },
                                );
                        }
                        ui.add_space(14.0);
                        ui.horizontal(|ui| {
                            if report.can_open
                                && widgets::quiet_button(
                                    ui,
                                    t(Key::ResultOpenFolder),
                                    vec2(120.0, 32.0),
                                    theme::font_body_bold(),
                                )
                                .clicked()
                            {
                                actions.push(Action::OpenResultFolder);
                            }
                            if widgets::quiet_button(
                                ui,
                                t(Key::Close),
                                vec2(90.0, 32.0),
                                theme::font_body(),
                            )
                            .clicked()
                            {
                                actions.push(Action::CloseResult);
                            }
                        });
                    });
                });
        });
    }

    fn draw_narrow_center(&mut self, ui: &mut Ui, actions: &mut Vec<Action>) {
        ScrollArea::vertical()
            .id_salt("narrow_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                Frame::new()
                    .inner_margin(Margin::symmetric(PAD_LARGE as i8, PAD_MEDIUM as i8))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width() - 6.0);
                        if self.report.is_some() {
                            self.draw_result_card(ui, actions, 460.0);
                            ui.add_space(PAD_MEDIUM);
                        }
                        let embedded = self.settings.extract_embedded;
                        if self.preview.is_empty() {
                            Frame::new()
                                .fill(PHOTO_SURROUND)
                                .corner_radius(CornerRadius::same(RADIUS_LARGE))
                                .inner_margin(Margin::same(8))
                                .show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    self.preview.draw(ui, NARROW_EMPTY_HEIGHT, embedded);
                                });
                        } else {
                            ui.horizontal(|ui| {
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    let text = if self.preview.open {
                                        t(Key::HidePreview)
                                    } else {
                                        t(Key::ShowPreview)
                                    };
                                    let button = widgets::quiet_button(
                                        ui,
                                        text,
                                        vec2(120.0, 24.0),
                                        theme::font_small(),
                                    );
                                    if tip(button, t(Key::TipPreviewToggle)).clicked() {
                                        self.preview.open = !self.preview.open;
                                    }
                                });
                            });
                            if self.preview.open {
                                Frame::new()
                                    .fill(PHOTO_SURROUND)
                                    .corner_radius(CornerRadius::same(RADIUS_LARGE))
                                    .inner_margin(Margin::same(8))
                                    .show(ui, |ui| {
                                        ui.set_width(ui.available_width());
                                        self.preview.draw(ui, NARROW_PREVIEW_HEIGHT, embedded);
                                    });
                            }
                        }
                        ui.add_space(PAD_LARGE);
                        self.draw_adjustments(ui, actions);
                    });
            });
    }

    /// One frame of the whole window. `frame` is `None` in the UI tests.
    fn run_frame(&mut self, ui: &mut Ui, frame: Option<&eframe::Frame>) {
        let ctx = ui.ctx().clone();
        if !self.window_prepared {
            if let Some(native) = frame {
                ctx.send_viewport_cmd(ViewportCommand::SetTheme(egui::SystemTheme::Dark));
                crate::titlebar::apply(native);
                if let Some(monitor) = ctx.input(|i| i.viewport().monitor_size) {
                    let [w, h] = fitted_window_size(DEFAULT_WINDOW, monitor);
                    if w < DEFAULT_WINDOW[0] || h < DEFAULT_WINDOW[1] {
                        ctx.send_viewport_cmd(ViewportCommand::InnerSize(vec2(w, h)));
                    }
                }
            }
            self.window_prepared = true;
        }
        self.poll_job(&ctx);
        self.poll_update(&ctx);
        self.handle_close_request(&ctx, frame);
        self.handle_drops(&ctx);
        if let Some(s) = &self.selection {
            // The selection belongs to its root; any other source replaces it.
            if self.source.trim() != s.root.to_string_lossy() {
                self.selection = None;
            }
        }

        let wide = ctx.content_rect().width() >= WIDE_LAYOUT_WIDTH;
        let visible = wide || self.preview.open;
        self.preview.tick(
            &ctx,
            &self.source,
            self.selection.as_ref(),
            &self.settings.to_convert(),
            self.is_converting(),
            visible,
        );

        let mut actions = Vec::new();

        Panel::bottom("run_bar")
            .exact_size(RUN_BAR_HEIGHT)
            .resizable(false)
            .show_separator_line(false)
            .frame(
                Frame::new()
                    .fill(PANEL)
                    .inner_margin(Margin::symmetric(PAD_LARGE as i8, 10)),
            )
            .show(ui, |ui| self.draw_run_bar(ui, &mut actions));

        Panel::top("folders")
            .resizable(false)
            .show_separator_line(false)
            .frame(
                Frame::new()
                    .fill(BG)
                    .inner_margin(Margin::symmetric(PAD_LARGE as i8, 8)),
            )
            .show(ui, |ui| self.draw_paths(ui, &mut actions));

        if wide {
            Panel::left("adjustments")
                .exact_size(ADJUSTMENTS_WIDTH)
                .resizable(false)
                .show_separator_line(false)
                .frame(Frame::new().fill(BG))
                .show(ui, |ui| {
                    ScrollArea::vertical()
                        .id_salt("adjustments_scroll")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            Frame::new()
                                .inner_margin(Margin {
                                    left: PAD_LARGE as i8,
                                    right: 10,
                                    top: PAD_MEDIUM as i8,
                                    bottom: PAD_LARGE as i8,
                                })
                                .show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    self.draw_adjustments(ui, &mut actions);
                                });
                        });
                });
            egui::CentralPanel::default()
                .frame(
                    Frame::new()
                        .fill(PHOTO_SURROUND)
                        .inner_margin(Margin::same(PAD_MEDIUM as i8)),
                )
                .show(ui, |ui| {
                    let height = ui.available_height();
                    if self.report.is_some() {
                        ui.add_space((height * 0.06).min(40.0));
                        self.draw_result_card(ui, &mut actions, height);
                    } else {
                        let embedded = self.settings.extract_embedded;
                        let image_h = (height - 32.0).max(120.0);
                        self.preview.draw(ui, image_h, embedded);
                    }
                });
        } else {
            egui::CentralPanel::default()
                .frame(Frame::new().fill(BG))
                .show(ui, |ui| self.draw_narrow_center(ui, &mut actions));
        }

        self.draw_preset_modal(&ctx, &mut actions);
        self.draw_update_modal(&ctx, &mut actions);
        self.draw_output_options_modal(&ctx);
        self.draw_overlays(&ctx);

        for action in actions {
            self.run_action(action, &ctx, frame);
        }
    }

    fn draw_output_options_modal(&mut self, ctx: &egui::Context) {
        if !self.output_options_open {
            return;
        }
        let s = &mut self.settings;
        let mut close = false;
        let modal = egui::Modal::new(Id::unique("output_options_modal"))
            .frame(modal_frame())
            .show(ctx, |ui| {
                let width = (ctx.content_rect().width() - 80.0).clamp(300.0, 480.0);
                ui.set_width(width);
                modal_title(ui, t(Key::OutputOptionsTitle));
                ui.add_space(6.0);
                let combo_w = width - MODAL_LABEL_WIDTH - ui.spacing().item_spacing.x;
                let row_label = |ui: &mut Ui, text: &str, tooltip: &str| {
                    widgets::cell(ui, MODAL_LABEL_WIDTH, 26.0, Align::Min, |ui| {
                        tip(widgets::dim_label(ui, text), tooltip);
                    });
                };
                ui.add_enabled_ui(!s.extract_embedded, |ui| {
                    ui.horizontal(|ui| {
                        row_label(ui, t(Key::FormatLabel), t(Key::TipFormat));
                        let combo = ComboBox::from_id_salt("output_format")
                            .width(combo_w)
                            .selected_text(s.output_format.label())
                            .show_ui(ui, |ui| {
                                for f in OutputFormat::ALL {
                                    ui.selectable_value(&mut s.output_format, f, f.label());
                                }
                            });
                        tip(combo.response, t(Key::TipFormat));
                    });
                    widgets::note(ui, t(format_note(s.output_format)));
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        row_label(ui, t(Key::ColorSpaceLabel), t(Key::TipColorSpace));
                        let combo = ComboBox::from_id_salt("color_space")
                            .width(combo_w)
                            .selected_text(s.color_space.label())
                            .show_ui(ui, |ui| {
                                for c in ColorSpace::ALL {
                                    ui.selectable_value(&mut s.color_space, c, c.label());
                                }
                            });
                        tip(combo.response, t(Key::TipColorSpace));
                    });
                    widgets::note(ui, t(color_note(s.color_space)));
                });
                if s.extract_embedded {
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(t(Key::CameraJpegOnly))
                            .font(theme::font_small_bold())
                            .color(TEXT),
                    );
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    row_label(ui, t(Key::NameTemplateLabel), t(Key::TipNameTemplate));
                    let field = widgets::text_field(ui, &mut s.filename_template, "", combo_w);
                    tip(field, t(Key::TipNameTemplate));
                });
                widgets::note(ui, t(Key::NameTokens));
                match s.name_example() {
                    Ok(name) => {
                        ui.label(
                            RichText::new(tf(Key::NameExample, &[&name]))
                                .font(theme::font_body_bold())
                                .color(TEXT),
                        );
                    }
                    Err(e) => {
                        ui.label(RichText::new(e).font(theme::font_body_bold()).color(DANGER));
                    }
                }
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if widgets::quiet_button(
                            ui,
                            t(Key::Close),
                            vec2(100.0, 30.0),
                            theme::font_body_bold(),
                        )
                        .clicked()
                        {
                            close = true;
                        }
                    });
                });
            });
        if close || modal.should_close() {
            self.output_options_open = false;
        }
    }

    fn draw_preset_modal(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        let Some(name) = self.preset_name_input.as_mut() else {
            return;
        };
        let mut close = false;
        let modal = egui::Modal::new(Id::unique("save_preset_modal"))
            .frame(modal_frame())
            .show(ctx, |ui| {
                ui.set_width(300.0);
                modal_title(ui, t(Key::SavePresetTitle));
                ui.add_space(4.0);
                widgets::dim_label(ui, t(Key::EnterPresetName));
                let resp = widgets::text_field(ui, name, "", 300.0);
                if !resp.has_focus() && !resp.lost_focus() {
                    resp.request_focus();
                }
                let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if widgets::quiet_button(
                            ui,
                            t(Key::Save),
                            vec2(80.0, 30.0),
                            theme::font_body_bold(),
                        )
                        .clicked()
                            || enter
                        {
                            actions.push(Action::SavePreset(name.clone()));
                        }
                        if widgets::quiet_button(
                            ui,
                            t(Key::CancelButton),
                            vec2(80.0, 30.0),
                            theme::font_body(),
                        )
                        .clicked()
                        {
                            close = true;
                        }
                    });
                });
            });
        if close || modal.should_close() {
            self.preset_name_input = None;
        }
    }

    fn draw_update_modal(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        let Some(info) = &self.update_info else {
            return;
        };
        let mut close = false;
        let modal = egui::Modal::new(Id::unique("update_modal"))
            .frame(modal_frame())
            .show(ctx, |ui| {
                let width = (ctx.content_rect().width() - 80.0).clamp(280.0, 460.0);
                ui.set_width(width);
                modal_title(ui, t(Key::UpdateAvailable));
                ui.add_space(4.0);
                ui.label(
                    RichText::new(tf(
                        Key::UpdateVersions,
                        &[&config::APP_VERSION, &info.version],
                    ))
                    .font(theme::font_body_bold())
                    .color(TEXT),
                );
                ui.add_space(6.0);
                Frame::new()
                    .fill(BG)
                    .stroke(Stroke::new(1.0, BORDER))
                    .corner_radius(CornerRadius::same(RADIUS_MEDIUM))
                    .inner_margin(Margin::same(8))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ScrollArea::vertical()
                            .max_height(220.0)
                            .auto_shrink([false, true])
                            .show(ui, |ui| {
                                ui.label(
                                    RichText::new(&info.release_notes)
                                        .font(theme::font_body())
                                        .color(TEXT),
                                );
                            });
                    });
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if widgets::quiet_button(
                            ui,
                            t(Key::Download),
                            vec2(120.0, 30.0),
                            theme::font_body_bold(),
                        )
                        .clicked()
                        {
                            actions.push(Action::OpenUrl(info.html_url.clone()));
                            close = true;
                        }
                        if widgets::quiet_button(
                            ui,
                            t(Key::Later),
                            vec2(100.0, 30.0),
                            theme::font_body(),
                        )
                        .clicked()
                        {
                            close = true;
                        }
                    });
                });
            });
        if close || modal.should_close() {
            self.update_info = None;
        }
    }
}

impl eframe::App for CcooNverterApp {
    fn ui(&mut self, ui: &mut Ui, frame: &mut eframe::Frame) {
        self.run_frame(ui, Some(frame));
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        BG.to_normalized_gamma_f32()
    }

    // eframe always compiles the glow backend here, so the trait method has
    // the `gl` parameter in every build.
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.shutdown();
    }
}

/// One settings row with a slider and its value; `true` while the slider is moving.
#[allow(clippy::too_many_arguments)]
fn slider_row(
    ui: &mut Ui,
    cols: Columns,
    label: &str,
    tooltip: &str,
    value: &mut f32,
    spec: &SliderSpec<'_>,
    format: impl Fn(f32) -> String,
) -> bool {
    cols.row(
        ui,
        |ui| {
            tip(widgets::dim_label(ui, label), tooltip);
        },
        |ui, w| {
            let result = widgets::slider(ui, value, spec, w);
            tip(result.response, tooltip);
            (*value, result.active)
        },
        |ui, (shown, active)| {
            widgets::value_label(ui, &format(*shown), *active);
        },
    )
    .1
}

/// Big number with a small label under it.
fn count_tile(ui: &mut Ui, count: usize, label: &str, color: Color32) {
    ui.vertical(|ui| {
        ui.label(
            RichText::new(count.to_string())
                .font(egui::FontId::new(
                    24.0,
                    egui::FontFamily::Name(theme::BOLD_FAMILY.into()),
                ))
                .color(color),
        );
        ui.label(
            RichText::new(label)
                .font(theme::font_small())
                .color(TEXT_DIM),
        );
    });
}

fn modal_frame() -> Frame {
    Frame::new()
        .fill(PANEL)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(RADIUS_LARGE))
        .inner_margin(Margin::same(18))
}

fn modal_title(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).font(theme::font_header()).color(TEXT));
}

/// UV of the centre of a `size` image cropped to aspect ratio `aspect`.
fn cover_uv(size: [usize; 2], aspect: f32) -> Rect {
    let image_aspect = size[0] as f32 / size[1] as f32;
    if image_aspect > aspect {
        let w = aspect / image_aspect;
        Rect::from_min_max(pos2(0.5 - w / 2.0, 0.0), pos2(0.5 + w / 2.0, 1.0))
    } else {
        let h = image_aspect / aspect;
        Rect::from_min_max(pos2(0.0, 0.5 - h / 2.0), pos2(1.0, 0.5 + h / 2.0))
    }
}

/// Language combo text.
fn language_label(choice: UiLanguage) -> &'static str {
    match choice {
        UiLanguage::Auto => t(Key::LanguageSystem),
        UiLanguage::English => t(Key::LanguageEnglish),
        UiLanguage::Korean => t(Key::LanguageKorean),
    }
}

/// Native folder picker starting at the current text (if it is a folder).
fn pick_folder(current: &str, title: &str) -> Option<PathBuf> {
    let mut dialog = rfd::FileDialog::new().set_title(title);
    let cur = Path::new(current.trim());
    if !current.trim().is_empty() && cur.is_dir() {
        dialog = dialog.set_directory(cur);
    }
    dialog.pick_folder()
}

/// Open a folder or URL in the operating system's handler.
fn open_external(target: &Path) {
    if cfg!(test) {
        log::info!("Would open {}", target.display());
        return;
    }
    if let Err(e) = open::that_detached(target) {
        log::error!("Could not open {}: {e}", target.display());
    }
}

/// Background update check, started 1 s after launch (Python `_check_for_updates`).
fn spawn_update_check(cache_file: PathBuf) -> Receiver<ReleaseInfo> {
    let (tx, rx) = mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("ccoon-update".into())
        .spawn(move || {
            std::thread::sleep(Duration::from_secs(1));
            log::info!("Starting background update check");
            match update::check_for_update(config::APP_VERSION, &cache_file) {
                Ok(Some(info)) => {
                    let _ = tx.send(info);
                }
                Ok(None) => log::info!("No updates found"),
                Err(e) => log::warn!("Update check failed: {e}"),
            }
        });
    if let Err(e) = spawned {
        log::warn!("Could not start update check thread: {e}");
    }
    rx
}

#[cfg(test)]
#[path = "ui_tests.rs"]
mod ui_tests;
