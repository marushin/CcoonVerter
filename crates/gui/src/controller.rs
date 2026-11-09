//! Conversion orchestration used by the GUI, kept free of egui so it can be
//! unit-tested without a window:
//!
//! UI values ([`UiSettings`]) → [`ConvertSettings`] → [`prepare_batch`]
//! (validate source, resolve output, scan) → [`RunningBatch`] (one worker
//! thread calling `job::run_batch` with an mpsc progress channel and an
//! `Arc<AtomicBool>` cancel flag) → [`BatchSummary`] → [`completion_message`].

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use ccoonverter_core::config;
use ccoonverter_core::job::{self, BatchSummary, FileStatus, Progress};
use ccoonverter_core::{ConflictPolicy, ConvertSettings, FileTask, PictureStyle, WhiteBalance};

// ---------------------------------------------------------------------------
// UI settings
// ---------------------------------------------------------------------------

/// The values shown by the SETTINGS panel. Everything except the resize
/// text box comes from a slider/combo/checkbox and is therefore always in range.
#[derive(Debug, Clone, PartialEq)]
pub struct UiSettings {
    /// Raw text of the "Resize (Long Edge px)" entry.
    pub resize_text: String,
    pub quality: f32,
    pub brightness: f32,
    pub white_balance: WhiteBalance,
    pub style: PictureStyle,
    pub rgb: [f32; 3],
    pub black_point: f32,
    pub white_point: f32,
    pub extract_embedded: bool,
    pub preserve_exif: bool,
    pub on_conflict: ConflictPolicy,
}

impl Default for UiSettings {
    fn default() -> Self {
        Self::from_convert(&ConvertSettings::default())
    }
}

/// Parse the resize entry like Python's `get_settings`: anything that is
/// not a non-negative integer becomes 0 (with a warning in the log).
pub fn parse_resize(text: &str) -> u32 {
    let t = text.trim();
    match t.parse::<i64>() {
        Ok(v) if v >= 0 => {
            if v > i64::from(config::MAX_RESIZE) {
                log::warn!(
                    "Resize value {v} above {}, using {}",
                    config::MAX_RESIZE,
                    config::MAX_RESIZE
                );
                config::MAX_RESIZE
            } else {
                v as u32
            }
        }
        Ok(v) => {
            log::warn!("Invalid resize value {v}, using 0");
            config::DEFAULT_RESIZE
        }
        Err(_) => {
            if !t.is_empty() {
                log::warn!("Invalid resize input '{t}', using 0");
            }
            config::DEFAULT_RESIZE
        }
    }
}

impl UiSettings {
    pub fn from_convert(s: &ConvertSettings) -> Self {
        let s = s.sanitized();
        Self {
            resize_text: s.resize.to_string(),
            quality: f32::from(s.quality),
            brightness: s.brightness,
            white_balance: s.white_balance,
            style: s.style,
            rgb: s.rgb(),
            black_point: f32::from(s.black_point),
            white_point: f32::from(s.white_point),
            extract_embedded: s.extract_embedded,
            preserve_exif: s.preserve_exif,
            on_conflict: s.on_conflict,
        }
    }

    /// Build the core settings (Python `get_settings`).
    pub fn to_convert(&self) -> ConvertSettings {
        let u8_of = |v: f32, lo: u8, hi: u8| v.round().clamp(f32::from(lo), f32::from(hi)) as u8;
        ConvertSettings {
            resize: parse_resize(&self.resize_text),
            quality: u8_of(
                self.quality,
                config::MIN_JPG_QUALITY,
                config::MAX_JPG_QUALITY,
            ),
            brightness: self
                .brightness
                .clamp(config::MIN_BRIGHTNESS, config::MAX_BRIGHTNESS),
            white_balance: self.white_balance,
            style: self.style,
            rgb_red: self.rgb[0].clamp(config::MIN_RGB_VALUE, config::MAX_RGB_VALUE),
            rgb_green: self.rgb[1].clamp(config::MIN_RGB_VALUE, config::MAX_RGB_VALUE),
            rgb_blue: self.rgb[2].clamp(config::MIN_RGB_VALUE, config::MAX_RGB_VALUE),
            black_point: u8_of(
                self.black_point,
                config::MIN_BLACK_POINT,
                config::MAX_BLACK_POINT,
            ),
            white_point: u8_of(
                self.white_point,
                config::MIN_WHITE_POINT,
                config::MAX_WHITE_POINT,
            ),
            extract_embedded: self.extract_embedded,
            preserve_exif: self.preserve_exif,
            on_conflict: self.on_conflict,
        }
    }

    /// RESET button: RAW adjustments back to defaults (Python `reset_adjustments`).
    /// Resize, quality, EXIF, embedded mode and conflict policy are kept.
    pub fn reset_raw_adjustments(&mut self) {
        let d = ConvertSettings::default();
        self.white_balance = d.white_balance;
        self.style = d.style;
        self.brightness = d.brightness;
        self.rgb = d.rgb();
        self.black_point = f32::from(d.black_point);
        self.white_point = f32::from(d.white_point);
    }

    /// Apply a preset like Python `_apply_settings_to_ui`: every saved value is
    /// applied, except the output conflict policy which is an app preference.
    pub fn apply_preset(&mut self, preset: &ConvertSettings) {
        let keep = self.on_conflict;
        *self = Self::from_convert(preset);
        self.on_conflict = keep;
    }
}

/// Labels of the conflict policy combo, in display order.
pub const CONFLICT_CHOICES: [(ConflictPolicy, &str); 3] = [
    (ConflictPolicy::Suffix, "Add suffix"),
    (ConflictPolicy::Skip, "Skip"),
    (ConflictPolicy::Overwrite, "Overwrite"),
];

pub fn conflict_label(p: ConflictPolicy) -> &'static str {
    CONFLICT_CHOICES
        .iter()
        .find(|(c, _)| *c == p)
        .map(|(_, l)| *l)
        .unwrap_or("Add suffix")
}

// ---------------------------------------------------------------------------
// Preparing a batch
// ---------------------------------------------------------------------------

/// Output folder rule from Python `run_conversion`: the custom folder if it
/// exists, otherwise `<source>/Converted_JPG`.
pub fn resolve_output_folder(source: &Path, custom: &str) -> PathBuf {
    let custom = custom.trim();
    if !custom.is_empty() {
        let p = PathBuf::from(custom);
        if p.is_dir() {
            log::info!("Using custom output folder: {}", p.display());
            return p;
        }
        log::warn!(
            "Output folder '{}' does not exist, using the default sub-folder",
            p.display()
        );
    }
    let out = source.join(config::DEFAULT_OUTPUT_FOLDER_NAME);
    log::info!("Using default output folder: {}", out.display());
    out
}

/// Why a batch could not start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrepareError {
    /// Empty or missing source folder (Python: "Please select a valid source folder.").
    InvalidSource,
    /// No RAW files found (Python info box).
    NoFiles,
    /// File system error while scanning.
    Scan(String),
}

impl PrepareError {
    /// (title, message) for the native message box, matching Python's texts.
    pub fn dialog(&self) -> (&'static str, String) {
        match self {
            PrepareError::InvalidSource => ("Error", "Please select a valid source folder.".into()),
            PrepareError::NoFiles => ("Info", "No RAW files found in the selected folder.".into()),
            PrepareError::Scan(e) => ("Error", format!("File system error: {e}")),
        }
    }
}

/// Scanned and planned batch, ready to run.
#[derive(Debug, Clone)]
pub struct PreparedBatch {
    pub output_dir: PathBuf,
    pub tasks: Vec<FileTask>,
}

/// Validate the source folder, resolve the output folder and scan for RAW files.
pub fn prepare_batch(source_text: &str, output_text: &str) -> Result<PreparedBatch, PrepareError> {
    let source_text = source_text.trim();
    if source_text.is_empty() {
        return Err(PrepareError::InvalidSource);
    }
    let source = PathBuf::from(source_text);
    if !source.is_dir() {
        log::warn!("Invalid source folder: {}", source.display());
        return Err(PrepareError::InvalidSource);
    }
    let output_dir = resolve_output_folder(&source, output_text);
    log::info!("Scanning folder: {}", source.display());
    let (tasks, output_dir) = job::prepare(&source, Some(&output_dir)).map_err(|e| {
        log::error!("Scan failed: {e}");
        PrepareError::Scan(e.to_string())
    })?;
    if tasks.is_empty() {
        log::warn!("No RAW files found");
        return Err(PrepareError::NoFiles);
    }
    log::info!("Found {} RAW files to convert", tasks.len());
    Ok(PreparedBatch { output_dir, tasks })
}

// ---------------------------------------------------------------------------
// Running a batch
// ---------------------------------------------------------------------------

/// How a finished batch ended.
#[derive(Debug, Clone)]
pub enum BatchOutcome {
    Finished(BatchSummary),
    /// `run_batch` returned an error (e.g. invalid settings) or the worker panicked.
    Error(String),
}

/// A batch running on one worker thread.
pub struct RunningBatch {
    pub output_dir: PathBuf,
    pub total: usize,
    cancel: Arc<AtomicBool>,
    progress_rx: Receiver<Progress>,
    handle: Option<JoinHandle<Result<BatchSummary, String>>>,
}

impl RunningBatch {
    /// Spawn the worker thread. `threads = None` uses the core default.
    pub fn spawn(
        batch: PreparedBatch,
        settings: ConvertSettings,
        threads: Option<usize>,
    ) -> std::io::Result<Self> {
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        let total = batch.tasks.len();
        let output_dir = batch.output_dir.clone();
        let worker_cancel = Arc::clone(&cancel);
        log::info!("Starting conversion with settings: {settings:?}");
        let handle = std::thread::Builder::new()
            .name("ccoon-batch".into())
            .spawn(move || {
                job::run_batch(&batch.tasks, &settings, threads, Some(tx), &worker_cancel)
                    .map_err(|e| e.to_string())
            })?;
        Ok(Self {
            output_dir,
            total,
            cancel,
            progress_rx: rx,
            handle: Some(handle),
        })
    }

    /// Request cancellation (files in progress stop at the next stage).
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn is_cancel_requested(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    /// Drain all progress messages received so far (non-blocking).
    pub fn drain_progress(&self) -> Vec<Progress> {
        let mut out = Vec::new();
        while let Ok(p) = self.progress_rx.try_recv() {
            out.push(p);
        }
        out
    }

    /// `Some` once the worker thread has finished (non-blocking).
    pub fn try_finish(&mut self) -> Option<BatchOutcome> {
        if self.handle.as_ref().is_some_and(|h| h.is_finished()) {
            return self.join();
        }
        None
    }

    /// Block until the worker finishes (tests; the GUI polls [`Self::try_finish`]).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn wait(mut self) -> BatchOutcome {
        self.join()
            .unwrap_or_else(|| BatchOutcome::Error("batch already finished".into()))
    }

    /// Wait at most `timeout` (used on window close after cancelling).
    pub fn wait_timeout(&mut self, timeout: Duration) -> Option<BatchOutcome> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            self.handle.as_ref()?;
            if let Some(o) = self.try_finish() {
                return Some(o);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        None
    }

    fn join(&mut self) -> Option<BatchOutcome> {
        let h = self.handle.take()?;
        Some(match h.join() {
            Ok(Ok(summary)) => BatchOutcome::Finished(summary),
            Ok(Err(e)) => BatchOutcome::Error(e),
            Err(_) => BatchOutcome::Error("conversion worker panicked".into()),
        })
    }
}

// ---------------------------------------------------------------------------
// Progress panel state and texts
// ---------------------------------------------------------------------------

pub const READY: &str = "READY";

/// Texts of the progress panel.
#[derive(Debug, Clone, PartialEq)]
pub struct ProgressView {
    pub status: String,
    pub eta: String,
    pub fraction: f32,
}

impl Default for ProgressView {
    fn default() -> Self {
        Self {
            status: READY.into(),
            eta: format_eta(None),
            fraction: 0.0,
        }
    }
}

impl ProgressView {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Python `update_progress`: "Processing (i/n): <file> (p%)" and "ETA: H:MM:SS".
    pub fn apply(&mut self, p: &Progress) {
        if p.total == 0 {
            self.reset();
            return;
        }
        let fraction = p.done as f32 / p.total as f32;
        let pct = (fraction * 100.0) as u32;
        let name = progress_file_name(p);
        self.status = format!("Processing ({}/{}): {} ({}%)", p.done, p.total, name, pct);
        let eta = if p.done >= p.total {
            Some(Duration::ZERO)
        } else {
            p.eta()
        };
        self.eta = format_eta(eta);
        self.fraction = fraction;
    }

    /// Status shown right after the batch starts, before the first file finishes.
    pub fn starting(total: usize) -> Self {
        Self {
            status: format!("Processing (0/{total}): ... (0%)"),
            eta: format_eta(None),
            fraction: 0.0,
        }
    }
}

/// File name shown in the status line: the output name on success (Python
/// showed `basename(output)`), "Error" on failure.
fn progress_file_name(p: &Progress) -> String {
    let file_name = |path: &Path| {
        path.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    match p.last.status {
        FileStatus::Converted | FileStatus::Skipped => p
            .last
            .output
            .as_deref()
            .map(file_name)
            .unwrap_or_else(|| file_name(&p.last.source)),
        FileStatus::Failed => "Error".into(),
        FileStatus::Cancelled => file_name(&p.last.source),
    }
}

/// "ETA: --:--" when unknown, else "ETA: HH:MM:SS".
pub fn format_eta(eta: Option<Duration>) -> String {
    match eta {
        None => "ETA: --:--".into(),
        Some(d) => {
            let s = d.as_secs();
            format!("ETA: {:02}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
        }
    }
}

/// Python `_format_time`: "12.3s" below a minute, else "2m 30s".
pub fn format_elapsed(seconds: f64) -> String {
    if seconds < 60.0 {
        format!("{seconds:.1}s")
    } else {
        let total = seconds as u64;
        format!("{}m {}s", total / 60, total % 60)
    }
}

/// Kind of the completion dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageKind {
    Info,
    Warning,
    Error,
}

/// Completion dialog content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionMessage {
    pub kind: MessageKind,
    pub title: String,
    pub body: String,
    /// Offer "Open output folder?".
    pub offer_open: bool,
}

/// Build the completion message (Python texts, plus a skipped count when
/// files were skipped because the output existed).
pub fn completion_message(
    outcome: &BatchOutcome,
    output_dir: &Path,
    log_file: &Path,
) -> CompletionMessage {
    match outcome {
        BatchOutcome::Error(e) => CompletionMessage {
            kind: MessageKind::Error,
            title: "Error".into(),
            body: format!("An unexpected error occurred:\n{e}"),
            offer_open: false,
        },
        BatchOutcome::Finished(s) if s.was_cancelled => CompletionMessage {
            kind: MessageKind::Info,
            title: "Cancelled".into(),
            body: "Conversion cancelled.".into(),
            offer_open: s.converted > 0,
        },
        BatchOutcome::Finished(s) => {
            let elapsed = format_elapsed(s.elapsed_secs);
            let skipped = if s.skipped > 0 {
                format!("Skipped (output exists): {}\n", s.skipped)
            } else {
                String::new()
            };
            if s.failed == 0 {
                CompletionMessage {
                    kind: MessageKind::Info,
                    title: "Success".into(),
                    body: format!(
                        "Successfully converted {} file(s).\n{skipped}Time taken: {elapsed}\nSaved to: {}",
                        s.converted,
                        output_dir.display()
                    ),
                    offer_open: true,
                }
            } else {
                CompletionMessage {
                    kind: MessageKind::Warning,
                    title: "Completed with Errors".into(),
                    body: format!(
                        "Conversion complete with some errors:\nSuccessful: {}\nFailed: {}\n{skipped}Time taken: {elapsed}\n\nCheck {} for details.\nSaved to: {}",
                        s.converted,
                        s.failed,
                        log_file.display(),
                        output_dir.display()
                    ),
                    offer_open: s.converted > 0,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ccoonverter_core::testutil::{synthetic_dng, DngSpec};

    #[test]
    fn ui_settings_roundtrip() {
        let core = ConvertSettings {
            resize: 2000,
            quality: 77,
            brightness: 1.25,
            white_balance: WhiteBalance::Auto,
            style: PictureStyle::BlackAndWhite,
            rgb_red: 0.5,
            rgb_green: 1.5,
            rgb_blue: 2.0,
            black_point: 12,
            white_point: 240,
            extract_embedded: true,
            preserve_exif: true,
            on_conflict: ConflictPolicy::Skip,
        };
        let ui = UiSettings::from_convert(&core);
        assert_eq!(ui.resize_text, "2000");
        assert_eq!(ui.to_convert(), core);
        assert_eq!(
            UiSettings::default().to_convert(),
            ConvertSettings::default()
        );
    }

    #[test]
    fn slider_values_are_rounded_and_clamped() {
        let ui = UiSettings {
            quality: 89.6,
            black_point: 12.4,
            white_point: 300.0,
            brightness: 2.5,
            ..Default::default()
        };
        let s = ui.to_convert();
        assert_eq!(s.quality, 90);
        assert_eq!(s.black_point, 12);
        assert_eq!(s.white_point, 255);
        assert_eq!(s.brightness, 2.0);
        assert!(s.validate().is_ok());
    }

    #[test]
    fn invalid_resize_becomes_zero() {
        assert_eq!(parse_resize("1920"), 1920);
        assert_eq!(parse_resize(" 800 "), 800);
        assert_eq!(parse_resize(""), 0);
        assert_eq!(parse_resize("abc"), 0);
        assert_eq!(parse_resize("-5"), 0);
        assert_eq!(parse_resize("12.5"), 0);
        assert_eq!(parse_resize("99999999"), config::MAX_RESIZE);
    }

    #[test]
    fn reset_keeps_non_raw_settings() {
        let mut ui = UiSettings {
            resize_text: "1000".into(),
            quality: 50.0,
            brightness: 1.7,
            style: PictureStyle::Sepia,
            white_balance: WhiteBalance::Auto,
            rgb: [0.2, 0.3, 0.4],
            black_point: 20.0,
            white_point: 200.0,
            preserve_exif: true,
            extract_embedded: true,
            on_conflict: ConflictPolicy::Overwrite,
        };
        ui.reset_raw_adjustments();
        assert_eq!(ui.brightness, 1.0);
        assert_eq!(ui.style, PictureStyle::Standard);
        assert_eq!(ui.white_balance, WhiteBalance::CameraDefault);
        assert_eq!(ui.rgb, [1.0; 3]);
        assert_eq!((ui.black_point, ui.white_point), (0.0, 255.0));
        assert_eq!(ui.resize_text, "1000");
        assert_eq!(ui.quality, 50.0);
        assert!(ui.preserve_exif && ui.extract_embedded);
        assert_eq!(ui.on_conflict, ConflictPolicy::Overwrite);
    }

    #[test]
    fn preset_keeps_conflict_policy() {
        let mut ui = UiSettings {
            on_conflict: ConflictPolicy::Skip,
            ..Default::default()
        };
        let preset = ConvertSettings {
            style: PictureStyle::Vivid,
            quality: 60,
            on_conflict: ConflictPolicy::Overwrite,
            ..Default::default()
        };
        ui.apply_preset(&preset);
        assert_eq!(ui.style, PictureStyle::Vivid);
        assert_eq!(ui.quality, 60.0);
        assert_eq!(ui.on_conflict, ConflictPolicy::Skip);
    }

    #[test]
    fn eta_and_elapsed_formatting() {
        assert_eq!(format_eta(None), "ETA: --:--");
        assert_eq!(format_eta(Some(Duration::ZERO)), "ETA: 00:00:00");
        assert_eq!(format_eta(Some(Duration::from_secs(20))), "ETA: 00:00:20");
        assert_eq!(
            format_eta(Some(Duration::from_secs(3 * 3600 + 25 * 60 + 7))),
            "ETA: 03:25:07"
        );
        assert_eq!(
            format_eta(Some(Duration::from_millis(59_999))),
            "ETA: 00:00:59"
        );
        assert_eq!(format_elapsed(4.26), "4.3s");
        assert_eq!(format_elapsed(150.0), "2m 30s");
        assert_eq!(format_elapsed(3600.5), "60m 0s");
    }

    fn summary(converted: usize, failed: usize, skipped: usize, cancelled: bool) -> BatchSummary {
        BatchSummary {
            total: converted + failed + skipped,
            converted,
            skipped,
            failed,
            cancelled: 0,
            was_cancelled: cancelled,
            threads: 2,
            elapsed_secs: 75.0,
            results: vec![],
        }
    }

    #[test]
    fn completion_texts_match_python() {
        let out = Path::new("D:/사진/out");
        let log = Path::new("C:/logs/ccoonverter.log");
        let ok = completion_message(&BatchOutcome::Finished(summary(3, 0, 0, false)), out, log);
        assert_eq!(ok.kind, MessageKind::Info);
        assert_eq!(ok.title, "Success");
        assert_eq!(
            ok.body,
            format!(
                "Successfully converted 3 file(s).\nTime taken: 1m 15s\nSaved to: {}",
                out.display()
            )
        );
        assert!(ok.offer_open);

        let err = completion_message(&BatchOutcome::Finished(summary(2, 1, 0, false)), out, log);
        assert_eq!(err.kind, MessageKind::Warning);
        assert_eq!(err.title, "Completed with Errors");
        assert!(err.body.starts_with(
            "Conversion complete with some errors:\nSuccessful: 2\nFailed: 1\nTime taken: 1m 15s\n\nCheck "
        ));
        assert!(err.body.contains("ccoonverter.log for details."));
        assert!(err.body.ends_with(&format!("Saved to: {}", out.display())));

        let skipped =
            completion_message(&BatchOutcome::Finished(summary(1, 0, 2, false)), out, log);
        assert!(skipped.body.contains("Skipped (output exists): 2\n"));

        let cancelled =
            completion_message(&BatchOutcome::Finished(summary(0, 0, 0, true)), out, log);
        assert_eq!(cancelled.title, "Cancelled");
        assert_eq!(cancelled.body, "Conversion cancelled.");
        assert!(!cancelled.offer_open);

        let e = completion_message(&BatchOutcome::Error("boom".into()), out, log);
        assert_eq!(e.kind, MessageKind::Error);
        assert!(e.body.contains("boom"));
    }

    #[test]
    fn progress_view_texts() {
        use ccoonverter_core::FileResult;
        let mut v = ProgressView::default();
        assert_eq!(v.status, "READY");
        assert_eq!(v.eta, "ETA: --:--");
        let mk = |done, status, output: Option<&str>| Progress {
            done,
            total: 4,
            ok: done,
            skipped: 0,
            failed: 0,
            elapsed: Duration::from_secs(10 * done as u64),
            last: FileResult {
                source: PathBuf::from("in/IMG_1.ARW"),
                status,
                output: output.map(PathBuf::from),
                method: None,
                width: None,
                height: None,
                message: None,
                duration_ms: 0,
            },
        };
        v.apply(&mk(1, FileStatus::Converted, Some("out/IMG_1.jpg")));
        assert_eq!(v.status, "Processing (1/4): IMG_1.jpg (25%)");
        assert_eq!(v.eta, "ETA: 00:00:30");
        assert!((v.fraction - 0.25).abs() < 1e-6);
        v.apply(&mk(3, FileStatus::Failed, None));
        assert_eq!(v.status, "Processing (3/4): Error (75%)");
        v.apply(&mk(4, FileStatus::Converted, Some("out/IMG_4.jpg")));
        assert_eq!(v.eta, "ETA: 00:00:00");
        assert_eq!(v.fraction, 1.0);
        v.reset();
        assert_eq!(v, ProgressView::default());
    }

    #[test]
    fn output_folder_resolution() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("원본");
        let custom = dir.path().join("출력");
        std::fs::create_dir_all(&src).unwrap();
        assert_eq!(resolve_output_folder(&src, ""), src.join("Converted_JPG"));
        // Missing custom folder falls back to the default (Python behaviour).
        assert_eq!(
            resolve_output_folder(&src, &custom.to_string_lossy()),
            src.join("Converted_JPG")
        );
        std::fs::create_dir_all(&custom).unwrap();
        assert_eq!(
            resolve_output_folder(&src, &custom.to_string_lossy()),
            custom
        );
    }

    #[test]
    fn prepare_rejects_bad_sources() {
        assert_eq!(
            prepare_batch("", "").unwrap_err(),
            PrepareError::InvalidSource
        );
        assert_eq!(
            prepare_batch("Z:/definitely/not/here", "").unwrap_err(),
            PrepareError::InvalidSource
        );
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("photo.jpg"), b"x").unwrap();
        assert_eq!(
            prepare_batch(&dir.path().to_string_lossy(), "").unwrap_err(),
            PrepareError::NoFiles
        );
        assert_eq!(
            PrepareError::NoFiles.dialog(),
            (
                "Info",
                "No RAW files found in the selected folder.".to_string()
            )
        );
        assert_eq!(
            PrepareError::InvalidSource.dialog().1,
            "Please select a valid source folder."
        );
    }

    /// Full wiring: UI settings -> scan -> worker thread -> progress -> summary,
    /// on synthetic DNGs in a Korean-named folder, both conversion paths.
    #[test]
    fn end_to_end_batch_with_synthetic_dng() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("사진 폴더");
        std::fs::create_dir_all(src.join("sub")).unwrap();
        let dng = synthetic_dng(&DngSpec::default());
        std::fs::write(src.join("a.DNG"), &dng).unwrap();
        std::fs::write(src.join("sub").join("b.dng"), &dng).unwrap();
        std::fs::write(src.join("broken.NEF"), [0u8; 64]).unwrap();

        for embedded in [false, true] {
            let ui = UiSettings {
                resize_text: "40".into(),
                extract_embedded: embedded,
                style: PictureStyle::Landscape,
                on_conflict: ConflictPolicy::Overwrite,
                ..Default::default()
            };
            let batch = prepare_batch(&src.to_string_lossy(), "").unwrap();
            assert_eq!(batch.tasks.len(), 3);
            assert_eq!(batch.output_dir, src.join("Converted_JPG"));
            let mut running = RunningBatch::spawn(batch, ui.to_convert(), Some(2)).unwrap();
            let mut view = ProgressView::starting(running.total);
            let outcome = loop {
                for p in running.drain_progress() {
                    view.apply(&p);
                }
                if let Some(o) = running.try_finish() {
                    break o;
                }
                std::thread::sleep(Duration::from_millis(10));
            };
            // Messages sent before the thread ended are still in the channel.
            for p in running.drain_progress() {
                view.apply(&p);
            }
            assert_eq!(view.fraction, 1.0, "embedded={embedded}");
            assert!(view.status.starts_with("Processing (3/3)"));
            let BatchOutcome::Finished(s) = &outcome else {
                panic!("unexpected {outcome:?}");
            };
            assert_eq!((s.converted, s.failed), (2, 1), "embedded={embedded}");
            let out = src.join("Converted_JPG");
            for f in [out.join("a.jpg"), out.join("sub").join("b.jpg")] {
                let img = image::open(&f).unwrap();
                assert_eq!(img.width().max(img.height()), 40, "{}", f.display());
            }
            let msg = completion_message(&outcome, &out, Path::new("x.log"));
            assert_eq!(msg.kind, MessageKind::Warning);
            assert!(msg.body.contains("Successful: 2\nFailed: 1"));
        }
    }

    #[test]
    fn cancel_before_start_converts_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let dng = synthetic_dng(&DngSpec::default());
        for i in 0..4 {
            std::fs::write(dir.path().join(format!("f{i}.DNG")), &dng).unwrap();
        }
        let batch = prepare_batch(&dir.path().to_string_lossy(), "").unwrap();
        let out = batch.output_dir.clone();
        // Spawn with the flag already set by cancelling immediately; the worker
        // may finish a file that started before the flag was seen.
        let running = RunningBatch::spawn(batch, ConvertSettings::default(), Some(1)).unwrap();
        running.cancel();
        assert!(running.is_cancel_requested());
        let BatchOutcome::Finished(s) = running.wait() else {
            panic!("batch errored");
        };
        assert!(s.was_cancelled);
        assert!(
            s.converted <= 1,
            "at most the in-flight file: {}",
            s.converted
        );
        assert_eq!(s.converted + s.cancelled + s.failed + s.skipped, 4);
        let msg = completion_message(&BatchOutcome::Finished(s), &out, Path::new("x.log"));
        assert_eq!(msg.title, "Cancelled");
    }

    #[test]
    fn invalid_settings_surface_as_error() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.DNG"), synthetic_dng(&DngSpec::default())).unwrap();
        let batch = prepare_batch(&dir.path().to_string_lossy(), "").unwrap();
        // Not reachable from the UI (sliders are clamped), but must not hang or panic.
        let bad = ConvertSettings {
            white_point: 100,
            ..Default::default()
        };
        match RunningBatch::spawn(batch, bad, None).unwrap().wait() {
            BatchOutcome::Error(e) => assert!(e.contains("White point"), "{e}"),
            other => panic!("expected error, got {other:?}"),
        }
    }
}
