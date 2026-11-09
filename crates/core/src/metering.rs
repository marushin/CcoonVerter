//! Auto exposure metering (new in 0.11.0).
//!
//! The base gain before brightness and EV. It runs once per file on the full
//! linear image, inside `develop_linear`, so the batch output and the preview
//! share one code path and one result.
//!
//! Two sources, tried in order:
//!
//! 1. **The embedded JPEG** (when the RAW carries one of usable size): the
//!    gain is solved so the developed image has the camera JPEG's mean
//!    luminance. The camera already metered the scene and rendered it the way
//!    the photographer saw it on the back screen, so that is the reference
//!    the before/after comparison in the preview is judged against.
//!    The solution is limited to [`ANCHOR_RANGE_EV`] around the scene model,
//!    so an odd JPEG (extreme picture style, wrong preview) cannot wreck
//!    exposure.
//! 2. **Scene metering** (no usable preview, for example some DNGs):
//!    `log2 gain = 0.36 log2(1/white) + 0.43 log2(0.18/key) + 0.16`. `white`
//!    is the old 1 % clip point, `key` the geometric mean luminance (scene
//!    key). The weights were fitted against camera JPEGs on the tuning files
//!    of the regression matrix (see `docs/V0.11_REPORT.md`). It fixes the
//!    failure of the pure 1 % clip, which pushed low-key frames up and left
//!    high-key frames short.
//!
//! Everything here is deterministic: decimation, not random sampling, and
//! sequential sums over the small metering image, so results do not depend
//! on the thread count.

use std::io::Cursor;

use image::{ImageFormat, ImageReader};

use crate::develop::MAX_AUTO_GAIN;
use crate::embedded::JpegCandidate;
use crate::render::mean_display_luma;
use crate::RgbF32Image;

/// Long edge of the decimated image the metering works on.
const METER_EDGE: u32 = 320;
/// Weights of the scene model (`log2 gain`), fitted on the tuning split.
const SCENE_WEIGHT_WHITE: f64 = 0.36;
const SCENE_WEIGHT_KEY: f64 = 0.43;
const SCENE_OFFSET_EV: f64 = 0.16;
/// Lower limit of the auto exposure gain (never pull a frame below this).
pub const MIN_AUTO_GAIN: f32 = 0.2;
/// A preview must have at least this long edge to be trusted (the 160 px
/// thumbnails are often letterboxed and do not represent the frame).
const MIN_PREVIEW_EDGE: u32 = 512;
/// Tolerated difference between the aspect ratios of preview and RAW.
const MAX_ASPECT_DIFF: f64 = 0.03;
/// Preview brightness outside this range is treated as unusable.
const MIN_PREVIEW_LUMA: f64 = 6.0;
const MAX_PREVIEW_LUMA: f64 = 245.0;
/// The JPEG anchor may move the exposure this far from the scene model.
pub const ANCHOR_RANGE_EV: f32 = 1.25;
/// Bisection steps for the anchor (resolution far below one luma level).
const ANCHOR_STEPS: usize = 16;

/// Where the gain came from (diagnostics only).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MeterSource {
    /// Mean luminance of the embedded JPEG.
    EmbeddedJpeg,
    /// Scene model, no usable preview.
    #[default]
    Scene,
}

/// Auto exposure gains for the two white balance modes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Metering {
    pub gain_camera: f32,
    pub gain_auto: f32,
    pub source: MeterSource,
}

/// Every k-th pixel of every k-th row, at most [`METER_EDGE`] long.
fn meter_image(img: &RgbF32Image) -> RgbF32Image {
    let (w, h) = img.dimensions();
    let long = w.max(h).max(1);
    let step = (long / METER_EDGE).max(1);
    let (mw, mh) = (w.div_ceil(step).max(1), h.div_ceil(step).max(1));
    let mut out = RgbF32Image::new(mw, mh);
    for (x, y, p) in out.enumerate_pixels_mut() {
        *p = *img.get_pixel(x * step, y * step);
    }
    out
}

/// Geometric mean of linear luminance (the scene key).
fn scene_key(meter: &RgbF32Image, gains: [f32; 3]) -> f32 {
    let mut sum = 0f64;
    for p in meter.pixels() {
        let y =
            0.2126 * p.0[0] * gains[0] + 0.7152 * p.0[1] * gains[1] + 0.0722 * p.0[2] * gains[2];
        sum += (y.max(1e-4) as f64).ln();
    }
    let n = (meter.width() * meter.height()).max(1) as f64;
    ((sum / n).exp()) as f32
}

/// JPEG-free model: blend of the 1 % clip gain and the key-based gain.
fn scene_gain(white: f32, key: f32) -> f32 {
    let white_ev = (1.0 / white.max(1e-4) as f64).log2();
    let key_ev = (0.18 / key.max(1e-4) as f64).log2();
    let ev = SCENE_WEIGHT_WHITE * white_ev + SCENE_WEIGHT_KEY * key_ev + SCENE_OFFSET_EV;
    (2f64.powf(ev) as f32).clamp(MIN_AUTO_GAIN, MAX_AUTO_GAIN)
}

/// The gain, within `center` ± [`ANCHOR_RANGE_EV`], that gives `meter`
/// a displayed mean luminance of `target` (8-bit scale).
fn solve_anchor(meter: &RgbF32Image, wb: [f32; 3], center: f32, target: f64) -> f32 {
    let ev_c = center.log2();
    let (mut lo, mut hi) = (ev_c - ANCHOR_RANGE_EV, ev_c + ANCHOR_RANGE_EV);
    for _ in 0..ANCHOR_STEPS {
        let mid = 0.5 * (lo + hi);
        let g = 2f32.powf(mid);
        if mean_display_luma(meter, [wb[0] * g, wb[1] * g, wb[2] * g]) < target {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    2f32.powf(0.5 * (lo + hi))
        .clamp(MIN_AUTO_GAIN, MAX_AUTO_GAIN)
}

/// Meter `img`. `target` is the displayed mean luminance of the camera JPEG
/// (8-bit scale, Rec.601), when one is available.
pub fn meter(
    img: &RgbF32Image,
    white_camera: f32,
    white_auto: f32,
    auto_gains: [f32; 3],
    target: Option<f64>,
) -> Metering {
    let m = meter_image(img);
    let scene_cam = scene_gain(white_camera, scene_key(&m, [1.0; 3]));
    let scene_auto = scene_gain(white_auto, scene_key(&m, auto_gains));
    match target {
        Some(t) => Metering {
            gain_camera: solve_anchor(&m, [1.0; 3], scene_cam, t),
            gain_auto: solve_anchor(&m, auto_gains, scene_auto, t),
            source: MeterSource::EmbeddedJpeg,
        },
        None => Metering {
            gain_camera: scene_cam,
            gain_auto: scene_auto,
            source: MeterSource::Scene,
        },
    }
}

/// Mean Rec.601 luminance (8-bit scale) of an RGB8 buffer.
pub fn mean_luma_rgb8(rgb: &[u8]) -> f64 {
    let px = rgb.len() / 3;
    if px == 0 {
        return 0.0;
    }
    let mut sum = 0f64;
    for p in rgb.as_chunks::<3>().0 {
        sum += 0.299 * p[0] as f64 + 0.587 * p[1] as f64 + 0.114 * p[2] as f64;
    }
    sum / px as f64
}

/// The preview to meter against: the smallest one that is still large
/// enough and has the shape of the image (rotation ignored).
fn pick_preview(candidates: &[JpegCandidate], w: u32, h: u32) -> Option<JpegCandidate> {
    let aspect = |a: u32, b: u32| a.max(b) as f64 / a.min(b).max(1) as f64;
    let want = aspect(w, h);
    candidates
        .iter()
        .filter(|c| c.width.max(c.height) >= MIN_PREVIEW_EDGE)
        .filter(|c| (aspect(c.width, c.height) / want - 1.0).abs() <= MAX_ASPECT_DIFF)
        .min_by_key(|c| c.width as u64 * c.height as u64)
        .copied()
}

/// Mean displayed luminance of the camera JPEG in `file`, if it has a
/// suitable one. `w`, `h`: dimensions of the developed image.
pub fn preview_target_luma(file: &[u8], w: u32, h: u32) -> Option<f64> {
    let candidates = crate::embedded::find_jpegs(file);
    let c = pick_preview(&candidates, w, h)?;
    if !crate::thumbnail::plausible(&c) {
        return None;
    }
    let reader = ImageReader::with_format(Cursor::new(c.bytes(file)), ImageFormat::Jpeg);
    let img = reader.decode().ok()?.into_rgb8();
    let luma = mean_luma_rgb8(img.as_raw());
    (MIN_PREVIEW_LUMA..=MAX_PREVIEW_LUMA)
        .contains(&luma)
        .then_some(luma)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    fn gradient() -> RgbF32Image {
        let mut img = RgbF32Image::new(640, 480);
        for (x, y, p) in img.enumerate_pixels_mut() {
            let v = (x + y) as f32 / 1120.0;
            *p = Rgb([v * v, v * v * 0.8, v * v * 0.7]);
        }
        img
    }

    #[test]
    fn scene_model_lifts_low_key_less_than_a_one_percent_clip() {
        // Low-key: almost everything dark with a few bright pixels.
        let mut img = RgbF32Image::from_pixel(1000, 1000, Rgb([0.02, 0.02, 0.02]));
        for x in 0..100 {
            for y in 0..200 {
                img.put_pixel(x, y, Rgb([0.8, 0.8, 0.8]));
            }
        }
        let m = meter(&img, 0.8, 0.8, [1.0; 3], None);
        assert_eq!(m.source, MeterSource::Scene);
        // 1 % clip would be gain 1.25 here; the scene key must add gain, but
        // not chase the key all the way to mid-grey.
        assert!(m.gain_camera > 1.25 && m.gain_camera < 4.0, "{m:?}");
    }

    #[test]
    fn high_key_scene_is_not_pushed_above_clip_gain() {
        let img = RgbF32Image::from_pixel(100, 100, Rgb([0.7, 0.7, 0.7]));
        let m = meter(&img, 0.7, 0.7, [1.0; 3], None);
        assert!(m.gain_camera < 1.0 / 0.7, "{m:?}");
    }

    #[test]
    fn anchor_hits_target_within_range() {
        let img = gradient();
        let m = meter(&img, 0.9, 0.9, [1.0; 3], Some(120.0));
        assert_eq!(m.source, MeterSource::EmbeddedJpeg);
        let mimg = meter_image(&img);
        let got = mean_display_luma(&mimg, [m.gain_camera; 3]);
        assert!((got - 120.0).abs() < 1.0, "{got} {m:?}");
    }

    #[test]
    fn anchor_is_clamped_around_the_scene_model() {
        let img = gradient();
        let center = meter(&img, 0.9, 0.9, [1.0; 3], None);
        let m = meter(&img, 0.9, 0.9, [1.0; 3], Some(250.0));
        let ev = (m.gain_camera / center.gain_camera).log2();
        assert!((ev - ANCHOR_RANGE_EV).abs() < 0.01, "{ev}");
    }

    #[test]
    fn metering_is_deterministic() {
        let img = gradient();
        assert_eq!(
            meter(&img, 0.9, 0.8, [1.1, 1.0, 0.9], Some(100.0)),
            meter(&img, 0.9, 0.8, [1.1, 1.0, 0.9], Some(100.0))
        );
    }

    #[test]
    fn auto_white_balance_gets_its_own_gain() {
        let img = gradient();
        let m = meter(&img, 0.9, 0.5, [2.0, 1.0, 2.0], None);
        assert!(m.gain_auto != m.gain_camera, "{m:?}");
    }

    #[test]
    fn preview_selection_prefers_the_smallest_usable_size() {
        let mk = |w, h| JpegCandidate {
            offset: 0,
            len: 10,
            width: w,
            height: h,
            sof: 0xC0,
        };
        // 160 px thumbnail is ignored, the 3:2 mid-size is chosen.
        let picked = pick_preview(&[mk(160, 120), mk(1616, 1080), mk(6000, 4000)], 6000, 4000);
        assert_eq!(picked.map(|c| c.width), Some(1616));
        // Wrong shape (letterboxed preview) is rejected.
        assert!(pick_preview(&[mk(1920, 1080)], 6000, 4000).is_none());
        // Rotation does not matter.
        assert!(pick_preview(&[mk(1000, 1500)], 6000, 4000).is_some());
        // Nothing big enough.
        assert!(pick_preview(&[mk(320, 200)], 6000, 4000).is_none());
    }

    #[test]
    fn extreme_preview_luma_is_rejected() {
        let black = crate::embedded::tests::tiny_jpeg(640, 480, [0, 0, 0]);
        assert!(preview_target_luma(&black, 640, 480).is_none());
        let grey = crate::embedded::tests::tiny_jpeg(640, 480, [120, 120, 120]);
        let luma = preview_target_luma(&grey, 640, 480).unwrap();
        assert!((luma - 120.0).abs() < 2.0, "{luma}");
    }
}
