//! Thumbnails of the files in the current list: the worker that makes them,
//! the cache that holds them, and the virtualised grid and strip that show
//! them. Navigation only; every file is converted whatever happens here.
//!
//! Pictures come from the camera JPEG through
//! [`ccoonverter_core::thumbnail`], which never touches the RAW decoder.
//! Everything drawn here is neutral grey except the outline of the current
//! file (see [`crate::theme`]).

use std::collections::{HashMap, VecDeque};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use ccoonverter_core::settings::DEFAULT_THUMB_SIZE;
use ccoonverter_core::thumbnail::load_thumbnail;
use eframe::egui::text::{LayoutJob, TextWrapping};
use eframe::egui::{
    self, epaint, pos2, vec2, Color32, ColorImage, CursorIcon, Rect, Role, ScrollArea, Sense,
    Stroke, StrokeKind, TextFormat, TextureFilter, TextureHandle, TextureOptions, Ui, Vec2,
    WidgetInfo,
};

use crate::i18n::{t, Key};
use crate::marks::{self, Marks};
use crate::preview::Activity;
use crate::theme::{
    self, ACCENT, NEUTRAL_ACTIVE, NEUTRAL_CONTROL, NEUTRAL_DIM, NEUTRAL_TEXT, PHOTO_SURROUND,
    RADIUS_MEDIUM, RADIUS_SMALL, THUMB_TILE,
};
use crate::widgets::{self, SliderSpec};

/// Long edge of a thumbnail in pixels.
pub const THUMB_EDGE: u32 = 256;
/// Thumbnails kept. With `THUMB_EDGE` and mipmaps at most 112 MB of texture
/// (a square image); about 75 MB with the usual 3:2 photos. Whatever the
/// window size, what is on screen plus the margin stays well below this.
pub const CACHE_CAPACITY: usize = 320;
/// Rows generated beyond the visible ones, above and below.
const MARGIN_ROWS: usize = 2;
/// Cells generated beyond the visible ones, left and right of the strip.
const MARGIN_CELLS: usize = 6;
/// Thumbnails uploaded to the GPU in one frame at most.
const MAX_UPLOADS_PER_FRAME: usize = 16;
/// How long the preview worker must have been quiet before a thumbnail starts.
const QUIET: Duration = Duration::from_millis(120);
const POLL: Duration = Duration::from_millis(20);
/// A developed preview is reduced to about this size for a file with no JPEG.
const DEVELOPED_EDGE: usize = THUMB_EDGE as usize;

/// The thumbnail-size slider, in points.
pub const THUMB_SIZE_RANGE: std::ops::RangeInclusive<f32> = 64.0..=200.0;
/// Width of the panel, in points.
pub const PANEL_MIN_WIDTH: f32 = 170.0;
pub const PANEL_MAX_WIDTH: f32 = 560.0;

const PANEL_PADDING: f32 = 8.0;
/// Space kept free for the scroll bar on the right of the cells.
const SCROLLBAR_GUTTER: f32 = 6.0;
const CELL_GAP: f32 = 8.0;
const NAME_HEIGHT: f32 = 16.0;
/// Height of the row of stars under a cell's name.
const STARS_HEIGHT: f32 = 15.0;
/// Size of those stars.
const CELL_STAR: f32 = 10.0;
const CELL_STAR_PITCH: f32 = 13.0;
/// Height of the bottom row of the panel.
const FOOTER_HEIGHT: f32 = 28.0;
/// Space between the edge of the tile and the photo inside it.
const TILE_INSET: f32 = 5.0;
const THUMB_TILE_RADIUS: u8 = RADIUS_MEDIUM;

/// Tile height of the one-row strip used where the panel does not fit.
pub const STRIP_TILE: f32 = 64.0;
pub const STRIP_TILE_COMPACT: f32 = 48.0;
const STRIP_GAP: f32 = 5.0;
/// Space above a strip, separating it from the photo.
pub const STRIP_MARGIN: f32 = 6.0;

const TEX_OPTIONS: TextureOptions =
    TextureOptions::LINEAR.with_mipmap_mode(Some(TextureFilter::Linear));
const FULL_UV: Rect = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));

/// How many threads make thumbnails: one, or two on a machine with room.
fn thread_count() -> usize {
    match std::thread::available_parallelism().map_or(1, usize::from) {
        n if n >= 8 => 2,
        _ => 1,
    }
}

// ----------------------------------------------------------------------------
// Worker
// ----------------------------------------------------------------------------

/// One finished thumbnail. `image` is `None` when there is no usable JPEG.
#[derive(Debug)]
pub struct ThumbResult {
    pub generation: u64,
    pub index: usize,
    pub image: Option<ColorImage>,
}

#[derive(Debug)]
struct Job {
    generation: u64,
    index: usize,
    file: PathBuf,
}

#[derive(Debug)]
struct State {
    generation: u64,
    queue: VecDeque<Job>,
    /// Being made right now, for the current generation.
    in_flight: Vec<usize>,
    paused: bool,
    shutdown: bool,
    /// Trips when the generation changes or a batch starts.
    cancel: Arc<AtomicBool>,
}

struct Shared {
    state: Mutex<State>,
    cv: Condvar,
    activity: Arc<Activity>,
    repaint: Box<dyn Fn() + Send + Sync>,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Threads that make thumbnails, separate from the preview worker so a slow
/// decode here never holds up a preview there. They start a job only while
/// that worker is idle and no batch runs.
pub struct ThumbWorker {
    shared: Arc<Shared>,
    events: Receiver<ThumbResult>,
    threads: Vec<JoinHandle<()>>,
}

impl ThumbWorker {
    pub fn spawn(
        repaint: Box<dyn Fn() + Send + Sync>,
        activity: Arc<Activity>,
    ) -> std::io::Result<Self> {
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                generation: 0,
                queue: VecDeque::new(),
                in_flight: Vec::new(),
                paused: false,
                shutdown: false,
                cancel: Arc::new(AtomicBool::new(false)),
            }),
            cv: Condvar::new(),
            activity,
            repaint,
        });
        let (tx, events) = mpsc::channel();
        let mut threads = Vec::new();
        for i in 0..thread_count() {
            let shared = Arc::clone(&shared);
            let tx = tx.clone();
            let handle = std::thread::Builder::new()
                .name(format!("ccoon-thumb-{i}"))
                .spawn(move || run_worker(&shared, &tx))?;
            threads.push(handle);
        }
        Ok(Self {
            shared,
            events,
            threads,
        })
    }

    /// The jobs wanted now, most important first; replaces what was queued.
    /// A new generation drops everything of the previous one.
    pub fn request(&self, generation: u64, jobs: Vec<(usize, PathBuf)>) {
        let mut s = self.shared.lock();
        if generation != s.generation {
            s.generation = generation;
            s.cancel.store(true, Ordering::Relaxed);
            s.cancel = Arc::new(AtomicBool::new(false));
            s.in_flight.clear();
        }
        let busy = s.in_flight.clone();
        s.queue = jobs
            .into_iter()
            .filter(|(index, _)| !busy.contains(index))
            .map(|(index, file)| Job {
                generation,
                index,
                file,
            })
            .collect();
        self.shared.cv.notify_all();
    }

    /// While paused nothing starts and what is running stops.
    pub fn set_paused(&self, paused: bool) {
        let mut s = self.shared.lock();
        if s.paused == paused {
            return;
        }
        s.paused = paused;
        if paused {
            s.queue.clear();
            s.cancel.store(true, Ordering::Relaxed);
            s.cancel = Arc::new(AtomicBool::new(false));
            s.in_flight.clear();
        }
        self.shared.cv.notify_all();
    }

    fn drain(&self, max: usize) -> Vec<ThumbResult> {
        self.events.try_iter().take(max).collect()
    }
}

impl Drop for ThumbWorker {
    fn drop(&mut self) {
        let mut s = self.shared.lock();
        s.shutdown = true;
        s.cancel.store(true, Ordering::Relaxed);
        self.shared.cv.notify_all();
        drop(s);
        join_briefly(std::mem::take(&mut self.threads));
    }
}

/// Wait for worker threads to end, but never for long: a decode on a stalled
/// network drive must not hold up closing the window.
pub(crate) fn join_briefly(threads: Vec<JoinHandle<()>>) {
    let deadline = Instant::now() + Duration::from_millis(1500);
    for handle in threads {
        while !handle.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(2));
        }
        if handle.is_finished() {
            let _ = handle.join();
        }
    }
}

/// Next job the thread may start, waiting until it is allowed to.
fn next_job(shared: &Shared) -> Option<(Job, Arc<AtomicBool>)> {
    let mut s = shared.lock();
    loop {
        if s.shutdown {
            return None;
        }
        if s.paused || s.queue.is_empty() {
            s = shared.cv.wait(s).unwrap_or_else(|e| e.into_inner());
            continue;
        }
        if shared.activity.idle_for(QUIET) {
            let job = s.queue.pop_front().expect("not empty");
            s.in_flight.push(job.index);
            return Some((job, Arc::clone(&s.cancel)));
        }
        s = shared
            .cv
            .wait_timeout(s, POLL)
            .unwrap_or_else(|e| e.into_inner())
            .0;
    }
}

fn run_worker(shared: &Shared, tx: &Sender<ThumbResult>) {
    while let Some((job, cancel)) = next_job(shared) {
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            load_thumbnail(&job.file, THUMB_EDGE, &cancel)
        }));
        let image = match outcome {
            Ok(Ok(Some(img))) => Some(ColorImage::from_rgb(
                [img.width() as usize, img.height() as usize],
                img.as_raw(),
            )),
            Ok(Ok(None)) => None,
            Ok(Err(e)) if e.is_cancelled() => {
                forget(shared, &job);
                continue;
            }
            Ok(Err(e)) => {
                log::debug!("Thumbnail of {} failed: {e}", job.file.display());
                None
            }
            Err(_) => {
                log::warn!("Thumbnail decoder panicked on {}", job.file.display());
                None
            }
        };
        let current = shared.lock().generation == job.generation;
        forget(shared, &job);
        if current {
            let _ = tx.send(ThumbResult {
                generation: job.generation,
                index: job.index,
                image,
            });
            (shared.repaint)();
        }
    }
}

fn forget(shared: &Shared, job: &Job) {
    let mut s = shared.lock();
    if s.generation == job.generation {
        if let Some(i) = s.in_flight.iter().position(|&v| v == job.index) {
            s.in_flight.swap_remove(i);
        }
    }
}

// ----------------------------------------------------------------------------
// Cache and view state
// ----------------------------------------------------------------------------

/// What a cell shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThumbState {
    /// Nothing yet.
    Waiting,
    /// The camera JPEG.
    Camera,
    /// No camera JPEG, but the file was developed for the preview.
    Developed,
    /// No camera JPEG and nothing developed: the neutral placeholder.
    Placeholder,
}

struct Entry {
    picture: Option<(TextureHandle, [usize; 2], ThumbState)>,
    /// The worker has answered for this file.
    resolved: bool,
    last_used: u64,
}

/// One slot of the virtualised view, in points.
#[derive(Debug, Clone, Copy, PartialEq)]
struct GridLayout {
    columns: usize,
    inner_width: f32,
    tile: f32,
    pitch: Vec2,
}

fn grid_layout(panel_width: f32, size: f32) -> GridLayout {
    let inner_width = (panel_width - 2.0 * PANEL_PADDING - SCROLLBAR_GUTTER).max(40.0);
    let columns = (((inner_width + CELL_GAP) / (size + CELL_GAP)).floor() as usize).max(1);
    let tile = (inner_width - CELL_GAP * (columns as f32 - 1.0)) / columns as f32;
    GridLayout {
        columns,
        inner_width,
        tile,
        pitch: vec2(
            tile + CELL_GAP,
            tile + NAME_HEIGHT + STARS_HEIGHT + CELL_GAP,
        ),
    }
}

/// The files currently shown: the whole list, or those a filter let through.
#[derive(Clone, Copy)]
pub struct FileView<'a> {
    pub files: &'a [PathBuf],
    /// Indices into `files`, ascending. `None` means every file.
    pub order: Option<&'a [usize]>,
}

impl FileView<'_> {
    pub fn len(&self) -> usize {
        self.order.map_or(self.files.len(), <[usize]>::len)
    }

    /// The file index at position `pos`.
    pub fn index_at(&self, pos: usize) -> usize {
        self.order.map_or(pos, |o| o[pos])
    }

    /// Position of the file `index`, or of the nearest shown file.
    pub fn position_of(&self, index: usize) -> usize {
        match self.order {
            None => index,
            Some(o) => o.binary_search(&index).unwrap_or_else(|p| {
                if p >= o.len() || (p > 0 && index - o[p - 1] < o[p] - index) {
                    p.saturating_sub(1)
                } else {
                    p
                }
            }),
        }
    }
}

/// What the user did in the grid or strip this frame.
#[derive(Debug, Default, Clone, Copy)]
pub struct ViewClick {
    /// File to show.
    pub open: Option<usize>,
    /// Stars set on a file.
    pub stars: Option<(usize, u8)>,
}

/// `offset` moved as little as possible to show `pos..pos + extent`.
fn follow_offset(pos: f32, extent: f32, view: f32, offset: f32, pad: f32, total: f32) -> f32 {
    let (a, b) = (pos - pad, pos + extent + pad);
    let wanted = if a < offset {
        a
    } else if b > offset + view {
        b - view
    } else {
        offset
    };
    wanted.clamp(0.0, (total - view).max(0.0))
}

/// `size` scaled to fit `area`, centred.
fn fit(area: Rect, size: [usize; 2]) -> Rect {
    let scale = (area.width() / size[0] as f32).min(area.height() / size[1] as f32);
    Rect::from_center_size(
        area.center(),
        vec2(size[0] as f32 * scale, size[1] as f32 * scale),
    )
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
}

/// Box-reduce a developed preview to thumbnail size.
fn reduce(image: &ColorImage) -> ColorImage {
    let [w, h] = image.size;
    let k = (w.max(h) / DEVELOPED_EDGE).max(1);
    let (out_w, out_h) = ((w / k).max(1), (h / k).max(1));
    let mut rgb = Vec::with_capacity(out_w * out_h * 3);
    for y in 0..out_h {
        for x in 0..out_w {
            let mut sum = [0u32; 3];
            for yy in y * k..((y + 1) * k).min(h) {
                for xx in x * k..((x + 1) * k).min(w) {
                    let p = image.pixels[yy * w + xx];
                    sum[0] += u32::from(p.r());
                    sum[1] += u32::from(p.g());
                    sum[2] += u32::from(p.b());
                }
            }
            let n = (k * k) as u32;
            rgb.extend(sum.map(|s| (s / n) as u8));
        }
    }
    ColorImage::from_rgb([out_w, out_h], &rgb)
}

/// The cache and scroll state of the thumbnail view.
pub struct ThumbBrowser {
    worker: Option<ThumbWorker>,
    /// Bumped when the file list changes; older results are dropped.
    generation: u64,
    cache: HashMap<usize, Entry>,
    /// Frames seen, for the least-recently-used eviction.
    clock: u64,
    /// The jobs last sent to the worker.
    last_wanted: Vec<usize>,
    /// Scroll position along the scrolling axis, as of the last frame.
    offset: f32,
    /// (current file, columns) the view was last scrolled for.
    followed: Option<(usize, usize)>,
    /// Columns of the grid when one was drawn in the last frame.
    columns: Option<usize>,
    /// The view was drawn in the last frame.
    drawn: bool,
    paused: bool,
    #[cfg(test)]
    visible_range: std::ops::Range<usize>,
    /// Test scroll target, used instead of the current file.
    #[cfg(test)]
    jump: Option<usize>,
}

impl ThumbBrowser {
    pub fn new(worker: Option<ThumbWorker>) -> Self {
        Self {
            worker,
            generation: 0,
            cache: HashMap::new(),
            clock: 0,
            last_wanted: Vec::new(),
            offset: 0.0,
            followed: None,
            columns: None,
            drawn: false,
            paused: false,
            #[cfg(test)]
            visible_range: 0..0,
            #[cfg(test)]
            jump: None,
        }
    }

    /// Forget everything: the file list changed.
    pub fn reset(&mut self) {
        self.generation += 1;
        self.cache.clear();
        self.last_wanted.clear();
        self.offset = 0.0;
        self.followed = None;
        if let Some(w) = &self.worker {
            w.request(self.generation, Vec::new());
        }
    }

    /// Once per frame, whether or not the view will be drawn.
    pub fn begin_frame(&mut self, ctx: &egui::Context, paused: bool) {
        self.clock += 1;
        if let Some(w) = &self.worker {
            if paused != self.paused {
                w.set_paused(paused);
                self.last_wanted.clear();
            }
            let results = w.drain(MAX_UPLOADS_PER_FRAME);
            let more = results.len() == MAX_UPLOADS_PER_FRAME;
            for r in results {
                self.accept(ctx, r);
            }
            if more {
                ctx.request_repaint();
            }
        }
        self.paused = paused;
        if !self.drawn && !self.last_wanted.is_empty() {
            // Nothing on screen needs these any more.
            self.last_wanted.clear();
            if let Some(w) = &self.worker {
                w.request(self.generation, Vec::new());
            }
        }
        self.drawn = false;
        self.columns = None;
    }

    fn accept(&mut self, ctx: &egui::Context, result: ThumbResult) {
        if result.generation != self.generation {
            log::debug!("Dropping a thumbnail of an earlier file list");
            return;
        }
        let clock = self.clock;
        let entry = self.cache.entry(result.index).or_insert(Entry {
            picture: None,
            resolved: false,
            last_used: clock,
        });
        entry.resolved = true;
        entry.last_used = clock;
        if let Some(image) = result.image {
            let size = image.size;
            let texture = ctx.load_texture("file_thumbnail", image, TEX_OPTIONS);
            entry.picture = Some((texture, size, ThumbState::Camera));
        }
        self.evict();
    }

    fn evict(&mut self) {
        while self.cache.len() > CACHE_CAPACITY {
            let Some(oldest) = self
                .cache
                .iter()
                .min_by_key(|(_, e)| e.last_used)
                .map(|(&i, _)| i)
            else {
                return;
            };
            self.cache.remove(&oldest);
        }
    }

    /// The main preview developed the file at `index`; a file without a
    /// camera JPEG may show that from now on. Only the first frame is kept.
    pub fn offer_developed(&mut self, ctx: &egui::Context, index: usize, image: &ColorImage) {
        let clock = self.clock;
        let entry = self.cache.entry(index).or_insert(Entry {
            picture: None,
            resolved: false,
            last_used: clock,
        });
        if entry.picture.is_some() {
            return;
        }
        let small = reduce(image);
        let size = small.size;
        let texture = ctx.load_texture("file_thumbnail", small, TEX_OPTIONS);
        entry.picture = Some((texture, size, ThumbState::Developed));
        entry.last_used = clock;
        self.evict();
    }

    /// The list shown changed (a filter): scroll to the current file again.
    pub fn view_changed(&mut self) {
        self.followed = None;
    }

    /// The view is on screen but has nothing to show.
    pub fn draw_nothing(&mut self) {
        self.drawn = true;
    }

    /// Ask the worker for what is shown and not known yet; refresh the LRU.
    /// `visible` is in positions of `view`.
    fn request_visible(
        &mut self,
        view: FileView<'_>,
        current_pos: usize,
        visible: std::ops::Range<usize>,
        margin: usize,
    ) {
        let (lo, hi) = (
            visible.start.saturating_sub(margin),
            (visible.end + margin).min(view.len()),
        );
        let clock = self.clock;
        let mut wanted = Vec::new();
        for pos in lo..hi {
            let i = view.index_at(pos);
            match self.cache.get_mut(&i) {
                Some(e) => {
                    e.last_used = clock;
                    if !e.resolved {
                        wanted.push(pos);
                    }
                }
                None => wanted.push(pos),
            }
        }
        wanted.sort_by_key(|&pos| (!visible.contains(&pos), pos.abs_diff(current_pos)));
        let wanted: Vec<usize> = wanted.into_iter().map(|p| view.index_at(p)).collect();
        if wanted == self.last_wanted {
            return;
        }
        if let Some(w) = &self.worker {
            let jobs = wanted.iter().map(|&i| (i, view.files[i].clone())).collect();
            w.request(self.generation, jobs);
        }
        self.last_wanted = wanted;
    }

    fn state_of(&self, index: usize) -> ThumbState {
        match self.cache.get(&index) {
            Some(Entry {
                picture: Some((_, _, state)),
                ..
            }) => *state,
            Some(Entry { resolved: true, .. }) => ThumbState::Placeholder,
            _ => ThumbState::Waiting,
        }
    }

    /// Paint one cell and report a click on its picture.
    fn cell(
        &self,
        ui: &mut Ui,
        file: &Path,
        index: usize,
        tile: Rect,
        name: Option<Rect>,
        current: bool,
    ) -> bool {
        let area = name.map_or(tile, |n| tile.union(n));
        let id = egui::Id::unique(("thumb_cell", index));
        let resp = ui.interact(area, id, Sense::click());
        let label = file_name(file);
        let state = self.state_of(index);
        resp.widget_info(|| WidgetInfo::labeled(Role::Button, true, &label));

        let painter = ui.painter_at(area);
        painter.rect_filled(tile, THUMB_TILE_RADIUS, THUMB_TILE);
        let inner = tile.shrink(TILE_INSET);
        match self.cache.get(&index).and_then(|e| e.picture.as_ref()) {
            Some((texture, size, _)) => {
                let shape =
                    epaint::RectShape::filled(fit(inner, *size), RADIUS_SMALL, Color32::WHITE)
                        .with_texture(texture.id(), FULL_UV);
                painter.add(shape);
            }
            None if state == ThumbState::Placeholder => {
                let extension = file
                    .extension()
                    .map_or_else(String::new, |e| e.to_string_lossy().to_uppercase());
                painter.text(
                    tile.center(),
                    egui::Align2::CENTER_CENTER,
                    extension,
                    theme::font_small_bold(),
                    NEUTRAL_DIM,
                );
            }
            None => {}
        }
        if current {
            painter.rect_stroke(
                tile,
                THUMB_TILE_RADIUS,
                Stroke::new(2.0, ACCENT),
                StrokeKind::Inside,
            );
        } else if resp.hovered() {
            painter.rect_stroke(
                tile,
                THUMB_TILE_RADIUS,
                Stroke::new(1.0, NEUTRAL_ACTIVE),
                StrokeKind::Inside,
            );
        }
        if let Some(name_rect) = name {
            draw_name(ui, &painter, &label, name_rect, current);
        }
        if resp.hovered() {
            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
            let note = t(match state {
                ThumbState::Developed => Key::TipThumbDeveloped,
                ThumbState::Placeholder => Key::TipThumbNoJpeg,
                _ => Key::TipThumbCamera,
            });
            let _ = resp.clone().on_hover_text(format!("{label}\n{note}"));
        }
        resp.clicked()
    }

    /// The right-hand panel: the grid, then its footer.
    pub fn draw_grid(
        &mut self,
        ui: &mut Ui,
        view: FileView<'_>,
        marks: &Marks,
        current: usize,
        size: &mut f32,
        count: &str,
    ) -> ViewClick {
        self.drawn = true;
        neutral_scroll_style(ui);
        let available = ui.available_size();
        let layout = grid_layout(available.x + 2.0 * PANEL_PADDING, *size);
        self.columns = Some(layout.columns);
        let rows = view.len().div_ceil(layout.columns);
        let total = rows as f32 * layout.pitch.y;
        let view_h = (available.y - FOOTER_HEIGHT).max(layout.pitch.y);
        let current_pos = view.position_of(current);

        let mut area = ScrollArea::vertical()
            .id_salt("thumb_grid")
            .auto_shrink([false, false])
            .max_height(view_h);
        let key = (current_pos, layout.columns);
        if self.followed != Some(key) {
            let target = self.follow_target(current_pos);
            let y = follow_offset(
                (target / layout.columns) as f32 * layout.pitch.y,
                layout.pitch.y,
                view_h,
                self.offset,
                CELL_GAP,
                total,
            );
            area = area.vertical_scroll_offset(y);
            self.followed = Some(key);
        }

        let mut click = ViewClick::default();
        let out = area.show_viewport(ui, |ui, viewport| {
            ui.set_width(layout.inner_width);
            ui.set_height(total);
            let origin = ui.min_rect().min;
            let pitch = layout.pitch;
            let first_row = (viewport.min.y / pitch.y).floor().max(0.0) as usize;
            let last_row = ((viewport.max.y / pitch.y).ceil() as usize).min(rows);
            let visible = first_row * layout.columns..(last_row * layout.columns).min(view.len());
            #[cfg(test)]
            {
                self.visible_range = visible.clone();
            }
            for pos in visible.clone() {
                let i = view.index_at(pos);
                let (row, col) = (pos / layout.columns, pos % layout.columns);
                let at = origin + vec2(col as f32 * pitch.x, row as f32 * pitch.y);
                let tile = Rect::from_min_size(at, vec2(layout.tile, layout.tile));
                let name = Rect::from_min_size(
                    at + vec2(0.0, layout.tile + 1.0),
                    vec2(layout.tile, NAME_HEIGHT),
                );
                if self.cell(ui, &view.files[i], i, tile, Some(name), i == current) {
                    click.open = Some(i);
                }
                let stars_at = at + vec2(0.0, layout.tile + 1.0 + NAME_HEIGHT);
                let stars = marks::star_row(
                    ui,
                    Rect::from_min_size(stars_at, vec2(layout.tile, STARS_HEIGHT)),
                    CELL_STAR,
                    CELL_STAR_PITCH.min(layout.tile / 5.0),
                    marks.stars(i),
                    &file_name(&view.files[i]),
                    ("cell", i),
                    t(Key::TipStars),
                );
                if let Some(n) = stars {
                    click.stars = Some((i, n));
                }
            }
            self.request_visible(view, current_pos, visible, MARGIN_ROWS * layout.columns);
        });
        self.offset = out.state.clamped_offset().y;
        draw_footer(ui, count, size, available.x);
        click
    }

    /// One row of thumbnails, for windows too narrow for the panel.
    pub fn draw_strip(
        &mut self,
        ui: &mut Ui,
        view: FileView<'_>,
        current: usize,
        tile: f32,
    ) -> Option<usize> {
        self.drawn = true;
        neutral_scroll_style(ui);
        let view_w = ui.available_width();
        let pitch = tile + STRIP_GAP;
        let total = view.len() as f32 * pitch;
        let current_pos = view.position_of(current);
        let mut area = ScrollArea::horizontal()
            .id_salt("thumb_strip")
            .auto_shrink([false, false])
            .max_height(tile);
        if self.followed != Some((current_pos, 0)) {
            let target = self.follow_target(current_pos);
            let x = follow_offset(
                target as f32 * pitch,
                pitch,
                view_w,
                self.offset,
                STRIP_GAP,
                total,
            );
            area = area.horizontal_scroll_offset(x);
            self.followed = Some((current_pos, 0));
        }
        let mut clicked = None;
        let out = area.show_viewport(ui, |ui, viewport| {
            ui.set_width(total);
            ui.set_height(tile);
            let origin = ui.min_rect().min;
            let first = (viewport.min.x / pitch).floor().max(0.0) as usize;
            let last = ((viewport.max.x / pitch).ceil() as usize).min(view.len());
            let visible = first..last;
            #[cfg(test)]
            {
                self.visible_range = visible.clone();
            }
            for pos in visible.clone() {
                let i = view.index_at(pos);
                let at = origin + vec2(pos as f32 * pitch, 0.0);
                let rect = Rect::from_min_size(at, vec2(tile, tile));
                if self.cell(ui, &view.files[i], i, rect, None, i == current) {
                    clicked = Some(i);
                }
            }
            self.request_visible(view, current_pos, visible, MARGIN_CELLS);
        });
        self.offset = out.state.clamped_offset().x;
        clicked
    }

    /// Where the view should go: the current file (tests can pick elsewhere).
    fn follow_target(&mut self, current: usize) -> usize {
        #[cfg(test)]
        if let Some(index) = self.jump.take() {
            return index;
        }
        current
    }

    /// Columns of the grid drawn this frame, `None` if the grid is not shown.
    pub fn columns(&self) -> Option<usize> {
        self.columns
    }

    /// Nothing the view wants is outstanding.
    #[cfg(feature = "capture")]
    pub fn is_settled(&self) -> bool {
        self.last_wanted.is_empty()
    }
}

fn neutral_scroll_style(ui: &mut Ui) {
    let v = ui.visuals_mut();
    v.extreme_bg_color = PHOTO_SURROUND;
    for w in [
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.noninteractive,
    ] {
        w.bg_fill = NEUTRAL_ACTIVE;
        w.weak_bg_fill = NEUTRAL_CONTROL;
        w.fg_stroke.color = NEUTRAL_ACTIVE;
        w.bg_stroke.color = NEUTRAL_CONTROL;
    }
}

fn draw_name(ui: &Ui, painter: &egui::Painter, name: &str, rect: Rect, current: bool) {
    let color = if current { NEUTRAL_TEXT } else { NEUTRAL_DIM };
    let mut job = LayoutJob::single_section(
        name.to_owned(),
        TextFormat::simple(theme::font_small(), color),
    );
    job.wrap = TextWrapping::truncate_at_width(rect.width());
    job.halign = egui::Align::Center;
    let galley = ui.ctx().fonts_mut(|f| f.layout_job(job));
    let pos = pos2(rect.center().x, rect.center().y - galley.size().y / 2.0);
    painter.galley(pos, galley, color);
}

/// Bottom row of the panel: item count and thumbnail size.
pub fn draw_footer(ui: &mut Ui, count: &str, size: &mut f32, width: f32) {
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let slider_w = (width * 0.42).clamp(70.0, 110.0);
        let text_w = (width - slider_w - 8.0).max(30.0);
        widgets::cell(ui, text_w, 20.0, egui::Align::Min, |ui| {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(count)
                        .font(theme::font_small())
                        .color(NEUTRAL_DIM),
                )
                .truncate(),
            );
        });
        let label = t(Key::ThumbSizeLabel);
        let result = widgets::neutral_slider(
            ui,
            size,
            &SliderSpec {
                name: label,
                range: THUMB_SIZE_RANGE,
                origin: *THUMB_SIZE_RANGE.start(),
                default: DEFAULT_THUMB_SIZE,
                step: 4.0,
            },
            slider_w,
        );
        widgets::tip(result.response, label);
    });
}

#[cfg(test)]
impl ThumbBrowser {
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn state(&self, index: usize) -> ThumbState {
        self.state_of(index)
    }

    pub fn cached(&self) -> usize {
        self.cache.len()
    }

    pub fn accept_for_test(&mut self, ctx: &egui::Context, result: ThumbResult) {
        self.accept(ctx, result);
    }

    pub fn visible(&self) -> std::ops::Range<usize> {
        self.visible_range.clone()
    }

    pub fn wanted(&self) -> &[usize] {
        &self.last_wanted
    }

    /// Scroll the view to `index` without changing the current file.
    pub fn accept_view_jump(&mut self, index: usize) {
        self.followed = None;
        self.jump = Some(index);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn activity() -> Arc<Activity> {
        Arc::new(Activity::new())
    }

    fn worker_with(activity: Arc<Activity>) -> ThumbWorker {
        ThumbWorker::spawn(Box::new(|| {}), activity).unwrap()
    }

    fn wait_for(w: &ThumbWorker) -> ThumbResult {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(r) = w.drain(1).pop() {
                return r;
            }
            assert!(Instant::now() < deadline, "no result");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn file_with_jpeg(dir: &Path, name: &str) -> PathBuf {
        let img = image::RgbImage::from_pixel(300, 200, image::Rgb([180, 90, 40]));
        let mut jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new(&mut jpeg)
            .encode_image(&img)
            .unwrap();
        let mut blob = vec![0x42u8; 64];
        blob.extend(jpeg);
        let path = dir.join(name);
        std::fs::write(&path, blob).unwrap();
        path
    }

    #[test]
    fn columns_fit_the_panel_and_grow_when_tiles_shrink() {
        let small = grid_layout(240.0, 72.0);
        let large = grid_layout(240.0, 200.0);
        assert_eq!(large.columns, 1);
        assert!(small.columns > large.columns);
        // Default panel holds two default thumbnails.
        assert_eq!(grid_layout(240.0, DEFAULT_THUMB_SIZE).columns, 2);
        for l in [small, large] {
            let used = l.tile * l.columns as f32 + CELL_GAP * (l.columns as f32 - 1.0);
            assert!((used - l.inner_width).abs() < 0.01);
        }
    }

    #[test]
    fn following_moves_only_as_far_as_needed() {
        // Inside the view: unchanged.
        assert_eq!(follow_offset(100.0, 50.0, 300.0, 0.0, 0.0, 1000.0), 0.0);
        // Below: brought in at the far edge.
        assert_eq!(follow_offset(400.0, 50.0, 300.0, 0.0, 0.0, 1000.0), 150.0);
        // Above: brought in at the near edge.
        assert_eq!(follow_offset(100.0, 50.0, 300.0, 400.0, 10.0, 1000.0), 90.0);
        // Never past the end of the content.
        assert_eq!(follow_offset(990.0, 50.0, 300.0, 0.0, 0.0, 1000.0), 700.0);
    }

    #[test]
    fn nothing_starts_while_the_preview_worker_is_busy() {
        let dir = tempfile::tempdir().unwrap();
        let file = file_with_jpeg(dir.path(), "a.CR2");
        let activity = activity();
        let busy = activity.enter();
        let w = worker_with(Arc::clone(&activity));
        w.request(1, vec![(0, file)]);
        std::thread::sleep(Duration::from_millis(250));
        assert!(w.drain(1).is_empty(), "started during preview work");
        drop(busy);
        let r = wait_for(&w);
        assert_eq!(r.index, 0);
        assert!(r.image.is_some());
    }

    #[test]
    fn nothing_starts_during_a_batch_and_work_resumes_after() {
        let dir = tempfile::tempdir().unwrap();
        let file = file_with_jpeg(dir.path(), "a.CR2");
        let w = worker_with(activity());
        w.set_paused(true);
        w.request(1, vec![(0, file.clone())]);
        std::thread::sleep(Duration::from_millis(250));
        assert!(w.drain(1).is_empty(), "started during a batch");
        w.set_paused(false);
        w.request(1, vec![(0, file)]);
        assert_eq!(wait_for(&w).index, 0);
    }

    #[test]
    fn a_result_of_the_previous_generation_never_comes_back() {
        let dir = tempfile::tempdir().unwrap();
        let file = file_with_jpeg(dir.path(), "a.CR2");
        let w = worker_with(activity());
        w.request(1, vec![(0, file.clone())]);
        // New list while the first may still be queued or decoding.
        w.request(2, vec![(5, file)]);
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            for r in w.drain(4) {
                assert_ne!(r.generation, 1, "stale result");
                if r.index == 5 {
                    return;
                }
            }
            assert!(Instant::now() < deadline, "no result");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn the_cache_is_bounded_and_keeps_what_was_used_last() {
        let ctx = egui::Context::default();
        let mut b = ThumbBrowser::new(None);
        let image = || ColorImage::from_rgb([2, 2], &[9u8; 12]);
        for i in 0..CACHE_CAPACITY + 100 {
            b.clock += 1;
            b.accept(
                &ctx,
                ThumbResult {
                    generation: 0,
                    index: i,
                    image: Some(image()),
                },
            );
        }
        assert_eq!(b.cache.len(), CACHE_CAPACITY);
        assert!(b.cache.contains_key(&(CACHE_CAPACITY + 99)));
        assert!(!b.cache.contains_key(&0), "oldest evicted");
    }

    #[test]
    fn a_stale_result_is_refused_by_the_browser() {
        let ctx = egui::Context::default();
        let mut b = ThumbBrowser::new(None);
        let old = b.generation;
        b.reset();
        b.accept(
            &ctx,
            ThumbResult {
                generation: old,
                index: 0,
                image: Some(ColorImage::from_rgb([1, 1], &[1, 2, 3])),
            },
        );
        assert!(b.cache.is_empty());
        assert_eq!(b.state_of(0), ThumbState::Waiting);
    }

    #[test]
    fn a_developed_picture_replaces_a_placeholder_once_only() {
        let ctx = egui::Context::default();
        let mut b = ThumbBrowser::new(None);
        let frame = |v: u8| ColorImage::from_rgb([1600, 1067], &vec![v; 1600 * 1067 * 3]);
        b.accept(
            &ctx,
            ThumbResult {
                generation: 0,
                index: 3,
                image: None,
            },
        );
        assert_eq!(b.state_of(3), ThumbState::Placeholder);
        b.offer_developed(&ctx, 3, &frame(100));
        assert_eq!(b.state_of(3), ThumbState::Developed);
        let size = b.cache[&3].picture.as_ref().unwrap().1;
        assert!(size[0] <= DEVELOPED_EDGE * 2 && size[0] >= DEVELOPED_EDGE / 2);
        // A later frame (slider moved) changes nothing.
        let id = b.cache[&3].picture.as_ref().unwrap().0.id();
        b.offer_developed(&ctx, 3, &frame(200));
        assert_eq!(b.cache[&3].picture.as_ref().unwrap().0.id(), id);
    }

    #[test]
    fn a_camera_jpeg_arriving_after_a_developed_frame_wins() {
        let ctx = egui::Context::default();
        let mut b = ThumbBrowser::new(None);
        b.offer_developed(&ctx, 1, &ColorImage::from_rgb([4, 4], &[9u8; 48]));
        b.accept(
            &ctx,
            ThumbResult {
                generation: 0,
                index: 1,
                image: Some(ColorImage::from_rgb([2, 2], &[1u8; 12])),
            },
        );
        assert_eq!(b.state_of(1), ThumbState::Camera);
    }

    /// Round trip of a slider update through the real preview worker, with
    /// and without a full queue of thumbnails waiting. Run in release:
    /// `cargo test --release -p ccoonverter slider_updates_with -- --ignored --nocapture --test-threads=1`
    #[test]
    #[ignore]
    fn slider_updates_with_and_without_a_thumbnail_queue() {
        use crate::preview::{PreviewEvent, PreviewWorker, RenderRequest};
        use ccoonverter_core::render::RenderParams;
        use ccoonverter_core::ConvertSettings;
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/samples");
        let Ok(files) = ccoonverter_core::scan::scan_folder(&dir) else {
            return;
        };
        let target = files[1].clone();
        let mut preview = PreviewWorker::spawn(Box::new(|| {})).unwrap();
        let thumbs = ThumbWorker::spawn(Box::new(|| {}), preview.activity()).unwrap();
        let settings = |ev: f32| ConvertSettings {
            exposure_ev: ev,
            ..Default::default()
        };
        let send = |w: &mut PreviewWorker, ev: f32| {
            w.request_render(RenderRequest {
                file: target.clone(),
                params: Some(RenderParams::for_display(&settings(ev))),
                session: 0,
            })
        };
        let wait_for_id = |w: &PreviewWorker, id: u64| {
            let deadline = Instant::now() + Duration::from_secs(60);
            loop {
                let done = w.drain().iter().any(
                    |e| matches!(e, PreviewEvent::Developed { request, .. } if *request == id),
                );
                if done {
                    return;
                }
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_micros(300));
            }
        };
        let id = send(&mut preview, 0.0);
        wait_for_id(&preview, id);
        let queue = |generation: u64| {
            let jobs = (0..300usize)
                .map(|i| (i, files[i % files.len()].clone()))
                .collect();
            thumbs.request(generation, jobs);
        };
        let run = |preview: &mut PreviewWorker, label: &str| {
            let mut times = Vec::new();
            for i in 0..40 {
                let t = Instant::now();
                let id = send(preview, i as f32 * 0.05);
                wait_for_id(preview, id);
                times.push(t.elapsed().as_secs_f64() * 1000.0);
            }
            let mean = times.iter().sum::<f64>() / times.len() as f64;
            let max = times.iter().cloned().fold(0.0, f64::max);
            println!("{label}: mean {mean:.1} ms, max {max:.1} ms");
        };
        run(&mut preview, "thumbnail queue empty");
        queue(1);
        run(&mut preview, "thumbnail queue full (300 waiting)");
        let done = thumbs.drain(1000).len();
        println!("thumbnails finished during the slider run: {done}");
        queue(2);
        std::thread::sleep(Duration::from_millis(600));
        let during_quiet = thumbs.drain(1000).len();
        println!("thumbnails finished in the next 600 ms of quiet: {during_quiet}");
    }

    /// Real files, one worker thread at a time. Run in release:
    /// `cargo test --release -p ccoonverter thumbnail_throughput -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn thumbnail_throughput_on_real_samples() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/samples");
        let Ok(files) = ccoonverter_core::scan::scan_folder(&dir) else {
            return;
        };
        let cancel = AtomicBool::new(false);
        println!("file,ms_mean,ms_max,thumbnail");
        let mut times = Vec::new();
        for f in &files {
            let mut each = Vec::new();
            let mut size = (0, 0);
            for _ in 0..8 {
                let t = Instant::now();
                let img = load_thumbnail(f, THUMB_EDGE, &cancel).unwrap();
                each.push(t.elapsed().as_secs_f64() * 1000.0);
                // A file with no camera JPEG (a real case: the 5D III DNG).
                size = img.map_or((0, 0), |i| i.dimensions());
            }
            let mean = each.iter().sum::<f64>() / each.len() as f64;
            let max = each.iter().cloned().fold(0.0, f64::max);
            println!(
                "{},{mean:.1},{max:.1},{}x{}",
                f.file_name().unwrap().to_string_lossy(),
                size.0,
                size.1
            );
            times.push(mean);
        }
        let per_file = times.iter().sum::<f64>() / times.len() as f64;
        println!(
            "mean {per_file:.1} ms/file; {:.1} thumbnails/s on one thread",
            1000.0 / per_file
        );
    }
}
