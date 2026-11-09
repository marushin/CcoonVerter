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

/// File type of the output (new in 0.12.0).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub enum OutputFormat {
    #[default]
    Jpeg,
    /// 8-bit PNG.
    Png,
    /// 16 bits per channel, uncompressed.
    Tiff16,
    /// Lossy WebP.
    Webp,
}

impl OutputFormat {
    pub const ALL: [OutputFormat; 4] = [
        OutputFormat::Jpeg,
        OutputFormat::Png,
        OutputFormat::Tiff16,
        OutputFormat::Webp,
    ];

    /// JSON and command line name.
    pub fn as_str(self) -> &'static str {
        match self {
            OutputFormat::Jpeg => "jpeg",
            OutputFormat::Png => "png",
            OutputFormat::Tiff16 => "tiff",
            OutputFormat::Webp => "webp",
        }
    }

    /// Short name for display.
    pub fn label(self) -> &'static str {
        match self {
            OutputFormat::Jpeg => "JPEG",
            OutputFormat::Png => "PNG",
            OutputFormat::Tiff16 => "TIFF 16-bit",
            OutputFormat::Webp => "WebP",
        }
    }

    /// File extension, without the dot.
    pub fn extension(self) -> &'static str {
        match self {
            OutputFormat::Jpeg => "jpg",
            OutputFormat::Png => "png",
            OutputFormat::Tiff16 => "tif",
            OutputFormat::Webp => "webp",
        }
    }

    /// True when the quality setting has an effect.
    pub fn uses_quality(self) -> bool {
        matches!(self, OutputFormat::Jpeg | OutputFormat::Webp)
    }
}

impl FromStr for OutputFormat {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match normalize_name(s).as_str() {
            "jpeg" | "jpg" => Ok(OutputFormat::Jpeg),
            "png" => Ok(OutputFormat::Png),
            "tiff" | "tif" | "tiff16" | "tiff16bit" => Ok(OutputFormat::Tiff16),
            "webp" => Ok(OutputFormat::Webp),
            _ => Err(format!(
                "unknown output format '{s}' (expected jpeg, png, tiff or webp)"
            )),
        }
    }
}

/// Colour space of the output (new in 0.12.0).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub enum ColorSpace {
    #[default]
    Srgb,
    AdobeRgb,
}

impl ColorSpace {
    pub const ALL: [ColorSpace; 2] = [ColorSpace::Srgb, ColorSpace::AdobeRgb];

    pub fn as_str(self) -> &'static str {
        match self {
            ColorSpace::Srgb => "srgb",
            ColorSpace::AdobeRgb => "adobergb",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ColorSpace::Srgb => "sRGB",
            ColorSpace::AdobeRgb => "AdobeRGB",
        }
    }
}

impl FromStr for ColorSpace {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match normalize_name(s).as_str() {
            "srgb" => Ok(ColorSpace::Srgb),
            "adobergb" | "adobe" | "argb" => Ok(ColorSpace::AdobeRgb),
            _ => Err(format!(
                "unknown color space '{s}' (expected srgb or adobergb)"
            )),
        }
    }
}

/// Language of the user interface (new in 0.11.0).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub enum UiLanguage {
    /// Follow the system locale.
    #[default]
    Auto,
    English,
    Korean,
}

impl UiLanguage {
    pub fn as_str(self) -> &'static str {
        match self {
            UiLanguage::Auto => "auto",
            UiLanguage::English => "en",
            UiLanguage::Korean => "ko",
        }
    }
}

impl FromStr for UiLanguage {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match normalize_name(s).as_str() {
            "auto" | "system" => Ok(UiLanguage::Auto),
            "en" | "eng" | "english" => Ok(UiLanguage::English),
            "ko" | "kr" | "korean" | "kor" => Ok(UiLanguage::Korean),
            _ => Err(format!("unknown language '{s}' (expected auto, en or ko)")),
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
string_enum_serde!(UiLanguage);
string_enum_serde!(OutputFormat);
string_enum_serde!(ColorSpace);

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
    /// Give the output the modification time of the source (new in 0.11.0).
    #[serde(deserialize_with = "lenient::bool_")]
    pub keep_source_date: bool,
    /// Output file type (new in 0.12.0). Camera JPEG mode is always JPEG.
    pub output_format: OutputFormat,
    /// Output colour space (new in 0.12.0). Not applied in camera JPEG mode.
    pub color_space: ColorSpace,
    /// Output file name template (new in 0.12.0), see [`crate::naming`].
    #[serde(deserialize_with = "lenient::template")]
    pub filename_template: String,
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
            preserve_exif: true,
            on_conflict: ConflictPolicy::Suffix,
            exposure_ev: DEFAULT_EXPOSURE_EV,
            tone_curve: true,
            sharpen: DEFAULT_SHARPEN,
            keep_source_date: false,
            output_format: OutputFormat::Jpeg,
            color_space: ColorSpace::Srgb,
            filename_template: crate::naming::DEFAULT_TEMPLATE.to_string(),
        }
    }
}

impl ConvertSettings {
    /// The format that is actually written: camera JPEG mode stays JPEG.
    pub fn effective_format(&self) -> OutputFormat {
        if self.extract_embedded {
            OutputFormat::Jpeg
        } else {
            self.output_format
        }
    }

    /// The colour space that is actually written: camera JPEG mode keeps the
    /// camera's pixels, so nothing is converted (a fallback development is sRGB).
    pub fn effective_color_space(&self) -> ColorSpace {
        if self.extract_embedded {
            ColorSpace::Srgb
        } else {
            self.color_space
        }
    }

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
        crate::naming::NameTemplate::parse(&self.filename_template).map_err(|e| e.to_string())?;
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
        if let Err(e) = crate::naming::NameTemplate::parse(&s.filename_template) {
            log::warn!("settings: ignoring file name template: {e}");
            s.filename_template = crate::naming::DEFAULT_TEMPLATE.to_string();
        }
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
    /// Interface language (new in 0.11.0).
    pub language: UiLanguage,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            convert: ConvertSettings::default(),
            output_folder: None,
            last_source: None,
            check_updates: true,
            preview_open: false,
            language: UiLanguage::Auto,
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

    /// A template of the wrong JSON type must not lose the whole settings file.
    pub fn template<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
        let v = Value::deserialize(d)?;
        Ok(v.as_str()
            .map(str::to_string)
            .unwrap_or_else(|| crate::naming::DEFAULT_TEMPLATE.to_string()))
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
        assert!(!s.convert.keep_source_date);
        assert_eq!(s.language, UiLanguage::Auto);

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

    /// Fresh installs, v0.11 files and Python files all have update checks on
    /// unless the file says otherwise.
    #[test]
    fn update_check_defaults_to_on() {
        assert!(AppSettings::default().check_updates);
        assert!(AppSettings::from_json("{}").unwrap().check_updates);
        assert!(
            AppSettings::from_json(PYTHON_SETTINGS)
                .unwrap()
                .check_updates
        );
        let v011 = r#"{"quality": 90, "language": "ko", "preview_open": true}"#;
        assert!(AppSettings::from_json(v011).unwrap().check_updates);
        let off = AppSettings::from_json(r#"{"check_updates": false}"#).unwrap();
        assert!(!off.check_updates);
    }

    #[test]
    fn v011_files_load_with_output_defaults() {
        let v011 = r#"{"resize": 0, "quality": 90, "brightness": 1.0,
            "white_balance": "Auto", "style": "HDR", "extract_embedded": false,
            "preserve_exif": true, "on_conflict": "skip", "exposure_ev": 0.5,
            "tone_curve": true, "sharpen": 0.35, "keep_source_date": true,
            "output_folder": "", "last_source": "", "check_updates": true,
            "preview_open": false, "language": "en"}"#;
        let s = AppSettings::from_json(v011).unwrap();
        assert_eq!(s.convert.output_format, OutputFormat::Jpeg);
        assert_eq!(s.convert.color_space, ColorSpace::Srgb);
        assert_eq!(s.convert.filename_template, "{name}");
        assert_eq!(s.convert.exposure_ev, 0.5);
        assert!(s.convert.validate().is_ok());
        // v0.11 presets have the same shape.
        let p = crate::presets::PresetStore::from_json(
            r#"{"old": {"resize": 2000, "style": "Vivid"}}"#,
        )
        .unwrap();
        let old = p.get("old").unwrap();
        assert_eq!(old.output_format, OutputFormat::Jpeg);
        assert_eq!(old.color_space, ColorSpace::Srgb);
        assert_eq!(old.filename_template, "{name}");
    }

    #[test]
    fn output_settings_roundtrip_and_bad_values() {
        let mut s = AppSettings::default();
        s.convert.output_format = OutputFormat::Tiff16;
        s.convert.color_space = ColorSpace::AdobeRgb;
        s.convert.filename_template = "{date}_{seq}".into();
        let json = s.to_json();
        assert!(json.contains("\"output_format\": \"tiff\""), "{json}");
        assert!(json.contains("\"color_space\": \"adobergb\""), "{json}");
        assert_eq!(AppSettings::from_json(&json).unwrap(), s);

        let bad = AppSettings::from_json(
            r#"{"output_format": "bmp", "color_space": 7, "filename_template": "../{name}"}"#,
        )
        .unwrap();
        assert_eq!(bad.convert.output_format, OutputFormat::Jpeg);
        assert_eq!(bad.convert.color_space, ColorSpace::Srgb);
        assert_eq!(bad.convert.filename_template, "{name}");
        let wrong_type = AppSettings::from_json(r#"{"filename_template": 5}"#).unwrap();
        assert_eq!(wrong_type.convert.filename_template, "{name}");
    }

    #[test]
    fn camera_jpeg_mode_is_always_jpeg_srgb() {
        let s = ConvertSettings {
            extract_embedded: true,
            output_format: OutputFormat::Png,
            color_space: ColorSpace::AdobeRgb,
            ..Default::default()
        };
        assert_eq!(s.effective_format(), OutputFormat::Jpeg);
        assert_eq!(s.effective_color_space(), ColorSpace::Srgb);
        let d = ConvertSettings {
            extract_embedded: false,
            ..s
        };
        assert_eq!(d.effective_format(), OutputFormat::Png);
        assert_eq!(d.effective_color_space(), ColorSpace::AdobeRgb);
    }

    #[test]
    fn template_is_validated_before_a_batch() {
        let bad = ConvertSettings {
            filename_template: "a/{name}".into(),
            ..Default::default()
        };
        assert!(bad.validate().unwrap_err().contains("template"));
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
    fn language_setting_roundtrip_and_unknown_values() {
        let mut s = AppSettings {
            language: UiLanguage::Korean,
            ..Default::default()
        };
        s.convert.keep_source_date = true;
        let json = s.to_json();
        assert!(json.contains("\"language\": \"ko\""), "{json}");
        let back = AppSettings::from_json(&json).unwrap();
        assert_eq!(back, s);
        let en = AppSettings::from_json(r#"{"language": "English"}"#).unwrap();
        assert_eq!(en.language, UiLanguage::English);
        let unknown = AppSettings::from_json(r#"{"language": "Klingon"}"#).unwrap();
        assert_eq!(unknown.language, UiLanguage::Auto);
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
