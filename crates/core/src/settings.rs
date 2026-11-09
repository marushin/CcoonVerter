//! Conversion and application settings.
//!
//! The JSON layout is intentionally identical to the Python v0.8.x
//! `converter_settings.json` / `presets.json` entries (flat object with
//! `resize`, `quality`, `brightness`, `white_balance`, `style`, `rgb_red`,
//! `rgb_green`, `rgb_blue`, `black_point`, `white_point`, `extract_embedded`,
//! `preserve_exif`, `output_folder`), so old files can be read unchanged.
//! New keys (`on_conflict`, `check_updates`, `last_source`) have defaults.
//!
//! Deserialisation is lenient: numbers may be given as int or float, unknown
//! enum strings fall back to the default, and out-of-range values are clamped
//! by [`ConvertSettings::sanitized`]. Strict checking (used before a batch
//! starts) is done by [`ConvertSettings::validate`].

use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::config::*;

/// White balance mode (Python `WHITE_BALANCE_OPTIONS`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub enum WhiteBalance {
    #[default]
    CameraDefault,
    Auto,
}

impl WhiteBalance {
    pub const ALL: [WhiteBalance; 2] = [WhiteBalance::CameraDefault, WhiteBalance::Auto];

    /// Display / JSON name, identical to Python.
    pub fn as_str(self) -> &'static str {
        match self {
            WhiteBalance::CameraDefault => "Camera Default",
            WhiteBalance::Auto => "Auto",
        }
    }
}

impl fmt::Display for WhiteBalance {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for WhiteBalance {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let n = normalize_name(s);
        match n.as_str() {
            "cameradefault" | "camera" | "default" | "asshot" => Ok(WhiteBalance::CameraDefault),
            "auto" => Ok(WhiteBalance::Auto),
            _ => Err(format!(
                "unknown white balance '{s}' (expected 'Camera Default' or 'Auto')"
            )),
        }
    }
}

/// Picture style (Python `STYLE_OPTIONS`, same order).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub enum PictureStyle {
    #[default]
    Standard,
    Portrait,
    Landscape,
    Neutral,
    Vivid,
    Monochrome,
    Sepia,
    Cinematic,
    Hdr,
    Soft,
    Dramatic,
    BlackAndWhite,
}

impl PictureStyle {
    /// All styles in the UI order used by Python.
    pub const ALL: [PictureStyle; 12] = [
        PictureStyle::Standard,
        PictureStyle::Portrait,
        PictureStyle::Landscape,
        PictureStyle::Neutral,
        PictureStyle::Vivid,
        PictureStyle::Monochrome,
        PictureStyle::Sepia,
        PictureStyle::Cinematic,
        PictureStyle::Hdr,
        PictureStyle::Soft,
        PictureStyle::Dramatic,
        PictureStyle::BlackAndWhite,
    ];

    /// Display / JSON name, identical to Python.
    pub fn as_str(self) -> &'static str {
        match self {
            PictureStyle::Standard => "Standard",
            PictureStyle::Portrait => "Portrait",
            PictureStyle::Landscape => "Landscape",
            PictureStyle::Neutral => "Neutral",
            PictureStyle::Vivid => "Vivid",
            PictureStyle::Monochrome => "Monochrome",
            PictureStyle::Sepia => "Sepia",
            PictureStyle::Cinematic => "Cinematic",
            PictureStyle::Hdr => "HDR",
            PictureStyle::Soft => "Soft",
            PictureStyle::Dramatic => "Dramatic",
            PictureStyle::BlackAndWhite => "Black & White",
        }
    }
}

impl fmt::Display for PictureStyle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for PictureStyle {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let n = normalize_name(s);
        if let Some(st) = PictureStyle::ALL
            .iter()
            .find(|st| normalize_name(st.as_str()) == n)
        {
            return Ok(*st);
        }
        match n.as_str() {
            "bw" | "blackandwhite" | "blackwhite" | "bnw" => Ok(PictureStyle::BlackAndWhite),
            "mono" => Ok(PictureStyle::Monochrome),
            _ => Err(format!("unknown picture style '{s}'")),
        }
    }
}

/// What to do when the output JPEG already exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConflictPolicy {
    /// Leave the existing file and report the source as skipped.
    Skip,
    /// Replace the existing file (Python v0.8.x behaviour).
    Overwrite,
    /// Write `name_<ext>.jpg`, `name_<ext>_1.jpg`, ... instead.
    #[default]
    Suffix,
}

impl ConflictPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            ConflictPolicy::Skip => "skip",
            ConflictPolicy::Overwrite => "overwrite",
            ConflictPolicy::Suffix => "suffix",
        }
    }
}

impl FromStr for ConflictPolicy {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match normalize_name(s).as_str() {
            "skip" => Ok(ConflictPolicy::Skip),
            "overwrite" | "replace" => Ok(ConflictPolicy::Overwrite),
            "suffix" | "rename" => Ok(ConflictPolicy::Suffix),
            _ => Err(format!(
                "unknown conflict policy '{s}' (expected skip, overwrite or suffix)"
            )),
        }
    }
}

fn normalize_name(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

macro_rules! string_enum_serde {
    ($t:ty) => {
        impl Serialize for $t {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(self.as_str())
            }
        }
        impl<'de> Deserialize<'de> for $t {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let v = serde_json::Value::deserialize(d)?;
                Ok(match v.as_str().map(<$t>::from_str) {
                    Some(Ok(x)) => x,
                    _ => {
                        log::warn!(
                            "settings: unrecognised value {} for {}, using default",
                            v,
                            stringify!($t)
                        );
                        <$t>::default()
                    }
                })
            }
        }
    };
}
string_enum_serde!(WhiteBalance);
string_enum_serde!(PictureStyle);

/// Settings that affect the conversion of each file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ConvertSettings {
    /// Target long edge in pixels, 0 = original size.
    #[serde(deserialize_with = "lenient::u32_")]
    pub resize: u32,
    /// JPEG quality 1..=100.
    #[serde(deserialize_with = "lenient::u8_")]
    pub quality: u8,
    /// Linear brightness multiplier 0..=2 (LibRaw `bright`).
    #[serde(deserialize_with = "lenient::f32_")]
    pub brightness: f32,
    pub white_balance: WhiteBalance,
    pub style: PictureStyle,
    #[serde(deserialize_with = "lenient::f32_")]
    pub rgb_red: f32,
    #[serde(deserialize_with = "lenient::f32_")]
    pub rgb_green: f32,
    #[serde(deserialize_with = "lenient::f32_")]
    pub rgb_blue: f32,
    /// Levels black point on the 8-bit scale (0..=100).
    #[serde(deserialize_with = "lenient::u8_")]
    pub black_point: u8,
    /// Levels white point on the 8-bit scale (150..=255).
    #[serde(deserialize_with = "lenient::u8_")]
    pub white_point: u8,
    /// Use the camera's embedded JPEG instead of developing the RAW.
    #[serde(deserialize_with = "lenient::bool_")]
    pub extract_embedded: bool,
    /// Copy EXIF metadata into the output JPEG.
    #[serde(deserialize_with = "lenient::bool_")]
    pub preserve_exif: bool,
    /// Output file conflict policy (new in 0.9.0).
    pub on_conflict: ConflictPolicy,
    /// Exposure compensation in EV (-3..=+3, new in 0.10.0). Combined with
    /// `brightness` as one linear gain: `brightness * 2^exposure_ev`.
    #[serde(deserialize_with = "lenient::f32_")]
    pub exposure_ev: f32,
    /// Apply the default filmic tone curve (new in 0.10.0).
    #[serde(deserialize_with = "lenient::bool_")]
    pub tone_curve: bool,
    /// Output sharpening amount (0 = off, new in 0.10.0).
    #[serde(deserialize_with = "lenient::f32_")]
    pub sharpen: f32,
}

impl Default for ConvertSettings {
    fn default() -> Self {
        Self {
            resize: DEFAULT_RESIZE,
            quality: DEFAULT_JPG_QUALITY,
            brightness: DEFAULT_BRIGHTNESS,
            white_balance: WhiteBalance::CameraDefault,
            style: PictureStyle::Standard,
            rgb_red: DEFAULT_RGB_VALUE,
            rgb_green: DEFAULT_RGB_VALUE,
            rgb_blue: DEFAULT_RGB_VALUE,
            black_point: DEFAULT_BLACK_POINT,
            white_point: DEFAULT_WHITE_POINT,
            extract_embedded: false,
            preserve_exif: false,
            on_conflict: ConflictPolicy::Suffix,
            exposure_ev: DEFAULT_EXPOSURE_EV,
            tone_curve: true,
            sharpen: DEFAULT_SHARPEN,
        }
    }
}

impl ConvertSettings {
    /// RGB multipliers as an array.
    pub fn rgb(&self) -> [f32; 3] {
        [self.rgb_red, self.rgb_green, self.rgb_blue]
    }

    /// Strict validation with the same ranges and messages as Python `validate_settings`.
    pub fn validate(&self) -> Result<(), String> {
        if !(MIN_JPG_QUALITY..=MAX_JPG_QUALITY).contains(&self.quality) {
            return Err(format!(
                "Quality must be between {MIN_JPG_QUALITY} and {MAX_JPG_QUALITY}, got {}",
                self.quality
            ));
        }
        if !self.brightness.is_finite()
            || !(MIN_BRIGHTNESS..=MAX_BRIGHTNESS).contains(&self.brightness)
        {
            return Err(format!(
                "Brightness must be between {MIN_BRIGHTNESS} and {MAX_BRIGHTNESS}, got {}",
                self.brightness
            ));
        }
        if !self.exposure_ev.is_finite()
            || !(MIN_EXPOSURE_EV..=MAX_EXPOSURE_EV).contains(&self.exposure_ev)
        {
            return Err(format!(
                "Exposure must be between {MIN_EXPOSURE_EV} and {MAX_EXPOSURE_EV} EV, got {}",
                self.exposure_ev
            ));
        }
        if !self.sharpen.is_finite() || !(MIN_SHARPEN..=MAX_SHARPEN).contains(&self.sharpen) {
            return Err(format!(
                "Sharpen must be between {MIN_SHARPEN} and {MAX_SHARPEN}, got {}",
                self.sharpen
            ));
        }
        if self.resize > MAX_RESIZE {
            return Err(format!(
                "Resize must be between {MIN_RESIZE} and {MAX_RESIZE}, got {}",
                self.resize
            ));
        }
        for (name, v) in [
            ("rgb_red", self.rgb_red),
            ("rgb_green", self.rgb_green),
            ("rgb_blue", self.rgb_blue),
        ] {
            if !v.is_finite() || !(MIN_RGB_VALUE..=MAX_RGB_VALUE).contains(&v) {
                return Err(format!(
                    "{name} must be between {MIN_RGB_VALUE} and {MAX_RGB_VALUE}, got {v}"
                ));
            }
        }
        if !(MIN_BLACK_POINT..=MAX_BLACK_POINT).contains(&self.black_point) {
            return Err(format!(
                "Black point must be between {MIN_BLACK_POINT} and {MAX_BLACK_POINT}, got {}",
                self.black_point
            ));
        }
        if !(MIN_WHITE_POINT..=MAX_WHITE_POINT).contains(&self.white_point) {
            return Err(format!(
                "White point must be between {MIN_WHITE_POINT} and {MAX_WHITE_POINT}, got {}",
                self.white_point
            ));
        }
        if self.black_point >= self.white_point {
            return Err(format!(
                "Black point ({}) must be less than white point ({})",
                self.black_point, self.white_point
            ));
        }
        Ok(())
    }

    /// Returns a copy with every value clamped into its valid range.
    /// Used when loading possibly hand-edited or legacy JSON.
    pub fn sanitized(&self) -> Self {
        let clampf = |v: f32, lo: f32, hi: f32, def: f32| {
            if v.is_finite() {
                v.clamp(lo, hi)
            } else {
                def
            }
        };
        let mut s = self.clone();
        s.quality = s.quality.clamp(MIN_JPG_QUALITY, MAX_JPG_QUALITY);
        s.brightness = clampf(
            s.brightness,
            MIN_BRIGHTNESS,
            MAX_BRIGHTNESS,
            DEFAULT_BRIGHTNESS,
        );
        s.resize = s.resize.min(MAX_RESIZE);
        s.rgb_red = clampf(s.rgb_red, MIN_RGB_VALUE, MAX_RGB_VALUE, DEFAULT_RGB_VALUE);
        s.rgb_green = clampf(s.rgb_green, MIN_RGB_VALUE, MAX_RGB_VALUE, DEFAULT_RGB_VALUE);
        s.rgb_blue = clampf(s.rgb_blue, MIN_RGB_VALUE, MAX_RGB_VALUE, DEFAULT_RGB_VALUE);
        s.black_point = s.black_point.clamp(MIN_BLACK_POINT, MAX_BLACK_POINT);
        s.white_point = s.white_point.clamp(MIN_WHITE_POINT, MAX_WHITE_POINT);
        s.exposure_ev = clampf(
            s.exposure_ev,
            MIN_EXPOSURE_EV,
            MAX_EXPOSURE_EV,
            DEFAULT_EXPOSURE_EV,
        );
        s.sharpen = clampf(s.sharpen, MIN_SHARPEN, MAX_SHARPEN, DEFAULT_SHARPEN);
        s
    }
}

/// Everything persisted between GUI sessions.
///
/// Serialised flat (the convert settings are `#[serde(flatten)]`) to stay
/// compatible with Python's `converter_settings.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    #[serde(flatten)]
    pub convert: ConvertSettings,
    /// Output folder; `None` = `<source>/Converted_JPG`. Stored as `""` when unset (Python format).
    #[serde(with = "opt_path")]
    pub output_folder: Option<PathBuf>,
    /// Last source folder used (new in 0.9.0).
    #[serde(with = "opt_path")]
    pub last_source: Option<PathBuf>,
    /// Check GitHub for updates on start (new in 0.9.0).
    #[serde(deserialize_with = "lenient::bool_")]
    pub check_updates: bool,
    /// Preview panel expanded in the narrow layout (new in 0.10.0).
    #[serde(deserialize_with = "lenient::bool_")]
    pub preview_open: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            convert: ConvertSettings::default(),
            output_folder: None,
            last_source: None,
            check_updates: true,
            preview_open: false,
        }
    }
}

impl AppSettings {
    /// Parse from JSON text (lenient) and clamp values into range.
    pub fn from_json(text: &str) -> Result<Self, serde_json::Error> {
        let mut s: AppSettings = serde_json::from_str(text)?;
        s.convert = s.convert.sanitized();
        Ok(s)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("settings serialise")
    }

    /// Load from a file. Missing or corrupt files yield defaults (logged).
    pub fn load(path: &std::path::Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(text) => match Self::from_json(&text) {
                Ok(s) => s,
                Err(e) => {
                    log::warn!("Invalid settings file {}: {e}", path.display());
                    Self::default()
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(e) => {
                log::warn!("Could not read settings {}: {e}", path.display());
                Self::default()
            }
        }
    }

    /// Save to a file (creating parent directories).
    pub fn save(&self, path: &std::path::Path) -> std::io::Result<()> {
        crate::paths::write_atomic(path, self.to_json().as_bytes())
    }
}

mod opt_path {
    use super::*;

    pub fn serialize<S: Serializer>(p: &Option<PathBuf>, s: S) -> Result<S::Ok, S::Error> {
        match p {
            Some(p) => s.serialize_str(&p.to_string_lossy()),
            None => s.serialize_str(""),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<PathBuf>, D::Error> {
        let v = serde_json::Value::deserialize(d)?;
        Ok(match v.as_str() {
            Some(s) if !s.trim().is_empty() => Some(PathBuf::from(s)),
            _ => None,
        })
    }
}

/// Lenient numeric / boolean deserialisers: Python may have written `90.0`
/// for an int slider value, or a number for a bool.
mod lenient {
    use super::*;
    use serde::de::Error;
    use serde_json::Value;

    fn as_f64<E: Error>(v: &Value) -> Result<f64, E> {
        match v {
            Value::Number(n) => n.as_f64().ok_or_else(|| E::custom("bad number")),
            Value::String(s) => s
                .trim()
                .parse::<f64>()
                .map_err(|_| E::custom(format!("expected a number, got '{s}'"))),
            Value::Bool(b) => Ok(if *b { 1.0 } else { 0.0 }),
            other => Err(E::custom(format!("expected a number, got {other}"))),
        }
    }

    pub fn f32_<'de, D: Deserializer<'de>>(d: D) -> Result<f32, D::Error> {
        let v = Value::deserialize(d)?;
        Ok(as_f64::<D::Error>(&v)? as f32)
    }

    pub fn u32_<'de, D: Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
        let v = Value::deserialize(d)?;
        let f = as_f64::<D::Error>(&v)?;
        if !f.is_finite() || f < 0.0 {
            return Ok(0);
        }
        Ok(f.round().min(u32::MAX as f64) as u32)
    }

    pub fn u8_<'de, D: Deserializer<'de>>(d: D) -> Result<u8, D::Error> {
        let v = Value::deserialize(d)?;
        let f = as_f64::<D::Error>(&v)?;
        if !f.is_finite() || f < 0.0 {
            return Ok(0);
        }
        Ok(f.round().min(255.0) as u8)
    }

    pub fn bool_<'de, D: Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
        let v = Value::deserialize(d)?;
        Ok(match v {
            Value::Bool(b) => b,
            Value::Number(n) => n.as_f64().unwrap_or(0.0) != 0.0,
            Value::String(s) => matches!(s.to_ascii_lowercase().as_str(), "true" | "1" | "yes"),
            _ => false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PYTHON_SETTINGS: &str = r#"{
  "resize": 0,
  "quality": 95,
  "brightness": 1.0,
  "white_balance": "Camera Default",
  "style": "Standard",
  "rgb_red": 1.0,
  "rgb_green": 1.0,
  "rgb_blue": 1.0,
  "black_point": 0,
  "white_point": 255,
  "extract_embedded": true,
  "preserve_exif": true,
  "output_folder": ""
}"#;

    #[test]
    fn reads_python_settings_file() {
        let s = AppSettings::from_json(PYTHON_SETTINGS).unwrap();
        assert_eq!(s.convert.quality, 95);
        assert!(s.convert.extract_embedded);
        assert!(s.convert.preserve_exif);
        assert_eq!(s.output_folder, None);
        assert_eq!(s.convert.on_conflict, ConflictPolicy::Suffix);
        assert!(s.check_updates);
        assert!(s.convert.validate().is_ok());
    }

    #[test]
    fn reads_python_floats_for_ints_and_style_names() {
        let json = r#"{"resize": 2000.0, "quality": 89.6, "brightness": 1.1422764227642275,
            "white_balance": "Auto", "style": "Black & White", "black_point": 12.0,
            "white_point": 240.4, "output_folder": "D:\\사진\\out"}"#;
        let s = AppSettings::from_json(json).unwrap();
        assert_eq!(s.convert.resize, 2000);
        assert_eq!(s.convert.quality, 90);
        assert!((s.convert.brightness - 1.142_276_4).abs() < 1e-6);
        assert_eq!(s.convert.white_balance, WhiteBalance::Auto);
        assert_eq!(s.convert.style, PictureStyle::BlackAndWhite);
        assert_eq!(s.convert.black_point, 12);
        assert_eq!(s.convert.white_point, 240);
        assert_eq!(s.output_folder, Some(PathBuf::from("D:\\사진\\out")));
        // Missing keys use defaults.
        assert!(!s.convert.extract_embedded);
        assert_eq!(s.convert.rgb(), [1.0, 1.0, 1.0]);
    }

    /// Files written by v0.9.0 and the Python app have none of the 0.10 keys.
    #[test]
    fn old_files_load_with_new_defaults() {
        let s = AppSettings::from_json(PYTHON_SETTINGS).unwrap();
        assert_eq!(s.convert.exposure_ev, DEFAULT_EXPOSURE_EV);
        assert!(s.convert.tone_curve);
        assert_eq!(s.convert.sharpen, DEFAULT_SHARPEN);
        assert!(!s.preview_open);

        let v090 = r#"{"resize": 0, "quality": 90, "brightness": 1.4,
            "white_balance": "Auto", "style": "HDR", "rgb_red": 1.0, "rgb_green": 1.0,
            "rgb_blue": 1.0, "black_point": 0, "white_point": 255, "extract_embedded": false,
            "preserve_exif": true, "on_conflict": "skip", "output_folder": "", "last_source": "",
            "check_updates": false}"#;
        let s = AppSettings::from_json(v090).unwrap();
        assert_eq!(s.convert.brightness, 1.4);
        assert_eq!(s.convert.exposure_ev, 0.0);
        assert!(s.convert.tone_curve);
        assert_eq!(s.convert.style, PictureStyle::Hdr);
        assert!(!s.check_updates);
        assert!(s.convert.validate().is_ok());
    }

    #[test]
    fn new_keys_roundtrip_and_clamp() {
        let mut s = AppSettings::default();
        s.convert.exposure_ev = -1.5;
        s.convert.tone_curve = false;
        s.convert.sharpen = 0.0;
        s.preview_open = true;
        let back = AppSettings::from_json(&s.to_json()).unwrap();
        assert_eq!(back, s);

        let clamped = AppSettings::from_json(
            r#"{"exposure_ev": 9.5, "tone_curve": 0, "sharpen": -1.0, "preview_open": "yes"}"#,
        )
        .unwrap();
        assert_eq!(clamped.convert.exposure_ev, MAX_EXPOSURE_EV);
        assert!(!clamped.convert.tone_curve);
        assert_eq!(clamped.convert.sharpen, MIN_SHARPEN);
        assert!(clamped.preview_open);
    }

    #[test]
    fn exposure_and_sharpen_validation() {
        let ok = ConvertSettings {
            exposure_ev: 3.0,
            ..Default::default()
        };
        assert!(ok.validate().is_ok());
        let bad = ConvertSettings {
            exposure_ev: 3.1,
            ..Default::default()
        };
        assert!(bad.validate().unwrap_err().contains("Exposure"));
        let nan = ConvertSettings {
            sharpen: f32::NAN,
            ..Default::default()
        };
        assert!(nan.validate().unwrap_err().contains("Sharpen"));
    }

    #[test]
    fn unknown_enum_values_fall_back() {
        let s =
            AppSettings::from_json(r#"{"white_balance": "Daylight", "style": "Weird"}"#).unwrap();
        assert_eq!(s.convert.white_balance, WhiteBalance::CameraDefault);
        assert_eq!(s.convert.style, PictureStyle::Standard);
    }

    #[test]
    fn out_of_range_values_are_clamped_on_load() {
        let s = AppSettings::from_json(
            r#"{"quality": 0, "brightness": 7, "rgb_red": -1, "black_point": 200, "white_point": 10}"#,
        )
        .unwrap();
        assert_eq!(s.convert.quality, 1);
        assert_eq!(s.convert.brightness, 2.0);
        assert_eq!(s.convert.rgb_red, 0.0);
        assert_eq!(s.convert.black_point, 100);
        assert_eq!(s.convert.white_point, 150);
    }

    #[test]
    fn roundtrip_uses_python_names() {
        let mut s = AppSettings::default();
        s.convert.style = PictureStyle::Hdr;
        s.convert.white_balance = WhiteBalance::Auto;
        s.output_folder = Some(PathBuf::from("/tmp/out"));
        let json = s.to_json();
        assert!(json.contains("\"style\": \"HDR\""));
        assert!(json.contains("\"white_balance\": \"Auto\""));
        assert!(json.contains("\"rgb_red\""));
        assert!(json.contains("\"on_conflict\": \"suffix\""));
        let back = AppSettings::from_json(&json).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn validation_ranges_match_python() {
        let ok = ConvertSettings::default();
        assert!(ok.validate().is_ok());

        let cases: Vec<(ConvertSettings, &str)> = vec![
            (
                ConvertSettings {
                    quality: 0,
                    ..ok.clone()
                },
                "Quality",
            ),
            (
                ConvertSettings {
                    quality: 101,
                    ..ok.clone()
                },
                "Quality",
            ),
            (
                ConvertSettings {
                    brightness: 2.01,
                    ..ok.clone()
                },
                "Brightness",
            ),
            (
                ConvertSettings {
                    brightness: -0.1,
                    ..ok.clone()
                },
                "Brightness",
            ),
            (
                ConvertSettings {
                    rgb_green: 2.5,
                    ..ok.clone()
                },
                "rgb_green",
            ),
            (
                ConvertSettings {
                    black_point: 101,
                    ..ok.clone()
                },
                "Black point",
            ),
            (
                ConvertSettings {
                    white_point: 149,
                    ..ok.clone()
                },
                "White point",
            ),
        ];
        for (s, needle) in cases {
            let err = s.validate().unwrap_err();
            assert!(err.contains(needle), "{err} should mention {needle}");
        }
        // Boundaries are inclusive.
        let edge = ConvertSettings {
            quality: 1,
            brightness: 2.0,
            rgb_red: 0.0,
            rgb_blue: 2.0,
            black_point: 100,
            white_point: 150,
            ..ok
        };
        assert!(edge.validate().is_ok());
    }

    #[test]
    fn style_and_wb_parsing() {
        for st in PictureStyle::ALL {
            assert_eq!(st.as_str().parse::<PictureStyle>().unwrap(), st);
        }
        assert_eq!(
            "black-and-white".parse::<PictureStyle>().unwrap(),
            PictureStyle::BlackAndWhite
        );
        assert_eq!("hdr".parse::<PictureStyle>().unwrap(), PictureStyle::Hdr);
        assert_eq!(
            "camera".parse::<WhiteBalance>().unwrap(),
            WhiteBalance::CameraDefault
        );
        assert!("x".parse::<WhiteBalance>().is_err());
        assert_eq!(
            "Overwrite".parse::<ConflictPolicy>().unwrap(),
            ConflictPolicy::Overwrite
        );
    }
}
