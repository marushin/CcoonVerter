//! Validation of a very large batch: 10,000 small synthetic DNGs in a temp
//! folder, through the same throttled progress path the GUI uses.
//!
//! `cargo test --release -p ccoonverter-core --test large_batch -- --ignored
//! --nocapture --test-threads=1`
//!
//! Measures throughput, messages received, the deepest channel backlog while
//! the reader polls like the GUI does (every 100 ms), the backlog when the
//! reader stalls completely, and the peak working set (sampled every 20 ms).

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use ccoonverter_core::job::{self, FileStatus};
use ccoonverter_core::testutil::{synthetic_dng, DngSpec};
use ccoonverter_core::{plan_tasks, ConvertSettings};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

const FILES: usize = 10_000;
/// Same limit as `controller::PROGRESS_INTERVAL`.
const INTERVAL: Duration = Duration::from_millis(50);

fn write_files(dir: &Path) {
    let dng = synthetic_dng(&DngSpec {
        width: 160,
        height: 120,
        preview: Some((80, 60)),
        ..Default::default()
    });
    for i in 0..FILES {
        let sub = dir.join(format!("d{:02}", i % 40));
        if i < 40 {
            std::fs::create_dir_all(&sub).unwrap();
        }
        std::fs::write(sub.join(format!("f{i:05}.DNG")), &dng).unwrap();
    }
}

fn working_set(sys: &mut System, pid: Pid) -> u64 {
    sys.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[pid]),
        true,
        ProcessRefreshKind::nothing().with_memory(),
    );
    sys.process(pid).map(|p| p.memory()).unwrap_or(0)
}

/// Runs one batch while sampling memory; returns (elapsed, messages, deepest backlog, peak, start RSS, final RSS).
fn run(
    settings: &ConvertSettings,
    dir: &Path,
    drain_every: Option<Duration>,
    threads: usize,
) -> Stats {
    let files = ccoonverter_core::scan::scan_folder(dir).unwrap();
    assert_eq!(files.len(), FILES);
    let out = dir.join("out");
    let tasks = plan_tasks(&files, dir, &out);
    let pid = sysinfo::get_current_pid().unwrap();
    let peak = AtomicU64::new(0);
    let stop = AtomicBool::new(false);
    let finished = AtomicBool::new(false);
    let mut sys = System::new();
    let start_rss = working_set(&mut sys, pid);
    let cancel = AtomicBool::new(false);
    let (tx, rx) = mpsc::channel::<job::Progress>();

    let t0 = Instant::now();
    let (summary, stats_rx) = std::thread::scope(|scope| {
        scope.spawn(|| {
            let mut sys = System::new();
            while !stop.load(Ordering::Relaxed) {
                peak.fetch_max(working_set(&mut sys, pid), Ordering::Relaxed);
                std::thread::sleep(Duration::from_millis(20));
            }
        });
        let reader = scope.spawn(|| {
            let mut received = 0usize;
            let mut deepest = 0usize;
            let mut last_done = 0usize;
            loop {
                match drain_every {
                    Some(d) => std::thread::sleep(d),
                    // A stalled UI thread: nothing is read until the batch ends.
                    None => {
                        while !finished.load(Ordering::Relaxed) {
                            std::thread::sleep(Duration::from_millis(20));
                        }
                    }
                }
                let mut got = 0usize;
                let mut closed = false;
                loop {
                    match rx.try_recv() {
                        Ok(p) => {
                            got += 1;
                            last_done = last_done.max(p.done);
                        }
                        Err(mpsc::TryRecvError::Empty) => break,
                        Err(mpsc::TryRecvError::Disconnected) => {
                            closed = true;
                            break;
                        }
                    }
                }
                received += got;
                deepest = deepest.max(got);
                if closed {
                    return (received, deepest, last_done);
                }
            }
        });
        let summary =
            job::run_batch_with(&tasks, settings, Some(threads), INTERVAL, Some(tx), &cancel)
                .unwrap();
        finished.store(true, Ordering::Relaxed);
        let read = reader.join().unwrap();
        stop.store(true, Ordering::Relaxed);
        (summary, read)
    });
    let elapsed = t0.elapsed();
    assert_eq!(summary.total, FILES);
    assert_eq!(
        summary
            .results
            .iter()
            .filter(|r| r.status == FileStatus::Failed)
            .count(),
        0
    );
    let _ = std::fs::remove_dir_all(&out);
    let end_rss = working_set(&mut sys, pid);
    Stats {
        elapsed,
        converted: summary.converted,
        messages: stats_rx.0,
        deepest: stats_rx.1,
        last_done: stats_rx.2,
        peak: peak.load(Ordering::Relaxed),
        start_rss,
        end_rss,
    }
}

struct Stats {
    elapsed: Duration,
    converted: usize,
    messages: usize,
    deepest: usize,
    last_done: usize,
    peak: u64,
    start_rss: u64,
    end_rss: u64,
}

fn report(label: &str, s: &Stats) {
    let secs = s.elapsed.as_secs_f64();
    println!(
        "{label}: {} files in {secs:.1} s ({:.0} files/s), {} progress messages, deepest backlog {}, last done {}, working set {} MB -> peak {} MB -> {} MB",
        s.converted,
        s.converted as f64 / secs,
        s.messages,
        s.deepest,
        s.last_done,
        s.start_rss / 1_048_576,
        s.peak / 1_048_576,
        s.end_rss / 1_048_576,
    );
}

fn check(s: &Stats) {
    let allowed = (s.elapsed.as_millis() / INTERVAL.as_millis()) as usize + 2;
    assert!(s.messages <= allowed, "{} > {allowed}", s.messages);
    assert_eq!(s.last_done, FILES, "the final count must arrive");
    // Worst case per message if the UI thread stalls: a few hundred bytes.
    assert!(
        s.deepest * 2048 < 4 * 1024 * 1024,
        "backlog {} messages",
        s.deepest
    );
}

#[test]
#[ignore = "writes 10,000 files; run explicitly"]
fn ten_thousand_files() {
    let cores = std::thread::available_parallelism().unwrap().get();
    let tmp = tempfile::tempdir().unwrap();
    let t0 = Instant::now();
    write_files(tmp.path());
    println!(
        "wrote {FILES} synthetic DNGs in {:.1} s to the system temp folder, {cores} threads",
        t0.elapsed().as_secs_f64()
    );

    let embedded = ConvertSettings {
        extract_embedded: true,
        ..Default::default()
    };
    let s = run(
        &embedded,
        tmp.path(),
        Some(Duration::from_millis(100)),
        cores,
    );
    report("camera JPEG, reader polling every 100 ms", &s);
    check(&s);

    let develop = ConvertSettings::default();
    let s = run(
        &develop,
        tmp.path(),
        Some(Duration::from_millis(100)),
        cores,
    );
    report("develop, reader polling every 100 ms", &s);
    check(&s);

    // A UI thread that stops reading for the whole batch: the queue is still
    // bounded by time, not by the number of files.
    let s = run(&develop, tmp.path(), None, cores);
    report("develop, reader not polling at all", &s);
    let allowed = (s.elapsed.as_millis() / INTERVAL.as_millis()) as usize + 2;
    assert!(s.deepest <= allowed, "{} > {allowed}", s.deepest);
    // tempdir removal cleans up the 10,000 sources.
}
