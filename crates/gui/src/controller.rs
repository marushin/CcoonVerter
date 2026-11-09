//! Conversion orchestration used by the GUI, kept free of egui so it can be
//! unit-tested without a window:
//!
//! UI values ([`UiSettings`]) → [`ConvertSettings`] → [`prepare_batch`]
//! (validate source, resolve output, scan) → [`RunningBatch`] (one worker
//! thread calling `job::run_batch` with an mpsc progress channel and an
//! `Arc<AtomicBool>` cancel flag) → [`BatchSummary`] → [`BatchReport`].

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use ccoonverter_core::config;
use ccoonverter_core::error::FailureKind;
use ccoonverter_core::job::{self, BatchSummary, FileStatus, Progress};
use ccoonverter_core::naming::{self, TemplateError};
use ccoonverter_core::{
    ColorSpace, ConflictPolicy, ConvertSettings, FileResult, FileTask, OutputFormat, PictureStyle,
    WhiteBalance,
};

use crate::i18n::{t, tf, Key};

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
    pub exposure_ev: f32,
    pub tone_curve: bool,
    /// Output sharpening (no slider in the UI; kept in presets and settings).
    pub sharpen: f32,
    pub keep_source_date: bool,
    pub output_format: OutputFormat,
    pub color_space: ColorSpace,
    /// Raw text of the file name template; validated when a batch starts.
    pub filename_template: String,
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
            exposure_ev: s.exposure_ev,
            tone_curve: s.tone_curve,
            sharpen: s.sharpen,
            keep_source_date: s.keep_source_date,
            output_format: s.output_format,
            color_space: s.color_space,
            filename_template: s.filename_template,
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
            exposure_ev: self
                .exposure_ev
                .clamp(config::MIN_EXPOSURE_EV, config::MAX_EXPOSURE_EV),
            tone_curve: self.tone_curve,
            sharpen: self.sharpen.clamp(config::MIN_SHARPEN, config::MAX_SHARPEN),
            keep_source_date: self.keep_source_date,
            output_format: self.output_format,
            color_space: self.color_space,
            filename_template: self.filename_template.clone(),
        }
    }

    /// The format actually written: camera JPEG mode stays JPEG.
    pub fn effective_format(&self) -> OutputFormat {
        if self.extract_embedded {
            OutputFormat::Jpeg
        } else {
            self.output_format
        }
    }

    /// Short description of the output, for the button that opens its options.
    pub fn output_summary(&self) -> String {
        if self.extract_embedded {
            t(Key::CameraJpegOutput).to_string()
        } else {
            format!(
                "{}  ·  {}",
                self.output_format.label(),
                self.color_space.label()
            )
        }
    }

    /// Live example of the resulting file name, or the reason it is invalid.
    pub fn name_example(&self) -> Result<String, String> {
        naming::example_name(&self.filename_template, self.effective_format().extension())
            .map_err(|e| template_error_text(&e))
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
        self.exposure_ev = d.exposure_ev;
        self.tone_curve = d.tone_curve;
        self.sharpen = d.sharpen;
    }

    /// Apply a preset like Python `_apply_settings_to_ui`: every saved value is
    /// applied, except the output conflict policy which is an app preference.
    pub fn apply_preset(&mut self, preset: &ConvertSettings) {
        let keep = self.on_conflict;
        *self = Self::from_convert(preset);
        self.on_conflict = keep;
    }
}

/// Label keys of the conflict policy combo, in display order.
pub const CONFLICT_CHOICES: [(ConflictPolicy, Key); 3] = [
    (ConflictPolicy::Suffix, Key::ConflictSuffix),
    (ConflictPolicy::Skip, Key::ConflictSkip),
    (ConflictPolicy::Overwrite, Key::ConflictOverwrite),
];

pub fn conflict_label(p: ConflictPolicy) -> &'static str {
    CONFLICT_CHOICES
        .iter()
        .find(|(c, _)| *c == p)
        .map(|(_, l)| t(*l))
        .unwrap_or_else(|| t(Key::ConflictSuffix))
}

/// One-line description of a format, shown under its combo box.
pub fn format_note(format: OutputFormat) -> Key {
    match format {
        OutputFormat::Jpeg => Key::NoteJpeg,
        OutputFormat::Png => Key::NotePng,
        OutputFormat::Tiff16 => Key::NoteTiff,
        OutputFormat::Webp => Key::NoteWebp,
    }
}

/// One-line description of a colour space, shown under its combo box.
pub fn color_note(space: ColorSpace) -> Key {
    match space {
        ColorSpace::Srgb => Key::NoteSrgb,
        ColorSpace::AdobeRgb => Key::NoteAdobe,
    }
}

fn template_error_text(e: &TemplateError) -> String {
    match e {
        TemplateError::Empty => t(Key::TemplateEmpty).to_string(),
        TemplateError::TooLong => t(Key::TemplateTooLong).to_string(),
        TemplateError::PathSeparator => t(Key::TemplatePathSeparator).to_string(),
        TemplateError::Traversal => t(Key::TemplateTraversal).to_string(),
        TemplateError::UnbalancedBrace => t(Key::TemplateBrace).to_string(),
        TemplateError::UnknownToken(token) => tf(Key::TemplateUnknownToken, &[token]),
    }
}

/// Display name of a white balance mode (the JSON name stays English).
pub fn wb_name(wb: WhiteBalance) -> Key {
    match wb {
        WhiteBalance::CameraDefault => Key::WbCameraDefault,
        WhiteBalance::Auto => Key::WbAuto,
    }
}

/// Display name of a picture style (the JSON name stays English).
pub fn style_name(s: PictureStyle) -> Key {
    match s {
        PictureStyle::Standard => Key::StyleStandard,
        PictureStyle::Portrait => Key::StylePortrait,
        PictureStyle::Landscape => Key::StyleLandscape,
        PictureStyle::Neutral => Key::StyleNeutral,
        PictureStyle::Vivid => Key::StyleVivid,
        PictureStyle::Monochrome => Key::StyleMonochrome,
        PictureStyle::Sepia => Key::StyleSepia,
        PictureStyle::Cinematic => Key::StyleCinematic,
        PictureStyle::Hdr => Key::StyleHdr,
        PictureStyle::Soft => Key::StyleSoft,
        PictureStyle::Dramatic => Key::StyleDramatic,
        PictureStyle::BlackAndWhite => Key::StyleBlackAndWhite,
    }
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

/// Progress messages are limited to this rate, so a large batch cannot flood
/// the channel while the UI thread is busy.
pub const PROGRESS_INTERVAL: Duration = Duration::from_millis(50);

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
    /// The sentence shown to the user.
    pub fn message(&self) -> String {
        match self {
            PrepareError::InvalidSource => t(Key::InvalidSourceFolder).to_string(),
            PrepareError::NoFiles => t(Key::NoRawFound).to_string(),
            PrepareError::Scan(e) => tf(Key::FileSystemError, &[e]),
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

/// Files staged by a drop: converted instead of scanning the source folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DropSelection {
    /// Common parent of the files; acts as the source root.
    pub root: PathBuf,
    pub files: Vec<PathBuf>,
}

/// Build a batch from dropped files. Same output rule as a folder batch:
/// the custom output folder if it exists, else `<root>/Converted_JPG`,
/// with sub-folders below `root` preserved.
pub fn prepare_selection(selection: &DropSelection, output_text: &str) -> PreparedBatch {
    let output_dir = resolve_output_folder(&selection.root, output_text);
    let tasks = ccoonverter_core::plan_tasks(&selection.files, &selection.root, &output_dir);
    PreparedBatch { output_dir, tasks }
}

/// Deepest folder that contains every path, by path components.
/// `None` when they share nothing (different drives).
pub fn common_parent(files: &[PathBuf]) -> Option<PathBuf> {
    let mut iter = files.iter().filter_map(|f| f.parent());
    let first = iter.next()?;
    let mut common: Vec<_> = first.components().collect();
    for parent in iter {
        let n = common
            .iter()
            .zip(parent.components())
            .take_while(|(a, b)| **a == *b)
            .count();
        common.truncate(n);
    }
    if common.is_empty() {
        None
    } else {
        Some(common.iter().map(|c| c.as_os_str()).collect())
    }
}

/// What a drop onto the window should do. `notice` is a short message for
/// the user (ignored files, or why nothing happened).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DropPlan {
    /// A folder was dropped: it becomes the source folder.
    SetSource {
        folder: PathBuf,
        notice: Option<String>,
    },
    /// RAW files were dropped: stage exactly these for the next conversion.
    Files {
        selection: DropSelection,
        notice: Option<String>,
    },
    /// Nothing usable was dropped.
    Ignored { notice: String },
}

/// Classify dropped paths. `is_dir` is a parameter so tests need no files.
pub fn plan_drop(paths: &[PathBuf], is_dir: impl Fn(&Path) -> bool) -> DropPlan {
    let (dirs, files): (Vec<&PathBuf>, Vec<&PathBuf>) = paths.iter().partition(|p| is_dir(p));
    if let Some(folder) = dirs.first() {
        let ignored = dirs.len() - 1 + files.len();
        let notice =
            (ignored > 0).then(|| tf(Key::DropFolderNotice, &[&folder.display(), &ignored]));
        return DropPlan::SetSource {
            folder: (*folder).clone(),
            notice,
        };
    }
    let mut raws: Vec<PathBuf> = files
        .iter()
        .filter(|p| ccoonverter_core::scan::is_raw_file(p))
        .map(|p| (*p).clone())
        .collect();
    let ignored = files.len() - raws.len();
    if raws.is_empty() {
        return DropPlan::Ignored {
            notice: tf(
                Key::DropNoRaw,
                &[&ignored, &config::SUPPORTED_EXTENSIONS.join(", ")],
            ),
        };
    }
    raws.sort();
    raws.dedup();
    let Some(root) = common_parent(&raws) else {
        return DropPlan::Ignored {
            notice: t(Key::DropSameDrive).to_string(),
        };
    };
    let notice = (ignored > 0).then(|| tf(Key::DropIgnoredFiles, &[&ignored]));
    DropPlan::Files {
        selection: DropSelection { root, files: raws },
        notice,
    }
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
                job::run_batch_with(
                    &batch.tasks,
                    &settings,
                    threads,
                    PROGRESS_INTERVAL,
                    Some(tx),
                    &worker_cancel,
                )
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
            status: t(Key::Ready).to_string(),
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
        self.status = tf(Key::ProgressLine, &[&p.done, &p.total, &name, &pct]);
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
            status: tf(Key::ProgressStarting, &[&total]),
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
        FileStatus::Failed => t(Key::ProgressError).to_string(),
        FileStatus::Cancelled => file_name(&p.last.source),
    }
}

/// "ETA: --:--" when unknown, else "ETA: HH:MM:SS".
pub fn format_eta(eta: Option<Duration>) -> String {
    match eta {
        None => t(Key::EtaUnknown).to_string(),
        Some(d) => {
            let s = d.as_secs();
            let hms = format!("{:02}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60);
            tf(Key::EtaValue, &[&hms])
        }
    }
}

/// Python `_format_time`: "12.3s" below a minute, else "2m 30s".
pub fn format_elapsed(seconds: f64) -> String {
    if seconds < 60.0 {
        tf(Key::ElapsedSeconds, &[&format!("{seconds:.1}")])
    } else {
        let total = seconds as u64;
        tf(Key::ElapsedMinutes, &[&(total / 60), &(total % 60)])
    }
}

/// A file that needs the user's attention after a batch: a failure, or a
/// fallback worth knowing about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchIssue {
    pub file: String,
    pub reason: String,
}

fn camera_model(message: &str) -> Option<&str> {
    let start = message.find("model '")? + "model '".len();
    let len = message[start..].find('\'')?;
    Some(&message[start..start + len]).filter(|m| !m.is_empty())
}

/// Readable, translated reason for one failed file.
pub fn failure_reason(r: &FileResult) -> String {
    let detail = r.message.as_deref().unwrap_or_default();
    let mut text = match r.failure.unwrap_or(FailureKind::Internal) {
        FailureKind::UnsupportedCamera => match camera_model(detail) {
            Some(model) => tf(Key::FailUnsupportedCamera, &[&model]),
            None => t(Key::FailUnsupportedCameraUnnamed).to_string(),
        },
        FailureKind::UnsupportedCompression => t(Key::FailUnsupportedCompression).to_string(),
        FailureKind::Unreadable => t(Key::FailUnreadable).to_string(),
        FailureKind::NoEmbeddedJpeg => t(Key::FailNoEmbeddedJpeg).to_string(),
        FailureKind::Disk => tf(Key::FailDisk, &[&detail]),
        FailureKind::Encode => tf(Key::FailEncode, &[&detail]),
        FailureKind::Internal => tf(Key::FailInternal, &[&detail]),
    };
    if r.camera_jpeg_available {
        text.push(' ');
        text.push_str(t(Key::FailCameraJpegHint));
    }
    text
}

/// Every failed file of a batch, plus files that fell back from camera JPEG
/// extraction to RAW development.
pub fn batch_issues(summary: &BatchSummary) -> Vec<BatchIssue> {
    let name = |r: &FileResult| {
        r.source
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| r.source.display().to_string())
    };
    summary
        .results
        .iter()
        .filter_map(|r| match r.status {
            FileStatus::Failed => Some(BatchIssue {
                file: name(r),
                reason: failure_reason(r),
            }),
            FileStatus::Converted if r.message.is_some() => Some(BatchIssue {
                file: name(r),
                reason: t(Key::NoteFellBack).to_string(),
            }),
            _ => None,
        })
        .collect()
}

/// Kind of a native message box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageKind {
    Warning,
    Error,
}

/// How a batch ended, for the result card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportKind {
    Done,
    Partial,
    Cancelled,
    Failed,
}

/// Everything the result card shows after a batch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchReport {
    pub kind: ReportKind,
    pub title: String,
    pub converted: usize,
    pub skipped: usize,
    pub failed: usize,
    /// "Time taken" line; empty when the batch never ran.
    pub elapsed: String,
    pub output_dir: PathBuf,
    pub log_file: PathBuf,
    pub issues: Vec<BatchIssue>,
    /// Message of a batch that stopped with an error.
    pub error: Option<String>,
    /// There is something in the output folder to look at.
    pub can_open: bool,
}

impl BatchReport {
    pub fn new(outcome: &BatchOutcome, output_dir: &Path, log_file: &Path) -> Self {
        let mut report = Self {
            kind: ReportKind::Failed,
            title: t(Key::ResultErrorTitle).to_string(),
            converted: 0,
            skipped: 0,
            failed: 0,
            elapsed: String::new(),
            output_dir: output_dir.to_path_buf(),
            log_file: log_file.to_path_buf(),
            issues: Vec::new(),
            error: None,
            can_open: false,
        };
        match outcome {
            BatchOutcome::Error(e) => report.error = Some(tf(Key::UnexpectedError, &[e])),
            BatchOutcome::Finished(s) => {
                report.converted = s.converted;
                report.skipped = s.skipped;
                report.failed = s.failed;
                report.elapsed = tf(Key::ResultElapsed, &[&format_elapsed(s.elapsed_secs)]);
                report.issues = batch_issues(s);
                report.can_open = s.converted > 0 || s.skipped > 0;
                (report.kind, report.title) = if s.was_cancelled {
                    (
                        ReportKind::Cancelled,
                        t(Key::ResultCancelledTitle).to_string(),
                    )
                } else if s.failed > 0 {
                    (ReportKind::Partial, t(Key::ResultPartialTitle).to_string())
                } else {
                    (ReportKind::Done, t(Key::ResultDoneTitle).to_string())
                };
            }
        }
        report
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
            exposure_ev: -1.25,
            tone_curve: false,
            sharpen: 0.8,
            keep_source_date: true,
            output_format: OutputFormat::Webp,
            color_space: ColorSpace::AdobeRgb,
            filename_template: "{date}_{seq}".into(),
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
            exposure_ev: 2.0,
            tone_curve: false,
            sharpen: 0.0,
            keep_source_date: true,
            output_format: OutputFormat::Png,
            color_space: ColorSpace::AdobeRgb,
            filename_template: "{seq}".into(),
        };
        ui.reset_raw_adjustments();
        assert_eq!(ui.exposure_ev, 0.0);
        assert!(ui.tone_curve);
        assert_eq!(ui.sharpen, config::DEFAULT_SHARPEN);
        assert_eq!(ui.brightness, 1.0);
        assert_eq!(ui.style, PictureStyle::Standard);
        assert_eq!(ui.white_balance, WhiteBalance::CameraDefault);
        assert_eq!(ui.rgb, [1.0; 3]);
        assert_eq!((ui.black_point, ui.white_point), (0.0, 255.0));
        assert_eq!(ui.resize_text, "1000");
        assert_eq!(ui.quality, 50.0);
        assert!(ui.preserve_exif && ui.extract_embedded);
        assert_eq!(ui.on_conflict, ConflictPolicy::Overwrite);
        assert!(ui.keep_source_date, "not a RAW adjustment");
        assert_eq!(ui.output_format, OutputFormat::Png);
        assert_eq!(ui.color_space, ColorSpace::AdobeRgb);
        assert_eq!(ui.filename_template, "{seq}");
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

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn dropping_a_folder_sets_the_source() {
        let paths = vec![p("D:/a/photos")];
        let plan = plan_drop(&paths, |_| true);
        assert_eq!(
            plan,
            DropPlan::SetSource {
                folder: p("D:/a/photos"),
                notice: None
            }
        );
    }

    #[test]
    fn folder_wins_over_files_with_a_notice() {
        let paths = vec![p("D:/a/one.jpg"), p("D:/a/photos"), p("D:/a/two")];
        let plan = plan_drop(&paths, |q| q.extension().is_none());
        match plan {
            DropPlan::SetSource { folder, notice } => {
                assert_eq!(folder, p("D:/a/photos"));
                assert!(notice.unwrap().contains("2 other"));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn dropping_files_selects_only_raw_files() {
        let paths = vec![
            p("D:/shoot/b.CR3"),
            p("D:/shoot/sub/a.nef"),
            p("D:/shoot/readme.txt"),
            p("D:/shoot/IMG.JPG"),
        ];
        match plan_drop(&paths, |_| false) {
            DropPlan::Files { selection, notice } => {
                assert_eq!(selection.root, p("D:/shoot"));
                assert_eq!(
                    selection.files,
                    vec![p("D:/shoot/b.CR3"), p("D:/shoot/sub/a.nef")]
                );
                assert_eq!(notice.as_deref(), Some("Ignored 2 non-RAW file(s)."));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn only_non_raw_files_are_ignored_with_a_message() {
        let paths = vec![p("D:/x/a.jpg"), p("D:/x/b.txt")];
        match plan_drop(&paths, |_| false) {
            DropPlan::Ignored { notice } => {
                assert!(notice.contains("No RAW files"), "{notice}");
                assert!(notice.contains('2'), "{notice}");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn common_parent_cases() {
        assert_eq!(
            common_parent(&[p("D:/a/b/x.dng"), p("D:/a/b/y.dng")]),
            Some(p("D:/a/b"))
        );
        assert_eq!(
            common_parent(&[p("D:/a/b/x.dng"), p("D:/a/c/y.dng")]),
            Some(p("D:/a"))
        );
        assert_eq!(common_parent(&[p("a.dng"), p("b.dng")]), None);
        assert_eq!(common_parent(&[]), None);
    }

    #[cfg(windows)]
    #[test]
    fn files_on_different_drives_are_rejected() {
        let paths = vec![p(r"C:\a\x.dng"), p(r"D:\a\y.dng")];
        assert!(matches!(
            plan_drop(&paths, |_| false),
            DropPlan::Ignored { .. }
        ));
    }

    #[test]
    fn dropped_files_use_the_normal_output_rule() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("촬영");
        std::fs::create_dir_all(root.join("sub")).unwrap();
        let dng = synthetic_dng(&DngSpec::default());
        let a = root.join("a.DNG");
        let b = root.join("sub").join("b.DNG");
        std::fs::write(&a, &dng).unwrap();
        std::fs::write(&b, &dng).unwrap();
        // A third RAW in the folder that was not dropped must stay untouched.
        std::fs::write(root.join("not_dropped.DNG"), &dng).unwrap();

        let DropPlan::Files { selection, .. } =
            plan_drop(&[a.clone(), b.clone()], |q: &Path| q.is_dir())
        else {
            panic!("expected files");
        };
        let batch = prepare_selection(&selection, "");
        assert_eq!(batch.output_dir, root.join("Converted_JPG"));
        assert_eq!(batch.tasks.len(), 2);
        let BatchOutcome::Finished(s) =
            RunningBatch::spawn(batch.clone(), ConvertSettings::default(), Some(1))
                .unwrap()
                .wait()
        else {
            panic!("batch errored");
        };
        assert_eq!(s.converted, 2);
        assert!(batch.output_dir.join("a.jpg").exists());
        assert!(batch.output_dir.join("sub").join("b.jpg").exists());
        assert!(!batch.output_dir.join("not_dropped.jpg").exists());

        // Custom output folder that exists is honoured, same as a folder batch.
        let out = dir.path().join("out");
        std::fs::create_dir_all(&out).unwrap();
        let batch = prepare_selection(&selection, &out.to_string_lossy());
        assert_eq!(batch.output_dir, out);
    }

    #[test]
    fn name_example_is_live_and_reports_errors() {
        use crate::i18n::{set_language, Lang};
        set_language(Lang::English);
        let mut ui = UiSettings::default();
        assert_eq!(ui.name_example().unwrap(), "IMG_1234.jpg");
        ui.output_format = OutputFormat::Tiff16;
        assert_eq!(ui.name_example().unwrap(), "IMG_1234.tif");
        ui.filename_template = "{date}_{camera}_{seq}".into();
        assert_eq!(ui.name_example().unwrap(), "2026-11-09_ILCE-7M4_0001.tif");
        ui.extract_embedded = true;
        assert!(ui.name_example().unwrap().ends_with(".jpg"), "camera JPEG");
        for bad in ["a/b", "x\\y", "..{name}", "{oops}", "{name"] {
            ui.filename_template = bad.into();
            assert!(ui.name_example().is_err(), "{bad}");
        }
        ui.filename_template = "{oops}".into();
        assert!(ui.name_example().unwrap_err().contains("oops"));
    }

    #[test]
    fn output_summary_says_camera_jpeg_when_it_applies() {
        use crate::i18n::{set_language, Lang};
        set_language(Lang::English);
        let mut ui = UiSettings {
            output_format: OutputFormat::Tiff16,
            color_space: ColorSpace::AdobeRgb,
            ..Default::default()
        };
        assert!(ui.output_summary().contains("TIFF 16-bit"));
        assert!(ui.output_summary().contains("AdobeRGB"));
        ui.extract_embedded = true;
        assert!(ui.output_summary().contains("camera"));
    }

    fn failed(message: &str, kind: FailureKind, jpeg: bool) -> FileResult {
        FileResult {
            source: PathBuf::from("D:/in/DSC_0001.NEF"),
            status: FileStatus::Failed,
            output: None,
            method: None,
            width: None,
            height: None,
            message: Some(message.into()),
            failure: Some(kind),
            camera_jpeg_available: jpeg,
            duration_ms: 3,
        }
    }

    #[test]
    fn failure_list_tells_the_user_about_camera_jpeg_mode() {
        use crate::i18n::{set_language, Lang};
        set_language(Lang::English);
        let he = failed(
            "Cannot develop this RAW file: NEF compression Some(HighEfficency) is not supported",
            FailureKind::UnsupportedCompression,
            true,
        );
        let a7v = failed(
            "Cannot develop: Error: Unknown camera, model 'ILCE-7M5', make: 'SONY', mode: 'arw6'",
            FailureKind::UnsupportedCamera,
            true,
        );
        let corrupt = failed("RAW processing error: bad", FailureKind::Unreadable, false);
        let he_text = failure_reason(&he);
        assert!(he_text.contains("compression"), "{he_text}");
        assert!(he_text.contains("Camera JPEG mode"), "{he_text}");
        let a7v_text = failure_reason(&a7v);
        assert!(a7v_text.contains("ILCE-7M5"), "{a7v_text}");
        assert!(a7v_text.contains("Camera JPEG mode"), "{a7v_text}");
        assert!(!failure_reason(&corrupt).contains("Camera JPEG mode"));

        set_language(Lang::Korean);
        assert!(failure_reason(&he).contains("카메라 JPEG 모드"));
        set_language(Lang::English);

        let mut s = summary(1, 3, 0, false);
        let fell_back = FileResult {
            status: FileStatus::Converted,
            message: Some("embedded JPEG not found".into()),
            failure: None,
            ..he.clone()
        };
        s.results = vec![he, a7v, corrupt, fell_back];
        let issues = batch_issues(&s);
        assert_eq!(issues.len(), 4);
        assert_eq!(issues[0].file, "DSC_0001.NEF");
        assert!(issues[3].reason.contains("developed"));
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
    fn report_counts_and_titles() {
        let out = Path::new("D:/사진/out");
        let log = Path::new("C:/logs/ccoonverter.log");
        let ok = BatchReport::new(&BatchOutcome::Finished(summary(3, 0, 0, false)), out, log);
        assert_eq!(ok.kind, ReportKind::Done);
        assert_eq!(ok.title, "All cooked!");
        assert_eq!((ok.converted, ok.skipped, ok.failed), (3, 0, 0));
        assert_eq!(ok.elapsed, "Time taken: 1m 15s");
        assert!(ok.can_open);

        let partial = BatchReport::new(&BatchOutcome::Finished(summary(2, 1, 0, false)), out, log);
        assert_eq!(partial.kind, ReportKind::Partial);
        assert_eq!(partial.title, "Some files could not be converted");
        assert_eq!(partial.failed, 1);
        assert!(partial.can_open);

        let all_failed =
            BatchReport::new(&BatchOutcome::Finished(summary(0, 2, 0, false)), out, log);
        assert_eq!(all_failed.kind, ReportKind::Partial);
        assert!(!all_failed.can_open, "nothing to open");

        let skipped = BatchReport::new(&BatchOutcome::Finished(summary(1, 0, 2, false)), out, log);
        assert_eq!(skipped.skipped, 2);

        let cancelled = BatchReport::new(&BatchOutcome::Finished(summary(0, 0, 0, true)), out, log);
        assert_eq!(cancelled.kind, ReportKind::Cancelled);
        assert_eq!(cancelled.title, "Conversion cancelled");
        assert!(!cancelled.can_open);

        let e = BatchReport::new(&BatchOutcome::Error("boom".into()), out, log);
        assert_eq!(e.kind, ReportKind::Failed);
        assert!(e.error.as_deref().unwrap().contains("boom"));
    }

    #[test]
    fn progress_view_texts() {
        use ccoonverter_core::FileResult;
        let mut v = ProgressView::default();
        assert_eq!(v.status, "Ready when you are");
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
                failure: None,
                camera_jpeg_available: false,
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
            PrepareError::NoFiles.message(),
            "No RAW files found in the selected folder."
        );
        assert_eq!(
            PrepareError::InvalidSource.message(),
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
            let report = BatchReport::new(&outcome, &out, Path::new("x.log"));
            assert_eq!(report.kind, ReportKind::Partial);
            assert_eq!((report.converted, report.failed), (2, 1));
            assert_eq!(report.issues.len(), 1, "the broken file is listed");
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
        let report = BatchReport::new(&BatchOutcome::Finished(s), &out, Path::new("x.log"));
        assert_eq!(report.kind, ReportKind::Cancelled);
    }

    #[test]
    fn invalid_template_stops_the_batch_with_a_message() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.DNG"), synthetic_dng(&DngSpec::default())).unwrap();
        let batch = prepare_batch(&dir.path().to_string_lossy(), "").unwrap();
        let out = batch.output_dir.clone();
        let bad = ConvertSettings {
            filename_template: "../{name}".into(),
            ..Default::default()
        };
        match RunningBatch::spawn(batch, bad, Some(1)).unwrap().wait() {
            BatchOutcome::Error(e) => assert!(e.contains("template"), "{e}"),
            other => panic!("expected error, got {other:?}"),
        }
        assert!(!out.exists(), "nothing written");
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
