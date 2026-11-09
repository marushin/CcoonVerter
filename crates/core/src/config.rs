//! Application-wide constants (port of the Python `config.py`).
//!
//! UI colours and fonts live in the GUI crate; everything that influences the
//! conversion result or file locations lives here.

/// Application name, used for window titles, directories and the User-Agent.
pub const APP_NAME: &str = "CcooNverter";
/// Application version, taken from the workspace `Cargo.toml`.
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// GitHub repository used for update checks.
pub const GITHUB_OWNER: &str = "marushin";
pub const GITHUB_REPO: &str = "CcoonVerter";
pub const GITHUB_API_LATEST_URL: &str =
    "https://api.github.com/repos/marushin/CcoonVerter/releases/latest";
pub const GITHUB_RELEASES_URL: &str = "https://github.com/marushin/CcoonVerter/releases";

/// Default sub-folder (inside the source folder) used when no output folder is chosen.
pub const DEFAULT_OUTPUT_FOLDER_NAME: &str = "Converted_JPG";

/// Legacy (Python 0.8.x) file names, looked up once for migration.
pub const LEGACY_SETTINGS_FILE: &str = "converter_settings.json";
pub const LEGACY_PRESETS_FILE: &str = "presets.json";

/// New file names inside the OS configuration directory.
pub const SETTINGS_FILE: &str = "settings.json";
pub const PRESETS_FILE: &str = "presets.json";
pub const LOG_FILE: &str = "ccoonverter.log";
pub const UPDATE_CACHE_FILE: &str = "update_check.json";

/// Supported RAW extensions (upper case, without dot): the five of Python
/// v0.8.7 plus the formats added in v0.11.0 after decoding real files
/// (`tests/regression/manifest.json`).
pub const SUPPORTED_EXTENSIONS: [&str; 10] = [
    "ARW", "CR2", "CR3", "DNG", "NEF", "RAF", "ORF", "RW2", "PEF", "SRW",
];

// JPEG quality
pub const DEFAULT_JPG_QUALITY: u8 = 90;
pub const MIN_JPG_QUALITY: u8 = 1;
pub const MAX_JPG_QUALITY: u8 = 100;

// RAW brightness (LibRaw `bright` equivalent)
pub const DEFAULT_BRIGHTNESS: f32 = 1.0;
pub const MIN_BRIGHTNESS: f32 = 0.0;
pub const MAX_BRIGHTNESS: f32 = 2.0;

// Exposure compensation in EV, applied as a linear gain together with
// brightness: `gain = auto_exposure * brightness * 2^ev` (see `render`).
pub const DEFAULT_EXPOSURE_EV: f32 = 0.0;
pub const MIN_EXPOSURE_EV: f32 = -3.0;
pub const MAX_EXPOSURE_EV: f32 = 3.0;

/// Light unsharp mask applied at output resolution (0 = off).
pub const DEFAULT_SHARPEN: f32 = 0.35;
pub const MIN_SHARPEN: f32 = 0.0;
pub const MAX_SHARPEN: f32 = 2.0;

// Resize (long edge in pixels, 0 = original)
pub const DEFAULT_RESIZE: u32 = 0;
pub const MIN_RESIZE: u32 = 0;
/// Upper bound to protect against absurd allocations (not present in Python).
pub const MAX_RESIZE: u32 = 30_000;

// RGB channel multipliers
pub const DEFAULT_RGB_VALUE: f32 = 1.0;
pub const MIN_RGB_VALUE: f32 = 0.0;
pub const MAX_RGB_VALUE: f32 = 2.0;

// Levels (8-bit scale)
pub const DEFAULT_BLACK_POINT: u8 = 0;
pub const MIN_BLACK_POINT: u8 = 0;
pub const MAX_BLACK_POINT: u8 = 100;
pub const DEFAULT_WHITE_POINT: u8 = 255;
pub const MIN_WHITE_POINT: u8 = 150;
pub const MAX_WHITE_POINT: u8 = 255;

/// Log file size that triggers rotation (bytes).
pub const LOG_ROTATE_BYTES: u64 = 5 * 1024 * 1024;

/// How long a cached update check stays valid.
pub const UPDATE_CACHE_SECS: u64 = 24 * 60 * 60;
/// HTTP timeout for the update check.
pub const UPDATE_TIMEOUT_SECS: u64 = 10;

/// User-Agent sent to GitHub.
pub fn user_agent() -> String {
    format!("{APP_NAME}/{APP_VERSION}")
}

/// Returns true if `ext` (with or without leading dot, any case) is a supported RAW extension.
pub fn is_supported_extension(ext: &str) -> bool {
    let ext = ext.trim_start_matches('.');
    SUPPORTED_EXTENSIONS
        .iter()
        .any(|e| e.eq_ignore_ascii_case(ext))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extensions_case_insensitive() {
        assert!(is_supported_extension("arw"));
        assert!(is_supported_extension(".Cr3"));
        assert!(is_supported_extension("NEF"));
        assert!(is_supported_extension("RAF"));
        assert!(is_supported_extension(".orf"));
        assert!(is_supported_extension("Rw2"));
        assert!(is_supported_extension("pef"));
        assert!(is_supported_extension("SRW"));
        assert!(!is_supported_extension("jpg"));
        assert!(!is_supported_extension("X3F"));
    }

    #[test]
    fn user_agent_has_version() {
        assert_eq!(user_agent(), format!("CcooNverter/{}", APP_VERSION));
    }
}
