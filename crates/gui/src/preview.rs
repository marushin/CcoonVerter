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
use std::time::SystemTime;

use ccoonverter_core::develop::LinearBase;
use ccoonverter_core::preview::{self, PREVIEW_LONG_EDGE};
use ccoonverter_core::render::{render, render_region, Region, RenderParams};
use ccoonverter_core::{resize, scan, ConvertError, PictureStyle, RgbF32Image};
use eframe::egui::ColorImage;
use image::RgbImage;

/// Developed bases kept in memory (about 25 MB each at 1600 px).
const CACHE_CAPACITY: usize = 6;
/// Long edge of the style thumbnails, in pixels.
const THUMB_EDGE: u32 = 200;
/// Extra pixels developed on each side of the 100 % view, so that short pans
/// stay on already developed pixels.
const ZOOM_OVERSCAN: u32 = 256;
/// The histogram samples about this many pixels.
const HISTOGRAM_SAMPLES: usize = 250_000;
pub const HISTOGRAM_BINS: usize = 64;

/// Share of a developed image in each brightness bin, per channel.
#[derive(Debug, Clone, PartialEq)]
pub struct Histogram {
    pub channels: [[u32; HISTOGRAM_BINS]; 3],
}

impl Histogram {
    pub fn of(image: &ColorImage) -> Self {
        let mut channels = [[0u32; HISTOGRAM_BINS]; 3];
        let step = (image.pixels.len() / HISTOGRAM_SAMPLES.max(1)).max(1);
        for p in image.pixels.iter().step_by(step) {
            for (c, v) in [p.r(), p.g(), p.b()].into_iter().enumerate() {
                channels[c][usize::from(v) * HISTOGRAM_BINS / 256] += 1;
            }
        }
        Self { channels }
    }
}

/// Pixel region shown in a 100 % view of `view_px` pixels around `center`
/// (0..1 on the whole image), as the image position of the view's top-left
/// corner. When the image is smaller than the view it is centred, which gives
/// a negative origin.
pub fn view_origin(full_size: [u32; 2], center: [f32; 2], view_px: [u32; 2]) -> [i64; 2] {
    [0, 1].map(|i| {
        let (full, view) = (i64::from(full_size[i]), i64::from(view_px[i]));
        if full <= view {
            -((view - full) / 2)
        } else {
            let wanted = (f64::from(center[i]) * full as f64 - view as f64 / 2.0).round() as i64;
            wanted.clamp(0, full - view)
        }
    })
}

/// The region to develop for a 100 % view: the view plus an overscan margin,
/// never larger than the GPU's largest texture.
pub fn zoom_region(
    full_size: [u32; 2],
    center: [f32; 2],
    view_px: [u32; 2],
    max_side: u32,
) -> Region {
    let origin = view_origin(full_size, center, view_px);
    let span = |i: usize| {
        let full = i64::from(full_size[i]);
        let view = i64::from(view_px[i]);
        let view_start = origin[i].max(0);
        let view_end = (origin[i] + view).min(full);
        let start = (view_start - i64::from(ZOOM_OVERSCAN)).clamp(0, full);
        let end = (view_end + i64::from(ZOOM_OVERSCAN)).clamp(start, full);
        // Too big for one texture: keep the visible part, centred.
        let max = i64::from(max_side);
        let (start, end) = if end - start > max {
            let middle = (view_start + view_end) / 2;
            let start = (middle - max / 2).clamp(0, (full - max).max(0));
            (start, (start + max).min(full))
        } else {
            (start, end)
        };
        (start as u32, (end - start) as u32)
    };
    let ((x, w), (y, h)) = (span(0), span(1));
    Region { x, y, w, h }
}

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

/// What a 100 % view needs: a region at full resolution.
#[derive(Debug, Clone, PartialEq)]
pub struct ZoomRequest {
    pub file: PathBuf,
    /// `None` in camera JPEG mode.
    pub params: Option<RenderParams>,
    /// Centre of the view, 0..1 on the whole image.
    pub center: [f32; 2],
    /// Size of the view in physical pixels.
    pub view_px: [u32; 2],
    /// Also crop the camera JPEG (split compare, camera JPEG mode).
    pub want_camera: bool,
    /// Largest texture side the GPU accepts.
    pub max_side: u32,
}

/// Style thumbnails for a file. The style inside `params` is ignored.
#[derive(Debug, Clone, PartialEq)]
pub struct ThumbRequest {
    pub file: PathBuf,
    pub params: RenderParams,
}

impl ThumbRequest {
    /// Two requests with equal keys produce the same thumbnails.
    pub fn new(file: PathBuf, params: &RenderParams) -> Self {
        Self {
            file,
            params: RenderParams {
                style: PictureStyle::Standard,
                ..params.clone()
            },
        }
    }
}

/// One rendered style thumbnail.
pub type Thumbnail = (PictureStyle, Arc<ColorImage>);

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
        histogram: Option<Histogram>,
    },
    /// A developed frame.
    Developed {
        file: PathBuf,
        image: ColorImage,
        histogram: Histogram,
        /// Upright size of the full-resolution image.
        full_size: [u32; 2],
        /// Id of the request that produced it (used by the coalescing test).
        #[cfg_attr(not(test), allow(dead_code))]
        request: u64,
    },
    /// Full-resolution pixels for a 100 % view.
    Zoomed {
        file: PathBuf,
        full_size: [u32; 2],
        region: Region,
        /// Developed pixels of `region`; `None` in camera JPEG mode.
        developed: Option<ColorImage>,
        /// Camera JPEG pixels covering the same part of the picture.
        camera: Option<ColorImage>,
    },
    /// The 100 % view cannot be shown for this file. The whole-photo preview
    /// is not affected.
    ZoomFailed {
        file: PathBuf,
        message: String,
    },
    /// All style thumbnails of a file.
    Thumbnails {
        request: ThumbRequest,
        images: Vec<Thumbnail>,
    },
    Failed {
        file: PathBuf,
        message: String,
    },
}

/// A decode in progress.
#[derive(Debug)]
struct Loading {
    file: PathBuf,
    cancel: Arc<AtomicBool>,
    /// Decoding for the 100 % view, not for the normal preview.
    full_res: bool,
}

#[derive(Debug, Default)]
struct Slots {
    scan: Option<PathBuf>,
    render: Option<(u64, RenderRequest)>,
    zoom: Option<(u64, ZoomRequest)>,
    thumbs: Option<ThumbRequest>,
    /// Drop the full-resolution image at the next opportunity.
    release_full: bool,
    loading: Option<Loading>,
    shutdown: bool,
}

struct Shared {
    slots: Mutex<Slots>,
    cv: Condvar,
    /// Full-resolution releases carried out, for tests.
    #[cfg(test)]
    full_releases: std::sync::atomic::AtomicUsize,
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
            #[cfg(test)]
            full_releases: Default::default(),
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
        if let Some(l) = s.loading.as_ref().filter(|l| l.file != req.file) {
            l.cancel.store(true, Ordering::Relaxed);
        }
        s.render = Some((id, req));
        self.shared.cv.notify_all();
        id
    }

    /// Ask for full-resolution pixels of a 100 % view (latest wins).
    pub fn request_zoom(&mut self, req: ZoomRequest) -> u64 {
        self.next_id += 1;
        let id = self.next_id;
        let mut s = self.shared.lock();
        if let Some(l) = s.loading.as_ref().filter(|l| l.file != req.file) {
            l.cancel.store(true, Ordering::Relaxed);
        }
        s.zoom = Some((id, req));
        self.shared.cv.notify_all();
        id
    }

    /// Drop the full-resolution image and any pending zoom request. It uses
    /// hundreds of megabytes, so it is only kept while a 100 % view is open.
    /// A full-resolution decode that is still running is cancelled too; a
    /// decode for the normal preview is left alone.
    pub fn release_full(&self) {
        let mut s = self.shared.lock();
        s.zoom = None;
        s.release_full = true;
        if let Some(l) = s.loading.as_ref().filter(|l| l.full_res) {
            l.cancel.store(true, Ordering::Relaxed);
        }
        self.shared.cv.notify_all();
    }

    #[cfg(test)]
    pub fn full_releases(&self) -> usize {
        self.shared.full_releases.load(Ordering::SeqCst)
    }

    /// Ask for the style thumbnails. Low priority: they run only when no
    /// preview update is waiting.
    pub fn request_thumbnails(&self, req: ThumbRequest) {
        let mut s = self.shared.lock();
        s.thumbs = Some(req);
        self.shared.cv.notify_all();
    }

    pub fn drain(&self) -> Vec<PreviewEvent> {
        self.events.try_iter().collect()
    }

    /// Stop the worker. Does not wait for a running decode.
    pub fn shutdown(&mut self) {
        let mut s = self.shared.lock();
        s.shutdown = true;
        if let Some(l) = &s.loading {
            l.cancel.store(true, Ordering::Relaxed);
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
    embedded_histogram: Option<Histogram>,
    /// Small copy of the base, the source of the style thumbnails.
    thumb_base: Option<Arc<RgbF32Image>>,
    thumbs: Option<(ThumbRequest, Vec<Thumbnail>)>,
}

/// The full-resolution data of the one file being viewed at 100 %.
#[derive(Default)]
struct FullCache {
    file: Option<PathBuf>,
    base: Option<Result<Arc<LinearBase>, String>>,
    camera: Option<Option<Arc<RgbImage>>>,
}

struct Worker {
    shared: Arc<Shared>,
    tx: Sender<PreviewEvent>,
    repaint: Box<dyn Fn() + Send + Sync>,
    cache: VecDeque<(PathBuf, CacheEntry)>,
    last_view: Option<(PathBuf, u64)>,
    full: FullCache,
}

enum Job {
    Scan(PathBuf),
    Render(u64, RenderRequest),
    Zoom(ZoomRequest),
    Thumbs(ThumbRequest),
    ReleaseFull,
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
            full: FullCache::default(),
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
            if s.release_full {
                s.release_full = false;
                return Job::ReleaseFull;
            }
            if let Some((_, req)) = s.zoom.take() {
                return Job::Zoom(req);
            }
            if let Some(req) = s.thumbs.take() {
                return Job::Thumbs(req);
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
                Job::Zoom(req) => self.do_zoom(req),
                Job::Thumbs(req) => self.do_thumbs(req),
                Job::ReleaseFull => {
                    self.full = FullCache::default();
                    #[cfg(test)]
                    self.shared.full_releases.fetch_add(1, Ordering::SeqCst);
                }
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
        s.shutdown
            || s.render.as_ref().is_some_and(|(_, r)| r.file != file)
            || s.zoom.as_ref().is_some_and(|(_, z)| z.file != file)
    }

    fn start_load(&self, file: &Path, full_res: bool) -> Arc<AtomicBool> {
        let cancel = Arc::new(AtomicBool::new(false));
        self.shared.lock().loading = Some(Loading {
            file: file.to_path_buf(),
            cancel: Arc::clone(&cancel),
            full_res,
        });
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
            let cancel = self.start_load(&file, false);
            let res = catch_unwind(AssertUnwindSafe(|| {
                preview::load_embedded(&file, PREVIEW_LONG_EDGE, &cancel)
            }));
            self.end_load();
            let image = match res {
                Ok(Ok(Some(img))) => {
                    let image = to_color_image(&img);
                    self.cache[index].1.embedded_histogram = Some(Histogram::of(&image));
                    Some(Arc::new(image))
                }
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
            let histogram = self.cache[index].1.embedded_histogram.clone();
            self.send(PreviewEvent::Embedded {
                file: file.clone(),
                image,
                histogram,
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
            let cancel = self.start_load(&file, false);
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
        let frame = catch_unwind(AssertUnwindSafe(|| {
            render(base.image.clone(), &base.stats, &params, 0, &cancel).map(|f| {
                let img8 = resize::quantize(&f);
                to_color_image(&img8)
            })
        }));
        match frame {
            Ok(Ok(image)) => {
                let histogram = Histogram::of(&image);
                self.send(PreviewEvent::Developed {
                    file,
                    image,
                    histogram,
                    full_size: [base.full_size.0, base.full_size.1],
                    request: id,
                });
            }
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

    /// Load the full-resolution base and camera JPEG the request needs.
    /// `false` when the request should be dropped.
    fn load_full(&mut self, req: &ZoomRequest) -> bool {
        if self.full.file.as_deref() != Some(req.file.as_path()) {
            self.full = FullCache {
                file: Some(req.file.clone()),
                ..Default::default()
            };
        }
        let file = req.file.clone();
        if req.params.is_some() && self.full.base.is_none() {
            if self.is_superseded(&file) {
                return false;
            }
            let cancel = self.start_load(&file, true);
            let res = catch_unwind(AssertUnwindSafe(|| {
                preview::load_base(&file, u32::MAX, &cancel)
            }));
            self.end_load();
            self.full.base = Some(match res {
                Ok(Ok(base)) => Ok(Arc::new(base)),
                Ok(Err(ConvertError::Cancelled)) => return false,
                Ok(Err(e)) => Err(e.to_string()),
                Err(_) => Err("the RAW decoder panicked".into()),
            });
        }
        if (req.want_camera || req.params.is_none()) && self.full.camera.is_none() {
            if self.is_superseded(&file) {
                return false;
            }
            let cancel = self.start_load(&file, true);
            let res = catch_unwind(AssertUnwindSafe(|| {
                preview::load_embedded(&file, u32::MAX, &cancel)
            }));
            self.end_load();
            self.full.camera = Some(match res {
                Ok(Ok(img)) => img.map(Arc::new),
                Ok(Err(ConvertError::Cancelled)) => return false,
                Ok(Err(e)) => {
                    log::warn!("Preview: camera JPEG of {} failed: {e}", file.display());
                    None
                }
                Err(_) => None,
            });
        }
        true
    }

    fn do_zoom(&mut self, req: ZoomRequest) {
        if !self.load_full(&req) {
            return;
        }
        // A newer request for the same file replaces this one: the decode
        // above may have taken a while.
        let req = {
            let mut s = self.shared.lock();
            match s.zoom.take() {
                Some((_, newer)) if newer.file == req.file => newer,
                other => {
                    s.zoom = other;
                    req
                }
            }
        };
        if !self.load_full(&req) {
            return;
        }
        let base = match (&req.params, &self.full.base) {
            (Some(_), Some(Ok(b))) => Some(Arc::clone(b)),
            (Some(_), Some(Err(msg))) => {
                let message = short_message(msg);
                self.send(PreviewEvent::ZoomFailed {
                    file: req.file,
                    message,
                });
                return;
            }
            _ => None,
        };
        let camera = self.full.camera.clone().flatten();
        let full_size = match (&base, &camera) {
            (Some(b), _) => b.full_size,
            (None, Some(c)) => c.dimensions(),
            (None, None) => {
                self.send(PreviewEvent::ZoomFailed {
                    file: req.file,
                    message: "no camera JPEG".into(),
                });
                return;
            }
        };
        let full_size = [full_size.0, full_size.1];
        let region = zoom_region(full_size, req.center, req.view_px, req.max_side);
        let cancel = AtomicBool::new(false);
        let result = catch_unwind(AssertUnwindSafe(|| match (&base, &req.params) {
            (Some(b), Some(p)) => render_region(&b.image, &b.stats, p, region, &cancel)
                .map(|img| Some(to_color_image(&resize::quantize(&img)))),
            _ => Ok(None),
        }));
        let developed = match result {
            Ok(Ok(img)) => img,
            Ok(Err(e)) => {
                let message = short_message(&e.to_string());
                self.send(PreviewEvent::ZoomFailed {
                    file: req.file,
                    message,
                });
                return;
            }
            Err(_) => {
                self.send(PreviewEvent::ZoomFailed {
                    file: req.file,
                    message: "the preview renderer panicked".into(),
                });
                return;
            }
        };
        let camera = camera
            .filter(|_| req.want_camera)
            .map(|c| crop_camera(&c, full_size, region));
        self.send(PreviewEvent::Zoomed {
            file: req.file,
            full_size,
            region,
            developed,
            camera,
        });
    }

    fn render_pending(&self) -> bool {
        let s = self.shared.lock();
        s.shutdown || s.render.is_some() || s.zoom.is_some()
    }

    /// All 12 styles through the real chain on a small copy of the base.
    fn do_thumbs(&mut self, req: ThumbRequest) {
        let Some(index) = self.cache.iter().position(|(p, _)| *p == req.file) else {
            return; // not developed yet; the panel asks again
        };
        if let Some((done, images)) = &self.cache[index].1.thumbs {
            if *done == req {
                let event = PreviewEvent::Thumbnails {
                    request: req,
                    images: images.clone(),
                };
                self.send(event);
                return;
            }
        }
        let Some(Ok(base)) = self.cache[index].1.base.clone() else {
            return;
        };
        let small = match &self.cache[index].1.thumb_base {
            Some(small) => Arc::clone(small),
            None => {
                let small = Arc::new(resize::reduce_long_edge_f32(base.image.clone(), THUMB_EDGE));
                self.cache[index].1.thumb_base = Some(Arc::clone(&small));
                small
            }
        };
        let cancel = AtomicBool::new(false);
        let mut images = Vec::with_capacity(PictureStyle::ALL.len());
        for style in PictureStyle::ALL {
            if self.render_pending() {
                return; // the panel asks again once the slider has settled
            }
            let params = RenderParams {
                style,
                ..req.params.clone()
            };
            let rendered = catch_unwind(AssertUnwindSafe(|| {
                render((*small).clone(), &base.stats, &params, 0, &cancel)
            }));
            let Ok(Ok(img)) = rendered else {
                return;
            };
            images.push((style, Arc::new(to_color_image(&resize::quantize(&img)))));
        }
        self.cache[index].1.thumbs = Some((req.clone(), images.clone()));
        self.send(PreviewEvent::Thumbnails {
            request: req,
            images,
        });
    }
}

/// The part of the camera JPEG showing the same part of the picture as
/// `region` of the developed image. Sizes may differ slightly.
fn crop_camera(camera: &RgbImage, full_size: [u32; 2], region: Region) -> ColorImage {
    let (cw, ch) = camera.dimensions();
    let map = |v: u32, full: u32, size: u32| {
        (u64::from(v) * u64::from(size) / u64::from(full.max(1))) as u32
    };
    let x0 = map(region.x, full_size[0], cw);
    let y0 = map(region.y, full_size[1], ch);
    let x1 = map(region.x + region.w, full_size[0], cw).clamp(x0 + 1, cw.max(x0 + 1));
    let y1 = map(region.y + region.h, full_size[1], ch).clamp(y0 + 1, ch.max(y0 + 1));
    let w = (x1 - x0).min(cw - x0.min(cw));
    let h = (y1 - y0).min(ch - y0.min(ch));
    to_color_image(&image::imageops::crop_imm(camera, x0, y0, w, h).to_image())
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
    // The panel draws on an sRGB display whatever the output colour space is.
    let params = (!settings.extract_embedded).then(|| RenderParams::for_display(settings));
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
    use std::time::{Duration, Instant};

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
    fn view_is_clamped_to_the_image_and_centred_when_smaller() {
        // Image larger than the view.
        assert_eq!(
            view_origin([6000, 4000], [0.5, 0.5], [1000, 800]),
            [2500, 1600]
        );
        assert_eq!(
            view_origin([6000, 4000], [0.0, 1.0], [1000, 800]),
            [0, 3200]
        );
        // Image smaller than the view: centred, negative origin.
        assert_eq!(
            view_origin([500, 400], [0.5, 0.5], [1000, 800]),
            [-250, -200]
        );
    }

    #[test]
    fn zoom_region_covers_the_view_with_overscan_and_stays_inside() {
        let r = zoom_region([6000, 4000], [0.5, 0.5], [1000, 800], 16384);
        assert_eq!((r.x, r.y, r.w, r.h), (2244, 1344, 1512, 1312));
        let r = zoom_region([6000, 4000], [0.0, 0.0], [1000, 800], 16384);
        assert_eq!((r.x, r.y, r.w, r.h), (0, 0, 1256, 1056));
        let r = zoom_region([500, 400], [0.5, 0.5], [1000, 800], 16384);
        assert_eq!((r.x, r.y, r.w, r.h), (0, 0, 500, 400));
        // A small GPU limit keeps the middle of the view and fits one texture.
        let r = zoom_region([6000, 4000], [0.5, 0.5], [3000, 2000], 2048);
        assert_eq!((r.x, r.y, r.w, r.h), (1976, 976, 2048, 2048));
    }

    #[test]
    fn histogram_sums_to_the_sampled_pixels() {
        let img = ColorImage::from_rgb([4, 2], &[255u8; 4 * 2 * 3]);
        let h = Histogram::of(&img);
        for c in h.channels {
            assert_eq!(c.iter().sum::<u32>(), 8);
            assert_eq!(c[HISTOGRAM_BINS - 1], 8);
        }
    }

    #[test]
    fn zoom_returns_full_resolution_pixels_of_the_chain() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("a.DNG");
        let spec = DngSpec {
            width: 320,
            height: 240,
            ..Default::default()
        };
        std::fs::write(&f, synthetic_dng(&spec)).unwrap();
        let mut w = worker();
        let s = ConvertSettings::default();
        w.request_zoom(ZoomRequest {
            file: f.clone(),
            params: params(&s),
            center: [0.5, 0.5],
            view_px: [100, 80],
            want_camera: true,
            max_side: 16384,
        });
        let seen = wait_for(&w, |e| matches!(e, PreviewEvent::Zoomed { .. }));
        let Some(PreviewEvent::Zoomed {
            full_size,
            region,
            developed,
            camera,
            ..
        }) = seen.last()
        else {
            panic!()
        };
        assert_eq!(*full_size, [320, 240]);
        assert_eq!(developed.as_ref().unwrap().size, [320, 240]);
        assert_eq!((region.w, region.h), (320, 240));
        assert!(camera.is_some(), "the synthetic DNG has a camera preview");
        w.join();
    }

    #[test]
    fn thumbnails_cover_every_style_and_are_cached_for_the_same_settings() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("a.DNG");
        let spec = DngSpec {
            width: 320,
            height: 240,
            ..Default::default()
        };
        std::fs::write(&f, synthetic_dng(&spec)).unwrap();
        let mut w = worker();
        let settings = ConvertSettings::default();
        w.request_render(RenderRequest {
            file: f.clone(),
            params: params(&settings),
            session: 0,
        });
        wait_for(&w, |e| matches!(e, PreviewEvent::Developed { .. }));
        let req = ThumbRequest::new(f.clone(), &RenderParams::from(&settings));
        w.request_thumbnails(req.clone());
        let seen = wait_for(&w, |e| matches!(e, PreviewEvent::Thumbnails { .. }));
        let Some(PreviewEvent::Thumbnails { images, .. }) = seen.last() else {
            panic!()
        };
        assert_eq!(images.len(), 12);
        assert!(images[0].1.size[0] <= THUMB_EDGE as usize);
        let first = Arc::clone(&images[0].1);
        // The same request again comes straight from the cache.
        w.request_thumbnails(req);
        let seen = wait_for(&w, |e| matches!(e, PreviewEvent::Thumbnails { .. }));
        let Some(PreviewEvent::Thumbnails { images, .. }) = seen.last() else {
            panic!()
        };
        assert!(Arc::ptr_eq(&first, &images[0].1), "served from the cache");
        w.join();
    }

    /// Worker-side costs on the real samples. Run in release:
    /// `cargo test --release -p ccoonverter -- --ignored --nocapture worker_timing`
    #[test]
    #[ignore]
    fn worker_timing_on_real_samples() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/samples");
        let Ok(files) = scan::scan_folder(&dir) else {
            return;
        };
        let cancel = AtomicBool::new(false);
        let ms = |t: Instant| t.elapsed().as_secs_f64() * 1000.0;
        let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
        println!("sample,update_ms_mean,update_ms_max,hist_ms,thumbs_ms,full_load_ms,region_ms_mean,region_ms_max,region_vs_full_max_of_255");
        for f in files {
            let settings = ConvertSettings {
                exposure_ev: 0.3,
                style: PictureStyle::Landscape,
                ..Default::default()
            };
            let params = RenderParams::for_display(&settings);
            let base = preview::load_base(&f, PREVIEW_LONG_EDGE, &cancel).unwrap();
            let mut updates = Vec::new();
            for i in 0..24 {
                let p = RenderParams {
                    exposure_ev: i as f32 * 0.05,
                    ..params.clone()
                };
                let t = Instant::now();
                let img = render(base.image.clone(), &base.stats, &p, 0, &cancel).unwrap();
                let img = to_color_image(&resize::quantize(&img));
                std::hint::black_box(Histogram::of(&img));
                updates.push(ms(t));
            }
            let img = render(base.image.clone(), &base.stats, &params, 0, &cancel).unwrap();
            let img8 = to_color_image(&resize::quantize(&img));
            let t = Instant::now();
            std::hint::black_box(Histogram::of(&img8));
            let hist_ms = ms(t);
            let t = Instant::now();
            let small = resize::reduce_long_edge_f32(base.image.clone(), THUMB_EDGE);
            for style in PictureStyle::ALL {
                let p = RenderParams {
                    style,
                    ..params.clone()
                };
                let img = render(small.clone(), &base.stats, &p, 0, &cancel).unwrap();
                std::hint::black_box(to_color_image(&resize::quantize(&img)));
            }
            let thumbs_ms = ms(t);

            let t = Instant::now();
            let full = preview::load_base(&f, u32::MAX, &cancel).unwrap();
            let full_ms = ms(t);
            let fs = [full.full_size.0, full.full_size.1];
            let region = zoom_region(fs, [0.5, 0.5], [1600, 1000], 16384);
            let mut crops = Vec::new();
            for i in 0..12 {
                let p = RenderParams {
                    exposure_ev: i as f32 * 0.05,
                    ..params.clone()
                };
                let t = Instant::now();
                let img = render_region(&full.image, &full.stats, &p, region, &cancel).unwrap();
                std::hint::black_box(to_color_image(&resize::quantize(&img)));
                crops.push(ms(t));
            }
            let whole = render(full.image.clone(), &full.stats, &params, 0, &cancel).unwrap();
            let part = render_region(&full.image, &full.stats, &params, region, &cancel).unwrap();
            let mut worst = 0f32;
            for (x, y, p) in part.enumerate_pixels() {
                let q = whole.get_pixel(x + region.x, y + region.y);
                for c in 0..3 {
                    worst = worst.max((p.0[c] - q.0[c]).abs() * 255.0);
                }
            }
            println!(
                "{},{:.1},{:.1},{:.2},{:.1},{:.0},{:.1},{:.1},{:.3}",
                f.file_stem().unwrap().to_string_lossy(),
                mean(&updates),
                updates.iter().cloned().fold(0.0, f64::max),
                hist_ms,
                thumbs_ms,
                full_ms,
                mean(&crops),
                crops.iter().cloned().fold(0.0, f64::max),
                worst
            );
        }
    }

    /// Regression: leaving the 100 % view (or starting a batch) did not stop
    /// a full-resolution decode, which kept running and using its memory.
    #[test]
    fn release_full_cancels_only_a_full_resolution_decode() {
        let w = worker();
        for (full_res, expected) in [(true, true), (false, false)] {
            let cancel = Arc::new(AtomicBool::new(false));
            w.shared.lock().loading = Some(Loading {
                file: PathBuf::from("a.CR2"),
                cancel: Arc::clone(&cancel),
                full_res,
            });
            w.release_full();
            assert_eq!(cancel.load(Ordering::Relaxed), expected, "{full_res}");
            w.shared.lock().loading = None;
        }
    }

    /// Regression: a 100 % request on a file that cannot be developed sent the
    /// event for a failed preview, so the panel waited for pixels forever.
    #[test]
    fn zoom_failure_is_reported_as_a_zoom_failure() {
        let dir = tempfile::tempdir().unwrap();
        let bad = dir.path().join("bad.NEF");
        std::fs::write(&bad, vec![0u8; 512]).unwrap();
        let mut w = worker();
        w.request_zoom(ZoomRequest {
            file: bad.clone(),
            params: params(&ConvertSettings::default()),
            center: [0.5, 0.5],
            view_px: [100, 80],
            want_camera: false,
            max_side: 16384,
        });
        let seen = wait_for(&w, |e| matches!(e, PreviewEvent::ZoomFailed { .. }));
        assert!(!seen
            .iter()
            .any(|e| matches!(e, PreviewEvent::Failed { .. })));
        w.join();
    }

    #[test]
    fn long_errors_are_shortened() {
        let long = "x".repeat(300);
        assert!(short_message(&long).chars().count() <= 91);
        assert_eq!(short_message("first\nsecond"), "first");
        assert_eq!(short_message(""), "unknown error");
    }
}
