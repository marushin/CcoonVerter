//! Geometry: long-edge resize (Lanczos3), EXIF orientation, quantisation.

use image::imageops::{self, FilterType};
use image::{ImageBuffer, Pixel, RgbImage};
use rayon::prelude::*;

use crate::RgbF32Image;

/// Target size for a long-edge resize, using Python's arithmetic:
/// the long side becomes `long_edge`, the short side is truncated (`int()`).
/// Returns `None` when `long_edge == 0` (keep original). Like Python, images
/// smaller than the target are scaled up.
pub fn target_size(width: u32, height: u32, long_edge: u32) -> Option<(u32, u32)> {
    if long_edge == 0 || width == 0 || height == 0 {
        return None;
    }
    let (w, h) = if width >= height {
        let nh = (height as f64 * (long_edge as f64 / width as f64)) as u32;
        (long_edge, nh)
    } else {
        let nw = (width as f64 * (long_edge as f64 / height as f64)) as u32;
        (nw, long_edge)
    };
    let (w, h) = (w.max(1), h.max(1));
    if (w, h) == (width, height) {
        None
    } else {
        Some((w, h))
    }
}

/// Resize any image buffer so its long edge equals `long_edge` (Lanczos3).
pub fn resize_long_edge<P>(
    img: ImageBuffer<P, Vec<P::Subpixel>>,
    long_edge: u32,
) -> ImageBuffer<P, Vec<P::Subpixel>>
where
    P: Pixel + 'static,
{
    match target_size(img.width(), img.height(), long_edge) {
        Some((w, h)) => imageops::resize(&img, w, h, FilterType::Lanczos3),
        None => img,
    }
}

/// Rotate / flip pixels so that an image with EXIF `orientation` displays
/// upright with orientation 1. Unknown values are treated as 1.
pub fn apply_orientation<P>(
    img: ImageBuffer<P, Vec<P::Subpixel>>,
    orientation: u16,
) -> ImageBuffer<P, Vec<P::Subpixel>>
where
    P: Pixel + 'static,
{
    match orientation {
        2 => imageops::flip_horizontal(&img),
        3 => imageops::rotate180(&img),
        4 => imageops::flip_vertical(&img),
        5 => imageops::flip_horizontal(&imageops::rotate90(&img)),
        6 => imageops::rotate90(&img),
        7 => imageops::flip_horizontal(&imageops::rotate270(&img)),
        8 => imageops::rotate270(&img),
        _ => img,
    }
}

/// Convert 0..1 floats to 8-bit with rounding and clamping.
pub fn quantize(img: &RgbF32Image) -> RgbImage {
    let src: &[f32] = img.as_ref();
    let mut out = vec![0u8; src.len()];
    out.par_chunks_mut(4096)
        .zip(src.par_chunks(4096))
        .for_each(|(d, s)| {
            for (o, v) in d.iter_mut().zip(s) {
                *o = (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
            }
        });
    RgbImage::from_raw(img.width(), img.height(), out).expect("buffer size matches")
}

/// Convert 8-bit RGB to 0..1 floats.
pub fn to_f32(img: &RgbImage) -> RgbF32Image {
    let data: Vec<f32> = img.as_raw().iter().map(|&v| v as f32 / 255.0).collect();
    RgbF32Image::from_raw(img.width(), img.height(), data).expect("buffer size matches")
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    #[test]
    fn target_size_matches_python() {
        assert_eq!(target_size(6000, 4000, 2000), Some((2000, 1333)));
        assert_eq!(target_size(4000, 6000, 2000), Some((1333, 2000)));
        assert_eq!(target_size(3000, 3000, 1000), Some((1000, 1000)));
        assert_eq!(target_size(100, 50, 400), Some((400, 200))); // upscales like Python
        assert_eq!(target_size(6000, 4000, 0), None);
        assert_eq!(target_size(2000, 1333, 2000), None);
        assert_eq!(target_size(10000, 1, 100), Some((100, 1)));
    }

    #[test]
    fn resize_f32_and_u8() {
        let f = RgbF32Image::from_pixel(60, 40, Rgb([0.5, 0.25, 1.0]));
        let r = resize_long_edge(f, 30);
        assert_eq!(r.dimensions(), (30, 20));
        let p = r.get_pixel(15, 10).0;
        assert!((p[0] - 0.5).abs() < 1e-3);
        let u = RgbImage::from_pixel(40, 60, Rgb([10, 20, 30]));
        assert_eq!(resize_long_edge(u, 30).dimensions(), (20, 30));
    }

    #[test]
    fn orientation_rotations() {
        // 2x1 image: left red, right blue.
        let mut img = RgbImage::new(2, 1);
        img.put_pixel(0, 0, Rgb([255, 0, 0]));
        img.put_pixel(1, 0, Rgb([0, 0, 255]));
        let r6 = apply_orientation(img.clone(), 6);
        assert_eq!(r6.dimensions(), (1, 2));
        assert_eq!(r6.get_pixel(0, 0).0, [255, 0, 0]); // rotate 90 CW: left goes to top
        let r8 = apply_orientation(img.clone(), 8);
        assert_eq!(r8.get_pixel(0, 0).0, [0, 0, 255]);
        let r3 = apply_orientation(img.clone(), 3);
        assert_eq!(r3.get_pixel(0, 0).0, [0, 0, 255]);
        let r2 = apply_orientation(img.clone(), 2);
        assert_eq!(r2.get_pixel(0, 0).0, [0, 0, 255]);
        assert_eq!(apply_orientation(img.clone(), 1), img);
        assert_eq!(apply_orientation(img.clone(), 0), img);
        // 5 and 7 are transposes: 2x1 -> 1x2
        assert_eq!(
            apply_orientation(img.clone(), 5).get_pixel(0, 0).0,
            [255, 0, 0]
        );
        assert_eq!(
            apply_orientation(img.clone(), 7).get_pixel(0, 1).0,
            [255, 0, 0]
        );
    }

    #[test]
    fn quantize_rounds_and_clamps() {
        let mut f = RgbF32Image::new(1, 1);
        f.put_pixel(0, 0, Rgb([-0.1, 0.5, 1.7]));
        assert_eq!(quantize(&f).get_pixel(0, 0).0, [0, 128, 255]);
        let u = RgbImage::from_pixel(1, 1, Rgb([0, 128, 255]));
        assert_eq!(quantize(&to_f32(&u)), u);
    }
}
