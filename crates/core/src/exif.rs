//! EXIF handling for output JPEGs.
//!
//! Strategy (see `pipeline.rs`):
//! 1. Reuse the APP1 Exif payload of the RAW's embedded JPEG when present
//!    (read with `img-parts`). If the pixels were rotated upright, the
//!    Orientation tag is patched in place to 1.
//! 2. If that payload misses GPS, lens or serial tags the RAW has, they are
//!    merged in with `kamadak-exif`'s writer (MakerNote is dropped then: its
//!    internal offsets do not survive the rewrite).
//! 3. Otherwise build an EXIF block from rawler's metadata: Make, Model,
//!    Orientation, Software, DateTime, ExposureTime, FNumber, ISO,
//!    DateTimeOriginal, ExposureBias, FocalLength, LensMake, LensModel,
//!    LensSpecification, Artist, Copyright, BodySerialNumber, LensSerialNumber,
//!    GPS.
//!
//! rawler 0.8.0 exposes no rating: `RawMetadata::rating` is always `None`, so
//! the rating comes from an `xmp:Rating` packet found in the file. Cameras
//! that keep the rating only in maker notes are not covered.

use std::io::Cursor;

use ::exif::experimental::Writer;
use ::exif::{Field, In, Tag, Value};
use img_parts::jpeg::{markers, Jpeg, JpegSegment};
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
    pub lens_serial_number: Option<String>,
    /// Focal range and aperture range of the lens, as four rationals.
    pub lens_spec: Option<[(u32, u32); 4]>,
    pub gps: Option<GpsInfo>,
    /// Rating (1..=5) from the file's XMP packet, if any.
    pub rating: Option<u32>,
}

/// The GPS tags carried into the output.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct GpsInfo {
    pub latitude: Option<[(u32, u32); 3]>,
    pub latitude_ref: Option<String>,
    pub longitude: Option<[(u32, u32); 3]>,
    pub longitude_ref: Option<String>,
    pub altitude: Option<(u32, u32)>,
    pub altitude_ref: Option<u8>,
    pub timestamp: Option<[(u32, u32); 3]>,
    pub date_stamp: Option<String>,
    pub img_direction: Option<(u32, u32)>,
    pub img_direction_ref: Option<String>,
    pub map_datum: Option<String>,
    pub speed: Option<(u32, u32)>,
    pub speed_ref: Option<String>,
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
            lens_serial_number: e.lens_serial_number.as_deref().and_then(non_empty),
            lens_spec: e.lens_spec.as_ref().and_then(|s| {
                if s.iter().any(|r| r.d == 0) {
                    return None;
                }
                Some([
                    (s[0].n, s[0].d),
                    (s[1].n, s[1].d),
                    (s[2].n, s[2].d),
                    (s[3].n, s[3].d),
                ])
            }),
            gps: e.gps.as_ref().and_then(GpsInfo::from_rawler),
            rating: md.rating.filter(|r| (1..=5).contains(r)),
        }
    }
}

impl GpsInfo {
    fn from_rawler(g: &rawler::exif::ExifGPS) -> Option<Self> {
        let rat = |r: &rawler::formats::tiff::Rational| (r.n, r.d);
        let tri = |a: &Option<[rawler::formats::tiff::Rational; 3]>| {
            a.as_ref().map(|a| [rat(&a[0]), rat(&a[1]), rat(&a[2])])
        };
        let single =
            |a: &Option<rawler::formats::tiff::Rational>| a.as_ref().filter(|r| r.d != 0).map(rat);
        let text = |a: &Option<String>| a.as_deref().and_then(non_empty);
        let out = Self {
            latitude: tri(&g.gps_latitude),
            latitude_ref: text(&g.gps_latitude_ref),
            longitude: tri(&g.gps_longitude),
            longitude_ref: text(&g.gps_longitude_ref),
            altitude: single(&g.gps_altitude),
            altitude_ref: g.gps_altitude_ref,
            timestamp: tri(&g.gps_timestamp),
            date_stamp: text(&g.gps_date_stamp),
            img_direction: single(&g.gps_img_direction),
            img_direction_ref: text(&g.gps_img_direction_ref),
            map_datum: text(&g.gps_map_datum),
            speed: single(&g.gps_speed),
            speed_ref: text(&g.gps_speed_ref),
        };
        (out != Self::default()).then_some(out)
    }
}

/// `xmp:Rating` (1..=5) found anywhere in the file, as attribute or element.
pub fn find_xmp_rating(buf: &[u8]) -> Option<u32> {
    const KEY: &[u8] = b"xmp:Rating";
    let mut start = 0usize;
    while let Some(rel) = buf.get(start..)?.windows(KEY.len()).position(|w| w == KEY) {
        let after = start + rel + KEY.len();
        if let Some(r) = parse_rating(buf.get(after..after + 12).unwrap_or(&buf[after..])) {
            return Some(r);
        }
        start = after;
    }
    None
}

fn parse_rating(s: &[u8]) -> Option<u32> {
    // `="4"` or `='4'` or `>4<`, spaces tolerated.
    let mut i = 0;
    while s.get(i).is_some_and(|c| c.is_ascii_whitespace()) {
        i += 1;
    }
    match s.get(i)? {
        b'=' => {
            i += 1;
            while s.get(i).is_some_and(|c| c.is_ascii_whitespace()) {
                i += 1;
            }
            if matches!(s.get(i)?, b'"' | b'\'') {
                i += 1;
            }
        }
        b'>' => i += 1,
        _ => return None,
    }
    let d = *s.get(i)?;
    if !d.is_ascii_digit() {
        return None;
    }
    let v = (d - b'0') as u32;
    (1..=5).contains(&v).then_some(v)
}

/// Adobe XMP packet carrying only the rating.
fn xmp_rating_packet(rating: u32) -> Vec<u8> {
    let mut seg = b"http://ns.adobe.com/xap/1.0/\0".to_vec();
    seg.extend_from_slice(
        format!(
            "<?xpacket begin='\u{feff}' id='W5M0MpCehiHzreSzNTczkc9d'?>\
             <x:xmpmeta xmlns:x='adobe:ns:meta/'>\
             <rdf:RDF xmlns:rdf='http://www.w3.org/1999/02/22-rdf-syntax-ns#'>\
             <rdf:Description rdf:about='' xmlns:xmp='http://ns.adobe.com/xap/1.0/' xmp:Rating='{rating}'/>\
             </rdf:RDF></x:xmpmeta><?xpacket end='w'?>"
        )
        .as_bytes(),
    );
    seg
}

/// Add an XMP APP1 segment with the rating, unless the JPEG already has one.
pub fn add_xmp_rating(jpeg: &[u8], rating: u32) -> Result<Vec<u8>> {
    let mut j = Jpeg::from_bytes(Bytes::copy_from_slice(jpeg))
        .map_err(|e| ConvertError::Exif(format!("cannot parse JPEG: {e}")))?;
    let already = j.segments().iter().any(|s| {
        s.marker() == markers::APP1 && s.contents().starts_with(b"http://ns.adobe.com/xap/1.0/\0")
    });
    if already {
        return Ok(jpeg.to_vec());
    }
    // After the leading APP0/APP1 segments (JFIF, Exif).
    let pos = j
        .segments()
        .iter()
        .position(|s| s.marker() != markers::APP0 && s.marker() != markers::APP1)
        .unwrap_or(j.segments().len());
    j.segments_mut().insert(
        pos,
        JpegSegment::new_with_contents(markers::APP1, Bytes::from(xmp_rating_packet(rating))),
    );
    let mut out = Vec::with_capacity(jpeg.len() + 1024);
    j.encoder()
        .write_to(&mut out)
        .map_err(|e| ConvertError::Exif(e.to_string()))?;
    Ok(out)
}

fn ascii(tag: Tag, s: &str) -> Field {
    Field {
        tag,
        ifd_num: In::PRIMARY,
        value: Value::Ascii(vec![s.as_bytes().to_vec()]),
    }
}

fn rationals(v: &[(u32, u32)]) -> Value {
    Value::Rational(v.iter().map(|&(n, d)| (n, d).into()).collect())
}

fn gps_field(tag: Tag, value: Value) -> Field {
    Field {
        tag,
        ifd_num: In::PRIMARY,
        value,
    }
}

fn gps_fields(g: &GpsInfo) -> Vec<Field> {
    let text = |tag, s: &Option<String>| {
        s.as_ref()
            .map(|v| gps_field(tag, Value::Ascii(vec![v.as_bytes().to_vec()])))
    };
    let tri = |tag, a: &Option<[(u32, u32); 3]>| a.as_ref().map(|a| gps_field(tag, rationals(a)));
    let single = |tag, a: &Option<(u32, u32)>| a.as_ref().map(|r| gps_field(tag, rationals(&[*r])));
    let mut f = vec![gps_field(Tag::GPSVersionID, Value::Byte(vec![2, 3, 0, 0]))];
    f.extend(tri(Tag::GPSLatitude, &g.latitude));
    f.extend(text(Tag::GPSLatitudeRef, &g.latitude_ref));
    f.extend(tri(Tag::GPSLongitude, &g.longitude));
    f.extend(text(Tag::GPSLongitudeRef, &g.longitude_ref));
    f.extend(single(Tag::GPSAltitude, &g.altitude));
    f.extend(
        g.altitude_ref
            .map(|r| gps_field(Tag::GPSAltitudeRef, Value::Byte(vec![r]))),
    );
    f.extend(tri(Tag::GPSTimeStamp, &g.timestamp));
    f.extend(text(Tag::GPSDateStamp, &g.date_stamp));
    f.extend(single(Tag::GPSImgDirection, &g.img_direction));
    f.extend(text(Tag::GPSImgDirectionRef, &g.img_direction_ref));
    f.extend(text(Tag::GPSMapDatum, &g.map_datum));
    f.extend(single(Tag::GPSSpeed, &g.speed));
    f.extend(text(Tag::GPSSpeedRef, &g.speed_ref));
    f
}

/// Tag 0x4746: the Exif "Rating" written by Microsoft and common tools.
const TAG_RATING: Tag = Tag(::exif::Context::Tiff, 0x4746);

/// Build a TIFF/EXIF payload (without the `Exif\0\0` prefix).
pub fn build_exif(info: &ExifInfo, orientation: u16) -> Result<Vec<u8>> {
    let fields = info_fields(info, orientation);
    write_fields(&fields)
}

fn write_fields(fields: &[Field]) -> Result<Vec<u8>> {
    let mut w = Writer::new();
    for f in fields {
        w.push_field(f);
    }
    let mut cur = Cursor::new(Vec::new());
    w.write(&mut cur, true)
        .map_err(|e| ConvertError::Exif(e.to_string()))?;
    Ok(cur.into_inner())
}

/// Tags that the RAW metadata can provide, for building or merging.
fn info_fields(info: &ExifInfo, orientation: u16) -> Vec<Field> {
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
    if let Some(s) = &info.lens_spec {
        fields.push(Field {
            tag: Tag::LensSpecification,
            ifd_num: In::PRIMARY,
            value: rationals(s),
        });
    }
    if let Some(v) = &info.lens_serial_number {
        fields.push(ascii(Tag::LensSerialNumber, v));
    }
    if let Some(g) = &info.gps {
        fields.extend(gps_fields(g));
    }
    if let Some(r) = info.rating {
        fields.push(Field {
            tag: TAG_RATING,
            ifd_num: In::PRIMARY,
            value: Value::Short(vec![r as u16]),
        });
    }
    fields
}

/// True when `info` has GPS, lens or serial tags the payload `tiff` lacks.
pub fn lacks_fields(tiff: &[u8], info: &ExifInfo) -> bool {
    let Ok(ex) = ::exif::Reader::new().read_raw(tiff.to_vec()) else {
        return true;
    };
    let has = |t: Tag| ex.get_field(t, In::PRIMARY).is_some();
    (info.gps.is_some() && !has(Tag::GPSLatitude) && !has(Tag::GPSLongitude))
        || (info.lens_model.is_some() && !has(Tag::LensModel))
        || (info.lens_make.is_some() && !has(Tag::LensMake))
        || (info.lens_spec.is_some() && !has(Tag::LensSpecification))
        || (info.serial_number.is_some() && !has(Tag::BodySerialNumber))
        || (info.lens_serial_number.is_some() && !has(Tag::LensSerialNumber))
}

/// The fields of `tiff` plus every field `info` adds, with `orientation` set.
/// The IFD1 thumbnail and MakerNote are not carried over.
pub fn merge_exif(tiff: &[u8], info: &ExifInfo, orientation: u16) -> Result<Vec<u8>> {
    let ex = ::exif::Reader::new()
        .read_raw(tiff.to_vec())
        .map_err(|e| ConvertError::Exif(e.to_string()))?;
    let mut fields: Vec<Field> = Vec::new();
    for f in ex.fields() {
        if f.ifd_num != In::PRIMARY || f.tag == Tag::MakerNote {
            continue;
        }
        if fields.iter().any(|g: &Field| g.tag == f.tag) {
            continue;
        }
        fields.push(f.clone());
    }
    for f in info_fields(info, orientation) {
        match fields.iter_mut().find(|g| g.tag == f.tag) {
            // The stored value wins, except the orientation of the pixels.
            Some(g) => {
                if f.tag == Tag::Orientation {
                    *g = f;
                }
            }
            None => fields.push(f),
        }
    }
    write_fields(&fields)
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
            lens_serial_number: Some("L77".into()),
            lens_spec: Some([(35, 1), (35, 1), (18, 10), (18, 10)]),
            gps: Some(sample_gps()),
            rating: Some(4),
        }
    }

    fn sample_gps() -> GpsInfo {
        GpsInfo {
            latitude: Some([(37, 1), (33, 1), (1, 1)]),
            latitude_ref: Some("N".into()),
            longitude: Some([(126, 1), (58, 1), (2, 1)]),
            longitude_ref: Some("E".into()),
            altitude: Some((123, 1)),
            altitude_ref: Some(0),
            ..Default::default()
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
    fn gps_lens_and_rating_tags_are_written() {
        let tiff = build_exif(&sample_info(), 1).unwrap();
        let ex = ::exif::Reader::new().read_raw(tiff).unwrap();
        let get = |t: Tag| ex.get_field(t, In::PRIMARY).unwrap();
        match &get(Tag::GPSLatitude).value {
            Value::Rational(v) => assert_eq!(v[0].num, 37),
            v => panic!("{v:?}"),
        }
        assert!(get(Tag::GPSLatitudeRef)
            .display_value()
            .to_string()
            .contains('N'));
        match &get(Tag::LensSpecification).value {
            Value::Rational(v) => assert_eq!(v.len(), 4),
            v => panic!("{v:?}"),
        }
        assert_eq!(get(TAG_RATING).value.get_uint(0), Some(4));
        assert!(get(Tag::LensSerialNumber)
            .display_value()
            .to_string()
            .contains("L77"));
    }

    #[test]
    fn merge_adds_missing_tags_and_keeps_existing_ones() {
        let base = {
            let mut info = ExifInfo {
                make: Some("X".into()),
                model: Some("Y".into()),
                ..Default::default()
            };
            info.iso = Some(100);
            build_exif(&info, 6).unwrap()
        };
        assert!(lacks_fields(&base, &sample_info()));
        let merged = merge_exif(&base, &sample_info(), 1).unwrap();
        let ex = ::exif::Reader::new().read_raw(merged.clone()).unwrap();
        // Existing value kept, orientation updated, GPS added.
        assert_eq!(
            ex.get_field(Tag::PhotographicSensitivity, In::PRIMARY)
                .unwrap()
                .value
                .get_uint(0),
            Some(100)
        );
        assert_eq!(tiff_orientation(&merged), Some(1));
        assert!(ex.get_field(Tag::GPSLongitude, In::PRIMARY).is_some());
        assert!(!lacks_fields(&merged, &sample_info()));
        let full = build_exif(&sample_info(), 1).unwrap();
        assert!(!lacks_fields(&full, &sample_info()));
    }

    #[test]
    fn xmp_rating_is_found_and_written() {
        let packet = b"<x:xmpmeta><rdf:Description xmp:Rating=\"4\"/></x:xmpmeta>";
        let mut blob = vec![0u8; 500];
        blob.extend_from_slice(packet);
        assert_eq!(find_xmp_rating(&blob), Some(4));
        assert_eq!(find_xmp_rating(b"xmp:Rating>3<"), Some(3));
        assert_eq!(find_xmp_rating(b"<xmp:Rating>5</xmp:Rating>"), Some(5));
        assert_eq!(find_xmp_rating(b"xmp:Rating=\"0\""), None);
        assert_eq!(find_xmp_rating(b"xmp:Rating=\"-1\""), None);
        assert_eq!(find_xmp_rating(b"nothing"), None);

        let jpg = crate::embedded::tests::tiny_jpeg(16, 16, [1, 2, 3]);
        let with = add_xmp_rating(&jpg, 3).unwrap();
        assert_eq!(find_xmp_rating(&with), Some(3));
        assert_eq!(image::load_from_memory(&with).unwrap().width(), 16);
        // A second call does not add another packet.
        assert_eq!(add_xmp_rating(&with, 2).unwrap(), with);
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
