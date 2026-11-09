//! The main window (port of Python `main.App` + `ui_components`).

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
    self, vec2, Align, Align2, Color32, ComboBox, Frame, Id, Label, LayerId, Layout, Margin, Order,
    RichText, ScrollArea, Stroke, StrokeKind, Ui, ViewportCommand,
};

use crate::controller::{
    batch_issues, color_note, completion_message, conflict_label, format_note, plan_drop,
    prepare_batch, prepare_selection, style_name, wb_name, BatchIssue, BatchOutcome, DropPlan,
    DropSelection, ProgressView, RunningBatch, UiSettings, CONFLICT_CHOICES,
};
use crate::dialogs;
use crate::i18n::{self, t, tf, Key};
use crate::preview::PreviewWorker;
use crate::preview_panel::PreviewPanel;
use crate::theme::{
    self, ACCENT, BG, BORDER, DANGER, PAD_LARGE, PAD_MEDIUM, PANEL, TEXT, TEXT_DIM,
};
use crate::widgets::{self, tip, Columns};

/// From this window width on, settings and preview sit side by side.
const WIDE_LAYOUT_WIDTH: f32 = 1040.0;
/// Width of the settings column in the wide layout.
const SETTINGS_COLUMN: f32 = 600.0;
/// Label column of the output options dialog.
const MODAL_LABEL_WIDTH: f32 = 110.0;
/// Width of the "if output exists" combo box beside the output options button.
const CONFLICT_COMBO_WIDTH: f32 = 118.0;
/// Height of one row in the list of files that need attention.
const ISSUE_ROW_HEIGHT: f32 = 58.0;
/// Height of the image area when the preview is expanded in the narrow layout.
const NARROW_PREVIEW_HEIGHT: f32 = 290.0;
/// How long a drop notice stays visible.
const NOTICE_TIME: Duration = Duration::from_secs(6);

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
}

pub struct CcooNverterApp {
    paths: AppPaths,
    source: String,
    output: String,
    settings: UiSettings,
    check_updates: bool,
    language: UiLanguage,

    presets: PresetStore,
    /// Name shown in the preset combo (`None` = "Select Preset...").
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
    /// Dark native title bar requested (sent on the first frame, once the window exists).
    title_bar_themed: bool,

    preview: PreviewPanel,
    /// Files staged by a drop: converted instead of scanning the source folder.
    selection: Option<DropSelection>,
    /// Transient message after a drop.
    notice: Option<(String, Instant)>,
    /// Output options modal is open.
    output_options_open: bool,
    /// Files that need attention after the last batch, shown in a modal.
    issues: Option<Vec<BatchIssue>>,
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
            title_bar_themed: false,
            preview: PreviewPanel::new(worker, app_settings.preview_open),
            selection: None,
            notice: None,
            output_options_open: false,
            issues: None,
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

    fn save_presets(&self, frame: &eframe::Frame) -> bool {
        let path = self.paths.presets_file();
        match self.presets.save(&path) {
            Ok(()) => true,
            Err(e) => {
                log::error!("Failed to save presets to {}: {e}", path.display());
                dialogs::error(
                    Some(frame),
                    t(Key::TitleError),
                    &tf(Key::SavePresetFailed, &[&e]),
                );
                false
            }
        }
    }

    // ------------------------------------------------------------------
    // Per-frame background work
    // ------------------------------------------------------------------

    fn poll_job(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
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
            // The panel returns to READY; the summary goes into the dialog.
            let output_dir = job.output_dir.clone();
            self.job = None;
            self.progress.reset();
            self.on_batch_finished(outcome, &output_dir, frame);
        } else {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    fn on_batch_finished(
        &mut self,
        outcome: BatchOutcome,
        output_dir: &Path,
        frame: &eframe::Frame,
    ) {
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
        if let BatchOutcome::Finished(s) = &outcome {
            let issues = batch_issues(s);
            if !issues.is_empty() {
                self.issues = Some(issues);
            }
        }
        let msg = completion_message(&outcome, output_dir, &self.paths.log_file());
        if msg.offer_open {
            let body = tf(Key::OpenOutputFolderQuestion, &[&msg.body]);
            if dialogs::ask_yes_no(Some(frame), msg.kind, &msg.title, &body) {
                if let Err(e) = open::that_detached(output_dir) {
                    log::error!("Could not open {}: {e}", output_dir.display());
                }
            }
        } else {
            dialogs::show(Some(frame), msg.kind, &msg.title, &msg.body);
        }
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

    fn handle_close_request(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        if !ctx.input(|i| i.viewport().close_requested()) {
            return;
        }
        if self.is_converting() && !self.close_confirmed {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            if dialogs::confirm(Some(frame), t(Key::TitleQuit), t(Key::QuitWhileConverting)) {
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

    fn run_action(&mut self, action: Action, ctx: &egui::Context, frame: &eframe::Frame) {
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
                            Some(frame),
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
                dialogs::info(Some(frame), t(Key::TitleReset), t(Key::ResetDone));
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
                    dialogs::info(
                        Some(frame),
                        t(Key::TitleSuccess),
                        &tf(Key::PresetSaved, &[&name]),
                    );
                }
            }
            Action::DeletePreset => {
                let Some(name) = self.selected_preset.clone() else {
                    return;
                };
                if dialogs::confirm(
                    Some(frame),
                    t(Key::TitleConfirm),
                    &tf(Key::ConfirmDeletePreset, &[&name]),
                ) && self.presets.remove(&name)
                {
                    self.selected_preset = None;
                    if self.save_presets(frame) {
                        log::info!("Deleted preset '{name}'");
                        dialogs::info(
                            Some(frame),
                            t(Key::TitleSuccess),
                            &tf(Key::PresetDeleted, &[&name]),
                        );
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
            Action::OpenUrl(url) => {
                if let Err(e) = open::that_detached(&url) {
                    log::error!("Could not open {url}: {e}");
                }
            }
        }
    }

    fn start_conversion(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
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
                    let (kind, title, msg) = e.dialog();
                    dialogs::show(Some(frame), kind, &title, &msg);
                    return;
                }
            },
        };
        let settings = self.settings.to_convert();
        match RunningBatch::spawn(batch, settings, None) {
            Ok(job) => {
                self.progress = ProgressView::starting(job.total);
                self.job = Some(job);
                ctx.request_repaint();
            }
            Err(e) => {
                log::error!("Could not start worker thread: {e}");
                dialogs::error(
                    Some(frame),
                    t(Key::TitleError),
                    &tf(Key::CouldNotStart, &[&e]),
                );
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
                0.0,
                Stroke::new(3.0, ACCENT),
                StrokeKind::Inside,
            );
            painter.text(
                rect.center() - vec2(0.0, 14.0),
                Align2::CENTER_CENTER,
                t(Key::DropHoverTitle),
                theme::font_header(),
                ACCENT,
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
            let w = galley.size().x + 28.0;
            let h = galley.size().y + 14.0;
            let rect = egui::Rect::from_center_size(
                egui::pos2(rect.center().x, rect.bottom() - h / 2.0 - 12.0),
                vec2(w, h),
            );
            painter.rect_filled(rect, 0.0, PANEL);
            painter.rect_stroke(rect, 0.0, Stroke::new(1.0, ACCENT), StrokeKind::Inside);
            painter.galley(rect.center() - galley.size() / 2.0, galley.clone(), TEXT);
        }
    }

    fn draw(&mut self, ui: &mut Ui, actions: &mut Vec<Action>, narrow: bool) {
        panel(ui, |ui| self.draw_paths(ui, actions, narrow));
        if narrow && self.preview.open {
            ui.add_space(PAD_MEDIUM);
            let embedded = self.settings.extract_embedded;
            panel(ui, |ui| {
                self.preview.draw(ui, NARROW_PREVIEW_HEIGHT, embedded);
            });
        }
        ui.add_space(PAD_MEDIUM);
        panel(ui, |ui| self.draw_settings(ui, actions));
        ui.add_space(PAD_MEDIUM);
        self.draw_actions(ui, actions);
        ui.add_space(PAD_MEDIUM);
        panel(ui, |ui| self.draw_progress(ui));
        ui.add_space(PAD_MEDIUM);
        self.draw_footer(ui);
    }

    fn draw_scrolling_column(&mut self, ui: &mut Ui, actions: &mut Vec<Action>, narrow: bool) {
        ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                Frame::new()
                    .inner_margin(Margin::symmetric(PAD_LARGE as i8, 0))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        self.draw(ui, actions, narrow);
                    });
            });
    }

    fn draw_paths(&mut self, ui: &mut Ui, actions: &mut Vec<Action>, narrow: bool) {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(t(Key::DirectoryPaths))
                    .font(theme::font_body_bold())
                    .color(ACCENT),
            );
            if narrow {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let text = if self.preview.open {
                        t(Key::HidePreview)
                    } else {
                        t(Key::ShowPreview)
                    };
                    let button = widgets::outline_button(
                        ui,
                        text,
                        vec2(112.0, 20.0),
                        theme::font_small_bold(),
                        ACCENT,
                        Color32::TRANSPARENT,
                        BORDER,
                    );
                    if tip(button, t(Key::TipPreviewToggle)).clicked() {
                        self.preview.open = !self.preview.open;
                    }
                });
            }
        });
        let cols = Columns {
            label: 186.0,
            value: 84.0,
            row_height: 28.0,
        };
        let folder_row =
            |ui: &mut Ui, title: &str, text: &mut String, hint: &str, tooltip: &str| -> bool {
                let mut browse = false;
                cols.row(
                    ui,
                    |ui| {
                        let label = ui.label(
                            RichText::new(title)
                                .font(theme::font_body_bold())
                                .color(TEXT_DIM),
                        );
                        tip(label, tooltip);
                    },
                    |ui, w| {
                        tip(widgets::text_field(ui, text, hint, w), tooltip);
                    },
                    |ui, _| {
                        let button = widgets::gold_button(
                            ui,
                            t(Key::Browse),
                            vec2(80.0, 28.0),
                            theme::font_body_bold(),
                        );
                        browse = tip(button, t(Key::TipBrowse)).clicked();
                    },
                );
                browse
            };
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
                ui.add_space(cols.label + ui.spacing().item_spacing.x);
                ui.label(
                    RichText::new(tf(Key::DroppedSelected, &[&count]))
                        .font(theme::font_small())
                        .color(ACCENT),
                );
                if widgets::outline_button(
                    ui,
                    t(Key::Clear),
                    vec2(48.0, 18.0),
                    theme::font_small_bold(),
                    TEXT_DIM,
                    Color32::TRANSPARENT,
                    BORDER,
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
        let summary = self.settings.output_summary();
        let on_conflict = &mut self.settings.on_conflict;
        let open_options = &mut self.output_options_open;
        wide_row(
            ui,
            cols,
            |ui| {
                let label = ui.label(
                    RichText::new(t(Key::IfOutputExists))
                        .font(theme::font_body())
                        .color(TEXT_DIM),
                );
                tip(label, t(Key::TipConflict));
            },
            |ui, w| {
                let combo_w = CONFLICT_COMBO_WIDTH;
                let combo = ComboBox::from_id_salt("on_conflict")
                    .width(combo_w)
                    .selected_text(conflict_label(*on_conflict))
                    .show_ui(ui, |ui| {
                        for (p, label) in CONFLICT_CHOICES {
                            ui.selectable_value(on_conflict, p, t(label));
                        }
                    });
                tip(combo.response, t(Key::TipConflict));
                let button_w = (w - combo_w - ui.spacing().item_spacing.x - 12.0).max(120.0);
                let button = widgets::outline_button(
                    ui,
                    &tf(Key::OutputButton, &[&summary]),
                    vec2(button_w, 24.0),
                    theme::font_small_bold(),
                    ACCENT,
                    Color32::TRANSPARENT,
                    BORDER,
                );
                if tip(button, t(Key::TipOutputOptions)).clicked() {
                    *open_options = true;
                }
            },
        );
    }

    fn draw_settings(&mut self, ui: &mut Ui, actions: &mut Vec<Action>) {
        let cols = Columns::SETTINGS;
        let s = &mut self.settings;

        // SETTINGS header + embedded checkbox (right-aligned).
        ui.horizontal(|ui| {
            widgets::header(ui, t(Key::Settings));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let check = widgets::checkbox(
                    ui,
                    &mut s.extract_embedded,
                    t(Key::ExtractEmbedded),
                    theme::font_body_bold(),
                );
                tip(check, t(Key::TipEmbedded));
            });
        });

        // EXIF PRESERVATION header + checkbox, "keep file date" on the same
        // row; the "not all metadata" note lives in the tooltip (row height is tight).
        ui.horizontal(|ui| {
            widgets::cell(ui, cols.label, cols.row_height, Align::Min, |ui| {
                tip(
                    widgets::header(ui, t(Key::ExifPreservation)),
                    t(Key::TipExif),
                );
            });
            let check = widgets::checkbox(ui, &mut s.preserve_exif, "", theme::font_body_bold());
            tip(check, t(Key::TipExif));
            ui.add_space(14.0);
            let keep = widgets::checkbox(
                ui,
                &mut s.keep_source_date,
                t(Key::KeepFileDate),
                theme::font_body(),
            );
            tip(keep, t(Key::TipKeepDate));
        });

        // Resize.
        cols.row(
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
            |ui, _| {
                ui.label(
                    RichText::new(t(Key::ResizeOriginal))
                        .font(theme::font_small())
                        .color(TEXT_DIM),
                );
            },
        );

        // Quality.
        cols.row(
            ui,
            |ui| {
                tip(
                    widgets::dim_label(ui, t(Key::QualityLabel)),
                    t(Key::TipQuality),
                );
            },
            |ui, w| {
                // PNG and TIFF ignore the quality; embedded mode is JPEG.
                let uses_quality = s.effective_format().uses_quality();
                ui.add_enabled_ui(uses_quality, |ui| {
                    let slider = widgets::color_slider(
                        ui,
                        &mut s.quality,
                        f32::from(config::MIN_JPG_QUALITY)..=f32::from(config::MAX_JPG_QUALITY),
                        Some(1.0),
                        ACCENT,
                        theme::ACCENT_HOVER,
                        w,
                    );
                    tip(slider, t(Key::TipQuality));
                });
                s.quality
            },
            |ui, q| {
                widgets::value_label(ui, &format!("{}", q.round() as i32));
            },
        );

        // RAW ADJUSTMENTS + PRESETS: disabled in embedded mode (Python toggle_raw_controls).
        let raw_enabled = !s.extract_embedded;
        let selected_preset = self.selected_preset.clone();
        let preset_names = self.presets.names();
        ui.add_enabled_ui(raw_enabled, |ui| {
            ui.add_space(PAD_MEDIUM);
            ui.horizontal(|ui| {
                widgets::header(ui, t(Key::RawAdjustments));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.add_space(cols.value - 60.0 - 2.0);
                    let reset = widgets::outline_button(
                        ui,
                        t(Key::Reset),
                        vec2(60.0, 20.0),
                        theme::font_small_bold(),
                        ACCENT,
                        Color32::TRANSPARENT,
                        BORDER,
                    );
                    if tip(reset, t(Key::TipReset)).clicked() {
                        actions.push(Action::ResetAdjustments);
                    }
                    ui.add_space(10.0);
                    let curve = widgets::checkbox(
                        ui,
                        &mut s.tone_curve,
                        t(Key::ToneCurve),
                        theme::font_body(),
                    );
                    tip(curve, t(Key::TipToneCurve));
                });
            });

            // Labels stay dim; the disabled Ui fades them further.
            let label = |ui: &mut Ui, text: &str, tooltip: &str| {
                let resp = ui.label(RichText::new(text).font(theme::font_body()).color(TEXT_DIM));
                tip(resp, tooltip);
            };

            // White balance (spans control + value columns like Python).
            wide_row(
                ui,
                cols,
                |ui| label(ui, t(Key::WhiteBalanceLabel), t(Key::TipWhiteBalance)),
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

            wide_row(
                ui,
                cols,
                |ui| label(ui, t(Key::PictureStyleLabel), t(Key::TipStyle)),
                |ui, w| {
                    let combo = ComboBox::from_id_salt("picture_style")
                        .width(w)
                        .selected_text(t(style_name(s.style)))
                        .show_ui(ui, |ui| {
                            for st in PictureStyle::ALL {
                                ui.selectable_value(&mut s.style, st, t(style_name(st)));
                            }
                        });
                    tip(combo.response, t(Key::TipStyle));
                },
            );

            cols.row(
                ui,
                |ui| label(ui, t(Key::ExposureLabel), t(Key::TipExposure)),
                |ui, w| {
                    let slider = widgets::color_slider(
                        ui,
                        &mut s.exposure_ev,
                        config::MIN_EXPOSURE_EV..=config::MAX_EXPOSURE_EV,
                        Some(0.05),
                        ACCENT,
                        theme::ACCENT_HOVER,
                        w,
                    );
                    tip(slider, t(Key::TipExposure));
                    s.exposure_ev
                },
                |ui, ev| {
                    widgets::value_label(ui, &format!("{ev:+.2}"));
                },
            );

            cols.row(
                ui,
                |ui| label(ui, t(Key::BrightnessLabel), t(Key::TipBrightness)),
                |ui, w| {
                    let slider = widgets::color_slider(
                        ui,
                        &mut s.brightness,
                        config::MIN_BRIGHTNESS..=config::MAX_BRIGHTNESS,
                        Some(0.01),
                        ACCENT,
                        theme::ACCENT_HOVER,
                        w,
                    );
                    tip(slider, t(Key::TipBrightness));
                    s.brightness
                },
                |ui, b| {
                    widgets::value_label(ui, &format!("{b:.2}"));
                },
            );

            let rgb = [
                (
                    t(Key::RgbBalanceLabel),
                    "R",
                    theme::SLIDER_RED,
                    theme::SLIDER_RED_HOVER,
                ),
                ("", "G", theme::SLIDER_GREEN, theme::SLIDER_GREEN_HOVER),
                ("", "B", theme::SLIDER_BLUE, theme::SLIDER_BLUE_HOVER),
            ];
            for (i, (title, ch, color, hover)) in rgb.into_iter().enumerate() {
                let v = &mut s.rgb[i];
                cols.row(
                    ui,
                    |ui| label(ui, title, t(Key::TipRgbBalance)),
                    |ui, w| {
                        let slider = widgets::color_slider(
                            ui,
                            v,
                            config::MIN_RGB_VALUE..=config::MAX_RGB_VALUE,
                            Some(0.01),
                            color,
                            hover,
                            w,
                        );
                        tip(slider, t(Key::TipRgbBalance));
                        *v
                    },
                    |ui, v| {
                        widgets::value_label(ui, &format!("{ch}: {v:.2}"));
                    },
                );
            }

            cols.row(
                ui,
                |ui| label(ui, t(Key::LevelsLabel), t(Key::TipLevels)),
                |ui, w| {
                    let slider = widgets::color_slider(
                        ui,
                        &mut s.black_point,
                        f32::from(config::MIN_BLACK_POINT)..=f32::from(config::MAX_BLACK_POINT),
                        Some(1.0),
                        TEXT_DIM,
                        TEXT,
                        w,
                    );
                    tip(slider, t(Key::TipLevels));
                    s.black_point
                },
                |ui, bp| {
                    widgets::value_label(ui, &format!("BP: {}", bp.round() as i32));
                },
            );
            cols.row(
                ui,
                |ui| label(ui, "", t(Key::TipLevels)),
                |ui, w| {
                    let slider = widgets::color_slider(
                        ui,
                        &mut s.white_point,
                        f32::from(config::MIN_WHITE_POINT)..=f32::from(config::MAX_WHITE_POINT),
                        Some(1.0),
                        theme::SLIDER_WHITE,
                        theme::SLIDER_WHITE_HOVER,
                        w,
                    );
                    tip(slider, t(Key::TipLevels));
                    s.white_point
                },
                |ui, wp| {
                    widgets::value_label(ui, &format!("WP: {}", wp.round() as i32));
                },
            );

            // PRESETS.
            ui.add_space(PAD_MEDIUM);
            tip(widgets::header(ui, t(Key::Presets)), t(Key::TipPreset));
            ui.horizontal(|ui| {
                let btn = vec2(64.0, 28.0);
                let gap = ui.spacing().item_spacing.x;
                let combo_w = (ui.available_width() - 2.0 * btn.x - 2.0 * gap - 8.0).max(120.0);
                let select_preset = t(Key::SelectPreset);
                let shown = selected_preset.as_deref().unwrap_or(select_preset);
                let combo = ComboBox::from_id_salt("presets")
                    .width(combo_w)
                    .selected_text(shown)
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(selected_preset.is_none(), select_preset)
                            .clicked()
                        {
                            // Python: choosing the placeholder does nothing.
                        }
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
                let save = widgets::gold_button(ui, t(Key::Save), btn, theme::font_body_bold());
                if tip(save, t(Key::TipPresetSave)).clicked() {
                    actions.push(Action::OpenSavePreset);
                }
                let delete = widgets::outline_button(
                    ui,
                    t(Key::Delete),
                    btn,
                    theme::font_body_bold(),
                    DANGER,
                    PANEL,
                    BG,
                );
                if tip(delete, t(Key::TipPresetDelete)).clicked() {
                    actions.push(Action::DeletePreset);
                }
            });
        });
    }

    fn draw_actions(&mut self, ui: &mut Ui, actions: &mut Vec<Action>) {
        let converting = self.is_converting();
        ui.horizontal(|ui| {
            let gap = PAD_MEDIUM;
            let total = ui.available_width();
            let main_w = ((total - gap) * 0.75).floor();
            let cancel_w = total - gap - main_w;
            ui.spacing_mut().item_spacing.x = gap;
            let text = if converting {
                t(Key::Converting)
            } else {
                t(Key::StartButton)
            };
            let main = ui
                .add_enabled_ui(!converting, |ui| {
                    widgets::gold_button(ui, text, vec2(main_w, 50.0), theme::font_header())
                })
                .inner;
            if main.clicked() {
                actions.push(Action::Convert);
            }
            let cancel = ui
                .add_enabled_ui(converting, |ui| {
                    widgets::outline_button(
                        ui,
                        t(Key::CancelButton),
                        vec2(cancel_w, 50.0),
                        theme::font_header(),
                        DANGER,
                        BG,
                        PANEL,
                    )
                })
                .inner;
            if cancel.clicked() {
                actions.push(Action::Cancel);
            }
        });
    }

    fn draw_progress(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            let eta_w = 140.0;
            let h = 22.0;
            let status_w = (ui.available_width() - eta_w - ui.spacing().item_spacing.x).max(80.0);
            widgets::cell(ui, status_w, h, Align::Min, |ui| {
                ui.add(
                    Label::new(
                        RichText::new(&self.progress.status)
                            .font(theme::font_body_bold())
                            .color(TEXT_DIM),
                    )
                    .truncate(),
                );
            });
            widgets::cell(ui, eta_w, h, Align::Max, |ui| {
                ui.label(
                    RichText::new(&self.progress.eta)
                        .font(theme::font_body())
                        .color(TEXT_DIM),
                );
            });
        });
        ui.add_space(2.0);
        widgets::thin_progress(ui, self.progress.fraction, 4.0);
        ui.add_space(2.0);
    }

    fn draw_footer(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            let resp = widgets::checkbox(
                ui,
                &mut self.check_updates,
                t(Key::CheckUpdates),
                theme::font_small(),
            );
            let resp = tip(resp, t(Key::TipCheckUpdates));
            if resp.changed() {
                log::info!("Update check on startup: {}", self.check_updates);
            }
            let current = self.language;
            ui.scope(|ui| {
                // Keep the footer row at the height of the checkbox.
                ui.spacing_mut().interact_size.y = 18.0;
                ui.spacing_mut().button_padding.y = 0.0;
                let combo = ComboBox::from_id_salt("language")
                    .width(72.0)
                    .selected_text(
                        RichText::new(language_label(self.language)).font(theme::font_small()),
                    )
                    .show_ui(ui, |ui| {
                        for choice in [UiLanguage::Auto, UiLanguage::Korean, UiLanguage::English] {
                            ui.selectable_value(&mut self.language, choice, language_label(choice));
                        }
                    });
                tip(combo.response, t(Key::TipLanguage));
            });
            if self.language != current {
                i18n::apply(self.language);
                log::info!("Language: {}", self.language.as_str());
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(
                    RichText::new(tf(Key::FooterCredit, &[&config::APP_VERSION]))
                        .font(theme::font_small())
                        .color(Color32::from_gray(0x80)),
                );
            });
        });
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
                ui.label(
                    RichText::new(t(Key::OutputOptionsTitle))
                        .font(theme::font_header())
                        .color(ACCENT),
                );
                ui.add_space(6.0);
                let note = |ui: &mut Ui, text: &str| {
                    ui.label(
                        RichText::new(text)
                            .font(theme::font_small())
                            .color(TEXT_DIM),
                    );
                };
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
                    note(ui, t(format_note(s.output_format)));
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
                    note(ui, t(color_note(s.color_space)));
                });
                if s.extract_embedded {
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(t(Key::CameraJpegOnly))
                            .font(theme::font_small_bold())
                            .color(ACCENT),
                    );
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    row_label(ui, t(Key::NameTemplateLabel), t(Key::TipNameTemplate));
                    let field = widgets::text_field(ui, &mut s.filename_template, "", combo_w);
                    tip(field, t(Key::TipNameTemplate));
                });
                note(ui, t(Key::NameTokens));
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
                        if widgets::gold_button(
                            ui,
                            t(Key::Close),
                            vec2(100.0, 28.0),
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

    fn draw_issues_modal(&mut self, ctx: &egui::Context) {
        let Some(issues) = &self.issues else {
            return;
        };
        let mut close = false;
        let modal = egui::Modal::new(Id::unique("batch_issues_modal"))
            .frame(modal_frame())
            .show(ctx, |ui| {
                let width = (ctx.content_rect().width() - 80.0).clamp(300.0, 540.0);
                ui.set_width(width);
                ui.label(
                    RichText::new(tf(Key::IssuesTitle, &[&issues.len()]))
                        .font(theme::font_header())
                        .color(ACCENT),
                );
                ui.add_space(6.0);
                ScrollArea::vertical()
                    .max_height((ctx.content_rect().height() - 180.0).max(120.0))
                    .auto_shrink([false, true])
                    .show_rows(ui, ISSUE_ROW_HEIGHT, issues.len(), |ui, range| {
                        for issue in &issues[range] {
                            ui.allocate_ui(vec2(width - 16.0, ISSUE_ROW_HEIGHT), |ui| {
                                ui.set_min_height(ISSUE_ROW_HEIGHT);
                                ui.add(
                                    Label::new(
                                        RichText::new(&issue.file)
                                            .font(theme::font_body_bold())
                                            .color(TEXT),
                                    )
                                    .truncate(),
                                );
                                ui.label(
                                    RichText::new(&issue.reason)
                                        .font(theme::font_small())
                                        .color(TEXT_DIM),
                                );
                            });
                        }
                    });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if widgets::gold_button(
                            ui,
                            t(Key::Close),
                            vec2(100.0, 28.0),
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
            self.issues = None;
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
                ui.label(
                    RichText::new(t(Key::SavePresetTitle))
                        .font(theme::font_header())
                        .color(ACCENT),
                );
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
                        if widgets::gold_button(
                            ui,
                            t(Key::Save),
                            vec2(80.0, 28.0),
                            theme::font_body_bold(),
                        )
                        .clicked()
                            || enter
                        {
                            actions.push(Action::SavePreset(name.clone()));
                        }
                        if widgets::outline_button(
                            ui,
                            t(Key::CancelButton),
                            vec2(80.0, 28.0),
                            theme::font_body_bold(),
                            TEXT_DIM,
                            Color32::TRANSPARENT,
                            BORDER,
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
                ui.label(
                    RichText::new(t(Key::UpdateAvailable))
                        .font(theme::font_header())
                        .color(ACCENT),
                );
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
                        if widgets::gold_button(
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
                        if widgets::outline_button(
                            ui,
                            t(Key::Later),
                            vec2(100.0, 30.0),
                            theme::font_body_bold(),
                            TEXT_DIM,
                            Color32::TRANSPARENT,
                            PANEL,
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
        let ctx = ui.ctx().clone();
        if !self.title_bar_themed {
            // Dark native title bar: winit theme (macOS/Linux) plus a
            // best-effort DWM call on Windows (see `titlebar`).
            ctx.send_viewport_cmd(ViewportCommand::SetTheme(egui::SystemTheme::Dark));
            crate::titlebar::apply(frame);
            self.title_bar_themed = true;
        }
        self.poll_job(&ctx, frame);
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
        egui::CentralPanel::default()
            .frame(
                Frame::new()
                    .fill(BG)
                    .inner_margin(Margin::symmetric(0, PAD_MEDIUM as i8)),
            )
            .show(ui, |ui| {
                let size = ui.available_size();
                if wide {
                    // Two columns: settings left (scrolls), preview right.
                    ui.horizontal_top(|ui| {
                        ui.allocate_ui_with_layout(
                            vec2(SETTINGS_COLUMN, size.y),
                            Layout::top_down(Align::Min),
                            |ui| {
                                ui.set_max_width(SETTINGS_COLUMN);
                                self.draw_scrolling_column(ui, &mut actions, false);
                            },
                        );
                        let right = (size.x - SETTINGS_COLUMN - PAD_LARGE).max(240.0);
                        ui.allocate_ui_with_layout(
                            vec2(right, size.y),
                            Layout::top_down(Align::Min),
                            |ui| {
                                ui.set_min_width(right);
                                let embedded = self.settings.extract_embedded;
                                panel(ui, |ui| {
                                    let image_h = (size.y - 64.0).max(120.0);
                                    self.preview.draw(ui, image_h, embedded);
                                });
                            },
                        );
                    });
                } else {
                    self.draw_scrolling_column(ui, &mut actions, true);
                }
            });
        self.draw_preset_modal(&ctx, &mut actions);
        self.draw_update_modal(&ctx, &mut actions);
        self.draw_output_options_modal(&ctx);
        self.draw_issues_modal(&ctx);
        self.draw_overlays(&ctx);

        for action in actions {
            self.run_action(action, &ctx, frame);
        }
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

/// Python panel: `#2b2b2b` frame with square corners.
fn panel(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    Frame::new()
        .fill(PANEL)
        .inner_margin(Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui);
        });
}

fn modal_frame() -> Frame {
    Frame::new()
        .fill(PANEL)
        .stroke(Stroke::new(1.0, BORDER))
        .inner_margin(Margin::same(18))
}

/// Row whose control spans the control and value columns (Python `columnspan=2`).
fn wide_row(
    ui: &mut Ui,
    cols: Columns,
    label: impl FnOnce(&mut Ui),
    control: impl FnOnce(&mut Ui, f32),
) {
    ui.horizontal(|ui| {
        widgets::cell(ui, cols.label, cols.row_height, Align::Min, label);
        let w = (ui.available_width() - 4.0).max(80.0);
        widgets::cell(ui, w, cols.row_height, Align::Min, |ui| control(ui, w));
    });
}

/// Footer language combo text.
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
mod layout_tests {
    use super::*;
    use crate::fonts;
    use crate::i18n::Lang;
    use eframe::egui::{FontId, RawInput, Rect, Vec2};

    const WIDTH: f32 = 600.0;
    const HEIGHT: f32 = 820.0;

    fn input() -> RawInput {
        RawInput {
            screen_rect: Some(Rect::from_min_size(
                egui::Pos2::ZERO,
                Vec2::new(WIDTH, HEIGHT),
            )),
            ..Default::default()
        }
    }

    /// One frame with no renderer: texture deltas are discarded on purpose.
    fn run_frame(ctx: &egui::Context, ui: impl FnMut(&mut Ui)) {
        let mut out = ctx.run_ui(input(), ui);
        out.textures_delta.clear();
    }

    fn context() -> egui::Context {
        let ctx = egui::Context::default();
        fonts::install(&ctx);
        theme::apply(&ctx);
        // Fonts become available after the first frame.
        run_frame(&ctx, |_| {});
        ctx
    }

    fn text_width(ctx: &egui::Context, text: &str, font: FontId) -> f32 {
        ctx.fonts_mut(|f| f.layout_no_wrap(text.to_owned(), font, TEXT).size().x)
    }

    /// Every label fits its fixed cell, in both languages.
    fn check_cells(lang: Lang) {
        i18n::set_language(lang);
        let ctx = context();
        let body = [
            Key::ResizeLabel,
            Key::QualityLabel,
            Key::WhiteBalanceLabel,
            Key::PictureStyleLabel,
            Key::ExposureLabel,
            Key::BrightnessLabel,
            Key::RgbBalanceLabel,
            Key::LevelsLabel,
        ];
        let settings_label = Columns::SETTINGS.label;
        for key in body {
            let w = text_width(&ctx, t(key), theme::font_body());
            assert!(
                w <= settings_label,
                "{key:?} {lang:?}: {w} > {settings_label}"
            );
        }
        let w = text_width(&ctx, t(Key::ExifPreservation), theme::font_header());
        assert!(w <= settings_label, "ExifPreservation {lang:?}: {w}");
        let path_label = 186.0;
        for key in [Key::SourceFolder, Key::OutputFolderOptional] {
            let w = text_width(&ctx, t(key), theme::font_body_bold());
            assert!(w <= path_label, "{key:?} {lang:?}: {w}");
        }
        let w = text_width(&ctx, t(Key::IfOutputExists), theme::font_body());
        assert!(w <= path_label, "IfOutputExists {lang:?}: {w}");
        let buttons = [
            (Key::Browse, 80.0, theme::font_body_bold()),
            (Key::Save, 64.0, theme::font_body_bold()),
            (Key::Delete, 64.0, theme::font_body_bold()),
            (Key::Reset, 60.0, theme::font_small_bold()),
            (Key::ShowPreview, 112.0, theme::font_small_bold()),
            (Key::HidePreview, 112.0, theme::font_small_bold()),
            (Key::ShowBefore, 120.0, theme::font_small_bold()),
            (Key::ShowAfter, 120.0, theme::font_small_bold()),
            (Key::CameraJpegTag, 92.0, theme::font_small_bold()),
            (Key::Clear, 48.0, theme::font_small_bold()),
            (Key::Download, 120.0, theme::font_body_bold()),
            (Key::Later, 100.0, theme::font_body_bold()),
            (Key::Close, 100.0, theme::font_body_bold()),
        ];
        for (key, width, font) in buttons {
            let w = text_width(&ctx, t(key), font);
            assert!(w <= width - 4.0, "{key:?} {lang:?}: {w} > {width}");
        }
        // Output options: the widest summary fits its button.
        let button = WIDTH - 2.0 * PAD_LARGE - 20.0 - 186.0 - CONFLICT_COMBO_WIDTH - 30.0;
        for text in [
            tf(Key::OutputButton, &[&"TIFF 16-bit  ·  AdobeRGB"]),
            tf(Key::OutputButton, &[&t(Key::CameraJpegOutput)]),
        ] {
            let w = text_width(&ctx, &text, theme::font_small_bold());
            assert!(w <= button, "output button {lang:?}: {w} > {button}");
        }
        for (key, font) in [
            (Key::FormatLabel, theme::font_body()),
            (Key::ColorSpaceLabel, theme::font_body()),
            (Key::NameTemplateLabel, theme::font_body()),
        ] {
            let w = text_width(&ctx, t(key), font);
            assert!(w <= MODAL_LABEL_WIDTH, "{key:?} {lang:?}: {w}");
        }
        for (_, label) in CONFLICT_CHOICES {
            let w = text_width(&ctx, t(label), theme::font_body()) + 30.0;
            assert!(w <= CONFLICT_COMBO_WIDTH, "{label:?} {lang:?}: {w}");
        }
        // Footer row: checkbox, language combo, credit.
        let footer = text_width(&ctx, t(Key::CheckUpdates), theme::font_small())
            + text_width(
                &ctx,
                &tf(Key::FooterCredit, &[&config::APP_VERSION]),
                theme::font_small(),
            )
            + 26.0 // checkbox box and gaps
            + 72.0 + 30.0; // combo
        let row = WIDTH - 2.0 * PAD_LARGE - 20.0;
        assert!(footer <= row, "footer {lang:?}: {footer} > {row}");
        i18n::set_language(Lang::English);
    }

    fn content_height(lang: Lang) -> f32 {
        i18n::set_language(lang);
        let ctx = context();
        let dir = tempfile::tempdir().unwrap();
        let paths = AppPaths::with_root(dir.path().to_path_buf());
        let settings = AppSettings {
            check_updates: false,
            ..Default::default()
        };
        let presets = PresetStore::default();
        let mut app = CcooNverterApp::new(ctx.clone(), paths, settings, presets);
        let mut height = 0.0f32;
        // Two frames: text layout settles after the first.
        for _ in 0..2 {
            run_frame(&ctx, |ui| {
                Frame::new()
                    .inner_margin(Margin::symmetric(PAD_LARGE as i8, PAD_MEDIUM as i8))
                    .show(ui, |ui| {
                        ui.set_width(WIDTH - 2.0 * PAD_LARGE);
                        let mut actions = Vec::new();
                        app.draw(ui, &mut actions, true);
                        height = ui.min_rect().height();
                    });
            });
        }
        i18n::set_language(Lang::English);
        height
    }

    fn app_for_test(ctx: &egui::Context, dir: &Path) -> CcooNverterApp {
        let settings = AppSettings {
            check_updates: false,
            ..Default::default()
        };
        CcooNverterApp::new(
            ctx.clone(),
            AppPaths::with_root(dir.to_path_buf()),
            settings,
            PresetStore::default(),
        )
    }

    #[test]
    fn output_and_issue_modals_fit_the_window() {
        for lang in [Lang::English, Lang::Korean] {
            i18n::set_language(lang);
            let ctx = context();
            let dir = tempfile::tempdir().unwrap();
            let mut app = app_for_test(&ctx, dir.path());
            app.output_options_open = true;
            app.settings.color_space = ColorSpace::AdobeRgb;
            app.issues = Some(vec![BatchIssue {
                file: "DSC_0001.NEF".into(),
                reason: t(Key::FailUnsupportedCompression).to_string(),
            }]);
            for _ in 0..3 {
                run_frame(&ctx, |_| {
                    app.draw_output_options_modal(&ctx);
                });
            }
            let area = ctx.memory(|m| m.area_rect(Id::unique("output_options_modal")));
            let rect = area.expect("modal area");
            assert!(rect.height() <= HEIGHT, "{lang:?}: {}", rect.height());
            assert!(rect.width() <= WIDTH, "{lang:?}: {}", rect.width());
            app.settings.extract_embedded = true;
            for _ in 0..3 {
                run_frame(&ctx, |_| {
                    app.draw_output_options_modal(&ctx);
                    app.draw_issues_modal(&ctx);
                });
            }
            i18n::set_language(Lang::English);
        }
    }

    /// The update check is on for a new user; it does not depend on the
    /// settings the layout tests use.
    #[test]
    fn update_check_is_on_by_default_for_a_new_install() {
        let dir = tempfile::tempdir().unwrap();
        let settings = AppSettings::load(&dir.path().join("settings.json"));
        assert!(settings.check_updates);
        let ctx = context();
        let app = CcooNverterApp::new(
            ctx,
            AppPaths::with_root(dir.path().to_path_buf()),
            settings,
            PresetStore::default(),
        );
        assert!(app.check_updates);
        assert!(app.update_rx.is_some(), "the background check was started");
    }

    #[test]
    fn labels_fit_their_cells_in_both_languages() {
        check_cells(Lang::English);
        check_cells(Lang::Korean);
    }

    #[test]
    fn default_window_fits_in_both_languages() {
        for lang in [Lang::English, Lang::Korean] {
            let h = content_height(lang);
            let limit = HEIGHT - 2.0 * PAD_MEDIUM;
            assert!(h <= limit, "{lang:?}: content {h} > {limit}");
            println!("{lang:?}: content height {h} of {limit}");
        }
    }
}
