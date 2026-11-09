//! Logging setup (port of `logger.py`): file + stderr via `fern`.
//!
//! The log file lives in `<data_dir>/logs/ccoonverter.log`. When it exceeds
//! [`LOG_ROTATE_BYTES`](crate::config::LOG_ROTATE_BYTES) at start-up it is
//! renamed to `ccoonverter.log.1` (one generation kept), so it never grows
//! without bound like Python's `converter.log` did.

use std::fs;
use std::path::{Path, PathBuf};

use log::LevelFilter;

use crate::config;

/// Logging options.
#[derive(Debug, Clone)]
pub struct LogOptions {
    /// Log file; `None` disables file logging.
    pub file: Option<PathBuf>,
    /// Level for our own crates (and the file).
    pub level: LevelFilter,
    /// Level written to stderr (`Off` for GUI release builds).
    pub stderr_level: LevelFilter,
}

impl LogOptions {
    /// Defaults: Info to file, Warn to stderr; `RUST_LOG` (a plain level
    /// name such as `debug`) overrides the file/main level.
    pub fn new(file: Option<PathBuf>, verbose: bool) -> Self {
        let env = std::env::var("RUST_LOG")
            .ok()
            .and_then(|v| v.parse::<LevelFilter>().ok());
        let level = env.unwrap_or(if verbose {
            LevelFilter::Debug
        } else {
            LevelFilter::Info
        });
        Self {
            file,
            level,
            stderr_level: if verbose { level } else { LevelFilter::Warn },
        }
    }
}

/// Rotate `path` to `path.1` if it is larger than `max_bytes`.
pub fn rotate_if_needed(path: &Path, max_bytes: u64) -> std::io::Result<bool> {
    match fs::metadata(path) {
        Ok(m) if m.len() > max_bytes => {
            let mut old = path.as_os_str().to_owned();
            old.push(".1");
            let old = PathBuf::from(old);
            let _ = fs::remove_file(&old);
            fs::rename(path, &old)?;
            Ok(true)
        }
        _ => Ok(false),
    }
}

/// Install the global logger. Safe to call once; a second call returns an error.
pub fn init(opts: &LogOptions) -> Result<(), String> {
    let main = opts.level.max(opts.stderr_level);
    // rawler is chatty (lens database misses etc.); keep it at warnings
    // unless debugging.
    let rawler_level = if main >= LevelFilter::Debug {
        LevelFilter::Info
    } else {
        LevelFilter::Error
    };
    let base = fern::Dispatch::new()
        .level(main)
        .level_for("rawler", rawler_level)
        .level_for("ureq", LevelFilter::Warn)
        .level_for("ureq_proto", LevelFilter::Warn)
        .level_for("rustls", LevelFilter::Warn);

    let stderr = fern::Dispatch::new()
        .level(opts.stderr_level)
        .format(|out, msg, rec| out.finish(format_args!("[{}] {}", rec.level(), msg)))
        .chain(std::io::stderr());

    let mut dispatch = base.chain(stderr);

    if let Some(file) = &opts.file {
        if let Some(dir) = file.parent() {
            fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        if let Err(e) = rotate_if_needed(file, config::LOG_ROTATE_BYTES) {
            eprintln!("log rotation failed: {e}");
        }
        let f = fern::log_file(file).map_err(|e| e.to_string())?;
        let file_dispatch = fern::Dispatch::new()
            .level(opts.level)
            .format(|out, msg, rec| {
                out.finish(format_args!(
                    "{} - {} - {} - {}",
                    chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f"),
                    rec.target(),
                    rec.level(),
                    msg
                ))
            })
            .chain(f);
        dispatch = dispatch.chain(file_dispatch);
    }
    dispatch.apply().map_err(|e| e.to_string())?;
    log::info!(
        "{} v{} logging initialised",
        config::APP_NAME,
        config::APP_VERSION
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotation_by_size() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x.log");
        fs::write(&p, vec![b'a'; 100]).unwrap();
        assert!(!rotate_if_needed(&p, 1000).unwrap());
        assert!(rotate_if_needed(&p, 10).unwrap());
        assert!(!p.exists());
        assert!(dir.path().join("x.log.1").exists());
        // Rotating again replaces the old generation.
        fs::write(&p, vec![b'b'; 100]).unwrap();
        assert!(rotate_if_needed(&p, 10).unwrap());
        assert_eq!(fs::read(dir.path().join("x.log.1")).unwrap()[0], b'b');
        assert!(!rotate_if_needed(&dir.path().join("missing.log"), 1).unwrap());
    }
}
