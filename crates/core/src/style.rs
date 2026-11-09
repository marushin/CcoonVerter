//! Picture styles: Pillow `ImageEnhance` / `ImageOps` equivalents in f32.
//!
//! Every Pillow enhancer is `out = degenerate + f * (img - degenerate)`
//! (clipped to the valid range, as Pillow does in 8 bit):
//!
//! * `Color`: degenerate = grayscale `L = 0.299 R + 0.587 G + 0.114 B` (3 channels)
//! * `Contrast`: degenerate = mean of `L` over the whole image (single value)
//! * `Brightness`: degenerate = black
//! * `Sharpness`: degenerate = 3x3 SMOOTH filter `[1 1 1; 1 5 1; 1 1 1] / 13`,
//!   border pixels keep the original value
//! * `grayscale` = `L` in all channels; `colorize(black, white)` = per-channel
//!   `black + L * (white - black)`

use rayon::prelude::*;

use crate::settings::PictureStyle;
use crate::RgbF32Image;

/// Pillow's ITU-R 601-2 luma.
#[inline]
pub fn luma(r: f32, g: f32, b: f32) -> f32 {
    0.299 * r + 0.587 * g + 0.114 * b
}

const PAR_CHUNK: usize = 3 * 4096;

fn for_each_px(img: &mut RgbF32Image, f: impl Fn(&mut [f32; 3]) + Sync) {
    let buf: &mut [f32] = img.as_mut();
    buf.par_chunks_mut(PAR_CHUNK)
        .for_each(|chunk| chunk.as_chunks_mut::<3>().0.iter_mut().for_each(&f));
}

#[inline]
fn blend(degenerate: f32, v: f32, f: f32) -> f32 {
    (degenerate + f * (v - degenerate)).clamp(0.0, 1.0)
}

/// `ImageEnhance.Color(img).enhance(f)`.
pub fn color(img: &mut RgbF32Image, f: f32) {
    for_each_px(img, |p| {
        let l = luma(p[0], p[1], p[2]);
        for v in p.iter_mut() {
            *v = blend(l, *v, f);
        }
    });
}

/// Mean of the luma channel (Pillow uses the 8-bit histogram mean).
pub fn mean_luma(img: &RgbF32Image) -> f32 {
    let buf: &[f32] = img.as_ref();
    let n = buf.len() / 3;
    if n == 0 {
        return 0.0;
    }
    let sum: f64 = buf
        .par_chunks(PAR_CHUNK)
        .map(|c| {
            c.as_chunks::<3>()
                .0
                .iter()
                .map(|p| luma(p[0], p[1], p[2]) as f64)
                .sum::<f64>()
        })
        .sum();
    (sum / n as f64) as f32
}

/// `ImageEnhance.Contrast(img).enhance(f)`.
pub fn contrast(img: &mut RgbF32Image, f: f32) {
    let mean = mean_luma(img);
    for_each_px(img, |p| {
        for v in p.iter_mut() {
            *v = blend(mean, *v, f);
        }
    });
}

/// `ImageEnhance.Brightness(img).enhance(f)`.
pub fn brightness(img: &mut RgbF32Image, f: f32) {
    for_each_px(img, |p| {
        for v in p.iter_mut() {
            *v = blend(0.0, *v, f);
        }
    });
}

/// `ImageEnhance.Sharpness(img).enhance(f)`.
pub fn sharpness(img: &mut RgbF32Image, f: f32) {
    let (w, h) = (img.width() as usize, img.height() as usize);
    if w < 3 || h < 3 {
        return; // SMOOTH leaves images this small unchanged
    }
    let src: Vec<f32> = img.as_raw().clone();
    let stride = w * 3;
    let dst: &mut [f32] = img.as_mut();
    dst.par_chunks_mut(stride)
        .enumerate()
        .skip(1)
        .take(h - 2)
        .for_each(|(y, row)| {
            let up = &src[(y - 1) * stride..y * stride];
            let mid = &src[y * stride..(y + 1) * stride];
            let down = &src[(y + 1) * stride..(y + 2) * stride];
            for x in 1..w - 1 {
                for c in 0..3 {
                    let i = x * 3 + c;
                    let s = up[i - 3]
                        + up[i]
                        + up[i + 3]
                        + mid[i - 3]
                        + 5.0 * mid[i]
                        + mid[i + 3]
                        + down[i - 3]
                        + down[i]
                        + down[i + 3];
                    let smooth = s / 13.0;
                    row[i] = blend(smooth, mid[i], f);
                }
            }
        });
}

/// `ImageOps.grayscale(img).convert("RGB")`.
pub fn grayscale(img: &mut RgbF32Image) {
    for_each_px(img, |p| {
        let l = luma(p[0], p[1], p[2]).clamp(0.0, 1.0);
        p.fill(l);
    });
}

fn hex_rgb(hex: u32) -> [f32; 3] {
    [
        ((hex >> 16) & 0xFF) as f32 / 255.0,
        ((hex >> 8) & 0xFF) as f32 / 255.0,
        (hex & 0xFF) as f32 / 255.0,
    ]
}

/// `ImageOps.colorize(ImageOps.grayscale(img), black, white)`.
pub fn colorize(img: &mut RgbF32Image, black: u32, white: u32) {
    let b = hex_rgb(black);
    let w = hex_rgb(white);
    for_each_px(img, |p| {
        let l = luma(p[0], p[1], p[2]).clamp(0.0, 1.0);
        for c in 0..3 {
            p[c] = b[c] + l * (w[c] - b[c]);
        }
    });
}

/// Sepia colours used by Python (`#704214` → `#C0A080`).
pub const SEPIA_BLACK: u32 = 0x704214;
pub const SEPIA_WHITE: u32 = 0xC0A080;

/// Apply a picture style in place, in the exact order Python used.
pub fn apply(img: &mut RgbF32Image, style: PictureStyle) {
    use PictureStyle::*;
    match style {
        Standard => {}
        Portrait => {
            color(img, 0.9);
            contrast(img, 0.95);
            sharpness(img, 0.9);
        }
        Landscape => {
            color(img, 1.3);
            contrast(img, 1.15);
            sharpness(img, 1.2);
        }
        Neutral => {
            color(img, 0.85);
            contrast(img, 0.9);
        }
        Vivid => {
            color(img, 1.4);
            contrast(img, 1.1);
        }
        Monochrome => {
            grayscale(img);
            contrast(img, 1.1);
            sharpness(img, 1.05);
        }
        Sepia => {
            // grayscale -> colorize (colorize itself takes luma)
            colorize(img, SEPIA_BLACK, SEPIA_WHITE);
        }
        Cinematic => {
            color(img, 0.8);
            contrast(img, 1.2);
            brightness(img, 0.95);
        }
        Hdr => {
            contrast(img, 1.3);
            color(img, 1.2);
            sharpness(img, 1.15);
        }
        Soft => {
            color(img, 0.95);
            contrast(img, 0.85);
            sharpness(img, 0.8);
            brightness(img, 1.05);
        }
        Dramatic => {
            color(img, 1.4);
            contrast(img, 1.3);
            sharpness(img, 1.1);
        }
        BlackAndWhite => grayscale(img),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-5
    }

    fn two_px() -> RgbF32Image {
        // Pixel 0: (200, 100, 50)/255, pixel 1: (20, 40, 60)/255
        let mut img = RgbF32Image::new(2, 1);
        img.put_pixel(0, 0, Rgb([200.0 / 255.0, 100.0 / 255.0, 50.0 / 255.0]));
        img.put_pixel(1, 0, Rgb([20.0 / 255.0, 40.0 / 255.0, 60.0 / 255.0]));
        img
    }

    #[test]
    fn color_enhance_hand_values() {
        // L(200,100,50) = 59.8 + 58.7 + 5.7 = 124.2
        // Color 1.4: 124.2 + 1.4*(200-124.2) = 230.32 ; G: 124.2+1.4*(100-124.2)=90.32 ; B: 124.2+1.4*(50-124.2)=20.32
        let mut img = two_px();
        color(&mut img, 1.4);
        let p = img.get_pixel(0, 0).0;
        assert!(approx(p[0] * 255.0, 230.32), "{}", p[0] * 255.0);
        assert!(approx(p[1] * 255.0, 90.32));
        assert!(approx(p[2] * 255.0, 20.32));
        // Factor 0 = grayscale
        let mut img = two_px();
        color(&mut img, 0.0);
        let p = img.get_pixel(0, 0).0;
        assert!(approx(p[0], p[1]) && approx(p[1], p[2]));
        assert!(approx(p[0] * 255.0, 124.2));
    }

    #[test]
    fn contrast_uses_global_mean_luma() {
        // L0 = 124.2, L1 = 0.299*20 + 0.587*40 + 0.114*60 = 5.98+23.48+6.84 = 36.3 ; mean = 80.25
        let mut img = two_px();
        contrast(&mut img, 1.2);
        let p0 = img.get_pixel(0, 0).0;
        let p1 = img.get_pixel(1, 0).0;
        assert!(approx(p0[0] * 255.0, 80.25 + 1.2 * (200.0 - 80.25)));
        assert!(approx(p1[2] * 255.0, 80.25 + 1.2 * (60.0 - 80.25)));
        // Values clip.
        let mut img = two_px();
        contrast(&mut img, 3.0);
        assert_eq!(img.get_pixel(0, 0).0[0], 1.0);
        assert_eq!(img.get_pixel(1, 0).0[0], 0.0);
    }

    #[test]
    fn brightness_scales_towards_black() {
        let mut img = two_px();
        brightness(&mut img, 0.95);
        assert!(approx(img.get_pixel(0, 0).0[0] * 255.0, 190.0));
    }

    #[test]
    fn sharpness_kernel_and_border() {
        // 3x3 image, centre 1.0, rest 0 -> SMOOTH centre = 5/13
        let mut img = RgbF32Image::new(3, 3);
        img.put_pixel(1, 1, Rgb([1.0, 1.0, 1.0]));
        let orig = img.clone();
        sharpness(&mut img, 2.0);
        // out = 5/13 + 2 * (1 - 5/13) = 1.615 -> clipped to 1
        assert_eq!(img.get_pixel(1, 1).0[0], 1.0);
        // Borders untouched.
        for (x, y) in [(0, 0), (1, 0), (2, 2), (0, 1)] {
            assert_eq!(img.get_pixel(x, y), orig.get_pixel(x, y));
        }
        let mut img = orig.clone();
        sharpness(&mut img, 0.5);
        let expected = 5.0 / 13.0 + 0.5 * (1.0 - 5.0 / 13.0);
        assert!(approx(img.get_pixel(1, 1).0[1], expected));
        // Factor 1 is identity.
        let mut img = orig.clone();
        sharpness(&mut img, 1.0);
        assert_eq!(img, orig);
    }

    #[test]
    fn sepia_endpoints() {
        let mut img = RgbF32Image::new(2, 1);
        img.put_pixel(0, 0, Rgb([0.0, 0.0, 0.0]));
        img.put_pixel(1, 0, Rgb([1.0, 1.0, 1.0]));
        apply(&mut img, PictureStyle::Sepia);
        let b = img.get_pixel(0, 0).0;
        let w = img.get_pixel(1, 0).0;
        assert!(
            approx(b[0] * 255.0, 112.0) && approx(b[1] * 255.0, 66.0) && approx(b[2] * 255.0, 20.0)
        );
        assert!(
            approx(w[0] * 255.0, 192.0)
                && approx(w[1] * 255.0, 160.0)
                && approx(w[2] * 255.0, 128.0)
        );
    }

    #[test]
    fn grayscale_styles_are_neutral() {
        for st in [PictureStyle::BlackAndWhite, PictureStyle::Monochrome] {
            let mut img = two_px();
            apply(&mut img, st);
            for p in img.pixels() {
                assert!(approx(p.0[0], p.0[1]) && approx(p.0[1], p.0[2]), "{st}");
            }
        }
    }

    #[test]
    fn every_style_is_distinct_branch() {
        // A gradient image: all non-Standard styles must change it, and
        // results must differ pairwise (proves each branch is wired).
        let mut base = RgbF32Image::new(8, 8);
        for (x, y, p) in base.enumerate_pixels_mut() {
            *p = Rgb([x as f32 / 8.0, y as f32 / 8.0, ((x + y) % 5) as f32 / 5.0]);
        }
        let mut outs = Vec::new();
        for st in PictureStyle::ALL {
            let mut img = base.clone();
            apply(&mut img, st);
            if st == PictureStyle::Standard {
                assert_eq!(img, base);
            } else {
                assert_ne!(img, base, "{st} changed nothing");
            }
            outs.push((st, img));
        }
        for i in 0..outs.len() {
            for j in i + 1..outs.len() {
                assert_ne!(outs[i].1, outs[j].1, "{} == {}", outs[i].0, outs[j].0);
            }
        }
    }
}
