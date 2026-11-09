//! The shared rendering chain: from the linear base image (`develop`) to
//! final display-referred pixels. Batch conversion and the GUI preview both
//! go through [`render`], so what the preview shows is what gets written.
//!
//! Order, on linear-sRGB values:
//!
//! 1. Exposure gain = white balance gains x auto exposure (metered once on
//!    the full-resolution image, see [`crate::metering`] and [`BaseStats`])
//!    x `brightness` x `2^exposure_ev`.
//!    **Brightness and EV are the same linear stage**: brightness 2.0 is
//!    exactly +1 EV. Brightness (0..2, 1 = neutral) is kept for compatibility
//!    with old settings and presets; EV (-3..+3) is the photographer's unit.
//!    The two simply multiply.
//! 2. Highlights: a shoulder compresses the top of the range smoothly, keeping
//!    RGB ratios (no hue shift); pixels near clipping are blended toward
//!    their brightest channel so clipped areas go white instead of pink/cyan.
//! 3. Tone curve (optional) + sRGB encoding through one lookup table.
//!
//! Then, on display-referred values (same as v0.9.0): RGB multipliers + Levels,
//! picture style, long-edge resize, and last the light unsharp mask, at
//! output resolution so a preview at 1600 px matches a batch at 1600 px.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

use rayon::prelude::*;

use crate::develop::{srgb_oetf, BaseStats};
use crate::error::{ConvertError, Result};
use crate::settings::{ConvertSettings, PictureStyle, WhiteBalance};
use crate::{adjust, resize, sharpen, style, RgbF32Image};

/// Everything the chain needs from the settings (no file or output options).
#[derive(Debug, Clone, PartialEq)]
pub struct RenderParams {
    pub white_balance: WhiteBalance,
    pub brightness: f32,
    pub exposure_ev: f32,
    pub tone_curve: bool,
    pub rgb: [f32; 3],
    pub black_point: u8,
    pub white_point: u8,
    pub style: PictureStyle,
    pub sharpen: f32,
}

impl From<&ConvertSettings> for RenderParams {
    fn from(s: &ConvertSettings) -> Self {
        Self {
            white_balance: s.white_balance,
            brightness: s.brightness,
            exposure_ev: s.exposure_ev,
            tone_curve: s.tone_curve,
            rgb: s.rgb(),
            black_point: s.black_point,
            white_point: s.white_point,
            style: s.style,
            sharpen: s.sharpen,
        }
    }
}

/// Per-channel gains of the exposure stage (white balance x auto exposure x
/// brightness x EV). Brightness 2.0 and EV +1 give identical gains.
pub fn exposure_gains(
    stats: &BaseStats,
    white_balance: WhiteBalance,
    brightness: f32,
    exposure_ev: f32,
) -> [f32; 3] {
    let (base, wb) = match white_balance {
        WhiteBalance::CameraDefault => (stats.exposure_camera, [1.0, 1.0, 1.0]),
        WhiteBalance::Auto => (stats.exposure_auto, stats.auto_gains),
    };
    let gain = (base * brightness).max(0.0) * 2f32.powf(exposure_ev);
    [wb[0] * gain, wb[1] * gain, wb[2] * gain]
}

/// Shoulder knee in linear light: below this, values pass through unchanged.
pub const SHOULDER_KNEE: f32 = 0.85;
/// Base (pre-gain) maximum where neutralising of sensor-clipped pixels starts
/// and ends. rawler's soft clip leaves values at or above 1.0 for pixels
/// with a saturated channel after white balance.
const CLIP_DESAT_START: f32 = 0.98;
const CLIP_DESAT_END: f32 = 1.20;
/// Post-gain maximum where over-exposed pixels finish fading to white.
const OVER_DESAT_END: f32 = 2.5;

#[inline]
fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Exponential shoulder: identity below the knee, continuous slope at the
/// knee, approaches 1.0 from below for large inputs.
#[inline]
pub fn shoulder(x: f32) -> f32 {
    if x <= SHOULDER_KNEE {
        x
    } else {
        let room = 1.0 - SHOULDER_KNEE;
        1.0 - room * (-(x - SHOULDER_KNEE) / room).exp()
    }
}

/// Highlight handling of one pixel. `gained` is the pixel after the exposure
/// gain, `base_max` the largest channel before the gain.
#[inline]
pub fn highlight_pixel(gained: [f32; 3], base_max: f32) -> [f32; 3] {
    let m = gained[0].max(gained[1]).max(gained[2]);
    let sensor = smoothstep(CLIP_DESAT_START, CLIP_DESAT_END, base_max);
    if m <= SHOULDER_KNEE && sensor <= 0.0 {
        return gained;
    }
    let t = sensor.max(smoothstep(1.0, OVER_DESAT_END, m));
    let mixed = gained.map(|v| v + t * (m - v));
    if m <= SHOULDER_KNEE {
        return mixed;
    }
    let s = shoulder(m) / m;
    mixed.map(|v| v * s)
}

// S-curve applied to sRGB-encoded values. TONE_GAMMA above 1 darkens the
// mid-tones a little (auto exposure maps the top 1 % to white and tends to
// run brighter than camera JPEGs); TONE_CONTRAST blends toward a smoothstep
// (toe + shoulder). Chosen by comparing the five sample files against their
// embedded JPEGs, measurements in docs/V0.10_REPORT.md.
const TONE_GAMMA: f32 = 1.12;
const TONE_CONTRAST: f32 = 0.45;

/// The default tone curve on sRGB-encoded values: gently S-shaped, fixed at
/// 0 and 1, strictly increasing.
#[inline]
pub fn tone_curve(y: f32) -> f32 {
    let y = y.clamp(0.0, 1.0).powf(TONE_GAMMA);
    y + TONE_CONTRAST * (y * y * (3.0 - 2.0 * y) - y)
}

const LUT_SIZE: usize = 1 << 16;

/// Lookup table from linear light (0..=1) to the display value:
/// sRGB encoding, followed by the tone curve when `tone` is set.
fn tone_lut(tone: bool) -> &'static [f32] {
    static ON: OnceLock<Vec<f32>> = OnceLock::new();
    static OFF: OnceLock<Vec<f32>> = OnceLock::new();
    let build = move || -> Vec<f32> {
        (0..LUT_SIZE)
            .map(|i| {
                let y = srgb_oetf(i as f32 / (LUT_SIZE - 1) as f32);
                if tone {
                    tone_curve(y)
                } else {
                    y
                }
            })
            .collect()
    };
    if tone {
        ON.get_or_init(build)
    } else {
        OFF.get_or_init(build)
    }
}

#[inline]
fn lut_lookup(lut: &[f32], x: f32) -> f32 {
    lut[(x.clamp(0.0, 1.0) * (LUT_SIZE - 1) as f32 + 0.5) as usize]
}

/// Linear to display-referred: exposure gain, highlights, tone curve + sRGB.
pub fn apply_exposure_and_tone(img: &mut RgbF32Image, stats: &BaseStats, params: &RenderParams) {
    let gains = exposure_gains(
        stats,
        params.white_balance,
        params.brightness,
        params.exposure_ev,
    );
    let lut = tone_lut(params.tone_curve);
    let buf: &mut [f32] = img.as_mut();
    buf.par_chunks_mut(3 * 4096).for_each(|chunk| {
        for px in chunk.as_chunks_mut::<3>().0 {
            let base_max = px[0].max(px[1]).max(px[2]);
            let gained = [px[0] * gains[0], px[1] * gains[1], px[2] * gains[2]];
            let q = highlight_pixel(gained, base_max);
            for c in 0..3 {
                px[c] = lut_lookup(lut, q[c]);
            }
        }
    });
}

/// Mean Rec.601 luminance (8-bit scale) of `img` after `gains`, highlights
/// and the default tone curve. Sequential on purpose (used by metering).
pub fn mean_display_luma(img: &RgbF32Image, gains: [f32; 3]) -> f64 {
    let lut = tone_lut(true);
    let mut sum = 0f64;
    for p in img.pixels() {
        let base_max = p.0[0].max(p.0[1]).max(p.0[2]);
        let q = highlight_pixel(
            [p.0[0] * gains[0], p.0[1] * gains[1], p.0[2] * gains[2]],
            base_max,
        );
        sum += 0.299 * lut_lookup(lut, q[0]) as f64
            + 0.587 * lut_lookup(lut, q[1]) as f64
            + 0.114 * lut_lookup(lut, q[2]) as f64;
    }
    255.0 * sum / (img.width() * img.height()).max(1) as f64
}

fn check(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        Err(ConvertError::Cancelled)
    } else {
        Ok(())
    }
}

/// The whole chain from a linear base image to final pixels. `long_edge` 0
/// keeps the size. Used by both batch conversion and the GUI preview.
pub fn render(
    mut img: RgbF32Image,
    stats: &BaseStats,
    params: &RenderParams,
    long_edge: u32,
    cancel: &AtomicBool,
) -> Result<RgbF32Image> {
    check(cancel)?;
    apply_exposure_and_tone(&mut img, stats, params);
    check(cancel)?;
    adjust::apply(&mut img, params.rgb, params.black_point, params.white_point);
    style::apply(&mut img, params.style);
    check(cancel)?;
    let mut img = resize::resize_long_edge(img, long_edge);
    check(cancel)?;
    sharpen::unsharp_luma(&mut img, params.sharpen);
    Ok(img)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    fn stats(white: f32) -> BaseStats {
        BaseStats {
            white_camera: white,
            white_auto: white,
            auto_gains: [1.0, 1.0, 1.0],
            exposure_camera: crate::develop::auto_gain(white, 1.0),
            exposure_auto: crate::develop::auto_gain(white, 1.0),
            meter_source: Default::default(),
        }
    }

    fn params() -> RenderParams {
        RenderParams::from(&ConvertSettings::default())
    }

    #[test]
    fn tone_curve_endpoints_and_monotonic() {
        assert_eq!(tone_curve(0.0), 0.0);
        assert!((tone_curve(1.0) - 1.0).abs() < 1e-5);
        let mut last = -1.0;
        for i in 0..=1000 {
            let v = tone_curve(i as f32 / 1000.0);
            assert!(v > last, "not increasing at {i}: {v} <= {last}");
            last = v;
        }
    }

    #[test]
    fn tone_curve_adds_contrast() {
        // S-curve: mid-tones gain slope, values above mid rise relative to below.
        let span = tone_curve(0.7) - tone_curve(0.3);
        assert!(span > 0.4, "{span}");
        assert!(tone_curve(0.1) < 0.12, "shadows kept down");
    }

    #[test]
    fn lut_matches_direct_curve() {
        for x in [0.0, 0.001, 0.18, 0.5, 0.9, 1.0] {
            let via_lut = lut_lookup(tone_lut(true), x);
            let direct = tone_curve(srgb_oetf(x));
            assert!(
                (via_lut - direct).abs() < 2e-3,
                "{x}: {via_lut} vs {direct}"
            );
            let off = lut_lookup(tone_lut(false), x);
            assert!((off - srgb_oetf(x)).abs() < 2e-3, "{x}");
        }
    }

    #[test]
    fn ev_is_a_power_of_two_gain() {
        let s = stats(0.5);
        let base = exposure_gains(&s, WhiteBalance::CameraDefault, 1.0, 0.0)[1];
        assert!((base - 2.0).abs() < 1e-5);
        for ev in [-3.0f32, -1.0, 1.0, 3.0] {
            let g = exposure_gains(&s, WhiteBalance::CameraDefault, 1.0, ev)[1];
            assert!((g - base * 2f32.powf(ev)).abs() < 1e-4, "{ev}");
        }
    }

    #[test]
    fn brightness_two_equals_plus_one_ev() {
        let s = stats(0.8);
        let by_brightness = exposure_gains(&s, WhiteBalance::CameraDefault, 2.0, 0.0);
        let by_ev = exposure_gains(&s, WhiteBalance::CameraDefault, 1.0, 1.0);
        for c in 0..3 {
            assert!((by_brightness[c] - by_ev[c]).abs() < 1e-5);
        }
        // Brightness and EV multiply.
        let both = exposure_gains(&s, WhiteBalance::CameraDefault, 0.5, 1.0);
        let unity = exposure_gains(&s, WhiteBalance::CameraDefault, 1.0, 0.0);
        assert!((both[0] - unity[0]).abs() < 1e-5);
    }

    #[test]
    fn auto_white_balance_uses_precomputed_gains() {
        let s = BaseStats {
            white_camera: 0.5,
            white_auto: 0.25,
            auto_gains: [2.0, 1.0, 0.5],
            exposure_camera: 2.0,
            exposure_auto: 4.0,
            meter_source: Default::default(),
        };
        let g = exposure_gains(&s, WhiteBalance::Auto, 1.0, 0.0);
        assert!((g[0] - 8.0).abs() < 1e-5 && (g[1] - 4.0).abs() < 1e-5);
    }

    #[test]
    fn shoulder_is_identity_below_knee_and_monotonic_above() {
        assert_eq!(shoulder(0.5), 0.5);
        assert_eq!(shoulder(SHOULDER_KNEE), SHOULDER_KNEE);
        let mut last = SHOULDER_KNEE;
        for i in 1..200 {
            let x = SHOULDER_KNEE + i as f32 * 0.05;
            let v = shoulder(x);
            // Saturates towards 1.0 at the very top, so not strictly increasing there.
            assert!(v >= last && v < 1.0 + 1e-6, "{x}: {v}");
            last = v;
        }
        assert!(shoulder(8.0) > 0.999);
    }

    #[test]
    fn midtones_pass_through_highlight_stage_unchanged() {
        let p = [0.3, 0.2, 0.1];
        assert_eq!(highlight_pixel(p, 0.3), p);
    }

    #[test]
    fn clipped_pixels_lose_their_colour_cast() {
        // A clipped pixel where white balance pushed R/B past G: without the
        // stage the channels would clamp unevenly and tint the highlight.
        let q = highlight_pixel([1.6, 1.0, 1.3], 1.3);
        let spread = q[0].max(q[1]).max(q[2]) - q[0].min(q[1]).min(q[2]);
        assert!(spread < 0.03, "{q:?}");
        assert!(q.iter().all(|v| *v <= 1.0 && *v > 0.9), "{q:?}");
    }

    #[test]
    fn bright_unclipped_colour_keeps_its_hue() {
        // Above the knee but well below sensor clip: ratios survive.
        let p = [0.95f32, 0.5, 0.25];
        let q = highlight_pixel(p, 0.8);
        assert!(q[0] < p[0] && q[0] > SHOULDER_KNEE);
        assert!(((q[1] / q[0]) - (p[1] / p[0])).abs() < 1e-3, "{q:?}");
        assert!(((q[2] / q[0]) - (p[2] / p[0])).abs() < 1e-3, "{q:?}");
    }

    #[test]
    fn output_stays_in_range_for_extreme_ev() {
        let img = RgbF32Image::from_pixel(8, 8, Rgb([0.9, 0.2, 0.05]));
        let p = RenderParams {
            exposure_ev: 3.0,
            ..params()
        };
        let cancel = AtomicBool::new(false);
        let out = render(img, &stats(0.5), &p, 0, &cancel).unwrap();
        for px in out.pixels() {
            assert!(px.0.iter().all(|v| (0.0..=1.0).contains(v)), "{px:?}");
        }
    }

    #[test]
    fn render_is_deterministic_and_respects_settings() {
        let mut base = RgbF32Image::new(32, 32);
        for (x, y, p) in base.enumerate_pixels_mut() {
            *p = Rgb([
                x as f32 / 40.0,
                y as f32 / 40.0,
                ((x + y) % 7) as f32 / 10.0,
            ]);
        }
        let cancel = AtomicBool::new(false);
        let s = stats(0.8);
        let a = render(base.clone(), &s, &params(), 0, &cancel).unwrap();
        let b = render(base.clone(), &s, &params(), 0, &cancel).unwrap();
        assert_eq!(a, b);
        let brighter = render(
            base.clone(),
            &s,
            &RenderParams {
                exposure_ev: 1.0,
                ..params()
            },
            0,
            &cancel,
        )
        .unwrap();
        assert!(mean(&brighter) > mean(&a) + 0.05);
        let flat = render(
            base.clone(),
            &s,
            &RenderParams {
                tone_curve: false,
                sharpen: 0.0,
                ..params()
            },
            0,
            &cancel,
        )
        .unwrap();
        assert_ne!(flat, a);
        let styled = render(
            base,
            &s,
            &RenderParams {
                style: PictureStyle::Sepia,
                ..params()
            },
            0,
            &cancel,
        )
        .unwrap();
        assert_ne!(styled, a);
    }

    fn mean(img: &RgbF32Image) -> f32 {
        let b: &[f32] = img.as_ref();
        b.iter().sum::<f32>() / b.len() as f32
    }

    #[test]
    fn render_honours_cancel_flag() {
        let img = RgbF32Image::from_pixel(8, 8, Rgb([0.5, 0.5, 0.5]));
        let cancel = AtomicBool::new(true);
        assert!(render(img, &stats(0.5), &params(), 0, &cancel)
            .unwrap_err()
            .is_cancelled());
    }

    #[test]
    fn resize_happens_in_the_chain() {
        let img = RgbF32Image::from_pixel(64, 48, Rgb([0.5, 0.4, 0.3]));
        let cancel = AtomicBool::new(false);
        let out = render(img, &stats(0.7), &params(), 32, &cancel).unwrap();
        assert_eq!(out.dimensions(), (32, 24));
    }
}
