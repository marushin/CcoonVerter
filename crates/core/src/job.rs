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

use rayon::prelude::*;
use serde::Serialize;

use crate::error::ConvertError;
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

/// Default worker count. Developing a 24-60 MP RAW needs roughly 1-1.5 GB
/// of RAM per file, so the develop path is capped; embedded extraction is
/// light and uses every core.
pub fn default_threads(settings: &ConvertSettings) -> usize {
    let cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    if settings.extract_embedded {
        cores
    } else {
        cores.clamp(1, 4)
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
                message: Some(e.to_string()),
                ..base
            }
        }
    }
}

/// Convert all tasks. Blocks until done or cancelled; run it on a worker
/// thread from a GUI. `threads = None` uses [`default_threads`].
pub fn run_batch(
    tasks: &[FileTask],
    settings: &ConvertSettings,
    threads: Option<usize>,
    progress: Option<Sender<Progress>>,
    cancel: &AtomicBool,
) -> Result<BatchSummary, ConvertError> {
    settings.validate().map_err(ConvertError::InvalidSettings)?;
    let threads = threads
        .filter(|n| *n > 0)
        .unwrap_or_else(|| default_threads(settings));
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .thread_name(|i| format!("ccoon-worker-{i}"))
        .build()
        .map_err(|e| ConvertError::Panic(format!("thread pool: {e}")))?;

    let total = tasks.len();
    let start = Instant::now();
    let counters = Mutex::new((0usize, 0usize, 0usize, 0usize)); // done, ok, skipped, failed
    let progress = progress.map(Mutex::new);
    log::info!("Starting batch: {total} files, {threads} threads");

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
                    if let Some(tx) = &progress {
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
                duration_ms: 0,
            },
        };
        assert_eq!(p.eta(), Some(Duration::from_secs(20)));
    }
}
