//! `ccoonverter-core`: the RAW → JPEG conversion engine of CcooNverter.
//!
//! Module map (Python v0.8.7 counterpart in brackets):
//! - [`config`] constants [`config.py`]
//! - [`settings`] conversion/app settings + validation [`get_settings`, `validate_settings`]
//! - [`paths`] OS directories and legacy migration [new]
//! - [`presets`] named presets [`preset_manager.py`]
//! - [`scan`] RAW discovery [`scan_folder`]
//! - [`embedded`] embedded JPEG extraction [`raw.extract_thumb()`]
//! - [`develop`] rawler development to linear RGB + exposure statistics
//! - [`render`] the shared chain from linear base to final pixels: exposure,
//!   highlights, tone curve, adjustments, style, resize, sharpening [new]
//! - [`preview`] reduced-resolution sources for the GUI preview [new]
//! - [`adjust`] RGB multipliers + Levels
//! - [`style`] 12 picture styles [Pillow `ImageEnhance`/`ImageOps`]
//! - [`sharpen`] output unsharp mask [new]
//! - [`resize`] long-edge Lanczos resize, orientation, quantisation
//! - [`exif`] EXIF reuse / builder
//! - [`encode`] JPEG encoder
//! - [`pipeline`] single file conversion [`convert_file`]
//! - [`job`] parallel batch [`run_conversion`]
//! - [`update`] GitHub release check [`update_checker.py`]
//! - [`logging`] log setup [`logger.py`]
//! - [`error`] error type [`ConversionError`]

pub mod adjust;
pub mod config;
pub mod develop;
pub mod embedded;
pub mod encode;
pub mod error;
pub mod exif;
pub mod job;
pub mod logging;
pub mod paths;
pub mod pipeline;
pub mod presets;
pub mod preview;
pub mod render;
pub mod resize;
pub mod scan;
pub mod settings;
pub mod sharpen;
pub mod style;
#[doc(hidden)]
pub mod testutil;
pub mod update;

/// Working image type: RGB, f32 per channel, sRGB-encoded values in 0..1.
pub type RgbF32Image = image::Rgb32FImage;

/// Re-export of the pinned rawler version (used by the CLI `info` command).
pub use rawler;

pub use error::{ConvertError, Result};
pub use job::{run_batch, BatchSummary, FileResult, FileStatus, Progress};
pub use pipeline::{convert_one, plan_tasks, ConvertOutcome, FileTask, Method};
pub use settings::{AppSettings, ConflictPolicy, ConvertSettings, PictureStyle, WhiteBalance};
