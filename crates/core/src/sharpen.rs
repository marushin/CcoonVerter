//! Light output sharpening: an unsharp mask on luma only, so colours do not
//! fringe. Runs last in the chain, at output resolution (see `render`).

use rayon::prelude::*;

use crate::style::luma;
use crate::RgbF32Image;

/// Largest luma change per pixel; keeps halos around hard edges small.
const HALO_LIMIT: f32 = 0.12;
/// Differences below this are mostly noise and are attenuated.
const CORING: f32 = 0.006;

#[inline]
fn blur5(a: f32, b: f32, c: f32, d: f32, e: f32) -> f32 {
    (a + e + 4.0 * (b + d) + 6.0 * c) / 16.0
}

/// Unsharp-mask `img` (display-referred, 0..1) in place with a 5-tap Gaussian
/// (sigma about 1 px). `amount` 0 does nothing.
pub fn unsharp_luma(img: &mut RgbF32Image, amount: f32) {
    let (w, h) = (img.width() as usize, img.height() as usize);
    if amount <= 0.0 || w < 5 || h < 5 {
        return;
    }
    let plane: Vec<f32> = img
        .as_raw()
        .par_chunks_exact(3)
        .map(|p| luma(p[0], p[1], p[2]))
        .collect();

    // Horizontal pass, edges clamped.
    let mut horiz = vec![0f32; w * h];
    horiz
        .par_chunks_mut(w)
        .zip(plane.par_chunks(w))
        .for_each(|(dst, row)| {
            for (x, d) in dst.iter_mut().enumerate() {
                *d = blur5(
                    row[x.saturating_sub(2)],
                    row[x.saturating_sub(1)],
                    row[x],
                    row[(x + 1).min(w - 1)],
                    row[(x + 2).min(w - 1)],
                );
            }
        });

    // Vertical pass.
    let mut blurred = vec![0f32; w * h];
    blurred.par_chunks_mut(w).enumerate().for_each(|(y, dst)| {
        let row = |dy: isize| {
            let yy = (y as isize + dy).clamp(0, h as isize - 1) as usize;
            &horiz[yy * w..(yy + 1) * w]
        };
        let (a, b, c, d, e) = (row(-2), row(-1), row(0), row(1), row(2));
        for (x, out) in dst.iter_mut().enumerate() {
            *out = blur5(a[x], b[x], c[x], d[x], e[x]);
        }
    });

    let buf: &mut [f32] = img.as_mut();
    buf.par_chunks_mut(w * 3)
        .zip(plane.par_chunks(w).zip(blurred.par_chunks(w)))
        .for_each(|(row, (luma_row, blur_row))| {
            for (x, px) in row.as_chunks_mut::<3>().0.iter_mut().enumerate() {
                let d = luma_row[x] - blur_row[x];
                let d = if d.abs() < CORING {
                    d * (d.abs() / CORING)
                } else {
                    d
                };
                let delta = (d * amount).clamp(-HALO_LIMIT, HALO_LIMIT);
                for v in px.iter_mut() {
                    *v = (*v + delta).clamp(0.0, 1.0);
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    fn step_edge(w: u32, h: u32) -> RgbF32Image {
        let mut img = RgbF32Image::new(w, h);
        for (x, _, p) in img.enumerate_pixels_mut() {
            let v = if x < w / 2 { 0.3 } else { 0.7 };
            *p = Rgb([v, v, v]);
        }
        img
    }

    #[test]
    fn zero_amount_is_identity() {
        let mut img = step_edge(16, 16);
        let before = img.clone();
        unsharp_luma(&mut img, 0.0);
        assert_eq!(img, before);
    }

    #[test]
    fn flat_regions_stay_flat() {
        let mut img = RgbF32Image::from_pixel(32, 32, Rgb([0.4, 0.5, 0.6]));
        let before = img.clone();
        unsharp_luma(&mut img, 1.0);
        for (a, b) in img.pixels().zip(before.pixels()) {
            for c in 0..3 {
                assert!((a.0[c] - b.0[c]).abs() < 1e-5);
            }
        }
    }

    #[test]
    fn edges_gain_contrast_within_the_halo_limit() {
        let mut img = step_edge(32, 8);
        let before = img.clone();
        unsharp_luma(&mut img, 1.0);
        let left = img.get_pixel(15, 4).0[0];
        let right = img.get_pixel(16, 4).0[0];
        assert!(
            left < before.get_pixel(15, 4).0[0],
            "dark side darker: {left}"
        );
        assert!(
            right > before.get_pixel(16, 4).0[0],
            "bright side brighter: {right}"
        );
        assert!(0.3 - left <= HALO_LIMIT + 1e-5 && right - 0.7 <= HALO_LIMIT + 1e-5);
    }

    #[test]
    fn tiny_images_are_left_alone() {
        let mut img = RgbF32Image::from_pixel(4, 4, Rgb([0.2, 0.2, 0.2]));
        let before = img.clone();
        unsharp_luma(&mut img, 2.0);
        assert_eq!(img, before);
    }
}
