//! Background worker for the preview panel. One thread decodes and develops
//! RAW files at reduced resolution (cached per file) and re-applies the
//! adjustment chain on the cached base. Requests go through latest-wins
//! slots, so a moving slider only ever renders its newest value.
//!
//! The pixels come from `ccoonverter_core::preview` / `render`, the same code
//! as a batch conversion. Nothing here runs on the UI thread except posting
//! requests and receiving finished images; a failed preview only produces a
//! message and cannot affect conversion.

use std::collections::VecDeque;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Instant, SystemTime};

use ccoonverter_core::develop::LinearBase;
use ccoonverter_core::preview::{self, PREVIEW_LONG_EDGE};
use ccoonverter_core::render::{render, RenderParams};
use ccoonverter_core::{resize, scan, ConvertError};
use eframe::egui::ColorImage;
use image::RgbImage;

/// Developed bases kept in memory (about 25 MB each at 1600 px).
const CACHE_CAPACITY: usize = 6;

/// What the preview should show. `params` is `None` in embedded mode: then
/// only the camera JPEG is produced.
#[derive(Debug, Clone, PartialEq)]
pub struct RenderRequest {
    pub file: PathBuf,
    pub params: Option<RenderParams>,
    /// Bumped by the panel whenever it clears its images, so the worker
    /// sends the camera JPEG again even for the same file.
    pub session: u64,
}

/// Events from the worker, delivered to the UI thread.
#[derive(Debug)]
pub enum PreviewEvent {
    Scanned {
        root: PathBuf,
        files: Vec<PathBuf>,
        error: Option<String>,
    },
    /// The worker started decoding this file.
    Developing {
        file: PathBuf,
    },
    /// Camera JPEG of the file, or `None` if it has none.
    Embedded {
        file: PathBuf,
        image: Option<Arc<ColorImage>>,
    },
    /// A developed frame. `millis` is the adjustment chain only.
    Developed {
        file: PathBuf,
        image: ColorImage,
        millis: f32,
        /// Id of the request that produced it (used by the coalescing test).
        #[cfg_attr(not(test), allow(dead_code))]
        request: u64,
    },
    Failed {
        file: PathBuf,
        message: String,
    },
}

#[derive(Debug, Default)]
struct Slots {
    scan: Option<PathBuf>,
    render: Option<(u64, RenderRequest)>,
    /// File being decoded and its cancel flag.
    loading: Option<(PathBuf, Arc<AtomicBool>)>,
    shutdown: bool,
}

struct Shared {
    slots: Mutex<Slots>,
    cv: Condvar,
}

impl Shared {
    fn lock(&self) -> std::sync::MutexGuard<'_, Slots> {
        self.slots.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Handle to the worker thread. Dropping it asks the thread to exit.
pub struct PreviewWorker {
    shared: Arc<Shared>,
    events: Receiver<PreviewEvent>,
    handle: Option<JoinHandle<()>>,
    next_id: u64,
}

impl PreviewWorker {
    pub fn spawn(repaint: Box<dyn Fn() + Send + Sync>) -> std::io::Result<Self> {
        let shared = Arc::new(Shared {
            slots: Mutex::new(Slots::default()),
            cv: Condvar::new(),
        });
        let (tx, events) = mpsc::channel();
        let worker_shared = Arc::clone(&shared);
        let handle = std::thread::Builder::new()
            .name("ccoon-preview".into())
            .spawn(move || Worker::new(worker_shared, tx, repaint).run())?;
        Ok(Self {
            shared,
            events,
            handle: Some(handle),
            next_id: 0,
        })
    }

    /// Ask for a folder scan (replaces a pending scan).
    pub fn request_scan(&self, root: PathBuf) {
        let mut s = self.shared.lock();
        s.scan = Some(root);
        self.shared.cv.notify_all();
    }

    /// Ask for a new preview. Replaces any pending render; a running decode
    /// of a different file is cancelled. Returns the request id.
    pub fn request_render(&mut self, req: RenderRequest) -> u64 {
        self.next_id += 1;
        let id = self.next_id;
        let mut s = self.shared.lock();
        if let Some((file, cancel)) = &s.loading {
            if *file != req.file {
                cancel.store(true, Ordering::Relaxed);
            }
        }
        s.render = Some((id, req));
        self.shared.cv.notify_all();
        id
    }

    pub fn drain(&self) -> Vec<PreviewEvent> {
        self.events.try_iter().collect()
    }

    /// Stop the worker. Does not wait for a running decode.
    pub fn shutdown(&mut self) {
        let mut s = self.shared.lock();
        s.shutdown = true;
        if let Some((_, cancel)) = &s.loading {
            cancel.store(true, Ordering::Relaxed);
        }
        self.shared.cv.notify_all();
        drop(s);
        self.handle.take();
    }

    /// Stop the worker and wait for the thread (tests only).
    #[cfg(test)]
    fn join(&mut self) {
        let mut s = self.shared.lock();
        s.shutdown = true;
        self.shared.cv.notify_all();
        drop(s);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for PreviewWorker {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[derive(Default)]
struct CacheEntry {
    modified: Option<SystemTime>,
    base: Option<Result<Arc<LinearBase>, String>>,
    embedded: Option<Option<Arc<ColorImage>>>,
}

struct Worker {
    shared: Arc<Shared>,
    tx: Sender<PreviewEvent>,
    repaint: Box<dyn Fn() + Send + Sync>,
    cache: VecDeque<(PathBuf, CacheEntry)>,
    last_view: Option<(PathBuf, u64)>,
}

enum Job {
    Scan(PathBuf),
    Render(u64, RenderRequest),
    Stop,
}

impl Worker {
    fn new(
        shared: Arc<Shared>,
        tx: Sender<PreviewEvent>,
        repaint: Box<dyn Fn() + Send + Sync>,
    ) -> Self {
        Self {
            shared,
            tx,
            repaint,
            cache: VecDeque::new(),
            last_view: None,
        }
    }

    fn send(&self, e: PreviewEvent) {
        let _ = self.tx.send(e);
        (self.repaint)();
    }

    fn next_job(&self) -> Job {
        let mut s = self.shared.lock();
        loop {
            if s.shutdown {
                return Job::Stop;
            }
            if let Some(root) = s.scan.take() {
                return Job::Scan(root);
            }
            if let Some((id, req)) = s.render.take() {
                return Job::Render(id, req);
            }
            s = self.shared.cv.wait(s).unwrap_or_else(|e| e.into_inner());
        }
    }

    fn run(mut self) {
        loop {
            match self.next_job() {
                Job::Stop => break,
                Job::Scan(root) => self.do_scan(root),
                Job::Render(id, req) => self.do_render(id, req),
            }
        }
        log::debug!("preview worker stopped");
    }

    fn do_scan(&self, root: PathBuf) {
        let (files, error) = if root.is_dir() {
            match scan::scan_folder(&root) {
                Ok(f) => (f, None),
                Err(e) => (Vec::new(), Some(e.to_string())),
            }
        } else {
            (Vec::new(), None)
        };
        self.send(PreviewEvent::Scanned { root, files, error });
    }

    /// True if a pending request supersedes the running one.
    fn is_superseded(&self, file: &Path) -> bool {
        let s = self.shared.lock();
        s.shutdown || s.render.as_ref().is_some_and(|(_, r)| r.file != file)
    }

    fn start_load(&self, file: &Path) -> Arc<AtomicBool> {
        let cancel = Arc::new(AtomicBool::new(false));
        self.shared.lock().loading = Some((file.to_path_buf(), Arc::clone(&cancel)));
        cancel
    }

    fn end_load(&self) {
        self.shared.lock().loading = None;
    }

    fn entry_index(&mut self, file: &Path) -> usize {
        let modified = std::fs::metadata(file).and_then(|m| m.modified()).ok();
        if let Some(i) = self.cache.iter().position(|(p, _)| p == file) {
            if self.cache[i].1.modified != modified {
                self.cache[i].1 = CacheEntry {
                    modified,
                    ..Default::default()
                };
            }
            return i;
        }
        if self.cache.len() >= CACHE_CAPACITY {
            self.cache.pop_front();
        }
        self.cache.push_back((
            file.to_path_buf(),
            CacheEntry {
                modified,
                ..Default::default()
            },
        ));
        self.cache.len() - 1
    }

    /// Mark an entry as most recently used.
    fn touch(&mut self, index: usize) {
        if let Some(e) = self.cache.remove(index) {
            self.cache.push_back(e);
        }
    }

    fn do_render(&mut self, id: u64, mut req: RenderRequest) {
        let mut id = id;
        let index = self.entry_index(&req.file);
        self.touch(index);
        let index = self.cache.len() - 1;
        let file = req.file.clone();

        // Camera JPEG first: it is fast, and doubles as a loading image.
        if self.cache[index].1.embedded.is_none() {
            let cancel = self.start_load(&file);
            let res = catch_unwind(AssertUnwindSafe(|| {
                preview::load_embedded(&file, PREVIEW_LONG_EDGE, &cancel)
            }));
            self.end_load();
            let image = match res {
                Ok(Ok(Some(img))) => Some(Arc::new(to_color_image(&img))),
                Ok(Ok(None)) => None,
                Ok(Err(ConvertError::Cancelled)) => return,
                Ok(Err(e)) => {
                    log::warn!("Preview: embedded JPEG of {} failed: {e}", file.display());
                    None
                }
                Err(_) => {
                    log::warn!("Preview: decoder panicked on {}", file.display());
                    None
                }
            };
            self.cache[index].1.embedded = Some(image);
            self.last_view = None; // force resend below
        }
        let view = (file.clone(), req.session);
        if self.last_view.as_ref() != Some(&view) {
            let image = self.cache[index].1.embedded.clone().flatten();
            self.send(PreviewEvent::Embedded {
                file: file.clone(),
                image,
            });
        }
        self.last_view = Some(view);

        if req.params.is_none() {
            return; // embedded mode
        }

        if self.cache[index].1.base.is_none() {
            if self.is_superseded(&file) {
                return;
            }
            self.send(PreviewEvent::Developing { file: file.clone() });
            let cancel = self.start_load(&file);
            let res = catch_unwind(AssertUnwindSafe(|| {
                preview::load_base(&file, PREVIEW_LONG_EDGE, &cancel)
            }));
            self.end_load();
            match res {
                Ok(Ok(base)) => self.cache[index].1.base = Some(Ok(Arc::new(base))),
                Ok(Err(ConvertError::Cancelled)) => return,
                Ok(Err(e)) => self.cache[index].1.base = Some(Err(e.to_string())),
                Err(_) => {
                    self.cache[index].1.base = Some(Err("the RAW decoder panicked".into()));
                }
            }
            // Use the newest request for this file instead of rendering the
            // stale settings of the one that started the load.
            let mut s = self.shared.lock();
            if let Some((newer_id, newer)) = s.render.take() {
                if newer.file == file {
                    id = newer_id;
                    req = newer;
                } else {
                    s.render = Some((newer_id, newer));
                    return;
                }
            }
        }

        let base = match self.cache[index].1.base.as_ref().expect("set above") {
            Ok(b) => Arc::clone(b),
            Err(msg) => {
                let message = short_message(msg);
                self.send(PreviewEvent::Failed { file, message });
                return;
            }
        };
        let Some(params) = req.params else {
            return;
        };
        let cancel = AtomicBool::new(false);
        let t0 = Instant::now();
        let frame = catch_unwind(AssertUnwindSafe(|| {
            render(base.image.clone(), &base.stats, &params, 0, &cancel).map(|f| {
                let img8 = resize::quantize(&f);
                to_color_image(&img8)
            })
        }));
        let millis = t0.elapsed().as_secs_f32() * 1000.0;
        match frame {
            Ok(Ok(image)) => self.send(PreviewEvent::Developed {
                file,
                image,
                millis,
                request: id,
            }),
            Ok(Err(e)) => self.send(PreviewEvent::Failed {
                file,
                message: short_message(&e.to_string()),
            }),
            Err(_) => self.send(PreviewEvent::Failed {
                file,
                message: "the preview renderer panicked".into(),
            }),
        }
    }
}

fn to_color_image(img: &RgbImage) -> ColorImage {
    ColorImage::from_rgb([img.width() as usize, img.height() as usize], img.as_raw())
}

/// Reduce an error to one short line for the panel.
pub fn short_message(msg: &str) -> String {
    let line = msg.lines().next().unwrap_or("").trim();
    let mut out: String = line.chars().take(90).collect();
    if line.chars().count() > 90 {
        out.push('…');
    }
    if out.is_empty() {
        out.push_str("unknown error");
    }
    out
}

/// The request the panel wants right now, or `None` if it has no file.
pub fn desired_request(
    files: &[PathBuf],
    index: usize,
    settings: &ccoonverter_core::ConvertSettings,
) -> Option<RenderRequest> {
    let file = files.get(index)?.clone();
    let params = (!settings.extract_embedded).then(|| RenderParams::from(settings));
    Some(RenderRequest {
        file,
        params,
        session: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ccoonverter_core::testutil::{synthetic_dng, DngSpec};
    use ccoonverter_core::{ConvertSettings, PictureStyle};
    use std::time::Duration;

    fn worker() -> PreviewWorker {
        PreviewWorker::spawn(Box::new(|| {})).unwrap()
    }

    fn params(s: &ConvertSettings) -> Option<RenderParams> {
        Some(RenderParams::from(s))
    }

    /// Drain events until `pred` matches one, or fail on timeout.
    fn wait_for(
        w: &PreviewWorker,
        mut pred: impl FnMut(&PreviewEvent) -> bool,
    ) -> Vec<PreviewEvent> {
        let deadline = Instant::now() + Duration::from_secs(60);
        let mut seen = Vec::new();
        loop {
            for e in w.drain() {
                let hit = pred(&e);
                seen.push(e);
                if hit {
                    return seen;
                }
            }
            assert!(Instant::now() < deadline, "timeout, saw {seen:?}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn develops_embedded_then_frame() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("a.DNG");
        std::fs::write(&f, synthetic_dng(&DngSpec::default())).unwrap();
        let mut w = worker();
        let id = w.request_render(RenderRequest {
            file: f.clone(),
            params: params(&ConvertSettings::default()),
            session: 0,
        });
        let seen = wait_for(&w, |e| matches!(e, PreviewEvent::Developed { .. }));
        match seen
            .iter()
            .find(|e| matches!(e, PreviewEvent::Embedded { .. }))
        {
            Some(PreviewEvent::Embedded { image, .. }) => {
                assert!(image.is_some(), "synthetic DNG has a preview")
            }
            other => panic!("no embedded event: {other:?}"),
        }
        let Some(PreviewEvent::Developed { request, image, .. }) = seen.last() else {
            panic!()
        };
        assert_eq!(*request, id);
        assert_eq!(image.size, [64, 48]);
        w.join();
    }

    #[test]
    fn burst_of_slider_changes_keeps_only_the_latest() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("a.DNG");
        let spec = DngSpec {
            width: 600,
            height: 400,
            ..Default::default()
        };
        std::fs::write(&f, synthetic_dng(&spec)).unwrap();
        let mut w = worker();
        // Slider dragged while the file is still decoding.
        let mut last = 0;
        for i in 0..200 {
            let s = ConvertSettings {
                exposure_ev: (i as f32 / 199.0) * 3.0,
                ..Default::default()
            };
            last = w.request_render(RenderRequest {
                file: f.clone(),
                params: params(&s),
                session: 0,
            });
        }
        let seen = wait_for(
            &w,
            |e| matches!(e, PreviewEvent::Developed { request, .. } if *request == last),
        );
        let frames = seen
            .iter()
            .filter(|e| matches!(e, PreviewEvent::Developed { .. }))
            .count();
        assert!(frames < 200, "requests were not coalesced: {frames}");
        assert!(frames >= 1);
        w.join();
    }

    #[test]
    fn cached_base_is_reused_after_navigating_away() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.DNG");
        let b = dir.path().join("b.DNG");
        let dng = synthetic_dng(&DngSpec::default());
        std::fs::write(&a, &dng).unwrap();
        std::fs::write(&b, &dng).unwrap();
        let mut w = worker();
        let d = params(&ConvertSettings::default());
        let req = |f: &Path| RenderRequest {
            file: f.to_path_buf(),
            params: d.clone(),
            session: 0,
        };
        w.request_render(req(&a));
        wait_for(
            &w,
            |e| matches!(e, PreviewEvent::Developed { file, .. } if *file == a),
        );
        w.request_render(req(&b));
        wait_for(
            &w,
            |e| matches!(e, PreviewEvent::Developed { file, .. } if *file == b),
        );
        // Back to the first file: no "Developing" event, the base is cached.
        w.request_render(req(&a));
        let seen = wait_for(
            &w,
            |e| matches!(e, PreviewEvent::Developed { file, .. } if *file == a),
        );
        assert!(!seen
            .iter()
            .any(|e| matches!(e, PreviewEvent::Developing { .. })));
        w.join();
    }

    #[test]
    fn unsupported_file_reports_failure_and_worker_survives() {
        let dir = tempfile::tempdir().unwrap();
        let bad = dir.path().join("bad.NEF");
        std::fs::write(&bad, vec![0u8; 512]).unwrap();
        let good = dir.path().join("g.DNG");
        std::fs::write(&good, synthetic_dng(&DngSpec::default())).unwrap();
        let mut w = worker();
        w.request_render(RenderRequest {
            file: bad.clone(),
            params: params(&ConvertSettings::default()),
            session: 0,
        });
        let seen = wait_for(&w, |e| matches!(e, PreviewEvent::Failed { .. }));
        assert!(matches!(seen.last(), Some(PreviewEvent::Failed { .. })));
        w.request_render(RenderRequest {
            file: good,
            params: params(&ConvertSettings::default()),
            session: 0,
        });
        wait_for(&w, |e| matches!(e, PreviewEvent::Developed { .. }));
        w.join();
    }

    #[test]
    fn embedded_mode_produces_only_the_camera_jpeg() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("a.DNG");
        std::fs::write(&f, synthetic_dng(&DngSpec::default())).unwrap();
        let mut w = worker();
        w.request_render(RenderRequest {
            file: f,
            params: None,
            session: 0,
        });
        let seen = wait_for(&w, |e| matches!(e, PreviewEvent::Embedded { .. }));
        assert!(!seen.iter().any(|e| matches!(
            e,
            PreviewEvent::Developed { .. } | PreviewEvent::Developing { .. }
        )));
        w.join();
    }

    #[test]
    fn scan_reports_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.DNG"), synthetic_dng(&DngSpec::default())).unwrap();
        std::fs::write(dir.path().join("skip.txt"), "x").unwrap();
        let mut w = worker();
        w.request_scan(dir.path().to_path_buf());
        let seen = wait_for(&w, |e| matches!(e, PreviewEvent::Scanned { .. }));
        let Some(PreviewEvent::Scanned { files, error, .. }) = seen.last() else {
            panic!()
        };
        assert_eq!(files.len(), 1);
        assert!(error.is_none());
        w.join();
    }

    #[test]
    fn desired_request_tracks_mode() {
        let files = vec![PathBuf::from("a.DNG"), PathBuf::from("b.DNG")];
        let s = ConvertSettings {
            style: PictureStyle::Vivid,
            ..Default::default()
        };
        let r = desired_request(&files, 1, &s).unwrap();
        assert_eq!(r.file, PathBuf::from("b.DNG"));
        assert_eq!(r.params, Some(RenderParams::from(&s)));
        let e = ConvertSettings {
            extract_embedded: true,
            ..Default::default()
        };
        assert_eq!(desired_request(&files, 0, &e).unwrap().params, None);
        assert_eq!(desired_request(&files, 5, &e), None);
    }

    #[test]
    fn long_errors_are_shortened() {
        let long = "x".repeat(300);
        assert!(short_message(&long).chars().count() <= 91);
        assert_eq!(short_message("first\nsecond"), "first");
        assert_eq!(short_message(""), "unknown error");
    }
}
