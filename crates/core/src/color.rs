//! Output colour spaces: matrices, transfer functions and ICC profiles.
//!
//! The working image is linear sRGB. For AdobeRGB output the pixels go through
//! the linear sRGB to linear AdobeRGB matrix (both D65, so no chromatic
//! adaptation) and the 563/256 gamma of the published specification.
//!
//! The profiles are generated here from the published primaries, white point
//! and transfer functions (IEC 61966-2-1 for sRGB, the AdobeRGB (1998) colour
//! image encoding document for the other). No profile file of any vendor is
//! shipped. The wide-gamut one is named "AdobeRGB-compatible" on purpose: it is
//! our own profile with the same colorimetry, not Adobe's file.
//!
//! Limit of the current pipeline: rawler's `Calibrate` step clips negative
//! linear sRGB channels (`clip_negative` in `rawler::imgop::raw`), so colours
//! outside sRGB are already gone when this module sees them. AdobeRGB output
//! holds correct colours in the larger container, but gains less than a true
//! wide-gamut pipeline would. Only later saturation stages can reach beyond
//! sRGB.

use std::sync::OnceLock;

use crate::settings::ColorSpace;

type Mat3 = [[f64; 3]; 3];

/// CIE xy of the red, green, blue primaries.
type Primaries = [[f64; 2]; 3];

const SRGB_PRIMARIES: Primaries = [[0.64, 0.33], [0.30, 0.60], [0.15, 0.06]];
const ADOBE_PRIMARIES: Primaries = [[0.64, 0.33], [0.21, 0.71], [0.15, 0.06]];
const D65: [f64; 2] = [0.3127, 0.3290];
/// ICC profile connection space illuminant (D50).
const ICC_D50: [f64; 3] = [0.9642, 1.0, 0.8249];

/// AdobeRGB gamma as published: 2 + 51/256.
pub const ADOBE_GAMMA: f32 = 563.0 / 256.0;

fn xy_to_xyz(xy: [f64; 2]) -> [f64; 3] {
    [xy[0] / xy[1], 1.0, (1.0 - xy[0] - xy[1]) / xy[1]]
}

fn mul(a: &Mat3, b: &Mat3) -> Mat3 {
    let mut m = [[0.0; 3]; 3];
    for (i, row) in m.iter_mut().enumerate() {
        for (j, v) in row.iter_mut().enumerate() {
            *v = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    m
}

fn apply(m: &Mat3, v: [f64; 3]) -> [f64; 3] {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
}

fn invert(m: &Mat3) -> Mat3 {
    let c = |a: usize, b: usize, x: usize, y: usize| m[a][b] * m[x][y] - m[a][y] * m[x][b];
    let cof = [
        [c(1, 1, 2, 2), c(0, 2, 2, 1), c(0, 1, 1, 2)],
        [c(1, 2, 2, 0), c(0, 0, 2, 2), c(0, 2, 1, 0)],
        [c(1, 0, 2, 1), c(0, 1, 2, 0), c(0, 0, 1, 1)],
    ];
    let det = m[0][0] * cof[0][0] + m[0][1] * cof[1][0] + m[0][2] * cof[2][0];
    let mut out = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            out[i][j] = cof[i][j] / det;
        }
    }
    out
}

/// Linear RGB to XYZ (white normalised to Y = 1).
fn rgb_to_xyz(primaries: &Primaries, white: [f64; 2]) -> Mat3 {
    let p: Vec<[f64; 3]> = primaries.iter().map(|xy| xy_to_xyz(*xy)).collect();
    let m = [
        [p[0][0], p[1][0], p[2][0]],
        [p[0][1], p[1][1], p[2][1]],
        [p[0][2], p[1][2], p[2][2]],
    ];
    let s = apply(&invert(&m), xy_to_xyz(white));
    let mut out = m;
    for row in out.iter_mut() {
        for (j, v) in row.iter_mut().enumerate() {
            *v *= s[j];
        }
    }
    out
}

/// Bradford adaptation from `src` to `dst` white (both XYZ).
fn bradford(src: [f64; 3], dst: [f64; 3]) -> Mat3 {
    const B: Mat3 = [
        [0.8951, 0.2664, -0.1614],
        [-0.7502, 1.7135, 0.0367],
        [0.0389, -0.0685, 1.0296],
    ];
    let s = apply(&B, src);
    let d = apply(&B, dst);
    let scale = [
        [d[0] / s[0], 0.0, 0.0],
        [0.0, d[1] / s[1], 0.0],
        [0.0, 0.0, d[2] / s[2]],
    ];
    mul(&invert(&B), &mul(&scale, &B))
}

/// Matrix from linear sRGB to linear AdobeRGB.
pub fn srgb_to_adobe_matrix() -> &'static [[f32; 3]; 3] {
    static M: OnceLock<[[f32; 3]; 3]> = OnceLock::new();
    M.get_or_init(|| {
        let srgb = rgb_to_xyz(&SRGB_PRIMARIES, D65);
        let adobe = rgb_to_xyz(&ADOBE_PRIMARIES, D65);
        mul(&invert(&adobe), &srgb).map(|r| r.map(|v| v as f32))
    })
}

/// sRGB decoding (inverse of [`crate::develop::srgb_oetf`]).
#[inline]
pub fn srgb_eotf(v: f32) -> f32 {
    if v <= 0.040_45 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

/// AdobeRGB encoding of a linear value in 0..=1.
#[inline]
pub fn adobe_oetf(v: f32) -> f32 {
    v.clamp(0.0, 1.0).powf(1.0 / ADOBE_GAMMA)
}

// ---------------------------------------------------------------------------
// ICC profile
// ---------------------------------------------------------------------------

/// Entries of the sampled sRGB curve (an ICC v2 `curv` has no parametric form).
const SRGB_CURVE_POINTS: usize = 1024;

fn s15f16(v: f64) -> [u8; 4] {
    ((v * 65536.0).round() as i32).to_be_bytes()
}

fn desc_tag(text: &str) -> Vec<u8> {
    let mut t = b"desc\0\0\0\0".to_vec();
    t.extend_from_slice(&(text.len() as u32 + 1).to_be_bytes());
    t.extend_from_slice(text.as_bytes());
    t.push(0);
    t.extend_from_slice(&[0; 8]); // Unicode language code and count
    t.extend_from_slice(&[0; 3]); // ScriptCode code and count
    t.extend_from_slice(&[0; 67]); // ScriptCode string
    t
}

fn text_tag(text: &str) -> Vec<u8> {
    let mut t = b"text\0\0\0\0".to_vec();
    t.extend_from_slice(text.as_bytes());
    t.push(0);
    t
}

fn xyz_tag(v: [f64; 3]) -> Vec<u8> {
    let mut t = b"XYZ \0\0\0\0".to_vec();
    for c in v {
        t.extend_from_slice(&s15f16(c));
    }
    t
}

fn curve_tag(points: &[u16]) -> Vec<u8> {
    let mut t = b"curv\0\0\0\0".to_vec();
    t.extend_from_slice(&(points.len() as u32).to_be_bytes());
    for p in points {
        t.extend_from_slice(&p.to_be_bytes());
    }
    t
}

/// An ICC v2.4 RGB matrix/TRC display profile.
fn build_profile(primaries: &Primaries, name: &str, curve: &[u16]) -> Vec<u8> {
    let white = xy_to_xyz(D65);
    let adapt = bradford(white, ICC_D50);
    let to_d50 = mul(&adapt, &rgb_to_xyz(primaries, D65));
    let column = |j: usize| [to_d50[0][j], to_d50[1][j], to_d50[2][j]];

    let curv = curve_tag(curve);
    let tags: Vec<(&[u8; 4], Vec<u8>)> = vec![
        (b"desc", desc_tag(name)),
        (
            b"cprt",
            text_tag("No copyright. Generated by CcooNverter from published primaries."),
        ),
        // Media white, stored unadapted like the reference profiles of v2.
        (b"wtpt", xyz_tag(white)),
        (b"rXYZ", xyz_tag(column(0))),
        (b"gXYZ", xyz_tag(column(1))),
        (b"bXYZ", xyz_tag(column(2))),
        (b"rTRC", curv.clone()),
        (b"gTRC", curv.clone()),
        (b"bTRC", curv),
    ];

    let table_len = 4 + tags.len() * 12;
    let mut body: Vec<u8> = Vec::new();
    let mut table = (tags.len() as u32).to_be_bytes().to_vec();
    let mut shared: Option<(u32, u32)> = None;
    for (sig, data) in &tags {
        // The three curves are identical and share one block of data.
        let is_trc = sig.ends_with(b"TRC");
        let (offset, len) = match (is_trc, shared) {
            (true, Some(s)) => s,
            _ => {
                let offset = (128 + table_len + body.len()) as u32;
                let len = data.len() as u32;
                body.extend_from_slice(data);
                while !body.len().is_multiple_of(4) {
                    body.push(0);
                }
                if is_trc {
                    shared = Some((offset, len));
                }
                (offset, len)
            }
        };
        table.extend_from_slice(*sig);
        table.extend_from_slice(&offset.to_be_bytes());
        table.extend_from_slice(&len.to_be_bytes());
    }

    let size = (128 + table.len() + body.len()) as u32;
    let mut h = Vec::with_capacity(size as usize);
    h.extend_from_slice(&size.to_be_bytes());
    h.extend_from_slice(&[0; 4]); // preferred CMM: none
    h.extend_from_slice(&[0x02, 0x40, 0, 0]); // version 2.4
    h.extend_from_slice(b"mntrRGB XYZ ");
    for v in [2026u16, 11, 9, 0, 0, 0] {
        h.extend_from_slice(&v.to_be_bytes());
    }
    h.extend_from_slice(b"acsp");
    h.extend_from_slice(&[0; 4]); // platform: none
    h.extend_from_slice(&[0; 4]); // flags
    h.extend_from_slice(&[0; 8]); // manufacturer, model
    h.extend_from_slice(&[0; 8]); // attributes
    h.extend_from_slice(&[0; 4]); // perceptual intent
    for c in ICC_D50 {
        h.extend_from_slice(&s15f16(c));
    }
    h.extend_from_slice(b"ccoo"); // creator
    h.resize(128, 0);
    h.extend_from_slice(&table);
    h.extend_from_slice(&body);
    h
}

/// The generated ICC profile of `space`. Deterministic, built once.
pub fn icc_profile(space: ColorSpace) -> &'static [u8] {
    static SRGB: OnceLock<Vec<u8>> = OnceLock::new();
    static ADOBE: OnceLock<Vec<u8>> = OnceLock::new();
    match space {
        ColorSpace::Srgb => SRGB.get_or_init(|| {
            let curve: Vec<u16> = (0..SRGB_CURVE_POINTS)
                .map(|i| {
                    let x = i as f32 / (SRGB_CURVE_POINTS - 1) as f32;
                    (srgb_eotf(x) * 65535.0 + 0.5) as u16
                })
                .collect();
            build_profile(&SRGB_PRIMARIES, "sRGB-compatible (CcooNverter)", &curve)
        }),
        ColorSpace::AdobeRgb => ADOBE.get_or_init(|| {
            // Single-entry `curv` is a u8.8 gamma: 0x0233 = 563/256.
            build_profile(
                &ADOBE_PRIMARIES,
                "AdobeRGB-compatible (CcooNverter)",
                &[0x0233],
            )
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tag<'a>(p: &'a [u8], sig: &[u8; 4]) -> &'a [u8] {
        let n = u32::from_be_bytes(p[128..132].try_into().unwrap()) as usize;
        for i in 0..n {
            let e = &p[132 + i * 12..144 + i * 12];
            if &e[..4] == sig {
                let o = u32::from_be_bytes(e[4..8].try_into().unwrap()) as usize;
                let l = u32::from_be_bytes(e[8..12].try_into().unwrap()) as usize;
                return &p[o..o + l];
            }
        }
        panic!("no tag {sig:?}");
    }

    fn xyz(p: &[u8], sig: &[u8; 4]) -> [f64; 3] {
        let t = tag(p, sig);
        assert_eq!(&t[..4], b"XYZ ");
        [8, 12, 16].map(|o| i32::from_be_bytes(t[o..o + 4].try_into().unwrap()) as f64 / 65536.0)
    }

    fn close(a: [f64; 3], b: [f64; 3], tol: f64) {
        for i in 0..3 {
            assert!((a[i] - b[i]).abs() < tol, "{a:?} vs {b:?}");
        }
    }

    #[test]
    fn colorants_match_the_published_d50_values() {
        let s = icc_profile(ColorSpace::Srgb);
        close(xyz(s, b"rXYZ"), [0.4360, 0.2225, 0.0139], 2e-4);
        close(xyz(s, b"gXYZ"), [0.3851, 0.7169, 0.0971], 2e-4);
        close(xyz(s, b"bXYZ"), [0.1431, 0.0606, 0.7139], 2e-4);
        let a = icc_profile(ColorSpace::AdobeRgb);
        close(xyz(a, b"rXYZ"), [0.6097, 0.3111, 0.0195], 2e-4);
        close(xyz(a, b"gXYZ"), [0.2053, 0.6257, 0.0609], 2e-4);
        close(xyz(a, b"bXYZ"), [0.1492, 0.0632, 0.7446], 2e-4);
    }

    #[test]
    fn colorants_add_up_to_d50() {
        for space in [ColorSpace::Srgb, ColorSpace::AdobeRgb] {
            let p = icc_profile(space);
            let (r, g, b) = (xyz(p, b"rXYZ"), xyz(p, b"gXYZ"), xyz(p, b"bXYZ"));
            close(
                [r[0] + g[0] + b[0], r[1] + g[1] + b[1], r[2] + g[2] + b[2]],
                ICC_D50,
                1e-4,
            );
        }
    }

    #[test]
    fn profile_header_and_name_are_valid() {
        for (space, name) in [
            (ColorSpace::Srgb, "sRGB-compatible"),
            (ColorSpace::AdobeRgb, "AdobeRGB-compatible"),
        ] {
            let p = icc_profile(space);
            assert_eq!(
                u32::from_be_bytes(p[..4].try_into().unwrap()) as usize,
                p.len()
            );
            assert_eq!(&p[36..40], b"acsp");
            assert_eq!(&p[12..24], b"mntrRGB XYZ ");
            assert!(p.len() < 64_000, "must fit one JPEG APP2 segment");
            let d = tag(p, b"desc");
            assert!(d.windows(name.len()).any(|w| w == name.as_bytes()));
        }
        let t = tag(icc_profile(ColorSpace::AdobeRgb), b"gTRC");
        assert_eq!(&t[8..14], &[0, 0, 0, 1, 0x02, 0x33]);
    }

    #[test]
    fn srgb_to_adobe_matrix_matches_reference() {
        let m = srgb_to_adobe_matrix();
        let want = [
            [0.71513, 0.28487, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.04116, 0.95884],
        ];
        for i in 0..3 {
            for j in 0..3 {
                assert!((m[i][j] - want[i][j]).abs() < 2e-3, "{m:?}");
            }
            // White stays white.
            assert!((m[i].iter().sum::<f32>() - 1.0).abs() < 1e-5, "{m:?}");
        }
    }

    #[test]
    fn transfer_functions_invert() {
        for i in 0..=1000 {
            let v = i as f32 / 1000.0;
            let back = srgb_eotf(crate::develop::srgb_oetf(v));
            assert!((back - v).abs() < 1e-4, "{v}: {back}");
            let a = adobe_oetf(v);
            assert!((a.powf(ADOBE_GAMMA) - v).abs() < 1e-4, "{v}");
        }
    }
}
