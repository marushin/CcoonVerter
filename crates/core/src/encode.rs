//! Output encoders: JPEG, PNG, 16-bit TIFF and WebP.
//!
//! JPEG uses the `jpeg-encoder` crate rather than `image`'s built-in encoder
//! because the latter only writes 4:4:4 (and is slower).
//! Settings mirror Pillow's `save("JPEG", quality=q)` default: 4:2:0 chroma
//! subsampling, standard quantisation tables scaled by quality.
//!
//! Python also passed `optimize=True` (optimised Huffman tables, a few percent
//! smaller files). With `jpeg-encoder` 0.7.1 optimised tables produced files
//! that `image` 0.25 / zune-jpeg decodes with wrong colours (caught by the
//! `colours_survive_roundtrip` test), so they stay disabled until verified
//! against a second decoder.
//!
//! What each format keeps:
//!
//! | Format | ICC   | EXIF                | Rating                 |
//! |--------|-------|---------------------|------------------------|
//! | JPEG   | APP2  | APP1, verbatim      | Exif tag + XMP APP1    |
//! | PNG    | iCCP  | eXIf, verbatim      | Exif tag + XMP iTXt    |
//! | WebP   | ICCP  | EXIF chunk          | Exif tag + XMP chunk   |
//! | TIFF   | 34675 | Native IFDs         | Exif tag + XMP tag 700 |
//!
//! TIFF stores the tags in its own directories, so a camera MakerNote cannot
//! come along: its internal offsets do not survive the move.
//!
//! WebP is lossy, through libwebp (`libwebp-sys`). The `image` crate only
//! writes lossless WebP, which is of little use for photographs.

use std::io::Cursor;

use ::exif::experimental::Writer;
use ::exif::{Field, In, Tag, Value};
use image::RgbImage;
use jpeg_encoder::{ColorType, Encoder, SamplingFactor};

use crate::error::{ConvertError, Result};
use crate::exif::{add_xmp_rating, xmp_rating_xml};
use crate::RgbF32Image;

/// Maximum dimension supported by baseline JPEG.
pub const MAX_JPEG_DIM: u32 = 65_535;
/// Maximum dimension of a WebP image.
pub const MAX_WEBP_DIM: u32 = 16_383;

/// Rows in one TIFF strip.
const TIFF_ROWS_PER_STRIP: usize = 256;

/// Metadata written next to the pixels.
#[derive(Debug, Clone, Copy, Default)]
pub struct OutputMeta<'a> {
    /// TIFF/EXIF payload starting with `II*\0` or `MM\0*`.
    pub exif: Option<&'a [u8]>,
    pub icc: Option<&'a [u8]>,
    /// Rating (1..=5), written as XMP where the format has a place for it.
    pub rating: Option<u32>,
}

/// Encode an RGB8 image to JPEG bytes. `exif_tiff`, if given, is the TIFF
/// payload (starting with `II*\0` / `MM\0*`) for an APP1 Exif segment.
pub fn encode_rgb8(img: &RgbImage, quality: u8, exif_tiff: Option<&[u8]>) -> Result<Vec<u8>> {
    encode_jpeg(
        img,
        quality,
        &OutputMeta {
            exif: exif_tiff,
            ..Default::default()
        },
    )
}

/// JPEG with EXIF (APP1), ICC profile (APP2) and XMP rating.
pub fn encode_jpeg(img: &RgbImage, quality: u8, meta: &OutputMeta) -> Result<Vec<u8>> {
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
    if let Some(tiff) = meta.exif {
        let mut seg = Vec::with_capacity(tiff.len() + 6);
        seg.extend_from_slice(b"Exif\0\0");
        seg.extend_from_slice(tiff);
        if let Err(e) = enc.add_app_segment(1, seg) {
            // Oversized EXIF (> 64 KiB) cannot be stored; keep the image.
            log::warn!("EXIF not written: {e}");
        }
    }
    if let Some(icc) = meta.icc {
        if let Err(e) = enc.add_icc_profile(icc) {
            log::warn!("ICC profile not written: {e}");
        }
    }
    enc.encode(img.as_raw(), w as u16, h as u16, ColorType::Rgb)
        .map_err(|e| ConvertError::Encode(e.to_string()))?;
    match meta.rating {
        Some(r) => Ok(add_xmp_rating(&out, r).unwrap_or_else(|e| {
            log::warn!("Could not write the XMP rating: {e}");
            out
        })),
        None => Ok(out),
    }
}

/// 8-bit PNG with `iCCP`, `eXIf` and an XMP `iTXt` chunk.
pub fn encode_png(img: &RgbImage, meta: &OutputMeta) -> Result<Vec<u8>> {
    let err = |e: png::EncodingError| ConvertError::Encode(e.to_string());
    let (w, h) = img.dimensions();
    let mut out = Vec::with_capacity(w as usize * h as usize + 4096);
    {
        let mut info = png::Info::with_size(w, h);
        info.color_type = png::ColorType::Rgb;
        info.bit_depth = png::BitDepth::Eight;
        info.icc_profile = meta.icc.map(Into::into);
        info.exif_metadata = meta.exif.map(Into::into);
        let mut enc = png::Encoder::with_info(&mut out, info).map_err(err)?;
        enc.set_compression(png::Compression::Balanced);
        if let Some(r) = meta.rating {
            enc.add_itxt_chunk("XML:com.adobe.xmp".to_string(), xmp_rating_xml(r))
                .map_err(err)?;
        }
        let mut writer = enc.write_header().map_err(err)?;
        writer.write_image_data(img.as_raw()).map_err(err)?;
        writer.finish().map_err(err)?;
    }
    Ok(out)
}

fn chunk(out: &mut Vec<u8>, fourcc: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(fourcc);
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(data);
    if data.len() % 2 == 1 {
        out.push(0);
    }
}

/// Wrap the single-image file libwebp produces into the extended container
/// with `ICCP`, `EXIF` and `XMP ` chunks. Unchanged when there is no metadata.
fn mux_webp(simple: &[u8], width: u32, height: u32, meta: &OutputMeta) -> Result<Vec<u8>> {
    let xmp = meta.rating.map(xmp_rating_xml);
    if meta.exif.is_none() && meta.icc.is_none() && xmp.is_none() {
        return Ok(simple.to_vec());
    }
    let bad = || ConvertError::Encode("libwebp returned an unexpected container".into());
    if simple.len() < 12 || &simple[..4] != b"RIFF" || &simple[8..12] != b"WEBP" {
        return Err(bad());
    }
    let image_chunks = &simple[12..];
    if !matches!(image_chunks.get(..4), Some(b"VP8 ") | Some(b"VP8L")) {
        return Err(bad());
    }

    let mut flags = 0u8;
    if meta.icc.is_some() {
        flags |= 0x20;
    }
    if meta.exif.is_some() {
        flags |= 0x08;
    }
    if xmp.is_some() {
        flags |= 0x04;
    }
    let mut vp8x = vec![flags, 0, 0, 0];
    vp8x.extend_from_slice(&(width - 1).to_le_bytes()[..3]);
    vp8x.extend_from_slice(&(height - 1).to_le_bytes()[..3]);

    let mut body = b"WEBP".to_vec();
    chunk(&mut body, b"VP8X", &vp8x);
    if let Some(icc) = meta.icc {
        chunk(&mut body, b"ICCP", icc);
    }
    body.extend_from_slice(image_chunks);
    if let Some(exif) = meta.exif {
        chunk(&mut body, b"EXIF", exif);
    }
    if let Some(x) = &xmp {
        chunk(&mut body, b"XMP ", x.as_bytes());
    }
    let size = u32::try_from(body.len())
        .map_err(|_| ConvertError::Encode("WebP file larger than 4 GiB".into()))?;
    let mut out = Vec::with_capacity(body.len() + 8);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}

/// Lossy WebP (libwebp) with `ICCP`, `EXIF` and `XMP ` chunks.
pub fn encode_webp(img: &RgbImage, quality: u8, meta: &OutputMeta) -> Result<Vec<u8>> {
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 || w > MAX_WEBP_DIM || h > MAX_WEBP_DIM {
        return Err(ConvertError::Encode(format!(
            "image size {w}x{h} cannot be stored as WebP (limit {MAX_WEBP_DIM} pixels per side)"
        )));
    }
    let mut output: *mut u8 = std::ptr::null_mut();
    // Safety: the buffer is a tightly packed RGB image of `w` x `h` (from an
    // `RgbImage`); libwebp allocates `output`, freed right after the copy.
    let encoded = unsafe {
        let len = libwebp_sys::WebPEncodeRGB(
            img.as_raw().as_ptr(),
            w as i32,
            h as i32,
            w as i32 * 3,
            f32::from(quality.clamp(1, 100)),
            &mut output,
        );
        if len == 0 || output.is_null() {
            return Err(ConvertError::Encode(
                "libwebp could not encode the image".into(),
            ));
        }
        let bytes = std::slice::from_raw_parts(output, len).to_vec();
        libwebp_sys::WebPFree(output.cast());
        bytes
    };
    mux_webp(&encoded, w, h, meta)
}

/// Values 16-bit precision is taken from directly: the f32 pixels are never
/// reduced to 8 bits on this path.
pub fn quantize16_le(img: &RgbF32Image) -> Vec<u8> {
    let mut out = Vec::with_capacity(img.as_raw().len() * 2);
    for v in img.as_raw() {
        let q = (v.clamp(0.0, 1.0) * 65535.0 + 0.5) as u16;
        out.extend_from_slice(&q.to_le_bytes());
    }
    out
}

fn is_image_structure_tag(tag: Tag) -> bool {
    matches!(
        tag,
        Tag::ImageWidth
            | Tag::ImageLength
            | Tag::BitsPerSample
            | Tag::Compression
            | Tag::PhotometricInterpretation
            | Tag::StripOffsets
            | Tag::SamplesPerPixel
            | Tag::RowsPerStrip
            | Tag::StripByteCounts
            | Tag::PlanarConfiguration
            | Tag::Orientation
            | Tag::XResolution
            | Tag::YResolution
            | Tag::ResolutionUnit
            | Tag::TransferFunction
            | Tag::WhitePoint
            | Tag::PrimaryChromaticities
            | Tag::YCbCrCoefficients
            | Tag::YCbCrSubSampling
            | Tag::YCbCrPositioning
            | Tag::ReferenceBlackWhite
            | Tag::JPEGInterchangeFormat
            | Tag::JPEGInterchangeFormatLength
            | Tag::MakerNote
            | Tag::PixelXDimension
            | Tag::PixelYDimension
    ) || matches!(
        tag,
        // No constants in kamadak-exif: NewSubfileType, SubfileType,
        // Predictor, SubIFDs, ExtraSamples, SampleFormat, tile layout,
        // JPEGTables, XMP and the ICC profile.
        Tag(
            ::exif::Context::Tiff,
            254 | 255 | 317 | 330 | 338 | 339 | 322..=325 | 347 | 700 | 34675
        )
    )
}

/// EXIF tags of `tiff` usable in a new TIFF file.
pub fn exif_fields_for_tiff(tiff: &[u8]) -> Vec<Field> {
    let Ok(ex) = ::exif::Reader::new().read_raw(tiff.to_vec()) else {
        return Vec::new();
    };
    let mut fields: Vec<Field> = Vec::new();
    for f in ex.fields() {
        let keep = f.ifd_num == In::PRIMARY
            && !is_image_structure_tag(f.tag)
            && !matches!(f.value, Value::Unknown(..))
            && !fields.iter().any(|g| g.tag == f.tag);
        if keep {
            fields.push(f.clone());
        }
    }
    fields
}

fn tiff_field(tag: Tag, value: Value) -> Field {
    Field {
        tag,
        ifd_num: In::PRIMARY,
        value,
    }
}

/// 16-bit-per-channel, uncompressed, little-endian RGB TIFF. EXIF becomes
/// native tags (Exif and GPS sub-directories), the ICC profile tag 34675.
pub fn encode_tiff16(img: &RgbF32Image, meta: &OutputMeta) -> Result<Vec<u8>> {
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        return Err(ConvertError::Encode(format!(
            "image size {w}x{h} cannot be stored as TIFF"
        )));
    }
    let samples = quantize16_le(img);
    if samples.len() as u64 > u64::from(u32::MAX) - (1 << 24) {
        return Err(ConvertError::Encode(format!(
            "image size {w}x{h} is too large for a 16-bit TIFF (4 GiB limit)"
        )));
    }

    let stride = w as usize * 6;
    let strip_bytes = stride * TIFF_ROWS_PER_STRIP;
    let strips: Vec<&[u8]> = samples.chunks(strip_bytes).collect();

    let mut fields = vec![
        tiff_field(Tag::ImageWidth, Value::Long(vec![w])),
        tiff_field(Tag::ImageLength, Value::Long(vec![h])),
        tiff_field(Tag::BitsPerSample, Value::Short(vec![16, 16, 16])),
        tiff_field(Tag::Compression, Value::Short(vec![1])),
        tiff_field(Tag::PhotometricInterpretation, Value::Short(vec![2])),
        tiff_field(Tag::SamplesPerPixel, Value::Short(vec![3])),
        tiff_field(
            Tag::RowsPerStrip,
            Value::Long(vec![TIFF_ROWS_PER_STRIP as u32]),
        ),
        tiff_field(Tag::PlanarConfiguration, Value::Short(vec![1])),
        tiff_field(Tag::Orientation, Value::Short(vec![1])),
        tiff_field(Tag::XResolution, Value::Rational(vec![(72, 1).into()])),
        tiff_field(Tag::YResolution, Value::Rational(vec![(72, 1).into()])),
        tiff_field(Tag::ResolutionUnit, Value::Short(vec![2])),
    ];
    if let Some(exif) = meta.exif {
        fields.extend(exif_fields_for_tiff(exif));
        fields.push(tiff_field(Tag::PixelXDimension, Value::Long(vec![w])));
        fields.push(tiff_field(Tag::PixelYDimension, Value::Long(vec![h])));
    }
    if let Some(icc) = meta.icc {
        fields.push(tiff_field(
            Tag(::exif::Context::Tiff, 34675),
            Value::Undefined(icc.to_vec(), 0),
        ));
    }
    if let Some(r) = meta.rating {
        fields.push(tiff_field(
            Tag(::exif::Context::Tiff, 700),
            Value::Byte(xmp_rating_xml(r).into_bytes()),
        ));
    }

    let mut writer = Writer::new();
    for f in &fields {
        writer.push_field(f);
    }
    writer.set_strips(&strips, In::PRIMARY);
    let mut cur = Cursor::new(Vec::with_capacity(samples.len() + 64 * 1024));
    writer
        .write(&mut cur, true)
        .map_err(|e| ConvertError::Encode(format!("TIFF: {e}")))?;
    Ok(cur.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::icc_profile;
    use crate::settings::ColorSpace;
    use image::{GenericImageView, Rgb};

    fn gradient() -> RgbImage {
        let mut img = RgbImage::new(64, 32);
        for (x, y, p) in img.enumerate_pixels_mut() {
            *p = Rgb([(x * 4) as u8, (y * 8) as u8, 128]);
        }
        img
    }

    fn sample_exif() -> Vec<u8> {
        crate::exif::build_exif(
            &crate::exif::ExifInfo {
                make: Some("SONY".into()),
                model: Some("ILCE-7M4".into()),
                iso: Some(400),
                gps: Some(crate::exif::GpsInfo {
                    latitude: Some([(37, 1), (33, 1), (1, 1)]),
                    latitude_ref: Some("N".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            1,
        )
        .unwrap()
    }

    fn contains(hay: &[u8], needle: &[u8]) -> bool {
        hay.windows(needle.len()).any(|w| w == needle)
    }

    #[test]
    fn encodes_decodable_jpeg_with_quality() {
        let img = gradient();
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

    #[test]
    fn jpeg_carries_icc_and_rating() {
        let meta = OutputMeta {
            icc: Some(icc_profile(ColorSpace::AdobeRgb)),
            rating: Some(4),
            ..Default::default()
        };
        let jpg = encode_jpeg(&gradient(), 90, &meta).unwrap();
        assert!(contains(&jpg, b"ICC_PROFILE\0\x01\x01"));
        assert!(contains(&jpg, b"AdobeRGB-compatible"));
        assert_eq!(crate::exif::find_xmp_rating(&jpg), Some(4));
        assert!(image::load_from_memory(&jpg).is_ok());
    }

    #[test]
    fn png_roundtrip_with_metadata() {
        let img = gradient();
        let exif = sample_exif();
        let icc = icc_profile(ColorSpace::Srgb);
        let png = encode_png(
            &img,
            &OutputMeta {
                exif: Some(&exif),
                icc: Some(icc),
                rating: Some(3),
            },
        )
        .unwrap();
        assert_eq!(&png[..4], b"\x89PNG");
        let back = image::load_from_memory(&png).unwrap().into_rgb8();
        assert_eq!(back, img, "PNG is lossless");

        let mut reader = png::Decoder::new(Cursor::new(png.as_slice()))
            .read_info()
            .unwrap();
        let info = reader.info();
        assert_eq!(info.icc_profile.as_deref(), Some(icc));
        assert_eq!(info.exif_metadata.as_deref(), Some(exif.as_slice()));
        let xmp = info.utf8_text.first().unwrap();
        assert_eq!(xmp.keyword, "XML:com.adobe.xmp");
        let mut buf = vec![0; reader.output_buffer_size().unwrap()];
        reader.next_frame(&mut buf).unwrap();
    }

    fn riff_chunks(b: &[u8]) -> Vec<(String, Vec<u8>)> {
        assert_eq!(&b[..4], b"RIFF");
        assert_eq!(
            u32::from_le_bytes(b[4..8].try_into().unwrap()) as usize + 8,
            b.len()
        );
        assert_eq!(&b[8..12], b"WEBP");
        let mut out = Vec::new();
        let mut i = 12;
        while i + 8 <= b.len() {
            let n = u32::from_le_bytes(b[i + 4..i + 8].try_into().unwrap()) as usize;
            out.push((
                String::from_utf8_lossy(&b[i..i + 4]).into_owned(),
                b[i + 8..i + 8 + n].to_vec(),
            ));
            i += 8 + n + (n & 1);
        }
        assert_eq!(i, b.len(), "chunks end exactly at the file end");
        out
    }

    #[test]
    fn webp_is_lossy_and_decodes_with_metadata() {
        let img = gradient();
        let hi = encode_webp(&img, 95, &OutputMeta::default()).unwrap();
        let lo = encode_webp(&img, 5, &OutputMeta::default()).unwrap();
        assert!(lo.len() < hi.len(), "quality changes the size");
        assert_eq!(&riff_chunks(&hi)[0].0, "VP8 ");

        let exif = sample_exif();
        let icc = icc_profile(ColorSpace::AdobeRgb);
        let full = encode_webp(
            &img,
            90,
            &OutputMeta {
                exif: Some(&exif),
                icc: Some(icc),
                rating: Some(5),
            },
        )
        .unwrap();
        let chunks = riff_chunks(&full);
        let names: Vec<&str> = chunks.iter().map(|c| c.0.as_str()).collect();
        assert_eq!(names, ["VP8X", "ICCP", "VP8 ", "EXIF", "XMP "]);
        assert_eq!(chunks[0].1[0], 0x20 | 0x08 | 0x04);
        assert_eq!(chunks[0].1[4], 63);
        assert_eq!(chunks[0].1[7], 31);
        assert_eq!(chunks[1].1, icc);
        assert_eq!(chunks[3].1, exif);
        let back = image::load_from_memory(&full).unwrap();
        assert_eq!(back.dimensions(), (64, 32));
    }

    #[test]
    fn webp_size_limit_is_a_clean_error() {
        let img = RgbImage::new(MAX_WEBP_DIM + 1, 2);
        let e = encode_webp(&img, 90, &OutputMeta::default()).unwrap_err();
        assert!(e.to_string().contains("16383"), "{e}");
    }

    #[test]
    fn tiff_keeps_sixteen_bits_and_metadata() {
        // A ramp finer than 8 bits: 1/65535 steps survive to the file.
        let mut img = RgbF32Image::new(600, 2);
        for (x, _, p) in img.enumerate_pixels_mut() {
            let v = (20_000 + x) as f32 / 65535.0;
            *p = Rgb([v, v, 1.0 - v]);
        }
        let exif = sample_exif();
        let icc = icc_profile(ColorSpace::AdobeRgb);
        let tif = encode_tiff16(
            &img,
            &OutputMeta {
                exif: Some(&exif),
                icc: Some(icc),
                rating: Some(2),
            },
        )
        .unwrap();

        let back = image::load_from_memory(&tif).unwrap().into_rgb16();
        assert_eq!(back.dimensions(), (600, 2));
        for x in 0..600u32 {
            let v = back.get_pixel(x, 1).0;
            assert_eq!(u32::from(v[0]), 20_000 + x, "x={x}");
        }

        let ex = ::exif::Reader::new().read_raw(tif).unwrap();
        let get = |t: Tag| ex.get_field(t, In::PRIMARY).unwrap();
        assert_eq!(get(Tag::ImageWidth).value.get_uint(0), Some(600));
        assert_eq!(
            get(Tag::PhotographicSensitivity).value.get_uint(0),
            Some(400)
        );
        assert!(ex.get_field(Tag::GPSLatitude, In::PRIMARY).is_some());
        match &get(Tag(::exif::Context::Tiff, 34675)).value {
            Value::Undefined(v, _) => assert_eq!(v, icc),
            v => panic!("{v:?}"),
        }
    }

    #[test]
    fn tiff_drops_foreign_structure_tags_from_the_camera_payload() {
        let mk = |tag, value| tiff_field(tag, value);
        let base = [
            mk(Tag::ImageWidth, Value::Long(vec![9999])),
            mk(Tag::Compression, Value::Short(vec![6])),
            mk(Tag::Make, Value::Ascii(vec![b"X".to_vec()])),
            mk(Tag::MakerNote, Value::Undefined(vec![1, 2, 3], 0)),
        ];
        let mut w = Writer::new();
        for f in &base {
            w.push_field(f);
        }
        let mut cur = Cursor::new(Vec::new());
        w.write(&mut cur, true).unwrap();
        let exif = cur.into_inner();
        let img = RgbF32Image::from_pixel(4, 4, Rgb([0.5, 0.5, 0.5]));
        let tif = encode_tiff16(
            &img,
            &OutputMeta {
                exif: Some(&exif),
                ..Default::default()
            },
        )
        .unwrap();
        let ex = ::exif::Reader::new().read_raw(tif).unwrap();
        let get = |t: Tag| ex.get_field(t, In::PRIMARY);
        assert_eq!(get(Tag::ImageWidth).unwrap().value.get_uint(0), Some(4));
        assert_eq!(get(Tag::Compression).unwrap().value.get_uint(0), Some(1));
        assert!(get(Tag::Make).is_some());
        assert!(get(Tag::MakerNote).is_none());
    }
}
