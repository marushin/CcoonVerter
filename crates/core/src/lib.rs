//! `ccoonverter-core`: the RAW → JPEG conversion engine of CcooNverter.
//!
//! Module map (Python v0.8.7 counterpart in brackets):
//! - [`config`] constants [`config.py`]
//! - [`settings`] conversion/app settings + validation [`get_settings`, `validate_settings`]
//! - [`paths`] OS directories and legacy migration [new]
//! - [`presets`] named presets [`preset_manager.py`]
//! - [`scan`] RAW discovery [`scan_folder`]
//! - [`embedded`] embedded JPEG extraction [`raw.extract_thumb()`]
//! - [`develop`] rawler development [`raw.postprocess()`]
//! - [`adjust`] RGB multipliers + Levels
//! - [`style`] 12 picture styles [Pillow `ImageEnhance`/`ImageOps`]
//! - [`resize`] long-edge Lanczos resize, orientation, quantisation
//! - [`exif`] EXIF reuse / builder
//! - [`encode`] JPEG encoder
//! - [`pipeline`] single file conversion [`convert_file`]
//! - [`job`] parallel batch [`run_conversion`]
//! - [`error`] error type [`ConversionError`]

pub mod adjust;
pub mod config;
pub mod develop;
pub mod embedded;
pub mod encode;
pub mod error;
pub mod exif;
pub mod job;
pub mod paths;
pub mod pipeline;
pub mod presets;
pub mod resize;
pub mod scan;
pub mod settings;
pub mod style;
#[doc(hidden)]
pub mod testutil;

/// Working image type: RGB, f32 per channel, sRGB-encoded values in 0..1.
pub type RgbF32Image = image::Rgb32FImage;

pub use error::{ConvertError, Result};
pub use job::{run_batch, BatchSummary, FileResult, FileStatus, Progress};
pub use pipeline::{convert_one, plan_tasks, ConvertOutcome, FileTask, Method};
pub use settings::{AppSettings, ConflictPolicy, ConvertSettings, PictureStyle, WhiteBalance};
