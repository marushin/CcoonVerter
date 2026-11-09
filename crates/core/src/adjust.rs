//! RGB channel multipliers and Levels, applied in f32 sRGB-gamma space (0..1).
//!
//! Python applied these to 8-bit sRGB values:
//! `rgb *= scale; clip(0,255)` then `(rgb - bp) * 255 / (wp - bp); clip`.
//! The same formulas are used here on 0..1 floats, so a value of `v/255`
//! maps to the same result `/255` as Python, without the 8-bit rounding.

use rayon::prelude::*;

use crate::config::{DEFAULT_BLACK_POINT, DEFAULT_WHITE_POINT};
use crate::RgbF32Image;

/// Levels mapping of a single value.
#[inline]
pub fn levels(x: f32, black_point: u8, white_point: u8) -> f32 {
    let bp = black_point as f32 / 255.0;
    let wp = (white_point.max(black_point.saturating_add(1))) as f32 / 255.0;
    ((x - bp) / (wp - bp)).clamp(0.0, 1.0)
}

/// Apply RGB multipliers then Levels in place. No-op for default values.
pub fn apply(img: &mut RgbF32Image, rgb: [f32; 3], black_point: u8, white_point: u8) {
    let scale = rgb != [1.0, 1.0, 1.0];
    let lv = black_point != DEFAULT_BLACK_POINT || white_point != DEFAULT_WHITE_POINT;
    if !scale && !lv {
        return;
    }
    let buf: &mut [f32] = img.as_mut();
    buf.par_chunks_mut(3 * 1024).for_each(|chunk| {
        for px in chunk.as_chunks_mut::<3>().0 {
            for c in 0..3 {
                let mut v = px[c];
                if scale {
                    v = (v * rgb[c]).clamp(0.0, 1.0);
                }
                if lv {
                    v = levels(v, black_point, white_point);
                }
                px[c] = v;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    fn px(v: [f32; 3]) -> RgbF32Image {
        RgbF32Image::from_pixel(1, 1, Rgb(v))
    }

    #[test]
    fn levels_matches_python_formula() {
        // Python: (x - bp) * 255 / (wp - bp), clipped, on 8-bit values.
        for (x8, bp, wp) in [
            (128u8, 30u8, 200u8),
            (30, 30, 200),
            (200, 30, 200),
            (10, 30, 200),
        ] {
            let expected = ((x8 as f32 - bp as f32) * (255.0 / (wp as f32 - bp as f32)))
                .clamp(0.0, 255.0)
                / 255.0;
            let got = levels(x8 as f32 / 255.0, bp, wp);
            assert!(
                (got - expected).abs() < 1e-6,
                "{x8} {bp} {wp}: {got} vs {expected}"
            );
        }
        assert_eq!(levels(0.5, 0, 255), 0.5);
    }

    #[test]
    fn rgb_scale_then_levels() {
        let mut img = px([0.5, 0.5, 0.8]);
        apply(&mut img, [1.5, 1.0, 1.5], 0, 255);
        let p = img.get_pixel(0, 0).0;
        assert!((p[0] - 0.75).abs() < 1e-6);
        assert!((p[1] - 0.5).abs() < 1e-6);
        assert_eq!(p[2], 1.0); // clipped

        let mut img = px([0.5, 0.2, 1.0]);
        apply(&mut img, [2.0, 1.0, 1.0], 51, 204);
        let p = img.get_pixel(0, 0).0;
        // R: 1.0 after scale -> levels -> 1.0
        assert_eq!(p[0], 1.0);
        // G: (0.2 - 0.2) / 0.6 = 0
        assert!(p[1].abs() < 1e-6);
        assert_eq!(p[2], 1.0);
    }

    #[test]
    fn defaults_are_identity() {
        let mut img = px([0.1, 0.2, 0.3]);
        apply(&mut img, [1.0, 1.0, 1.0], 0, 255);
        assert_eq!(img.get_pixel(0, 0).0, [0.1, 0.2, 0.3]);
    }
}
