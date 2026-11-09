//! Diagnostic capture of the real window. Only compiled with
//! `--features capture`; the shipped build has none of this.
//!
//! The pixels come from the renderer itself through
//! `Context::request_screenshot` (eframe reads the framebuffer it just drew),
//! so it works on the OpenGL surface that `PrintWindow` cannot see. Nothing
//! reads the desktop and nothing sends input.
//!
//! - `CCOON_CAPTURE_OUT`: PNG to write. Without it the app runs normally.
//! - `CCOON_CAPTURE_FOLDER`: RAW folder to open instead of the saved one.
//! - `CCOON_CAPTURE_FILE`: index of the file in that folder (default 0).

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;

pub const OUT_ENV: &str = "CCOON_CAPTURE_OUT";
pub const FOLDER_ENV: &str = "CCOON_CAPTURE_FOLDER";
pub const FILE_ENV: &str = "CCOON_CAPTURE_FILE";

/// Give up after this long, so a failed capture cannot leave a window open.
const TIMEOUT: Duration = Duration::from_secs(180);
/// Frames keep coming for this long after the preview is ready, so a texture
/// uploaded in the last frame is on screen in the captured one.
const SETTLE: Duration = Duration::from_millis(500);

pub struct Capture {
    out: PathBuf,
    file_index: usize,
    started: Instant,
    ready_since: Option<Instant>,
    requested: bool,
    finished: Arc<AtomicBool>,
}

impl Capture {
    pub fn from_env() -> Option<Self> {
        let out = PathBuf::from(std::env::var_os(OUT_ENV)?);
        let file_index = std::env::var(FILE_ENV)
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        Some(Self {
            out,
            file_index,
            started: Instant::now(),
            ready_since: None,
            requested: false,
            finished: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Folder to open instead of the one in the settings.
    pub fn folder_override() -> Option<String> {
        std::env::var(FOLDER_ENV).ok()
    }

    pub fn file_index(&self) -> usize {
        self.file_index
    }

    /// Once per frame. `ready` means the preview and its thumbnails are shown.
    pub fn tick(&mut self, ctx: &egui::Context, ready: bool) {
        // Keep frames coming even when the window is in the background.
        ctx.request_repaint_after(Duration::from_millis(50));
        if self.finished.load(Ordering::Relaxed) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        if self.started.elapsed() > TIMEOUT {
            log::error!("Capture: the preview was not ready after {TIMEOUT:?}");
            self.finished.store(true, Ordering::Relaxed);
            return;
        }
        if !ready || self.requested {
            return;
        }
        let since = *self.ready_since.get_or_insert_with(Instant::now);
        if since.elapsed() < SETTLE {
            return;
        }
        self.requested = true;
        let out = self.out.clone();
        let finished = Arc::clone(&self.finished);
        let repaint = ctx.clone();
        log::info!(
            "Capture: renderer screenshot at {} points per pixel, window {:?}",
            ctx.pixels_per_point(),
            ctx.content_rect().size()
        );
        ctx.request_screenshot(move |image| {
            match save(&image, &out) {
                Ok(()) => log::info!(
                    "Capture: wrote {} ({}x{})",
                    out.display(),
                    image.size[0],
                    image.size[1]
                ),
                Err(e) => log::error!("Capture: could not write {}: {e}", out.display()),
            }
            finished.store(true, Ordering::Relaxed);
            repaint.request_repaint();
        });
    }
}

fn save(image: &egui::ColorImage, out: &PathBuf) -> image::ImageResult<()> {
    // The window has no transparency, whatever the alpha plane holds.
    let bytes = image
        .pixels
        .iter()
        .flat_map(|p| [p.r(), p.g(), p.b(), 255])
        .collect();
    let [w, h] = image.size;
    image::RgbaImage::from_raw(w as u32, h as u32, bytes)
        .expect("buffer matches the image size")
        .save(out)
}
