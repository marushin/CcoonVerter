//! `ccoonverter-core`: the RAW to JPEG conversion engine of CcooNverter.

pub mod config;
pub mod error;
pub mod paths;
pub mod presets;
pub mod scan;
pub mod settings;

/// Working image type: RGB, f32 per channel, sRGB-encoded values in 0..1.
pub type RgbF32Image = image::Rgb32FImage;

pub use error::{ConvertError, Result};
pub use settings::{AppSettings, ConflictPolicy, ConvertSettings, PictureStyle, WhiteBalance};
