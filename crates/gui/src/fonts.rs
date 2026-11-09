//! Runtime font setup.
//!
//! egui's built-in fonts have no Hangul, so Korean folder names would render
//! as boxes. At start-up we look for a system CJK font and append it as a
//! fallback to the Proportional and Monospace families. Nothing is bundled
//! (a Korean font is several MB); if no font is found a warning is logged and
//! the app continues (eframe's own on-demand system font provider may still
//! cover the glyphs).
//!
//! On Windows we additionally use Segoe UI / Segoe UI Bold as the primary
//! fonts, matching the Python app (`FONT_BODY = ("Segoe UI", 12)`).
//! A `bold` family ([`crate::theme::BOLD_FAMILY`]) is always registered so
//! headers can use it on every platform.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use eframe::egui::{self, FontData, FontDefinitions, FontFamily};

use crate::theme::BOLD_FAMILY;

/// Candidate font files for one role, tried in order.
#[derive(Debug, Clone, Copy)]
pub struct FontCandidates {
    /// Latin UI font used before egui's default (Windows: Segoe UI).
    pub ui_regular: &'static [&'static str],
    pub ui_bold: &'static [&'static str],
    /// CJK fallback (regular).
    pub cjk_regular: &'static [&'static str],
    /// CJK fallback for the bold family.
    pub cjk_bold: &'static [&'static str],
}

#[cfg(target_os = "windows")]
pub const CANDIDATES: FontCandidates = FontCandidates {
    ui_regular: &[r"C:\Windows\Fonts\segoeui.ttf"],
    ui_bold: &[r"C:\Windows\Fonts\segoeuib.ttf"],
    cjk_regular: &[
        r"C:\Windows\Fonts\malgun.ttf",
        r"C:\Windows\Fonts\malgunbd.ttf",
        r"C:\Windows\Fonts\NanumGothic.ttf",
    ],
    cjk_bold: &[
        r"C:\Windows\Fonts\malgunbd.ttf",
        r"C:\Windows\Fonts\NanumGothicBold.ttf",
    ],
};

#[cfg(target_os = "macos")]
pub const CANDIDATES: FontCandidates = FontCandidates {
    ui_regular: &[],
    ui_bold: &[],
    cjk_regular: &[
        "/System/Library/Fonts/AppleSDGothicNeo.ttc",
        "/System/Library/Fonts/Supplemental/AppleGothic.ttf",
        "/Library/Fonts/NanumGothic.ttf",
    ],
    cjk_bold: &[],
};

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub const CANDIDATES: FontCandidates = FontCandidates {
    ui_regular: &[],
    ui_bold: &[],
    cjk_regular: &[
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/opentype/noto/NotoSansCJKkr-Regular.otf",
        "/usr/share/fonts/truetype/nanum/NanumGothic.ttf",
    ],
    cjk_bold: &[
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Bold.ttc",
        "/usr/share/fonts/noto-cjk/NotoSansCJK-Bold.ttc",
        "/usr/share/fonts/truetype/nanum/NanumGothicBold.ttf",
    ],
};

/// What [`install`] ended up loading (for logging and tests).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct FontReport {
    pub ui_regular: Option<PathBuf>,
    pub ui_bold: Option<PathBuf>,
    pub cjk_regular: Option<PathBuf>,
    pub cjk_bold: Option<PathBuf>,
}

fn load_first(paths: &[&str]) -> Option<(PathBuf, Vec<u8>)> {
    paths.iter().find_map(|p| {
        let p = Path::new(p);
        match std::fs::read(p) {
            Ok(bytes) if !bytes.is_empty() => Some((p.to_path_buf(), bytes)),
            Ok(_) => None,
            Err(e) => {
                if e.kind() != std::io::ErrorKind::NotFound {
                    log::debug!("Font {} not readable: {e}", p.display());
                }
                None
            }
        }
    })
}

/// Build font definitions from the given candidates. Pure apart from file reads.
pub fn build(candidates: &FontCandidates) -> (FontDefinitions, FontReport) {
    let mut fonts = FontDefinitions::default();
    let mut report = FontReport::default();
    let default_prop = fonts
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();

    let add = |fonts: &mut FontDefinitions, name: &str, bytes: Vec<u8>| {
        fonts
            .font_data
            .insert(name.to_owned(), Arc::new(FontData::from_owned(bytes)));
    };

    // Regular UI font (in front of egui's default).
    if let Some((path, bytes)) = load_first(candidates.ui_regular) {
        add(&mut fonts, "ui-regular", bytes);
        if let Some(list) = fonts.families.get_mut(&FontFamily::Proportional) {
            list.insert(0, "ui-regular".into());
        }
        report.ui_regular = Some(path);
    }

    // CJK fallback for Proportional + Monospace.
    if let Some((path, bytes)) = load_first(candidates.cjk_regular) {
        add(&mut fonts, "cjk-regular", bytes);
        for fam in [FontFamily::Proportional, FontFamily::Monospace] {
            fonts
                .families
                .entry(fam)
                .or_default()
                .push("cjk-regular".into());
        }
        report.cjk_regular = Some(path);
    }

    // Bold family: bold UI font, then bold CJK, then everything regular as fallback.
    let mut bold: Vec<String> = Vec::new();
    if let Some((path, bytes)) = load_first(candidates.ui_bold) {
        add(&mut fonts, "ui-bold", bytes);
        bold.push("ui-bold".into());
        report.ui_bold = Some(path);
    }
    bold.extend(default_prop.iter().cloned());
    if let Some((path, bytes)) = load_first(candidates.cjk_bold) {
        add(&mut fonts, "cjk-bold", bytes);
        bold.push("cjk-bold".into());
        report.cjk_bold = Some(path);
    }
    if report.cjk_regular.is_some() {
        bold.push("cjk-regular".into());
    }
    if report.ui_regular.is_some() {
        bold.push("ui-regular".into());
    }
    fonts
        .families
        .insert(FontFamily::Name(BOLD_FAMILY.into()), bold);

    (fonts, report)
}

/// Load fonts for this platform and install them into the context.
pub fn install(ctx: &egui::Context) -> FontReport {
    let (fonts, report) = build(&CANDIDATES);
    match &report.cjk_regular {
        Some(p) => log::info!("Loaded CJK font {}", p.display()),
        None => log::warn!(
            "No system CJK font found (tried {:?}); Korean text may not render",
            CANDIDATES.cjk_regular
        ),
    }
    if let Some(p) = &report.ui_regular {
        log::info!("Loaded UI font {}", p.display());
    }
    ctx.set_fonts(fonts);
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_fonts_still_define_bold_family() {
        let none = FontCandidates {
            ui_regular: &["/definitely/missing.ttf"],
            ui_bold: &[],
            cjk_regular: &["/definitely/missing-cjk.ttf"],
            cjk_bold: &[],
        };
        let (defs, report) = build(&none);
        assert_eq!(report, FontReport::default());
        let bold = defs
            .families
            .get(&FontFamily::Name(BOLD_FAMILY.into()))
            .unwrap();
        assert!(!bold.is_empty(), "bold family falls back to default fonts");
        // Every referenced font exists.
        for list in defs.families.values() {
            for name in list {
                assert!(defs.font_data.contains_key(name), "{name}");
            }
        }
    }

    #[test]
    fn platform_candidates_wire_up_consistently() {
        let (defs, report) = build(&CANDIDATES);
        for list in defs.families.values() {
            for name in list {
                assert!(defs.font_data.contains_key(name), "{name}");
            }
        }
        if report.cjk_regular.is_some() {
            let prop = &defs.families[&FontFamily::Proportional];
            assert_eq!(prop.last().map(String::as_str), Some("cjk-regular"));
        }
    }
}
