//! Named presets (port of `preset_manager.py`).
//!
//! File format is the Python one: a JSON object mapping preset name to a
//! (possibly partial) settings object. Missing keys take default values.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::settings::ConvertSettings;

/// In-memory preset collection, sorted by name.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PresetStore {
    presets: BTreeMap<String, ConvertSettings>,
}

impl PresetStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Parse JSON text. Invalid individual entries are skipped (logged).
    pub fn from_json(text: &str) -> Result<Self, serde_json::Error> {
        let raw: BTreeMap<String, serde_json::Value> = serde_json::from_str(text)?;
        let mut presets = BTreeMap::new();
        for (name, value) in raw {
            match serde_json::from_value::<ConvertSettings>(value) {
                Ok(s) => {
                    presets.insert(name, s.sanitized());
                }
                Err(e) => log::warn!("Skipping invalid preset '{name}': {e}"),
            }
        }
        Ok(Self { presets })
    }

    /// Load from disk. Missing or invalid files give an empty store (logged), like Python.
    pub fn load(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(text) => match Self::from_json(&text) {
                Ok(s) => {
                    log::info!("Loaded {} presets from {}", s.len(), path.display());
                    s
                }
                Err(e) => {
                    log::error!("Invalid JSON in preset file {}: {e}", path.display());
                    Self::default()
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(e) => {
                log::error!("Error loading presets {}: {e}", path.display());
                Self::default()
            }
        }
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let text = serde_json::to_string_pretty(self).expect("presets serialise");
        crate::paths::write_atomic(path, text.as_bytes())
    }

    /// Sorted preset names.
    pub fn names(&self) -> Vec<String> {
        self.presets.keys().cloned().collect()
    }

    pub fn get(&self, name: &str) -> Option<&ConvertSettings> {
        self.presets.get(name)
    }

    /// Insert or replace a preset.
    pub fn insert(&mut self, name: impl Into<String>, settings: ConvertSettings) {
        self.presets.insert(name.into(), settings);
    }

    /// Remove a preset; returns true if it existed.
    pub fn remove(&mut self, name: &str) -> bool {
        self.presets.remove(name).is_some()
    }

    pub fn len(&self) -> usize {
        self.presets.len()
    }

    pub fn is_empty(&self) -> bool {
        self.presets.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{PictureStyle, WhiteBalance};

    #[test]
    fn reads_python_presets() {
        let json = r#"{
          "0": {"resize": 2000, "quality": 90, "brightness": 1.1422764227642275,
                "white_balance": "Auto", "style": "Standard"},
          "Night": {"style": "Cinematic", "black_point": 10, "white_point": 230,
                    "extract_embedded": false, "preserve_exif": true}
        }"#;
        let s = PresetStore::from_json(json).unwrap();
        assert_eq!(s.names(), vec!["0".to_string(), "Night".to_string()]);
        let p = s.get("0").unwrap();
        assert_eq!(p.resize, 2000);
        assert_eq!(p.white_balance, WhiteBalance::Auto);
        let n = s.get("Night").unwrap();
        assert_eq!(n.style, PictureStyle::Cinematic);
        assert_eq!(n.black_point, 10);
        assert!(n.preserve_exif);
    }

    #[test]
    fn save_load_delete_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("presets.json");
        let mut s = PresetStore::new();
        s.insert(
            "한글 프리셋",
            ConvertSettings {
                quality: 80,
                ..Default::default()
            },
        );
        s.insert("B", ConvertSettings::default());
        s.save(&path).unwrap();
        let mut loaded = PresetStore::load(&path);
        assert_eq!(loaded, s);
        assert!(loaded.remove("B"));
        assert!(!loaded.remove("B"));
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded.get("한글 프리셋").unwrap().quality, 80);
    }

    #[test]
    fn invalid_file_gives_empty_store() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("presets.json");
        std::fs::write(&path, "{ not json").unwrap();
        assert!(PresetStore::load(&path).is_empty());
        assert!(PresetStore::load(&dir.path().join("missing.json")).is_empty());
    }
}
