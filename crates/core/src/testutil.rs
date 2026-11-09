//! Synthetic RAW fixtures for tests (no real camera files are committed).
//!
//! [`synthetic_dng`] writes a minimal, valid, uncompressed Bayer (RGGB) DNG
//! that rawler can decode, optionally followed by an embedded JPEG preview
//! blob (found by the embedded-JPEG scanner). It is public so the CLI's
//! integration tests can use it, but it is not part of the stable API.

use image::{Rgb, RgbImage};

/// Parameters of the synthetic DNG.
#[derive(Debug, Clone)]
pub struct DngSpec {
    pub width: u32,
    pub height: u32,
    /// EXIF orientation stored in IFD0 (1..=8).
    pub orientation: u16,
    /// As-shot neutral (camera RGB response to white).
    pub neutral: [f64; 3],
    /// Append an embedded JPEG preview of this size (unreferenced blob).
    pub preview: Option<(u32, u32)>,
}

impl Default for DngSpec {
    fn default() -> Self {
        Self {
            width: 64,
            height: 48,
            orientation: 1,
            neutral: [0.5, 1.0, 0.7],
            preview: Some((32, 24)),
        }
    }
}

/// Linear scene value (sRGB primaries) at a pixel: gradient + colour patches.
pub fn scene(x: u32, y: u32, w: u32, h: u32) -> [f64; 3] {
    let gx = x as f64 / w.max(1) as f64;
    if x < w / 2 {
        [gx * 0.8 + 0.05; 3]
    } else if y < h / 2 {
        [0.6, 0.1, 0.05] // red patch
    } else {
        [0.05, 0.15, 0.5] // blue patch
    }
}

struct Entry {
    tag: u16,
    typ: u16,
    count: u32,
    data: Vec<u8>,
}

fn short(tag: u16, v: &[u16]) -> Entry {
    Entry {
        tag,
        typ: 3,
        count: v.len() as u32,
        data: v.iter().flat_map(|x| x.to_le_bytes()).collect(),
    }
}
fn long(tag: u16, v: &[u32]) -> Entry {
    Entry {
        tag,
        typ: 4,
        count: v.len() as u32,
        data: v.iter().flat_map(|x| x.to_le_bytes()).collect(),
    }
}
fn byte(tag: u16, v: &[u8]) -> Entry {
    Entry {
        tag,
        typ: 1,
        count: v.len() as u32,
        data: v.to_vec(),
    }
}
fn ascii(tag: u16, s: &str) -> Entry {
    let mut d = s.as_bytes().to_vec();
    d.push(0);
    Entry {
        tag,
        typ: 2,
        count: d.len() as u32,
        data: d,
    }
}
fn rational(tag: u16, v: &[(u32, u32)]) -> Entry {
    Entry {
        tag,
        typ: 5,
        count: v.len() as u32,
        data: v
            .iter()
            .flat_map(|(n, d)| n.to_le_bytes().into_iter().chain(d.to_le_bytes()))
            .collect(),
    }
}
fn srational(tag: u16, v: &[(i32, i32)]) -> Entry {
    Entry {
        tag,
        typ: 10,
        count: v.len() as u32,
        data: v
            .iter()
            .flat_map(|(n, d)| n.to_le_bytes().into_iter().chain(d.to_le_bytes()))
            .collect(),
    }
}

/// Build the DNG bytes.
pub fn synthetic_dng(spec: &DngSpec) -> Vec<u8> {
    let (w, h) = (spec.width, spec.height);
    let white = 65535.0;
    // Raw Bayer RGGB data: camera value = scene * neutral (so WB = 1/neutral restores it).
    let mut pix = Vec::with_capacity((w * h * 2) as usize);
    for y in 0..h {
        for x in 0..w {
            let c = match (y % 2, x % 2) {
                (0, 0) => 0,
                (1, 1) => 2,
                _ => 1,
            };
            let v = scene(x, y, w, h)[c] * spec.neutral[c] * 0.9;
            let v = (v * white).round().clamp(0.0, white) as u16;
            pix.extend_from_slice(&v.to_le_bytes());
        }
    }

    // XYZ(D65) -> camera matrix; camera space == linear sRGB here.
    let xyz_to_srgb = [
        3.2406, -1.5372, -0.4986, -0.9689, 1.8758, 0.0415, 0.0557, -0.2040, 1.0570,
    ];
    let cm: Vec<(i32, i32)> = xyz_to_srgb
        .iter()
        .map(|v: &f64| ((v * 10000.0).round() as i32, 10000))
        .collect();
    let neutral: Vec<(u32, u32)> = spec
        .neutral
        .iter()
        .map(|v| ((v * 10000.0).round() as u32, 10000))
        .collect();

    let header_len = 8u32;
    let data_offset = header_len;
    let ifd_offset = data_offset + pix.len() as u32;

    let mut entries = vec![
        long(254, &[0]),
        long(256, &[w]),
        long(257, &[h]),
        short(258, &[16]),
        short(259, &[1]),
        short(262, &[32803]),
        ascii(271, "CcooNverter"),
        ascii(272, "Synthetic DNG"),
        long(273, &[data_offset]),
        short(274, &[spec.orientation]),
        short(277, &[1]),
        long(278, &[h]),
        long(279, &[pix.len() as u32]),
        short(33421, &[2, 2]),
        byte(33422, &[0, 1, 1, 2]),
        rational(33434, &[(1, 125)]),
        rational(33437, &[(28, 10)]),
        short(34855, &[200]),
        ascii(36867, "2026:11:09 12:00:00"),
        rational(37386, &[(35, 1)]),
        byte(50706, &[1, 4, 0, 0]),
        ascii(50708, "CcooNverter Synthetic DNG"),
        long(50714, &[0]),
        long(50717, &[65535]),
        srational(50721, &cm),
        rational(50728, &neutral),
        short(50778, &[21]),
    ];
    entries.sort_by_key(|e| e.tag);

    let n = entries.len() as u32;
    let ifd_size = 2 + n * 12 + 4;
    let mut extra_offset = ifd_offset + ifd_size;
    let mut ifd = Vec::new();
    let mut extra = Vec::new();
    ifd.extend_from_slice(&(n as u16).to_le_bytes());
    for e in &entries {
        ifd.extend_from_slice(&e.tag.to_le_bytes());
        ifd.extend_from_slice(&e.typ.to_le_bytes());
        ifd.extend_from_slice(&e.count.to_le_bytes());
        if e.data.len() <= 4 {
            let mut v = e.data.clone();
            v.resize(4, 0);
            ifd.extend_from_slice(&v);
        } else {
            ifd.extend_from_slice(&extra_offset.to_le_bytes());
            extra.extend_from_slice(&e.data);
            if e.data.len() % 2 == 1 {
                extra.push(0);
            }
            extra_offset = ifd_offset + ifd_size + extra.len() as u32;
        }
    }
    ifd.extend_from_slice(&0u32.to_le_bytes());

    let mut out = Vec::new();
    out.extend_from_slice(b"II*\0");
    out.extend_from_slice(&ifd_offset.to_le_bytes());
    out.extend_from_slice(&pix);
    out.extend_from_slice(&ifd);
    out.extend_from_slice(&extra);

    if let Some((pw, ph)) = spec.preview {
        let mut img = RgbImage::new(pw, ph);
        for (x, y, p) in img.enumerate_pixels_mut() {
            let s = scene(x * w / pw, y * h / ph, w, h);
            let e = |v: f64| (crate::develop::srgb_oetf(v as f32) * 255.0) as u8;
            *p = Rgb([e(s[0]), e(s[1]), e(s[2])]);
        }
        let jpg = crate::encode::encode_rgb8(&img, 90, None).expect("preview encodes");
        out.extend_from_slice(&jpg);
    }
    out
}
