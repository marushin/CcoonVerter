//! The main window (port of Python `main.App` + `ui_components`).

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::Duration;

use ccoonverter_core::config;
use ccoonverter_core::paths::AppPaths;
use ccoonverter_core::presets::PresetStore;
use ccoonverter_core::update::{self, ReleaseInfo};
use ccoonverter_core::{AppSettings, PictureStyle, WhiteBalance};
use eframe::egui::{
    self, vec2, Align, Color32, ComboBox, Frame, Id, Key, Label, Layout, Margin, RichText,
    ScrollArea, Stroke, Ui, ViewportCommand,
};

use crate::controller::{
    self, completion_message, conflict_label, prepare_batch, BatchOutcome, ProgressView,
    RunningBatch, UiSettings, CONFLICT_CHOICES,
};
use crate::dialogs;
use crate::theme::{
    self, ACCENT, BG, BORDER, DANGER, PAD_LARGE, PAD_MEDIUM, PANEL, TEXT, TEXT_DIM,
};
use crate::widgets::{self, Columns};

const SELECT_PRESET: &str = "Select Preset...";

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
}

impl CcooNverterApp {
    pub fn new(paths: AppPaths, app_settings: AppSettings, presets: PresetStore) -> Self {
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
                    "Error",
                    &format!("Could not save presets:\n{e}"),
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
        let msg = completion_message(&outcome, output_dir, &self.paths.log_file());
        if msg.offer_open {
            let body = format!("{}\n\nOpen output folder?", msg.body);
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
            if dialogs::confirm(
                Some(frame),
                "Quit",
                "A conversion is still running.\nCancel it and quit?",
            ) {
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
                if let Some(p) = pick_folder(&self.source) {
                    log::info!("Selected folder: {}", p.display());
                    self.source = p.to_string_lossy().into_owned();
                }
            }
            Action::BrowseOutput => {
                if let Some(p) = pick_folder(&self.output) {
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
                            "Cancel",
                            "Are you sure you want to cancel the conversion?",
                        )
                    {
                        if let Some(job) = &self.job {
                            job.cancel();
                        }
                        log::info!("Cancellation requested by user");
                        self.progress.reset();
                        self.progress.status = "Cancelling...".into();
                    }
                }
            }
            Action::ResetAdjustments => {
                self.settings.reset_raw_adjustments();
                log::info!("RAW adjustments reset to defaults");
                dialogs::info(
                    Some(frame),
                    "Reset",
                    "RAW adjustments have been reset to defaults.",
                );
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
                    dialogs::info(Some(frame), "Success", &format!("Preset '{name}' saved."));
                }
            }
            Action::DeletePreset => {
                let Some(name) = self.selected_preset.clone() else {
                    return;
                };
                if dialogs::confirm(Some(frame), "Confirm", &format!("Delete preset '{name}'?"))
                    && self.presets.remove(&name)
                {
                    self.selected_preset = None;
                    if self.save_presets(frame) {
                        log::info!("Deleted preset '{name}'");
                        dialogs::info(Some(frame), "Success", &format!("Preset '{name}' deleted."));
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
        let batch = match prepare_batch(&self.source, &self.output) {
            Ok(b) => b,
            Err(e) => {
                let (title, msg) = e.dialog();
                let kind = if title == "Info" {
                    controller::MessageKind::Info
                } else {
                    controller::MessageKind::Error
                };
                dialogs::show(Some(frame), kind, title, &msg);
                return;
            }
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
                    "Error",
                    &format!("Could not start conversion:\n{e}"),
                );
            }
        }
    }

    // ------------------------------------------------------------------
    // Drawing
    // ------------------------------------------------------------------

    fn draw(&mut self, ui: &mut Ui, actions: &mut Vec<Action>) {
        panel(ui, |ui| self.draw_paths(ui, actions));
        ui.add_space(PAD_MEDIUM);
        panel(ui, |ui| self.draw_settings(ui, actions));
        ui.add_space(PAD_MEDIUM);
        self.draw_actions(ui, actions);
        ui.add_space(PAD_MEDIUM);
        panel(ui, |ui| self.draw_progress(ui));
        ui.add_space(PAD_MEDIUM);
        self.draw_footer(ui);
    }

    fn draw_paths(&mut self, ui: &mut Ui, actions: &mut Vec<Action>) {
        ui.label(
            RichText::new("DIRECTORY PATHS")
                .font(theme::font_body_bold())
                .color(ACCENT),
        );
        let cols = Columns {
            label: 186.0,
            value: 84.0,
            row_height: 28.0,
        };
        let folder_row = |ui: &mut Ui, title: &str, text: &mut String, hint: &str| -> bool {
            let mut browse = false;
            cols.row(
                ui,
                |ui| {
                    ui.label(
                        RichText::new(title)
                            .font(theme::font_body_bold())
                            .color(TEXT_DIM),
                    );
                },
                |ui, w| {
                    widgets::text_field(ui, text, hint, w);
                },
                |ui, _| {
                    browse = widgets::gold_button(
                        ui,
                        "BROWSE",
                        vec2(80.0, 28.0),
                        theme::font_body_bold(),
                    )
                    .clicked();
                },
            );
            browse
        };
        if folder_row(ui, "SOURCE FOLDER", &mut self.source, "") {
            actions.push(Action::BrowseSource);
        }
        if folder_row(
            ui,
            "OUTPUT FOLDER (Optional)",
            &mut self.output,
            "Leave empty for default subfolder",
        ) {
            actions.push(Action::BrowseOutput);
        }
        let on_conflict = &mut self.settings.on_conflict;
        cols.row(
            ui,
            |ui| {
                ui.label(
                    RichText::new("If output exists:")
                        .font(theme::font_body())
                        .color(TEXT_DIM),
                );
            },
            |ui, _w| {
                ComboBox::from_id_salt("on_conflict")
                    .width(150.0)
                    .selected_text(conflict_label(*on_conflict))
                    .show_ui(ui, |ui| {
                        for (p, label) in CONFLICT_CHOICES {
                            ui.selectable_value(on_conflict, p, label);
                        }
                    });
            },
            |_ui, _| {},
        );
    }

    fn draw_settings(&mut self, ui: &mut Ui, actions: &mut Vec<Action>) {
        let cols = Columns::SETTINGS;
        let s = &mut self.settings;

        // SETTINGS header + embedded checkbox (right-aligned).
        ui.horizontal(|ui| {
            widgets::header(ui, "SETTINGS");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                widgets::checkbox(
                    ui,
                    &mut s.extract_embedded,
                    "Extract Embedded JPEG (Fast)",
                    theme::font_body_bold(),
                );
            });
        });

        // EXIF PRESERVATION header + checkbox, note below.
        ui.horizontal(|ui| {
            widgets::cell(ui, cols.label, cols.row_height, Align::Min, |ui| {
                widgets::header(ui, "EXIF PRESERVATION");
            });
            widgets::checkbox(ui, &mut s.preserve_exif, "", theme::font_body_bold());
        });
        let note_color = if s.preserve_exif { TEXT } else { TEXT_DIM };
        ui.label(
            RichText::new("Not all metadata may be copied identically")
                .font(theme::font_small())
                .color(note_color),
        );

        // Resize.
        cols.row(
            ui,
            |ui| {
                widgets::dim_label(ui, "Resize (Long Edge px):");
            },
            |ui, w| {
                widgets::text_field(ui, &mut s.resize_text, "0 for original", w);
            },
            |ui, _| {
                ui.label(
                    RichText::new("(0 = Original)")
                        .font(theme::font_small())
                        .color(TEXT_DIM),
                );
            },
        );

        // Quality.
        cols.row(
            ui,
            |ui| {
                widgets::dim_label(ui, "JPG Quality (1-100):");
            },
            |ui, w| {
                widgets::color_slider(
                    ui,
                    &mut s.quality,
                    f32::from(config::MIN_JPG_QUALITY)..=f32::from(config::MAX_JPG_QUALITY),
                    Some(1.0),
                    ACCENT,
                    theme::ACCENT_HOVER,
                    w,
                );
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
                widgets::header(ui, "RAW ADJUSTMENTS");
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.add_space(cols.value - 60.0 - 2.0);
                    if widgets::outline_button(
                        ui,
                        "RESET",
                        vec2(60.0, 20.0),
                        theme::font_small_bold(),
                        ACCENT,
                        Color32::TRANSPARENT,
                        BORDER,
                    )
                    .clicked()
                    {
                        actions.push(Action::ResetAdjustments);
                    }
                });
            });

            // Labels stay dim; the disabled Ui fades them further.
            let label = |ui: &mut Ui, t: &str| {
                ui.label(RichText::new(t).font(theme::font_body()).color(TEXT_DIM));
            };

            // White balance (spans control + value columns like Python).
            wide_row(
                ui,
                cols,
                |ui| label(ui, "White Balance:"),
                |ui, w| {
                    ComboBox::from_id_salt("white_balance")
                        .width(w)
                        .selected_text(s.white_balance.as_str())
                        .show_ui(ui, |ui| {
                            for wb in WhiteBalance::ALL {
                                ui.selectable_value(&mut s.white_balance, wb, wb.as_str());
                            }
                        });
                },
            );

            wide_row(
                ui,
                cols,
                |ui| label(ui, "Picture Style:"),
                |ui, w| {
                    ComboBox::from_id_salt("picture_style")
                        .width(w)
                        .selected_text(s.style.as_str())
                        .show_ui(ui, |ui| {
                            for st in PictureStyle::ALL {
                                ui.selectable_value(&mut s.style, st, st.as_str());
                            }
                        });
                },
            );

            cols.row(
                ui,
                |ui| label(ui, "Brightness (0.0-2.0):"),
                |ui, w| {
                    widgets::color_slider(
                        ui,
                        &mut s.brightness,
                        config::MIN_BRIGHTNESS..=config::MAX_BRIGHTNESS,
                        Some(0.01),
                        ACCENT,
                        theme::ACCENT_HOVER,
                        w,
                    );
                    s.brightness
                },
                |ui, b| {
                    widgets::value_label(ui, &format!("{b:.2}"));
                },
            );

            let rgb = [
                (
                    "RGB Channel Balance:",
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
                    |ui| label(ui, title),
                    |ui, w| {
                        widgets::color_slider(
                            ui,
                            v,
                            config::MIN_RGB_VALUE..=config::MAX_RGB_VALUE,
                            Some(0.01),
                            color,
                            hover,
                            w,
                        );
                        *v
                    },
                    |ui, v| {
                        widgets::value_label(ui, &format!("{ch}: {v:.2}"));
                    },
                );
            }

            cols.row(
                ui,
                |ui| label(ui, "Levels (Black/White):"),
                |ui, w| {
                    widgets::color_slider(
                        ui,
                        &mut s.black_point,
                        f32::from(config::MIN_BLACK_POINT)..=f32::from(config::MAX_BLACK_POINT),
                        Some(1.0),
                        TEXT_DIM,
                        TEXT,
                        w,
                    );
                    s.black_point
                },
                |ui, bp| {
                    widgets::value_label(ui, &format!("BP: {}", bp.round() as i32));
                },
            );
            cols.row(
                ui,
                |ui| label(ui, ""),
                |ui, w| {
                    widgets::color_slider(
                        ui,
                        &mut s.white_point,
                        f32::from(config::MIN_WHITE_POINT)..=f32::from(config::MAX_WHITE_POINT),
                        Some(1.0),
                        theme::SLIDER_WHITE,
                        theme::SLIDER_WHITE_HOVER,
                        w,
                    );
                    s.white_point
                },
                |ui, wp| {
                    widgets::value_label(ui, &format!("WP: {}", wp.round() as i32));
                },
            );

            // PRESETS.
            ui.add_space(PAD_MEDIUM);
            widgets::header(ui, "PRESETS");
            ui.horizontal(|ui| {
                let btn = vec2(64.0, 28.0);
                let gap = ui.spacing().item_spacing.x;
                let combo_w = (ui.available_width() - 2.0 * btn.x - 2.0 * gap - 8.0).max(120.0);
                let shown = selected_preset.as_deref().unwrap_or(SELECT_PRESET);
                ComboBox::from_id_salt("presets")
                    .width(combo_w)
                    .selected_text(shown)
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(selected_preset.is_none(), SELECT_PRESET)
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
                if widgets::gold_button(ui, "SAVE", btn, theme::font_body_bold()).clicked() {
                    actions.push(Action::OpenSavePreset);
                }
                if widgets::outline_button(
                    ui,
                    "DELETE",
                    btn,
                    theme::font_body_bold(),
                    DANGER,
                    PANEL,
                    BG,
                )
                .clicked()
                {
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
                "Converting..."
            } else {
                "Let's Get Cooking!"
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
                        "CANCEL",
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
                "Check for updates on startup",
                theme::font_small(),
            );
            if resp.changed() {
                log::info!("Update check on startup: {}", self.check_updates);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(
                    RichText::new(format!(
                        "v{}  ·  MIT  ·  RAW decoding: rawler (LGPL-2.1)",
                        config::APP_VERSION
                    ))
                    .font(theme::font_small())
                    .color(Color32::from_gray(0x80)),
                );
            });
        });
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
                    RichText::new("Save Preset")
                        .font(theme::font_header())
                        .color(ACCENT),
                );
                ui.add_space(4.0);
                widgets::dim_label(ui, "Enter preset name:");
                let resp = widgets::text_field(ui, name, "", 300.0);
                if !resp.has_focus() && !resp.lost_focus() {
                    resp.request_focus();
                }
                let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if widgets::gold_button(
                            ui,
                            "SAVE",
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
                            "CANCEL",
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
                    RichText::new("A New Version is Available!")
                        .font(theme::font_header())
                        .color(ACCENT),
                );
                ui.add_space(4.0);
                ui.label(
                    RichText::new(format!(
                        "Current: v{}  →  Latest: v{}",
                        config::APP_VERSION,
                        info.version
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
                            "DOWNLOAD",
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
                            "LATER",
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
            // Dark native title bar through winit (no direct DWM calls).
            ctx.send_viewport_cmd(ViewportCommand::SetTheme(egui::SystemTheme::Dark));
            self.title_bar_themed = true;
        }
        self.poll_job(&ctx, frame);
        self.poll_update(&ctx);
        self.handle_close_request(&ctx, frame);

        let mut actions = Vec::new();
        egui::CentralPanel::default()
            .frame(
                Frame::new()
                    .fill(BG)
                    .inner_margin(Margin::symmetric(0, PAD_MEDIUM as i8)),
            )
            .show(ui, |ui| {
                ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        Frame::new()
                            .inner_margin(Margin::symmetric(PAD_LARGE as i8, 0))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                self.draw(ui, &mut actions);
                            });
                    });
            });
        self.draw_preset_modal(&ctx, &mut actions);
        self.draw_update_modal(&ctx, &mut actions);

        for action in actions {
            self.run_action(action, &ctx, frame);
        }
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        BG.to_normalized_gamma_f32()
    }

    #[cfg(feature = "glow")]
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.shutdown();
    }

    #[cfg(not(feature = "glow"))]
    fn on_exit(&mut self) {
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

/// Native folder picker starting at the current text (if it is a folder).
fn pick_folder(current: &str) -> Option<PathBuf> {
    let mut dialog = rfd::FileDialog::new();
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
