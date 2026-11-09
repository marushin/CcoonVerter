//! Error types for the conversion engine.

use std::path::PathBuf;

use serde::Serialize;

/// Broad reason a file failed, for the user-facing failure list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureKind {
    /// The decoder does not know this camera.
    UnsupportedCamera,
    /// The camera is known but this RAW compression cannot be decoded.
    UnsupportedCompression,
    /// Damaged file, or not a RAW file the decoder understands.
    Unreadable,
    NoEmbeddedJpeg,
    /// Reading or writing a file failed (disk full, permissions, ...).
    Disk,
    Encode,
    Internal,
}

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

    /// rawler cannot develop this file. Not retried in another mode: the user
    /// chooses camera JPEG mode.
    #[error("Cannot develop this RAW file: {detail}{}", camera_jpeg_hint(*has_camera_jpeg))]
    UnsupportedRaw {
        detail: String,
        kind: FailureKind,
        has_camera_jpeg: bool,
    },

    #[error("No embedded JPEG preview found")]
    NoEmbeddedJpeg,

    #[error("Invalid settings: {0}")]
    InvalidSettings(String),

    #[error("Cancelled")]
    Cancelled,

    #[error("EXIF error: {0}")]
    Exif(String),

    #[error("Encoding error: {0}")]
    Encode(String),

    #[error("Image error: {0}")]
    Image(#[from] image::ImageError),

    #[error("Unexpected internal error: {0}")]
    Panic(String),
}

fn camera_jpeg_hint(has_camera_jpeg: bool) -> &'static str {
    if has_camera_jpeg {
        ". Camera JPEG mode (Extract Embedded JPEG) can still extract the camera's JPEG from it"
    } else {
        ""
    }
}

/// Classify a decoder message as an unsupported camera or compression.
pub fn classify_unsupported(message: &str) -> Option<FailureKind> {
    let m = message.to_ascii_lowercase();
    let cannot =
        m.contains("not supported") || m.contains("unsupported") || m.contains("don't know");
    if m.contains("compression") && cannot {
        Some(FailureKind::UnsupportedCompression)
    } else if m.contains("unknown camera")
        || (m.contains(", model '") && m.contains(", make: '") && !m.contains(", model ''"))
    {
        Some(FailureKind::UnsupportedCamera)
    } else {
        None
    }
}

impl ConvertError {
    /// Broad reason for the failure list.
    pub fn failure_kind(&self) -> FailureKind {
        match self {
            Self::Io { .. } => FailureKind::Disk,
            Self::Decode(_) => FailureKind::Unreadable,
            Self::UnsupportedRaw { kind, .. } => *kind,
            Self::NoEmbeddedJpeg => FailureKind::NoEmbeddedJpeg,
            Self::Encode(_) | Self::Image(_) | Self::Exif(_) => FailureKind::Encode,
            Self::InvalidSettings(_) | Self::Cancelled | Self::Panic(_) => FailureKind::Internal,
        }
    }

    /// True when camera JPEG mode would still work for this failed file.
    pub fn camera_jpeg_available(&self) -> bool {
        matches!(
            self,
            Self::UnsupportedRaw {
                has_camera_jpeg: true,
                ..
            }
        )
    }

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
