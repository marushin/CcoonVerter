//! Error types for the conversion engine.

use std::path::PathBuf;

/// Errors produced while converting a single file or loading configuration.
#[derive(Debug, thiserror::Error)]
pub enum ConvertError {
    #[error("File I/O error on {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("RAW processing error: {0}")]
    Decode(String),

    #[error("No embedded JPEG preview found")]
    NoEmbeddedJpeg,

    #[error("Invalid settings: {0}")]
    InvalidSettings(String),

    #[error("Cancelled")]
    Cancelled,

    #[error("EXIF error: {0}")]
    Exif(String),

    #[error("JPEG encoding error: {0}")]
    Encode(String),

    #[error("Image error: {0}")]
    Image(#[from] image::ImageError),

    #[error("Unexpected internal error: {0}")]
    Panic(String),
}

impl ConvertError {
    /// Helper to attach a path to an I/O error.
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }

    /// True if this error represents a user cancellation rather than a failure.
    pub fn is_cancelled(&self) -> bool {
        matches!(self, Self::Cancelled)
    }
}

impl From<rawler::RawlerError> for ConvertError {
    fn from(e: rawler::RawlerError) -> Self {
        Self::Decode(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, ConvertError>;
