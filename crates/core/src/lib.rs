//! `ccoonverter-core`: the RAW to JPEG conversion engine of CcooNverter.

pub mod adjust;
pub mod config;
pub mod embedded;
pub mod encode;
pub mod error;
pub mod exif;
pub mod paths;
pub mod presets;
pub mod resize;
pub mod scan;
pub mod settings;
pub mod style;

/// Working image type: RGB, f32 per channel, sRGB-encoded values in 0..1.
pub type RgbF32Image = image::Rgb32FImage;

pub use error::{ConvertError, Result};
pub use settings::{AppSettings, ConflictPolicy, ConvertSettings, PictureStyle, WhiteBalance};
