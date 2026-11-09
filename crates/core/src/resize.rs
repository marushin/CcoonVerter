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

/// Parallel box-filter reduction by an integer factor; used before Lanczos so
/// huge images reduce quickly and without aliasing. Three channels.
fn box_reduce<T: Copy + Send + Sync>(
    src: &[T],
    width: u32,
    height: u32,
    factor: u32,
    to_f32: impl Fn(T) -> f32 + Sync,
    from_f32: impl Fn(f32) -> T + Sync,
) -> (u32, u32, Vec<T>) {
    let f = factor as usize;
    let w = width as usize / f;
    let h = height as usize / f;
    let src_w = width as usize;
    let norm = 1.0 / (f * f) as f32;
    let zero = src[0];
    let mut out = vec![zero; w * h * 3];
    out.par_chunks_mut(w * 3).enumerate().for_each(|(y, row)| {
        for x in 0..w {
            let mut sum = [0f32; 3];
            for yy in y * f..(y + 1) * f {
                let start = (yy * src_w + x * f) * 3;
                for px in src[start..start + f * 3].as_chunks::<3>().0 {
                    sum[0] += to_f32(px[0]);
                    sum[1] += to_f32(px[1]);
                    sum[2] += to_f32(px[2]);
                }
            }
            row[x * 3] = from_f32(sum[0] * norm);
            row[x * 3 + 1] = from_f32(sum[1] * norm);
            row[x * 3 + 2] = from_f32(sum[2] * norm);
        }
    });
    (w as u32, h as u32, out)
}

/// Largest integer factor that keeps the long edge at or above `target`.
fn reduction_factor(long: u32, target: u32) -> u32 {
    (long / target.max(1)).max(1)
}

/// Downscale so the long edge equals `long_edge`; never upscales.
pub fn reduce_long_edge_f32(img: RgbF32Image, long_edge: u32) -> RgbF32Image {
    let long = img.width().max(img.height());
    if long_edge == 0 || long <= long_edge {
        return img;
    }
    let k = reduction_factor(long, long_edge);
    let img = if k > 1 {
        let (w, h, data) = box_reduce(
            img.as_raw(),
            img.width(),
            img.height(),
            k,
            |v: f32| v,
            |v| v,
        );
        RgbF32Image::from_raw(w, h, data).expect("reduced buffer")
    } else {
        img
    };
    resize_long_edge(img, long_edge)
}

/// [`reduce_long_edge_f32`] for 8-bit images.
pub fn reduce_long_edge_u8(img: RgbImage, long_edge: u32) -> RgbImage {
    let long = img.width().max(img.height());
    if long_edge == 0 || long <= long_edge {
        return img;
    }
    let k = reduction_factor(long, long_edge);
    let img = if k > 1 {
        let (w, h, data) = box_reduce(
            img.as_raw(),
            img.width(),
            img.height(),
            k,
            |v: u8| v as f32,
            |v| (v + 0.5).min(255.0) as u8,
        );
        RgbImage::from_raw(w, h, data).expect("reduced buffer")
    } else {
        img
    };
    resize_long_edge(img, long_edge)
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
    fn reduce_never_upscales_and_preserves_mean() {
        let f = RgbF32Image::from_pixel(5000, 3000, Rgb([0.25, 0.5, 0.75]));
        let r = reduce_long_edge_f32(f, 1600);
        assert_eq!(r.dimensions(), (1600, 960));
        let p = r.get_pixel(800, 480).0;
        assert!(
            (p[0] - 0.25).abs() < 1e-3 && (p[2] - 0.75).abs() < 1e-3,
            "{p:?}"
        );
        let small = RgbF32Image::new(800, 600);
        assert_eq!(reduce_long_edge_f32(small, 1600).dimensions(), (800, 600));
        let u = RgbImage::from_pixel(4800, 3200, Rgb([10, 200, 90]));
        let r8 = reduce_long_edge_u8(u, 1600);
        assert_eq!(r8.dimensions(), (1600, 1066));
        assert_eq!(r8.get_pixel(10, 10).0, [10, 200, 90]);
    }

    #[test]
    fn box_factor_is_only_integer_reduction() {
        assert_eq!(reduction_factor(6000, 1600), 3);
        assert_eq!(reduction_factor(1700, 1600), 1);
        assert_eq!(reduction_factor(1600, 1600), 1);
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
