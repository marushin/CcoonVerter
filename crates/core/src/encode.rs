//! JPEG encoding.
//!
//! Uses the `jpeg-encoder` crate rather than `image`'s built-in encoder
//! because the latter only writes 4:4:4 (and is slower).
//! Settings mirror Pillow's `save("JPEG", quality=q)` default: 4:2:0 chroma
//! subsampling, standard quantisation tables scaled by quality.
//!
//! Python also passed `optimize=True` (optimised Huffman tables, a few percent
//! smaller files). With `jpeg-encoder` 0.7.1 optimised tables produced files
//! that `image` 0.25 / zune-jpeg decodes with wrong colours (caught by the
//! `colours_survive_roundtrip` test), so they stay disabled until verified
//! against a second decoder.

use image::RgbImage;
use jpeg_encoder::{ColorType, Encoder, SamplingFactor};

use crate::error::{ConvertError, Result};

/// Maximum dimension supported by baseline JPEG.
pub const MAX_JPEG_DIM: u32 = 65_535;

/// Encode an RGB8 image to JPEG bytes. `exif_tiff`, if given, is the TIFF
/// payload (starting with `II*\0` / `MM\0*`) for an APP1 Exif segment.
pub fn encode_rgb8(img: &RgbImage, quality: u8, exif_tiff: Option<&[u8]>) -> Result<Vec<u8>> {
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 || w > MAX_JPEG_DIM || h > MAX_JPEG_DIM {
        return Err(ConvertError::Encode(format!(
            "image size {w}x{h} cannot be stored as JPEG"
        )));
    }
    let mut out = Vec::with_capacity((w as usize * h as usize) / 4 + 4096);
    let mut enc = Encoder::new(&mut out, quality.clamp(1, 100));
    enc.set_sampling_factor(SamplingFactor::R_4_2_0);
    enc.set_optimized_huffman_tables(false);
    if let Some(tiff) = exif_tiff {
        let mut seg = Vec::with_capacity(tiff.len() + 6);
        seg.extend_from_slice(b"Exif\0\0");
        seg.extend_from_slice(tiff);
        if let Err(e) = enc.add_app_segment(1, seg) {
            // Oversized EXIF (> 64 KiB) cannot be stored; keep the image.
            log::warn!("EXIF not written: {e}");
        }
    }
    enc.encode(img.as_raw(), w as u16, h as u16, ColorType::Rgb)
        .map_err(|e| ConvertError::Encode(e.to_string()))?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    #[test]
    fn encodes_decodable_jpeg_with_quality() {
        let mut img = RgbImage::new(64, 32);
        for (x, y, p) in img.enumerate_pixels_mut() {
            *p = Rgb([(x * 4) as u8, (y * 8) as u8, 128]);
        }
        let hi = encode_rgb8(&img, 95, None).unwrap();
        let lo = encode_rgb8(&img, 20, None).unwrap();
        assert!(hi.len() > lo.len());
        let back = image::load_from_memory(&hi).unwrap();
        assert_eq!((back.width(), back.height()), (64, 32));
    }

    #[test]
    fn colours_survive_roundtrip() {
        for c in [[200u8, 30, 40], [128, 128, 128], [20, 60, 220]] {
            let img = RgbImage::from_pixel(32, 32, Rgb(c));
            let jpg = encode_rgb8(&img, 95, None).unwrap();
            let back = image::load_from_memory(&jpg).unwrap().into_rgb8();
            let p = back.get_pixel(16, 16).0;
            for i in 0..3 {
                assert!((p[i] as i32 - c[i] as i32).abs() <= 4, "{c:?} -> {p:?}");
            }
        }
    }

    #[test]
    fn writes_exif_segment() {
        let img = RgbImage::from_pixel(8, 8, Rgb([1, 2, 3]));
        let tiff = b"II*\0\x08\0\0\0\0\0\0\0\0\0";
        let jpg = encode_rgb8(&img, 90, Some(tiff)).unwrap();
        let pos = jpg.windows(6).position(|w| w == b"Exif\0\0").unwrap();
        assert_eq!(&jpg[pos + 6..pos + 10], b"II*\0");
    }
}
