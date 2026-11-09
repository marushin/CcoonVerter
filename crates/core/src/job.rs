//! Batch conversion on a rayon pool (replaces Python's multiprocessing loop).
//!
//! * Progress is reported over a `std::sync::mpsc::Sender<Progress>` after each file.
//! * Cancellation: set the shared `AtomicBool`; files not yet started are
//!   reported as cancelled, files in progress stop at the next stage boundary.
//! * A panic inside rawler for one file is caught and reported as a failure of
//!   that file only (release profile uses `panic = "unwind"`).

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use rawler::decoders::RawDecodeParams;
use rawler::rawsource::RawSource;
use rayon::prelude::*;
use serde::Serialize;
use sysinfo::{MemoryRefreshKind, RefreshKind, System};

use crate::error::{ConvertError, FailureKind};
use crate::pipeline::{convert_one, ConvertOutcome, FileTask, Method};
use crate::settings::ConvertSettings;

/// Final state of one file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileStatus {
    Converted,
    Skipped,
    Failed,
    Cancelled,
}

/// Per-file result.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FileResult {
    pub source: PathBuf,
    pub status: FileStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<Method>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    /// Error message for failures; note for fallbacks.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Broad reason, for failed files.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure: Option<FailureKind>,
    /// The failed file holds a camera JPEG that camera JPEG mode can extract.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub camera_jpeg_available: bool,
    pub duration_ms: u64,
}

/// Progress message sent after each finished file.
#[derive(Debug, Clone)]
pub struct Progress {
    /// Files finished so far (any status).
    pub done: usize,
    pub total: usize,
    pub ok: usize,
    pub skipped: usize,
    pub failed: usize,
    pub elapsed: Duration,
    /// Result of the file that just finished.
    pub last: FileResult,
}

impl Progress {
    /// Simple ETA estimate from the average time per finished file.
    pub fn eta(&self) -> Option<Duration> {
        if self.done == 0 || self.done >= self.total {
            return None;
        }
        let per = self.elapsed.as_secs_f64() / self.done as f64;
        Some(Duration::from_secs_f64(
            per * (self.total - self.done) as f64,
        ))
    }
}

/// Summary returned by [`run_batch`].
#[derive(Debug, Clone, Serialize)]
pub struct BatchSummary {
    pub total: usize,
    pub converted: usize,
    pub skipped: usize,
    pub failed: usize,
    pub cancelled: usize,
    pub was_cancelled: bool,
    pub threads: usize,
    pub elapsed_secs: f64,
    pub results: Vec<FileResult>,
}

fn cores() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
}

/// Peak memory per pixel while developing: measured 29 to 31 bytes on files
/// of 8 to 28 MP. The peak is during demosaicing,
/// so the output format does not change it.
pub const DEVELOP_BYTES_PER_PIXEL: u64 = 32;
/// Peak unrelated to the image size (metering preview, decoder state).
const FIXED_BYTES_PER_FILE: u64 = 64 * 1024 * 1024;
/// Share of the available memory a batch may plan to use.
const MEMORY_BUDGET_PERCENT: u64 = 75;
/// Largest files looked at when estimating the largest image.
const SIZE_SAMPLES: usize = 3;

/// Peak bytes to expect while converting one image of `pixels` read from a
/// file of `file_bytes` (memory-mapped, so it counts).
pub fn estimate_file_memory(pixels: u64, file_bytes: u64) -> u64 {
    pixels
        .saturating_mul(DEVELOP_BYTES_PER_PIXEL)
        .saturating_add(file_bytes)
        .saturating_add(FIXED_BYTES_PER_FILE)
}

/// How many files fit in memory at once: at least 1, at most `cores`.
pub fn memory_threads(cores: usize, available_bytes: u64, per_file_bytes: u64) -> usize {
    let budget = available_bytes / 100 * MEMORY_BUDGET_PERCENT;
    let fit = budget / per_file_bytes.max(1);
    (fit.min(cores as u64) as usize).max(1)
}

/// Why a batch runs on this many threads (also logged).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThreadPlan {
    pub threads: usize,
    pub cores: usize,
    /// Available memory, `None` if the OS did not report it or it is unused.
    pub available_bytes: Option<u64>,
    pub largest_pixels: u64,
    pub per_file_bytes: u64,
}

impl ThreadPlan {
    fn describe(&self) -> String {
        match self.available_bytes {
            Some(avail) => format!(
                "{} threads: {} cores, {} MB available, largest image {:.1} MP, \
                 about {} MB per file",
                self.threads,
                self.cores,
                avail / 1_048_576,
                self.largest_pixels as f64 / 1e6,
                self.per_file_bytes / 1_048_576,
            ),
            None => format!(
                "{} threads: {} cores, not limited by memory",
                self.threads, self.cores
            ),
        }
    }
}

/// Pixel count of a RAW file from its header, without decoding it.
fn probe_pixels(path: &Path) -> Option<u64> {
    catch_unwind(|| {
        let source = RawSource::new(path).ok()?;
        let decoder = crate::rawguard::get_decoder(&source).ok()?;
        let raw = decoder
            .raw_image(&source, &RawDecodeParams::default(), true)
            .ok()?;
        Some(raw.width as u64 * raw.height as u64)
    })
    .ok()
    .flatten()
}

/// Largest image of the batch: the pixel count of the largest files by size.
/// Falls back to the file size itself (about one byte per pixel) for files
/// the decoder does not understand.
fn largest_image(tasks: &[FileTask]) -> (u64, u64) {
    let mut sized: Vec<(u64, &Path)> = tasks
        .iter()
        .map(|t| {
            let bytes = std::fs::metadata(&t.source).map(|m| m.len()).unwrap_or(0);
            (bytes, t.source.as_path())
        })
        .collect();
    sized.sort_unstable_by_key(|(b, _)| std::cmp::Reverse(*b));
    let largest_bytes = sized.first().map_or(0, |(b, _)| *b);
    let pixels = sized
        .iter()
        .take(SIZE_SAMPLES)
        .filter_map(|(_, p)| probe_pixels(p))
        .max()
        .unwrap_or(largest_bytes);
    (pixels, largest_bytes)
}

fn available_memory() -> Option<u64> {
    let ram = || MemoryRefreshKind::nothing().with_ram();
    let mut sys = System::new_with_specifics(RefreshKind::nothing().with_memory(ram()));
    sys.refresh_memory_specifics(ram());
    Some(sys.available_memory()).filter(|v| *v > 0)
}

/// Worker count when the caller does not set one. Camera JPEG extraction is
/// light and uses every core. Development is limited by the memory the
/// largest image needs.
pub fn plan_threads(tasks: &[FileTask], settings: &ConvertSettings) -> ThreadPlan {
    let cores = cores();
    if settings.extract_embedded {
        return ThreadPlan {
            threads: cores,
            cores,
            available_bytes: None,
            largest_pixels: 0,
            per_file_bytes: 0,
        };
    }
    let (largest_pixels, file_bytes) = largest_image(tasks);
    let per_file_bytes = estimate_file_memory(largest_pixels, file_bytes);
    let available_bytes = available_memory();
    let threads = match available_bytes {
        Some(avail) => memory_threads(cores, avail, per_file_bytes),
        // Without a memory reading, the old conservative cap.
        None => cores.clamp(1, 4),
    };
    ThreadPlan {
        threads,
        cores,
        available_bytes,
        largest_pixels,
        per_file_bytes,
    }
}

fn panic_message(p: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = p.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = p.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".into()
    }
}

fn run_task(task: &FileTask, settings: &ConvertSettings, cancel: &AtomicBool) -> FileResult {
    let t0 = Instant::now();
    let res = catch_unwind(AssertUnwindSafe(|| convert_one(task, settings, cancel)))
        .unwrap_or_else(|p| Err(ConvertError::Panic(panic_message(p.as_ref()))));
    let duration_ms = t0.elapsed().as_millis() as u64;
    let base = FileResult {
        source: task.source.clone(),
        status: FileStatus::Failed,
        output: None,
        method: None,
        width: None,
        height: None,
        message: None,
        failure: None,
        camera_jpeg_available: false,
        duration_ms,
    };
    match res {
        Ok(ConvertOutcome::Converted {
            output,
            method,
            width,
            height,
            fell_back,
            ..
        }) => FileResult {
            status: FileStatus::Converted,
            output: Some(output),
            method: Some(method),
            width: Some(width),
            height: Some(height),
            message: fell_back
                .then(|| "embedded JPEG not found, developed RAW instead".to_string()),
            ..base
        },
        Ok(ConvertOutcome::Skipped { output }) => FileResult {
            status: FileStatus::Skipped,
            output: Some(output),
            message: Some("output exists".into()),
            ..base
        },
        Err(ConvertError::Cancelled) => FileResult {
            status: FileStatus::Cancelled,
            ..base
        },
        Err(e) => {
            log::error!("{}: {e}", task.source.display());
            FileResult {
                failure: Some(e.failure_kind()),
                camera_jpeg_available: e.camera_jpeg_available(),
                message: Some(e.to_string()),
                ..base
            }
        }
    }
}

/// Convert all tasks, reporting every finished file. See [`run_batch_with`].
pub fn run_batch(
    tasks: &[FileTask],
    settings: &ConvertSettings,
    threads: Option<usize>,
    progress: Option<Sender<Progress>>,
    cancel: &AtomicBool,
) -> Result<BatchSummary, ConvertError> {
    run_batch_with(tasks, settings, threads, Duration::ZERO, progress, cancel)
}

/// Convert all tasks. Blocks until done or cancelled; run it on a worker
/// thread from a GUI. `threads = None` plans the count from memory
/// ([`plan_threads`]); an explicit count always wins.
///
/// `min_interval` limits progress messages to one per interval (the first and
/// the last file are always reported), so a large batch cannot flood or fill a
/// channel whose reader is busy. Per-file results stay complete in the summary.
pub fn run_batch_with(
    tasks: &[FileTask],
    settings: &ConvertSettings,
    threads: Option<usize>,
    min_interval: Duration,
    progress: Option<Sender<Progress>>,
    cancel: &AtomicBool,
) -> Result<BatchSummary, ConvertError> {
    settings.validate().map_err(ConvertError::InvalidSettings)?;
    let threads = match threads.filter(|n| *n > 0) {
        Some(n) => {
            log::info!("Thread count set explicitly: {n}");
            n
        }
        None => {
            let plan = plan_threads(tasks, settings);
            log::info!("Thread plan: {}", plan.describe());
            plan.threads
        }
    };
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .thread_name(|i| format!("ccoon-worker-{i}"))
        .build()
        .map_err(|e| ConvertError::Panic(format!("thread pool: {e}")))?;

    let tasks = pool
        .install(|| crate::naming::plan_output_names(tasks, settings, cancel))
        .map_err(|e| ConvertError::InvalidSettings(e.to_string()))?;
    let tasks = tasks.as_slice();

    let total = tasks.len();
    let start = Instant::now();
    // done, ok, skipped, failed, time of the last progress message
    let counters = Mutex::new((0usize, 0usize, 0usize, 0usize, None::<Instant>));
    let progress = progress.map(Mutex::new);
    log::info!(
        "Starting batch: {total} files, {threads} threads, {} {}",
        settings.effective_format().label(),
        settings.effective_color_space().label()
    );

    let mut results: Vec<(usize, FileResult)> = pool.install(|| {
        tasks
            .par_iter()
            .enumerate()
            .map(|(i, task)| {
                let r = if cancel.load(Ordering::Relaxed) {
                    FileResult {
                        source: task.source.clone(),
                        status: FileStatus::Cancelled,
                        output: None,
                        method: None,
                        width: None,
                        height: None,
                        message: None,
                        failure: None,
                        camera_jpeg_available: false,
                        duration_ms: 0,
                    }
                } else {
                    run_task(task, settings, cancel)
                };
                if r.status != FileStatus::Cancelled {
                    let mut c = counters.lock().unwrap_or_else(|e| e.into_inner());
                    c.0 += 1;
                    match r.status {
                        FileStatus::Converted => c.1 += 1,
                        FileStatus::Skipped => c.2 += 1,
                        FileStatus::Failed => c.3 += 1,
                        FileStatus::Cancelled => {}
                    }
                    let due = c.0 == total || c.4.is_none_or(|last| last.elapsed() >= min_interval);
                    if due {
                        c.4 = Some(Instant::now());
                    }
                    if let (true, Some(tx)) = (due, &progress) {
                        let msg = Progress {
                            done: c.0,
                            total,
                            ok: c.1,
                            skipped: c.2,
                            failed: c.3,
                            elapsed: start.elapsed(),
                            last: r.clone(),
                        };
                        // Receiver gone (GUI closed) is not an error for the batch.
                        let _ = tx.lock().unwrap_or_else(|e| e.into_inner()).send(msg);
                    }
                }
                (i, r)
            })
            .collect()
    });
    results.sort_by_key(|(i, _)| *i);
    let results: Vec<FileResult> = results.into_iter().map(|(_, r)| r).collect();

    let count = |s: FileStatus| results.iter().filter(|r| r.status == s).count();
    let summary = BatchSummary {
        total,
        converted: count(FileStatus::Converted),
        skipped: count(FileStatus::Skipped),
        failed: count(FileStatus::Failed),
        cancelled: count(FileStatus::Cancelled),
        was_cancelled: cancel.load(Ordering::Relaxed),
        threads,
        elapsed_secs: start.elapsed().as_secs_f64(),
        results,
    };
    log::info!(
        "Batch finished in {:.1}s: {} converted, {} skipped, {} failed, {} cancelled",
        summary.elapsed_secs,
        summary.converted,
        summary.skipped,
        summary.failed,
        summary.cancelled
    );
    Ok(summary)
}

/// Convenience: scan `source_root`, plan outputs into `output_root`
/// (default `<source>/Converted_JPG`) and return the tasks.
pub fn prepare(
    source_root: &Path,
    output_root: Option<&Path>,
) -> std::io::Result<(Vec<FileTask>, PathBuf)> {
    let files = crate::scan::scan_folder(source_root)?;
    let out = output_root
        .map(Path::to_path_buf)
        .unwrap_or_else(|| source_root.join(crate::config::DEFAULT_OUTPUT_FOLDER_NAME));
    Ok((crate::pipeline::plan_tasks(&files, source_root, &out), out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn fixture_dir() -> (tempfile::TempDir, Vec<FileTask>, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir_all(src.join("sub")).unwrap();
        let jpg = crate::embedded::tests::tiny_jpeg(40, 20, [10, 20, 30]);
        let mut blob = vec![7u8; 64];
        blob.extend_from_slice(&jpg);
        std::fs::write(src.join("a.ARW"), &blob).unwrap();
        std::fs::write(src.join("sub").join("b.CR3"), &blob).unwrap();
        std::fs::write(src.join("broken.NEF"), [0u8; 100]).unwrap();
        let (tasks, out) = prepare(&src, None).unwrap();
        (dir, tasks, out)
    }

    #[test]
    fn batch_reports_progress_and_failures() {
        let (_dir, tasks, out) = fixture_dir();
        assert_eq!(tasks.len(), 3);
        assert!(out.ends_with("Converted_JPG"));
        let settings = ConvertSettings {
            extract_embedded: true,
            ..Default::default()
        };
        let (tx, rx) = mpsc::channel();
        let cancel = AtomicBool::new(false);
        let summary = run_batch(&tasks, &settings, Some(2), Some(tx), &cancel).unwrap();
        let msgs: Vec<Progress> = rx.iter().collect();
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs.iter().map(|m| m.done).max(), Some(3));
        assert_eq!(summary.converted, 2);
        assert_eq!(summary.failed, 1);
        assert!(out.join("a.jpg").is_file());
        assert!(out.join("sub").join("b.jpg").is_file());
        let failed = summary
            .results
            .iter()
            .find(|r| r.status == FileStatus::Failed)
            .unwrap();
        assert!(failed.source.ends_with("broken.NEF"));
        assert!(failed.message.is_some());
    }

    #[test]
    fn cancelled_batch_does_nothing() {
        let (_dir, tasks, out) = fixture_dir();
        let cancel = AtomicBool::new(true);
        let s = ConvertSettings {
            extract_embedded: true,
            ..Default::default()
        };
        let summary = run_batch(&tasks, &s, Some(1), None, &cancel).unwrap();
        assert_eq!(summary.cancelled, 3);
        assert!(summary.was_cancelled);
        assert!(!out.exists());
    }

    #[test]
    fn invalid_settings_rejected() {
        let cancel = AtomicBool::new(false);
        let s = ConvertSettings {
            quality: 0,
            ..Default::default()
        };
        assert!(matches!(
            run_batch(&[], &s, None, None, &cancel),
            Err(ConvertError::InvalidSettings(_))
        ));
    }

    #[test]
    fn thread_count_follows_memory_and_cores() {
        const GB: u64 = 1024 * 1024 * 1024;
        // 24 MP file, 25 MB on disk.
        let per = estimate_file_memory(24_000_000, 25_000_000);
        assert_eq!(
            per,
            24_000_000 * DEVELOP_BYTES_PER_PIXEL + 25_000_000 + FIXED_BYTES_PER_FILE
        );
        // Plenty of RAM: limited by the cores, not the old cap of 4.
        assert_eq!(memory_threads(16, 64 * GB, per), 16);
        // 8 GB free, about 0.8 GB per file: 75 % fits 7.
        assert_eq!(memory_threads(16, 8 * GB, per), 7);
        // Short on memory: one file at a time, never zero.
        assert_eq!(memory_threads(16, GB / 2, per), 1);
        assert_eq!(memory_threads(16, 0, per), 1);
        // A bigger image means fewer threads.
        let big = estimate_file_memory(100_000_000, 100_000_000);
        assert!(memory_threads(32, 32 * GB, big) < memory_threads(32, 32 * GB, per));
        // Degenerate input cannot divide by zero or overflow.
        assert_eq!(memory_threads(4, u64::MAX, 0), 4);
        assert_eq!(estimate_file_memory(u64::MAX, u64::MAX), u64::MAX);
    }

    #[test]
    fn thread_plan_uses_the_memory_path_for_develop_only() {
        let (_dir, tasks, _out) = fixture_dir();
        let embedded = ConvertSettings {
            extract_embedded: true,
            ..Default::default()
        };
        let p = plan_threads(&tasks, &embedded);
        assert_eq!(p.threads, p.cores);
        assert_eq!(p.per_file_bytes, 0);
        let d = plan_threads(&tasks, &ConvertSettings::default());
        assert!(d.threads >= 1 && d.threads <= d.cores);
        assert!(d.per_file_bytes >= FIXED_BYTES_PER_FILE);
        assert!(!d.describe().is_empty());
    }

    #[test]
    fn explicit_thread_count_wins() {
        let (_dir, tasks, _out) = fixture_dir();
        let s = ConvertSettings {
            on_conflict: crate::ConflictPolicy::Overwrite,
            ..Default::default()
        };
        let summary = run_batch(&tasks, &s, Some(3), None, &AtomicBool::new(false)).unwrap();
        assert_eq!(summary.threads, 3);
    }

    #[test]
    fn progress_is_throttled_but_the_last_file_is_reported() {
        let (_dir, tasks, _out) = fixture_dir();
        let tasks: Vec<FileTask> = std::iter::repeat_n(tasks, 40).flatten().collect();
        let settings = ConvertSettings {
            extract_embedded: true,
            on_conflict: crate::ConflictPolicy::Overwrite,
            ..Default::default()
        };
        let (tx, rx) = mpsc::channel();
        let cancel = AtomicBool::new(false);
        let summary = run_batch_with(
            &tasks,
            &settings,
            Some(4),
            Duration::from_secs(3600),
            Some(tx),
            &cancel,
        )
        .unwrap();
        assert_eq!(summary.results.len(), 120, "results stay complete");
        let msgs: Vec<Progress> = rx.iter().collect();
        assert!(msgs.len() <= 2, "{} messages", msgs.len());
        assert_eq!(msgs.last().unwrap().done, 120);
    }

    #[test]
    fn failures_carry_a_kind() {
        let (_dir, tasks, _out) = fixture_dir();
        let settings = ConvertSettings {
            extract_embedded: true,
            ..Default::default()
        };
        let summary = run_batch(&tasks, &settings, Some(2), None, &AtomicBool::new(false)).unwrap();
        let failed = summary
            .results
            .iter()
            .find(|r| r.status == FileStatus::Failed)
            .unwrap();
        assert_eq!(failed.failure, Some(FailureKind::Unreadable));
        assert!(!failed.camera_jpeg_available);
    }

    #[test]
    fn eta_estimate() {
        let p = Progress {
            done: 2,
            total: 6,
            ok: 2,
            skipped: 0,
            failed: 0,
            elapsed: Duration::from_secs(10),
            last: FileResult {
                source: PathBuf::new(),
                status: FileStatus::Converted,
                output: None,
                method: None,
                width: None,
                height: None,
                message: None,
                failure: None,
                camera_jpeg_available: false,
                duration_ms: 0,
            },
        };
        assert_eq!(p.eta(), Some(Duration::from_secs(20)));
    }
}
