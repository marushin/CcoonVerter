//! Locations of settings, presets, logs and caches.
//!
//! Python 0.8.x wrote everything into the current working directory. 0.9.0
//! uses the OS standard directories via `directories::ProjectDirs`
//! (`%APPDATA%\ccoon\CcooNverter\config` + `%LOCALAPPDATA%\ccoon\CcooNverter\...`
//! on Windows, `~/Library/Application Support/ccoon.CcooNverter` on macOS) and
//! imports the legacy files from the executable's folder once.
//!
//! Setting the environment variable `CCOONVERTER_HOME` puts every file under
//! that directory instead (portable mode / tests).

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::config;

/// Environment variable that overrides all directories.
pub const HOME_ENV: &str = "CCOONVERTER_HOME";

/// Resolved application directories.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppPaths {
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
}

impl AppPaths {
    /// Resolve directories from `CCOONVERTER_HOME` or the OS conventions.
    /// Falls back to `./CcooNverter-data` if the OS gives no home directory.
    pub fn resolve() -> Self {
        if let Some(home) = std::env::var_os(HOME_ENV).filter(|v| !v.is_empty()) {
            return Self::with_root(PathBuf::from(home));
        }
        match directories::ProjectDirs::from("", "ccoon", config::APP_NAME) {
            Some(p) => Self {
                config_dir: p.config_dir().to_path_buf(),
                data_dir: p.data_local_dir().to_path_buf(),
                cache_dir: p.cache_dir().to_path_buf(),
            },
            None => Self::with_root(PathBuf::from("CcooNverter-data")),
        }
    }

    /// Put everything below one root directory.
    pub fn with_root(root: PathBuf) -> Self {
        Self {
            config_dir: root.join("config"),
            data_dir: root.join("data"),
            cache_dir: root.join("cache"),
        }
    }

    pub fn settings_file(&self) -> PathBuf {
        self.config_dir.join(config::SETTINGS_FILE)
    }

    pub fn presets_file(&self) -> PathBuf {
        self.config_dir.join(config::PRESETS_FILE)
    }

    pub fn log_dir(&self) -> PathBuf {
        self.data_dir.join("logs")
    }

    pub fn log_file(&self) -> PathBuf {
        self.log_dir().join(config::LOG_FILE)
    }

    pub fn update_cache_file(&self) -> PathBuf {
        self.cache_dir.join(config::UPDATE_CACHE_FILE)
    }

    /// Import legacy Python files (`converter_settings.json`, `presets.json`)
    /// from the executable's directory (where the Python exe kept them when
    /// double-clicked), but only when the corresponding new file does not
    /// exist yet. The current working directory is deliberately **not**
    /// searched: a shell started in some unrelated folder must not import its
    /// JSON files. Returns the imported files.
    pub fn migrate_legacy(&self) -> Vec<PathBuf> {
        self.migrate_legacy_from(&exe_dir().into_iter().collect::<Vec<_>>())
    }

    /// Same as [`migrate_legacy`](Self::migrate_legacy) with explicit search directories.
    pub fn migrate_legacy_from(&self, search_dirs: &[PathBuf]) -> Vec<PathBuf> {
        let mut imported = Vec::new();

        let settings_dst = self.settings_file();
        if !settings_dst.exists() {
            if let Some(src) = find_first(search_dirs, config::LEGACY_SETTINGS_FILE) {
                match fs::read_to_string(&src) {
                    Ok(text) => match crate::settings::AppSettings::from_json(&text) {
                        Ok(s) => match s.save(&settings_dst) {
                            Ok(()) => {
                                log::info!("Imported legacy settings from {}", src.display());
                                imported.push(src);
                            }
                            Err(e) => log::warn!("Could not write migrated settings: {e}"),
                        },
                        Err(e) => log::warn!("Legacy settings {} invalid: {e}", src.display()),
                    },
                    Err(e) => log::warn!("Could not read {}: {e}", src.display()),
                }
            }
        }

        let presets_dst = self.presets_file();
        if !presets_dst.exists() {
            // Never import our own new presets file as "legacy" (same name).
            let legacy_dirs: Vec<PathBuf> = search_dirs
                .iter()
                .filter(|d| !same_dir(d, &self.config_dir))
                .cloned()
                .collect();
            if let Some(src) = find_first(&legacy_dirs, config::LEGACY_PRESETS_FILE) {
                let store = crate::presets::PresetStore::load(&src);
                if !store.is_empty() {
                    match store.save(&presets_dst) {
                        Ok(()) => {
                            log::info!(
                                "Imported {} legacy presets from {}",
                                store.len(),
                                src.display()
                            );
                            imported.push(src);
                        }
                        Err(e) => log::warn!("Could not write migrated presets: {e}"),
                    }
                }
            }
        }
        imported
    }
}

/// Directory containing the running executable.
fn exe_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
}

fn find_first(dirs: &[PathBuf], name: &str) -> Option<PathBuf> {
    dirs.iter().map(|d| d.join(name)).find(|p| p.is_file())
}

fn same_dir(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// Write a file via a temporary sibling + rename so a crash never leaves a
/// half-written settings file. Creates parent directories.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrates_legacy_files_once() {
        let legacy = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        fs::write(
            legacy.path().join("converter_settings.json"),
            r#"{"quality": 77, "style": "Vivid", "output_folder": ""}"#,
        )
        .unwrap();
        fs::write(
            legacy.path().join("presets.json"),
            r#"{"0": {"resize": 2000, "quality": 90, "brightness": 1.14, "white_balance": "Auto", "style": "Standard"}}"#,
        )
        .unwrap();

        let paths = AppPaths::with_root(home.path().to_path_buf());
        let imported = paths.migrate_legacy_from(&[legacy.path().to_path_buf()]);
        assert_eq!(imported.len(), 2);

        let s = crate::settings::AppSettings::load(&paths.settings_file());
        assert_eq!(s.convert.quality, 77);
        let p = crate::presets::PresetStore::load(&paths.presets_file());
        assert_eq!(p.get("0").unwrap().resize, 2000);

        // Second run: nothing to do, existing files are not overwritten.
        fs::write(
            legacy.path().join("converter_settings.json"),
            r#"{"quality": 11}"#,
        )
        .unwrap();
        assert!(paths
            .migrate_legacy_from(&[legacy.path().to_path_buf()])
            .is_empty());
        let s = crate::settings::AppSettings::load(&paths.settings_file());
        assert_eq!(s.convert.quality, 77);
    }

    #[test]
    fn migrate_legacy_ignores_current_dir() {
        // Legacy files in the CWD must not be picked up; only the exe folder
        // (the test binary's `target/.../deps`, which has none) is searched.
        let cwd = std::env::current_dir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let paths = AppPaths::with_root(home.path().to_path_buf());
        let imported = paths.migrate_legacy();
        assert!(
            imported.iter().all(|p| p.parent() != Some(cwd.as_path())),
            "imported from CWD: {imported:?}"
        );
        assert_eq!(
            exe_dir(),
            std::env::current_exe()
                .unwrap()
                .parent()
                .map(Path::to_path_buf)
        );
    }

    #[test]
    fn with_root_layout() {
        let p = AppPaths::with_root(PathBuf::from("X"));
        assert_eq!(p.settings_file(), PathBuf::from("X/config/settings.json"));
        assert_eq!(p.log_file(), PathBuf::from("X/data/logs/ccoonverter.log"));
    }
}
