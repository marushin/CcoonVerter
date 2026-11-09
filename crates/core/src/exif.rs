//! EXIF handling for output JPEGs.
//!
//! Strategy (see `pipeline.rs`):
//! 1. Reuse the APP1 Exif payload of the RAW's embedded JPEG when present
//!    (read with `img-parts`). If the pixels were rotated upright, the
//!    Orientation tag is patched in place to 1.
//! 2. Otherwise build a minimal EXIF block from rawler's metadata with
//!    `kamadak-exif`'s writer: Make, Model, Orientation, Software, DateTime,
//!    ExposureTime, FNumber, ISO, DateTimeOriginal, ExposureBias, FocalLength,
//!    LensMake, LensModel, Artist, Copyright, BodySerialNumber.

use std::io::Cursor;

use ::exif::experimental::Writer;
use ::exif::{Field, In, Tag, Value};
use img_parts::jpeg::Jpeg;
use img_parts::{Bytes, ImageEXIF};
use rawler::decoders::RawMetadata;
use serde::Serialize;

use crate::config;
use crate::error::{ConvertError, Result};

/// The subset of RAW metadata we carry into the JPEG (also shown by `cli info`).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct ExifInfo {
    pub make: Option<String>,
    pub model: Option<String>,
    pub orientation: Option<u16>,
    pub date_time_original: Option<String>,
    pub exposure_time: Option<(u32, u32)>,
    pub fnumber: Option<(u32, u32)>,
    pub iso: Option<u32>,
    pub exposure_bias: Option<(i32, i32)>,
    pub focal_length: Option<(u32, u32)>,
    pub lens_make: Option<String>,
    pub lens_model: Option<String>,
    pub artist: Option<String>,
    pub copyright: Option<String>,
    pub serial_number: Option<String>,
}

fn non_empty(s: &str) -> Option<String> {
    let t = s.trim().trim_end_matches('\0').trim();
    (!t.is_empty()).then(|| t.to_string())
}

impl ExifInfo {
    pub fn from_rawler(md: &RawMetadata) -> Self {
        let e = &md.exif;
        let rat = |r: &Option<rawler::formats::tiff::Rational>| {
            r.as_ref().filter(|r| r.d != 0).map(|r| (r.n, r.d))
        };
        Self {
            make: non_empty(&md.make),
            model: non_empty(&md.model),
            orientation: e.orientation,
            date_time_original: e
                .date_time_original
                .as_deref()
                .or(e.create_date.as_deref())
                .and_then(non_empty),
            exposure_time: rat(&e.exposure_time),
            fnumber: rat(&e.fnumber),
            iso: e
                .iso_speed_ratings
                .map(u32::from)
                .or(e.iso_speed)
                .or(e.recommended_exposure_index)
                .filter(|v| *v > 0),
            exposure_bias: e
                .exposure_bias
                .as_ref()
                .filter(|r| r.d != 0)
                .map(|r| (r.n, r.d)),
            focal_length: rat(&e.focal_length),
            lens_make: e.lens_make.as_deref().and_then(non_empty),
            lens_model: e.lens_model.as_deref().and_then(non_empty),
            artist: e.artist.as_deref().and_then(non_empty),
            copyright: e.copyright.as_deref().and_then(non_empty),
            serial_number: e.serial_number.as_deref().and_then(non_empty),
        }
    }
}

fn ascii(tag: Tag, s: &str) -> Field {
    Field {
        tag,
        ifd_num: In::PRIMARY,
        value: Value::Ascii(vec![s.as_bytes().to_vec()]),
    }
}

/// Build a TIFF/EXIF payload (without the `Exif\0\0` prefix).
pub fn build_exif(info: &ExifInfo, orientation: u16) -> Result<Vec<u8>> {
    let software = format!("{} {}", config::APP_NAME, config::APP_VERSION);
    let now = chrono::Local::now().format("%Y:%m:%d %H:%M:%S").to_string();
    let mut fields: Vec<Field> = Vec::new();
    if let Some(v) = &info.make {
        fields.push(ascii(Tag::Make, v));
    }
    if let Some(v) = &info.model {
        fields.push(ascii(Tag::Model, v));
    }
    fields.push(Field {
        tag: Tag::Orientation,
        ifd_num: In::PRIMARY,
        value: Value::Short(vec![if (1..=8).contains(&orientation) {
            orientation
        } else {
            1
        }]),
    });
    fields.push(ascii(Tag::Software, &software));
    fields.push(ascii(Tag::DateTime, &now));
    if let Some(v) = &info.artist {
        fields.push(ascii(Tag::Artist, v));
    }
    if let Some(v) = &info.copyright {
        fields.push(ascii(Tag::Copyright, v));
    }
    fields.push(Field {
        tag: Tag::ExifVersion,
        ifd_num: In::PRIMARY,
        value: Value::Undefined(b"0232".to_vec(), 0),
    });
    if let Some((n, d)) = info.exposure_time {
        fields.push(Field {
            tag: Tag::ExposureTime,
            ifd_num: In::PRIMARY,
            value: Value::Rational(vec![(n, d).into()]),
        });
    }
    if let Some((n, d)) = info.fnumber {
        fields.push(Field {
            tag: Tag::FNumber,
            ifd_num: In::PRIMARY,
            value: Value::Rational(vec![(n, d).into()]),
        });
    }
    if let Some(iso) = info.iso {
        fields.push(Field {
            tag: Tag::PhotographicSensitivity,
            ifd_num: In::PRIMARY,
            value: Value::Short(vec![iso.min(u16::MAX as u32) as u16]),
        });
    }
    if let Some(v) = &info.date_time_original {
        fields.push(ascii(Tag::DateTimeOriginal, v));
        fields.push(ascii(Tag::DateTimeDigitized, v));
    }
    if let Some((n, d)) = info.exposure_bias {
        fields.push(Field {
            tag: Tag::ExposureBiasValue,
            ifd_num: In::PRIMARY,
            value: Value::SRational(vec![(n, d).into()]),
        });
    }
    if let Some((n, d)) = info.focal_length {
        fields.push(Field {
            tag: Tag::FocalLength,
            ifd_num: In::PRIMARY,
            value: Value::Rational(vec![(n, d).into()]),
        });
    }
    if let Some(v) = &info.serial_number {
        fields.push(ascii(Tag::BodySerialNumber, v));
    }
    if let Some(v) = &info.lens_make {
        fields.push(ascii(Tag::LensMake, v));
    }
    if let Some(v) = &info.lens_model {
        fields.push(ascii(Tag::LensModel, v));
    }

    let mut w = Writer::new();
    for f in &fields {
        w.push_field(f);
    }
    let mut cur = Cursor::new(Vec::new());
    w.write(&mut cur, true)
        .map_err(|e| ConvertError::Exif(e.to_string()))?;
    Ok(cur.into_inner())
}

/// Minimal EXIF with only the orientation tag (used when metadata is not
/// preserved but the stored pixels still need an orientation).
pub fn orientation_only_exif(orientation: u16) -> Result<Vec<u8>> {
    let f = Field {
        tag: Tag::Orientation,
        ifd_num: In::PRIMARY,
        value: Value::Short(vec![orientation]),
    };
    let mut w = Writer::new();
    w.push_field(&f);
    let mut cur = Cursor::new(Vec::new());
    w.write(&mut cur, true)
        .map_err(|e| ConvertError::Exif(e.to_string()))?;
    Ok(cur.into_inner())
}

/// TIFF payload of the first APP1 Exif segment of a JPEG.
pub fn jpeg_exif(jpeg: &[u8]) -> Option<Vec<u8>> {
    let j = Jpeg::from_bytes(Bytes::copy_from_slice(jpeg)).ok()?;
    j.exif().map(|b| b.to_vec()).filter(|b| b.len() >= 8)
}

/// Replace (or remove, with `None`) the Exif APP1 segment of a JPEG.
pub fn replace_jpeg_exif(jpeg: &[u8], exif_tiff: Option<&[u8]>) -> Result<Vec<u8>> {
    let mut j = Jpeg::from_bytes(Bytes::copy_from_slice(jpeg))
        .map_err(|e| ConvertError::Exif(format!("cannot parse JPEG: {e}")))?;
    j.set_exif(exif_tiff.map(Bytes::copy_from_slice));
    let mut out = Vec::with_capacity(jpeg.len() + 1024);
    j.encoder()
        .write_to(&mut out)
        .map_err(|e| ConvertError::Exif(e.to_string()))?;
    Ok(out)
}

struct TiffCursor<'a> {
    buf: &'a [u8],
    le: bool,
}

impl TiffCursor<'_> {
    fn u16(&self, pos: usize) -> Option<u16> {
        let b = self.buf.get(pos..pos + 2)?;
        Some(if self.le {
            u16::from_le_bytes([b[0], b[1]])
        } else {
            u16::from_be_bytes([b[0], b[1]])
        })
    }
    fn u32(&self, pos: usize) -> Option<u32> {
        let b = self.buf.get(pos..pos + 4)?;
        let a = [b[0], b[1], b[2], b[3]];
        Some(if self.le {
            u32::from_le_bytes(a)
        } else {
            u32::from_be_bytes(a)
        })
    }
}

/// Byte position of the Orientation SHORT value in IFD0, if present.
fn orientation_value_pos(tiff: &[u8]) -> Option<(usize, bool)> {
    let le = match tiff.get(0..4)? {
        b"II*\0" => true,
        b"MM\0*" => false,
        _ => return None,
    };
    let c = TiffCursor { buf: tiff, le };
    let ifd0 = c.u32(4)? as usize;
    let count = c.u16(ifd0)? as usize;
    for i in 0..count.min(512) {
        let e = ifd0 + 2 + i * 12;
        if c.u16(e)? == 0x0112 && c.u16(e + 2)? == 3 {
            return Some((e + 8, le));
        }
    }
    None
}

/// Orientation tag (0x0112) of a TIFF/EXIF payload.
pub fn tiff_orientation(tiff: &[u8]) -> Option<u16> {
    let (pos, le) = orientation_value_pos(tiff)?;
    TiffCursor { buf: tiff, le }
        .u16(pos)
        .filter(|v| (1..=8).contains(v))
}

/// Overwrite the Orientation value in place. Returns false if the tag is absent.
pub fn set_tiff_orientation(tiff: &mut [u8], value: u16) -> bool {
    match orientation_value_pos(tiff) {
        Some((pos, le)) => {
            let b = if le {
                value.to_le_bytes()
            } else {
                value.to_be_bytes()
            };
            tiff[pos..pos + 2].copy_from_slice(&b);
            true
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_info() -> ExifInfo {
        ExifInfo {
            make: Some("SONY".into()),
            model: Some("ILCE-7M4".into()),
            orientation: Some(6),
            date_time_original: Some("2026:01:02 03:04:05".into()),
            exposure_time: Some((1, 250)),
            fnumber: Some((28, 10)),
            iso: Some(400),
            exposure_bias: Some((-1, 3)),
            focal_length: Some((35, 1)),
            lens_make: None,
            lens_model: Some("FE 35mm F1.8".into()),
            artist: None,
            copyright: None,
            serial_number: Some("1234".into()),
        }
    }

    #[test]
    fn built_exif_is_readable_by_kamadak() {
        let tiff = build_exif(&sample_info(), 1).unwrap();
        let ex = ::exif::Reader::new().read_raw(tiff.clone()).unwrap();
        let get = |t: Tag| ex.get_field(t, In::PRIMARY).unwrap();
        assert_eq!(
            get(Tag::Make).display_value().to_string(),
            "\"SONY\"".to_string()
        );
        assert_eq!(get(Tag::Orientation).value.get_uint(0), Some(1));
        assert_eq!(
            get(Tag::PhotographicSensitivity).value.get_uint(0),
            Some(400)
        );
        match &get(Tag::FNumber).value {
            Value::Rational(v) => assert_eq!((v[0].num, v[0].denom), (28, 10)),
            v => panic!("{v:?}"),
        }
        assert!(get(Tag::LensModel)
            .display_value()
            .to_string()
            .contains("FE 35mm"));
        assert!(get(Tag::Software)
            .display_value()
            .to_string()
            .contains("CcooNverter"));
        assert_eq!(tiff_orientation(&tiff), Some(1));
    }

    #[test]
    fn orientation_patch_in_place_both_endians() {
        for le in [true, false] {
            let f = Field {
                tag: Tag::Orientation,
                ifd_num: In::PRIMARY,
                value: Value::Short(vec![8]),
            };
            let m = ascii(Tag::Make, "X");
            let mut w = Writer::new();
            w.push_field(&m);
            w.push_field(&f);
            let mut cur = Cursor::new(Vec::new());
            w.write(&mut cur, le).unwrap();
            let mut tiff = cur.into_inner();
            assert_eq!(tiff_orientation(&tiff), Some(8));
            assert!(set_tiff_orientation(&mut tiff, 1));
            assert_eq!(tiff_orientation(&tiff), Some(1));
            let ex = ::exif::Reader::new().read_raw(tiff).unwrap();
            assert_eq!(
                ex.get_field(Tag::Orientation, In::PRIMARY)
                    .unwrap()
                    .value
                    .get_uint(0),
                Some(1)
            );
        }
        let mut no_orient = build_exif(&ExifInfo::default(), 3).unwrap();
        assert_eq!(tiff_orientation(&no_orient), Some(3));
        assert!(set_tiff_orientation(&mut no_orient, 1));
        assert!(!set_tiff_orientation(&mut [0u8; 4], 1));
    }

    #[test]
    fn replace_and_read_jpeg_exif() {
        let jpg = crate::embedded::tests::tiny_jpeg(16, 16, [1, 2, 3]);
        assert!(jpeg_exif(&jpg).is_none());
        let tiff = orientation_only_exif(6).unwrap();
        let with = replace_jpeg_exif(&jpg, Some(&tiff)).unwrap();
        let got = jpeg_exif(&with).unwrap();
        assert_eq!(tiff_orientation(&got), Some(6));
        // Still a valid image.
        assert_eq!(image::load_from_memory(&with).unwrap().width(), 16);
        let stripped = replace_jpeg_exif(&with, None).unwrap();
        assert!(jpeg_exif(&stripped).is_none());
    }
}
