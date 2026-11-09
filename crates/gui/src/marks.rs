//! Star ratings of the files in the list: loaded at low priority, saved to
//! XMP sidecars on a background thread, and drawn as a row of five stars.
//! Everything here is grey except the filled stars.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use ccoonverter_core::sidecar::{
    load_rating, save_stars, sidecar_path, SidecarError, SidecarIndex,
};
use eframe::egui::epaint::{Mesh, Shape, Stroke};
use eframe::egui::{
    self, pos2, vec2, Color32, CursorIcon, Pos2, Rect, Role, Sense, Ui, WidgetInfo,
};

use crate::i18n::{tf, Key};
use crate::preview::Activity;
use crate::theme::{ACCENT, NEUTRAL_ACTIVE, NEUTRAL_TEXT};

/// Ratings handed to the UI in chunks of at most this many.
const CHUNK: usize = 128;
const CHUNK_TIME: Duration = Duration::from_millis(50);
const QUIET: Duration = Duration::from_millis(120);
const POLL: Duration = Duration::from_millis(20);
/// Events applied in one frame at most.
const MAX_EVENTS_PER_FRAME: usize = 32;

/// Why a rating could not be saved.
#[derive(Debug)]
struct WriteFailure {
    unparsable: bool,
    detail: String,
}

#[derive(Debug)]
enum Event {
    Read {
        generation: u64,
        items: Vec<(usize, u8)>,
        done: bool,
    },
    Wrote {
        file: PathBuf,
        stars: u8,
        result: Result<(), WriteFailure>,
    },
}

struct State {
    generation: u64,
    files: Arc<Vec<PathBuf>>,
    next: usize,
    writes: VecDeque<(PathBuf, u8)>,
    paused: bool,
    shutdown: bool,
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

/// One thread: saves ratings straight away, reads the rest of the list while
/// nothing more important is going on.
pub struct MarksWorker {
    shared: Arc<Shared>,
    events: Receiver<Event>,
    thread: Option<std::thread::JoinHandle<()>>,
}

enum Step {
    Write(PathBuf, u8),
    Read(u64, Arc<Vec<PathBuf>>, usize),
}

fn next_step(shared: &Shared) -> Option<Step> {
    let mut s = shared.lock();
    loop {
        if s.shutdown {
            return None;
        }
        if let Some((file, stars)) = s.writes.pop_front() {
            return Some(Step::Write(file, stars));
        }
        if s.paused || s.next >= s.files.len() {
            s = shared.cv.wait(s).unwrap_or_else(|e| e.into_inner());
            continue;
        }
        if shared.activity.idle_for(QUIET) {
            let i = s.next;
            s.next += 1;
            return Some(Step::Read(s.generation, Arc::clone(&s.files), i));
        }
        s = shared
            .cv
            .wait_timeout(s, POLL)
            .unwrap_or_else(|e| e.into_inner())
            .0;
    }
}

fn write_failure(error: &SidecarError) -> WriteFailure {
    WriteFailure {
        unparsable: matches!(error, SidecarError::Unparsable(_)),
        detail: match error {
            SidecarError::Unparsable(_) => String::new(),
            SidecarError::Io(_, e) => e.to_string(),
            other => other.to_string(),
        },
    }
}

fn run_worker(shared: &Shared, tx: &Sender<Event>) {
    let mut index: Option<(u64, SidecarIndex)> = None;
    let mut pending: Vec<(usize, u8)> = Vec::new();
    let mut last_sent = Instant::now();
    while let Some(step) = next_step(shared) {
        match step {
            Step::Write(file, stars) => {
                let result = save_stars(&file, stars).map_err(|e| {
                    log::warn!("Could not save the rating of {}: {e}", file.display());
                    write_failure(&e)
                });
                let _ = tx.send(Event::Wrote {
                    file,
                    stars,
                    result,
                });
                (shared.repaint)();
            }
            Step::Read(generation, files, i) => {
                if index.as_ref().is_none_or(|(g, _)| *g != generation) {
                    let listing = SidecarIndex::of_files(files.iter().map(PathBuf::as_path));
                    index = Some((generation, listing));
                    pending.clear();
                }
                let listing = index.as_ref().map(|(_, l)| l);
                let found = listing.and_then(|l| l.sidecar_for(&files[i]));
                let stars = load_rating(&files[i], found.as_deref()).stars;
                pending.push((i, stars));
                let done = i + 1 == files.len();
                if done || pending.len() >= CHUNK || last_sent.elapsed() >= CHUNK_TIME {
                    let items = std::mem::take(&mut pending);
                    let _ = tx.send(Event::Read {
                        generation,
                        items,
                        done,
                    });
                    (shared.repaint)();
                    last_sent = Instant::now();
                }
            }
        }
    }
}

impl MarksWorker {
    pub fn spawn(
        repaint: Box<dyn Fn() + Send + Sync>,
        activity: Arc<Activity>,
    ) -> std::io::Result<Self> {
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                generation: 0,
                files: Arc::new(Vec::new()),
                next: 0,
                writes: VecDeque::new(),
                paused: false,
                shutdown: false,
            }),
            cv: Condvar::new(),
            activity,
            repaint,
        });
        let (tx, events) = mpsc::channel();
        let thread_shared = Arc::clone(&shared);
        let thread = std::thread::Builder::new()
            .name("ccoon-marks".into())
            .spawn(move || run_worker(&thread_shared, &tx))?;
        Ok(Self {
            shared,
            events,
            thread: Some(thread),
        })
    }

    fn load(&self, generation: u64, files: Arc<Vec<PathBuf>>) {
        let mut s = self.shared.lock();
        s.generation = generation;
        s.files = files;
        s.next = 0;
        self.shared.cv.notify_all();
    }

    fn write(&self, file: PathBuf, stars: u8) {
        let mut s = self.shared.lock();
        s.writes.push_back((file, stars));
        self.shared.cv.notify_all();
    }

    fn set_paused(&self, paused: bool) {
        let mut s = self.shared.lock();
        s.paused = paused;
        self.shared.cv.notify_all();
    }
}

impl Drop for MarksWorker {
    fn drop(&mut self) {
        let mut s = self.shared.lock();
        s.shutdown = true;
        self.shared.cv.notify_all();
        drop(s);
        crate::thumbs::join_briefly(self.thread.take().into_iter().collect());
    }
}

/// The stars of every file in the list.
pub struct Marks {
    worker: Option<MarksWorker>,
    generation: u64,
    stars: Vec<u8>,
    /// Files whose stars were changed here; a late read does not replace them.
    touched: HashSet<usize>,
    /// Files that share a sidecar with another file (`A.CR2`, `A.NEF`).
    shares: HashMap<usize, Vec<usize>>,
    loaded: bool,
    /// Bumped whenever any file's stars change.
    revision: u64,
    /// Ratings that could not be saved, kept for this session.
    unsaved: HashMap<PathBuf, u8>,
    notice: Option<String>,
    paused: bool,
}

impl Marks {
    pub fn new(worker: Option<MarksWorker>) -> Self {
        Self {
            worker,
            generation: 0,
            stars: Vec::new(),
            touched: HashSet::new(),
            shares: HashMap::new(),
            loaded: true,
            revision: 0,
            unsaved: HashMap::new(),
            notice: None,
            paused: false,
        }
    }

    /// A new file list: everything starts unknown and is read in the background.
    pub fn reset(&mut self, files: &[PathBuf]) {
        self.generation += 1;
        self.stars = vec![0; files.len()];
        self.touched.clear();
        self.revision += 1;
        self.shares = shared_sidecars(files);
        self.loaded = self.worker.is_none() || files.is_empty();
        if !self.unsaved.is_empty() {
            for (i, f) in files.iter().enumerate() {
                if let Some(&stars) = self.unsaved.get(&sidecar_path(f)) {
                    self.stars[i] = stars;
                    self.touched.insert(i);
                }
            }
        }
        if let Some(w) = &self.worker {
            w.load(self.generation, Arc::new(files.to_vec()));
        }
    }

    /// Once per frame: pause with a batch, and apply what the worker found.
    pub fn poll(&mut self, ctx: &egui::Context, paused: bool) {
        let Some(w) = &self.worker else {
            return;
        };
        if paused != self.paused {
            w.set_paused(paused);
            self.paused = paused;
        }
        let events: Vec<Event> = w.events.try_iter().take(MAX_EVENTS_PER_FRAME).collect();
        let more = events.len() == MAX_EVENTS_PER_FRAME;
        for event in events {
            self.apply(event);
        }
        if more {
            ctx.request_repaint();
        }
    }

    fn apply(&mut self, event: Event) {
        match event {
            Event::Read {
                generation,
                items,
                done,
            } => {
                if generation != self.generation {
                    return;
                }
                let mut changed = false;
                for (i, stars) in items {
                    if self.touched.contains(&i) {
                        continue;
                    }
                    if let Some(slot) = self.stars.get_mut(i) {
                        if *slot != stars {
                            *slot = stars;
                            changed = true;
                        }
                    }
                }
                if changed {
                    self.revision += 1;
                }
                if done {
                    self.loaded = true;
                    self.revision += 1;
                }
            }
            Event::Wrote {
                file,
                stars,
                result,
            } => {
                let key = sidecar_path(&file);
                match result {
                    Ok(()) => {
                        self.unsaved.remove(&key);
                    }
                    Err(failure) => {
                        self.unsaved.insert(key, stars);
                        let name = display_name(&file);
                        self.notice = Some(if failure.unparsable {
                            tf(Key::SidecarUnsafe, &[&name])
                        } else {
                            tf(Key::MarksWriteFailed, &[&name, &failure.detail])
                        });
                    }
                }
            }
        }
    }

    pub fn stars(&self, index: usize) -> u8 {
        self.stars.get(index).copied().unwrap_or(0)
    }

    /// All files' ratings have been read.
    pub fn loaded(&self) -> bool {
        self.loaded
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn take_notice(&mut self) -> Option<String> {
        self.notice.take()
    }

    /// The user set the stars of the file at `index`; 0 clears them.
    pub fn set(&mut self, index: usize, stars: u8, files: &[PathBuf]) {
        if self.stars.get(index).copied() == Some(stars) || index >= files.len() {
            return;
        }
        let shared: Vec<usize> = std::iter::once(index)
            .chain(self.shares.get(&index).into_iter().flatten().copied())
            .collect();
        for i in shared {
            if let Some(slot) = self.stars.get_mut(i) {
                *slot = stars;
                self.touched.insert(i);
            }
        }
        self.revision += 1;
        if let Some(w) = &self.worker {
            w.write(files[index].clone(), stars);
        }
    }
}

/// Files that use the same sidecar, by index.
fn shared_sidecars(files: &[PathBuf]) -> HashMap<usize, Vec<usize>> {
    let mut by_sidecar: HashMap<PathBuf, Vec<usize>> = HashMap::new();
    for (i, f) in files.iter().enumerate() {
        by_sidecar.entry(sidecar_path(f)).or_default().push(i);
    }
    let mut shares = HashMap::new();
    for group in by_sidecar.into_values().filter(|g| g.len() > 1) {
        for &i in &group {
            let others = group.iter().copied().filter(|&o| o != i).collect();
            shares.insert(i, others);
        }
    }
    shares
}

/// Five points of a star around `center`, tip up.
fn star_points(center: Pos2, r: f32) -> [Pos2; 10] {
    let inner = r * 0.45;
    std::array::from_fn(|i| {
        let angle = -std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::PI / 5.0;
        let radius = if i % 2 == 0 { r } else { inner };
        center + vec2(angle.cos(), angle.sin()) * radius
    })
}

fn paint_star(painter: &egui::Painter, center: Pos2, r: f32, filled: bool, hot: bool) {
    let points = star_points(center, r);
    if filled {
        let mut mesh = Mesh::default();
        mesh.colored_vertex(center, ACCENT);
        for p in points {
            mesh.colored_vertex(p, ACCENT);
        }
        for i in 0..10u32 {
            mesh.add_triangle(0, i + 1, (i + 1) % 10 + 1);
        }
        painter.add(Shape::mesh(mesh));
    }
    let color = match (filled, hot) {
        (true, _) => ACCENT,
        (false, true) => NEUTRAL_TEXT,
        (false, false) => NEUTRAL_ACTIVE,
    };
    let outline: Color32 = color;
    painter.add(Shape::closed_line(
        points.to_vec(),
        Stroke::new(1.0, outline),
    ));
}

/// A row of five stars inside `rect`, on `ui`. The new number of stars when
/// clicked: the star clicked, or 0 when it was already the current count.
#[allow(clippy::too_many_arguments)]
pub fn star_row(
    ui: &Ui,
    rect: Rect,
    size: f32,
    pitch: f32,
    value: u8,
    name: &str,
    salt: impl std::hash::Hash + std::fmt::Debug + Copy,
    tip: &str,
) -> Option<u8> {
    let width = pitch * 5.0;
    let left = rect.center().x - width / 2.0;
    let painter = ui.painter_at(rect.expand(1.0));
    let mut result = None;
    for k in 1..=5u8 {
        let cell = Rect::from_min_size(
            pos2(left + f32::from(k - 1) * pitch, rect.top()),
            vec2(pitch, rect.height()),
        );
        let id = egui::Id::unique(("star", salt, k));
        let resp = ui.interact(cell, id, Sense::click());
        let enabled = ui.is_enabled();
        resp.widget_info(|| {
            WidgetInfo::labeled(Role::Button, enabled, tf(Key::StarButton, &[&name, &k]))
        });
        paint_star(
            &painter,
            cell.center(),
            size / 2.0,
            value >= k,
            resp.hovered(),
        );
        if resp.clicked() {
            result = Some(if value == k { 0 } else { k });
        }
        if resp.hovered() {
            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
            let _ = resp.on_hover_text(tip);
        }
    }
    result
}

/// The name of the file a view is about, for accessibility and tooltips.
pub fn display_name(path: &Path) -> String {
    path.file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ccoonverter_core::sidecar::{existing_sidecar, parse_rating};

    fn wait(mut done: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while !done() {
            assert!(Instant::now() < deadline, "timed out");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn saved_then_read_back_by_the_worker() {
        let dir = tempfile::tempdir().unwrap();
        let raw = dir.path().join("a.CR2");
        std::fs::write(&raw, b"RAW").unwrap();
        let worker = MarksWorker::spawn(Box::new(|| {}), Arc::new(Activity::new())).unwrap();
        let ctx = egui::Context::default();
        let files = vec![raw.clone()];
        let mut marks = Marks::new(Some(worker));
        marks.reset(&files);
        marks.set(0, 3, &files);
        wait(|| {
            marks.poll(&ctx, false);
            existing_sidecar(&raw).is_some() && marks.loaded()
        });
        assert_eq!(marks.stars(0), 3);
        let bytes = std::fs::read(existing_sidecar(&raw).unwrap()).unwrap();
        assert_eq!(parse_rating(&bytes), Ok(Some(3)));
        assert_eq!(std::fs::read(&raw).unwrap(), b"RAW", "RAW unchanged");
    }

    #[test]
    fn a_failed_save_keeps_the_mark_and_reports_it() {
        let worker = MarksWorker::spawn(Box::new(|| {}), Arc::new(Activity::new())).unwrap();
        let ctx = egui::Context::default();
        let files = vec![PathBuf::from("Z:/no/such/place/a.CR2")];
        let mut marks = Marks::new(Some(worker));
        marks.reset(&files);
        marks.set(0, 4, &files);
        let mut notice = None;
        wait(|| {
            marks.poll(&ctx, false);
            if notice.is_none() {
                notice = marks.take_notice();
            }
            notice.is_some()
        });
        assert_eq!(marks.stars(0), 4, "kept for this session");
        assert!(notice.unwrap().contains("a.CR2"));
    }

    #[test]
    fn files_with_one_stem_share_their_stars() {
        let files = vec![
            PathBuf::from("D:/s/x.CR2"),
            PathBuf::from("D:/s/x.NEF"),
            PathBuf::from("D:/s/y.NEF"),
        ];
        let mut marks = Marks::new(None);
        marks.reset(&files);
        marks.set(0, 5, &files);
        assert_eq!((marks.stars(0), marks.stars(1), marks.stars(2)), (5, 5, 0));
    }

    #[test]
    fn a_rating_read_after_a_set_does_not_replace_it() {
        let files = vec![PathBuf::from("D:/s/x.CR2")];
        let mut marks = Marks::new(None);
        marks.reset(&files);
        marks.set(0, 2, &files);
        let generation = marks.generation;
        marks.apply(Event::Read {
            generation,
            items: vec![(0, 5)],
            done: true,
        });
        assert_eq!(marks.stars(0), 2);
    }

    #[test]
    fn ratings_read_for_an_old_list_are_ignored() {
        let files = vec![PathBuf::from("D:/s/x.CR2")];
        let mut marks = Marks::new(None);
        marks.reset(&files);
        let old = marks.generation;
        marks.reset(&files);
        marks.apply(Event::Read {
            generation: old,
            items: vec![(0, 5)],
            done: false,
        });
        assert_eq!(marks.stars(0), 0);
    }
}
