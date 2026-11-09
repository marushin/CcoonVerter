//! Reduced-resolution sources for the GUI preview. The pixels are produced by
//! the same `develop` / `render` / `embedded` functions as a batch conversion;
//! only the size differs (decode, develop at full resolution, then reduce:
//! rawler 0.8.0 has no half-size development path).
//!
//! No GUI types here so the worker can be tested without a window.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use image::RgbImage;
use rawler::rawsource::RawSource;

use crate::develop::{self, LinearBase};
use crate::embedded::{self, EmbeddedPreview};
use crate::error::{ConvertError, Result};
use crate::pipeline::{embedded_orientation, RawCtx};
use crate::resize;

/// Long edge of the preview image in pixels.
pub const PREVIEW_LONG_EDGE: u32 = 1600;

fn check(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        Err(ConvertError::Cancelled)
    } else {
        Ok(())
    }
}

/// Develop `path` and reduce it to `long_edge`. The statistics stay those of
/// the full-resolution image, so the preview exposes exactly like the output.
pub fn load_base(path: &Path, long_edge: u32, cancel: &AtomicBool) -> Result<LinearBase> {
    let source = RawSource::new(path).map_err(|e| ConvertError::io(path, e))?;
    let ctx = RawCtx::new(&source);
    let decoder = ctx.decoder()?;
    let base = develop::develop_linear(ctx.source, decoder, ctx.raw_orientation(), cancel)?;
    check(cancel)?;
    Ok(LinearBase {
        image: resize::reduce_long_edge_f32(base.image, long_edge),
        stats: base.stats,
    })
}

/// The largest embedded JPEG of `path`, upright and reduced to `long_edge`.
/// `Ok(None)` when the file has no embedded preview.
pub fn load_embedded(path: &Path, long_edge: u32, cancel: &AtomicBool) -> Result<Option<RgbImage>> {
    let source = RawSource::new(path).map_err(|e| ConvertError::io(path, e))?;
    let ctx = RawCtx::new(&source);
    let Some(preview) = embedded::extract(ctx.source, ctx.decoder().ok()) else {
        return Ok(None);
    };
    check(cancel)?;
    let raw_orientation = ctx.raw_orientation();
    let img = match preview {
        EmbeddedPreview::Jpeg { data, .. } => {
            let orientation = embedded_orientation(&data, raw_orientation);
            let img =
                image::load_from_memory_with_format(&data, image::ImageFormat::Jpeg)?.into_rgb8();
            resize::apply_orientation(img, orientation)
        }
        EmbeddedPreview::Decoded(img) => {
            resize::apply_orientation(img.into_rgb8(), raw_orientation)
        }
    };
    check(cancel)?;
    Ok(Some(resize::reduce_long_edge_u8(img, long_edge)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::{render, RenderParams};
    use crate::settings::ConvertSettings;
    use crate::testutil::{synthetic_dng, DngSpec};

    fn dng(dir: &Path, name: &str, spec: &DngSpec) -> std::path::PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, synthetic_dng(spec)).unwrap();
        p
    }

    #[test]
    fn base_is_reduced_and_upright() {
        let dir = tempfile::tempdir().unwrap();
        let spec = DngSpec {
            width: 320,
            height: 240,
            orientation: 6,
            ..Default::default()
        };
        let p = dng(dir.path(), "a.DNG", &spec);
        let cancel = AtomicBool::new(false);
        let base = load_base(&p, 160, &cancel).unwrap();
        assert_eq!(base.image.dimensions(), (120, 160));
        // Never upscaled.
        let full = load_base(&p, 1600, &cancel).unwrap();
        assert_eq!(full.image.dimensions(), (240, 320));
        assert_eq!(full.stats, base.stats, "stats come from full resolution");
    }

    #[test]
    fn embedded_preview_is_found_or_absent() {
        let dir = tempfile::tempdir().unwrap();
        let cancel = AtomicBool::new(false);
        let with = dng(dir.path(), "with.DNG", &DngSpec::default());
        let img = load_embedded(&with, 1600, &cancel).unwrap().unwrap();
        assert_eq!(img.dimensions(), (32, 24));
        let without = dng(
            dir.path(),
            "without.DNG",
            &DngSpec {
                preview: None,
                ..Default::default()
            },
        );
        assert!(load_embedded(&without, 1600, &cancel).unwrap().is_none());
    }

    #[test]
    fn garbage_file_is_an_error_not_a_panic() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("bad.ARW");
        std::fs::write(&p, vec![0u8; 4096]).unwrap();
        let cancel = AtomicBool::new(false);
        assert!(load_base(&p, 1600, &cancel).is_err());
    }

    #[test]
    fn cancel_flag_is_honoured() {
        let dir = tempfile::tempdir().unwrap();
        let p = dng(dir.path(), "c.DNG", &DngSpec::default());
        let cancel = AtomicBool::new(true);
        assert!(load_base(&p, 1600, &cancel).unwrap_err().is_cancelled());
    }

    /// The preview chain and the batch chain are the same function: a base
    /// rendered in place equals one reduced first, up to resampling.
    #[test]
    fn preview_matches_batch_chain() {
        let dir = tempfile::tempdir().unwrap();
        let spec = DngSpec {
            width: 480,
            height: 320,
            ..Default::default()
        };
        let p = dng(dir.path(), "m.DNG", &spec);
        let cancel = AtomicBool::new(false);
        let params = RenderParams::from(&ConvertSettings::default());
        // Preview path: reduced base, render at its size.
        let base = load_base(&p, 240, &cancel).unwrap();
        let preview = render(base.image, &base.stats, &params, 0, &cancel).unwrap();
        // Batch path: full base, resized inside the chain.
        let full = load_base(&p, 4096, &cancel).unwrap();
        let batch = render(full.image, &full.stats, &params, 240, &cancel).unwrap();
        assert_eq!(preview.dimensions(), batch.dimensions());
        let mean = |img: &crate::RgbF32Image| {
            let b: &[f32] = img.as_ref();
            b.iter().sum::<f32>() / b.len() as f32
        };
        assert!((mean(&preview) - mean(&batch)).abs() < 0.01);
    }
}
