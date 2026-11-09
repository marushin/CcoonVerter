//! Small upright pictures for the thumbnail panel, from the camera JPEG only.
//!
//! The RAW decoder is deliberately not used here: scrolling past a damaged
//! file must not be able to reach it. The bytes are searched by the scanner in
//! [`crate::embedded`], read from the start of the file only as far as needed.

use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use image::{ImageFormat, RgbImage};

use crate::embedded::{self, JpegCandidate};
use crate::error::{ConvertError, Result};
use crate::exif;
use crate::pipeline::embedded_orientation;
use crate::resize;

/// How much of a file is read at each attempt.
const READ_STEPS: [usize; 4] = [2 << 20, 8 << 20, 32 << 20, MAX_READ];
/// Nothing past this offset is looked at, whatever the file claims to be.
const MAX_READ: usize = 256 << 20;
/// Where the RAW container's own orientation can be found.
const HEAD_LEN: usize = 256 << 10;
/// A JPEG with more pixels than this is not decoded for a thumbnail
/// (about 300 MB of RGB; the largest camera previews are about 102 MP).
pub const MAX_JPEG_PIXELS: u64 = 102_000_000;
/// A stream with fewer bytes than one per this many pixels cannot really hold
/// them: its header is damaged, and believing it would allocate for nothing.
/// Even a solid black frame uses far more than this.
const MAX_PIXELS_PER_BYTE: u64 = 256;

fn check(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        Err(ConvertError::Cancelled)
    } else {
        Ok(())
    }
}

/// Worth decoding: complete, and with a size its bytes can account for.
fn plausible(c: &JpegCandidate) -> bool {
    c.pixels() <= MAX_JPEG_PIXELS
        && (c.len as u64).saturating_mul(MAX_PIXELS_PER_BYTE) >= c.pixels()
}

fn long_edge_of(c: &JpegCandidate) -> u32 {
    c.width.max(c.height)
}

/// The cheapest candidate that still fills `long_edge`, else the largest.
fn pick(candidates: &[JpegCandidate], long_edge: u32) -> Option<JpegCandidate> {
    let usable = candidates.iter().filter(|c| plausible(c));
    usable
        .clone()
        .filter(|c| long_edge_of(c) >= long_edge)
        .min_by_key(|c| c.pixels())
        .or_else(|| usable.max_by_key(|c| (c.pixels(), c.len)))
        .copied()
}

/// EXIF orientation of the RAW container, from the start of the file.
/// 1 when the container has none or is not one of the TIFF-based layouts.
fn container_orientation(head: &[u8]) -> u16 {
    let plain_tiff = |t: &[u8]| exif::tiff_orientation(t);
    let found = match head.get(0..4) {
        Some(b"II*\0") | Some(b"MM\0*") => plain_tiff(head),
        // Panasonic and Olympus: a TIFF with its own magic number.
        Some(b"IIU\0") | Some(b"IIRO") | Some(b"IIRS") => {
            let mut t = head.to_vec();
            t[..4].copy_from_slice(b"II*\0");
            plain_tiff(&t)
        }
        Some(b"MMOR") => {
            let mut t = head.to_vec();
            t[..4].copy_from_slice(b"MM\0*");
            plain_tiff(&t)
        }
        // CR3: the camera settings are a TIFF stored in a CMT1 box.
        _ => head
            .windows(4)
            .position(|w| w == b"CMT1")
            .and_then(|pos| plain_tiff(&head[pos + 4..])),
    };
    found.unwrap_or(1)
}

/// Decode the chosen JPEG, reduce it to `long_edge` and make it upright.
fn render(jpeg: &[u8], raw_orientation: u16, long_edge: u32) -> Result<RgbImage> {
    let orientation = embedded_orientation(jpeg, raw_orientation);
    let img = image::load_from_memory_with_format(jpeg, ImageFormat::Jpeg)?.into_rgb8();
    let img = resize::reduce_long_edge_u8(img, long_edge);
    Ok(resize::apply_orientation(img, orientation))
}

/// The same work as [`load_thumbnail`] on bytes already in memory.
/// `Ok(None)` when they hold no usable JPEG.
pub fn thumbnail_from_bytes(buf: &[u8], long_edge: u32) -> Result<Option<RgbImage>> {
    let Some(c) = pick(&embedded::find_jpegs(buf), long_edge) else {
        return Ok(None);
    };
    let head = &buf[..buf.len().min(HEAD_LEN)];
    render(c.bytes(buf), container_orientation(head), long_edge).map(Some)
}

/// A thumbnail of `path` with a long edge of at most `long_edge`, upright.
/// `Ok(None)` when the file has no usable embedded JPEG. Only as much of the
/// file as is needed is read, and it is released before the decode.
pub fn load_thumbnail(
    path: &Path,
    long_edge: u32,
    cancel: &AtomicBool,
) -> Result<Option<RgbImage>> {
    let mut file = File::open(path).map_err(|e| ConvertError::io(path, e))?;
    let size = file.metadata().map_or(MAX_READ as u64, |m| m.len());
    let size = size.min(MAX_READ as u64) as usize;
    let mut buf = Vec::new();
    let mut found = None;
    for step in READ_STEPS {
        let target = step.min(size);
        if target > buf.len() {
            let wanted = (target - buf.len()) as u64;
            (&mut file)
                .take(wanted)
                .read_to_end(&mut buf)
                .map_err(|e| ConvertError::io(path, e))?;
        }
        check(cancel)?;
        let candidates = embedded::find_jpegs(&buf);
        found = pick(&candidates, long_edge);
        // A complete candidate of the right size ends the read early; a too
        // small one may yet be replaced by something later in the file.
        if found.is_some_and(|c| long_edge_of(&c) >= long_edge) || buf.len() >= size {
            break;
        }
    }
    let Some(chosen) = found else {
        return Ok(None);
    };
    let orientation = container_orientation(&buf[..buf.len().min(HEAD_LEN)]);
    let jpeg = chosen.bytes(&buf).to_vec();
    drop(buf);
    check(cancel)?;
    render(&jpeg, orientation, long_edge).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encode;
    use crate::testutil::{synthetic_dng, DngSpec};
    use image::Rgb;

    fn jpeg(w: u32, h: u32, color: [u8; 3]) -> Vec<u8> {
        encode::encode_rgb8(&RgbImage::from_pixel(w, h, Rgb(color)), 90, None).unwrap()
    }

    fn cancel() -> AtomicBool {
        AtomicBool::new(false)
    }

    #[test]
    fn this_module_never_names_the_raw_decoder() {
        let needle = ["raw", "ler"].concat();
        assert!(!include_str!("thumbnail.rs").contains(&needle));
    }

    #[test]
    fn takes_the_smallest_preview_that_fills_the_size() {
        let mut blob = vec![0u8; 400];
        blob.extend(jpeg(64, 48, [10, 10, 10])); // thumbnail
        blob.extend(jpeg(640, 480, [20, 200, 20]));
        blob.extend(jpeg(2000, 1500, [200, 20, 20]));
        let img = thumbnail_from_bytes(&blob, 256).unwrap().unwrap();
        assert_eq!(img.width(), 256);
        assert!(img.get_pixel(5, 5).0[1] > 150, "came from the 640 px JPEG");
    }

    #[test]
    fn a_small_camera_jpeg_is_still_used_and_never_enlarged() {
        let mut blob = vec![0u8; 400];
        blob.extend(jpeg(64, 48, [10, 200, 10]));
        let img = thumbnail_from_bytes(&blob, 256).unwrap().unwrap();
        assert_eq!(img.dimensions(), (64, 48));
    }

    #[test]
    fn a_file_without_a_camera_jpeg_gives_none() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("no_preview.DNG");
        let spec = DngSpec {
            preview: None,
            ..Default::default()
        };
        std::fs::write(&p, synthetic_dng(&spec)).unwrap();
        assert!(load_thumbnail(&p, 256, &cancel()).unwrap().is_none());
    }

    /// Reads only what it needs: a preview near the start ends the read, so
    /// a huge file does not cost a huge read.
    #[test]
    fn an_early_preview_stops_the_read() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("big.NEF");
        let mut blob = vec![0x42u8; 4096];
        blob.extend(jpeg(640, 480, [20, 200, 20]));
        // A second, larger JPEG far into the file must not be read.
        blob.resize(16 << 20, 0x11);
        blob.extend(jpeg(2000, 1500, [200, 20, 20]));
        std::fs::write(&p, &blob).unwrap();
        let img = load_thumbnail(&p, 256, &cancel()).unwrap().unwrap();
        assert!(img.get_pixel(5, 5).0[1] > 150, "came from the first JPEG");
    }

    #[test]
    fn a_portrait_container_gives_an_upright_thumbnail() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("portrait.DNG");
        // The synthetic DNG stores a 32x24 preview with no EXIF of its own.
        let spec = DngSpec {
            orientation: 6,
            ..Default::default()
        };
        std::fs::write(&p, synthetic_dng(&spec)).unwrap();
        let img = load_thumbnail(&p, 256, &cancel()).unwrap().unwrap();
        assert!(img.height() > img.width(), "{:?}", img.dimensions());
    }

    #[test]
    fn orientation_comes_from_every_tiff_flavour() {
        let tiff = |magic: &[u8; 4]| {
            let mut t = Vec::new();
            t.extend_from_slice(magic);
            t.extend_from_slice(&8u32.to_le_bytes());
            t.extend_from_slice(&1u16.to_le_bytes());
            t.extend_from_slice(&0x0112u16.to_le_bytes());
            t.extend_from_slice(&3u16.to_le_bytes());
            t.extend_from_slice(&1u32.to_le_bytes());
            t.extend_from_slice(&6u32.to_le_bytes());
            t
        };
        for magic in [b"II*\0", b"IIU\0", b"IIRO", b"IIRS"] {
            assert_eq!(container_orientation(&tiff(magic)), 6, "{magic:?}");
        }
        let mut cr3 = b"....ftypcrx ....CMT1".to_vec();
        cr3.extend(tiff(b"II*\0"));
        assert_eq!(container_orientation(&cr3), 6);
        assert_eq!(container_orientation(b"nothing useful"), 1);
    }

    #[test]
    fn damaged_files_are_not_errors_that_escape_and_do_not_allocate() {
        let dir = tempfile::tempdir().unwrap();
        let cases: Vec<(&str, Vec<u8>)> = vec![
            ("empty.CR2", Vec::new()),
            ("garbage.ARW", vec![0xFFu8; 4096]),
            ("zero_box.CR3", {
                // The zero-size box that stalls the RAW parser.
                let mut v = vec![0u8, 0, 0, 24];
                v.extend_from_slice(b"ftypcrx ");
                v.resize(24, 0);
                v.extend_from_slice(&[0, 0, 0, 16]);
                v.extend_from_slice(b"moov");
                v.extend_from_slice(&[0, 0, 0, 0]);
                v.extend_from_slice(b"trak");
                v
            }),
            ("short_jpeg.NEF", {
                let j = jpeg(800, 600, [9, 9, 9]);
                j[..j.len() / 2].to_vec()
            }),
        ];
        for (name, bytes) in cases {
            let p = dir.path().join(name);
            std::fs::write(&p, bytes).unwrap();
            assert!(
                matches!(load_thumbnail(&p, 256, &cancel()), Ok(None) | Err(_)),
                "{name}"
            );
        }
        assert!(load_thumbnail(&dir.path().join("gone.CR2"), 256, &cancel()).is_err());
    }

    /// Damaged header of a tiny stream claiming a huge picture: never decoded.
    #[test]
    fn a_header_claiming_too_many_pixels_is_ignored() {
        let mut j = jpeg(16, 16, [9, 9, 9]);
        let sof = j
            .windows(2)
            .position(|w| w == [0xFF, 0xC0])
            .expect("baseline frame");
        j[sof + 5..sof + 9].copy_from_slice(&[0xFF, 0xF0, 0xFF, 0xF0]);
        assert!(thumbnail_from_bytes(&j, 256).unwrap().is_none());
    }

    #[test]
    fn cancel_stops_the_work() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.DNG");
        std::fs::write(&p, synthetic_dng(&DngSpec::default())).unwrap();
        let cancelled = AtomicBool::new(true);
        assert!(load_thumbnail(&p, 256, &cancelled)
            .unwrap_err()
            .is_cancelled());
    }

    /// Orientation and contents against the path the main preview uses, on
    /// every real file available. Run with `--ignored --nocapture`.
    #[test]
    #[ignore]
    fn thumbnail_agrees_with_the_preview_on_real_files() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut files = Vec::new();
        for dir in ["tests/samples", "tests/regression/cache"] {
            if let Ok(found) = crate::scan::scan_folder(&root.join(dir)) {
                files.extend(found);
            }
        }
        println!("file,thumb,preview,mean_abs_diff");
        let (mut compared, mut none) = (0, 0);
        for f in &files {
            let name = f.file_name().unwrap().to_string_lossy().into_owned();
            let thumb = match load_thumbnail(f, 256, &cancel()) {
                Ok(Some(t)) => t,
                other => {
                    none += 1;
                    println!("{name},none ({:?})", other.err());
                    continue;
                }
            };
            let Ok(Some(preview)) = crate::preview::load_embedded(f, 256, &cancel()) else {
                println!("{name},{:?},preview none", thumb.dimensions());
                continue;
            };
            let portrait = |i: &RgbImage| i.height() > i.width();
            assert_eq!(portrait(&thumb), portrait(&preview), "{name}");
            // Different embedded JPEGs may be chosen, and rounding of the
            // short edge differs by a pixel, so compare on a common grid.
            let side = 32;
            let shrink = |i: &RgbImage| {
                image::imageops::resize(i, side, side, image::imageops::FilterType::Triangle)
            };
            let (a, b) = (shrink(&thumb), shrink(&preview));
            let diff: u64 = a
                .as_raw()
                .iter()
                .zip(b.as_raw())
                .map(|(x, y)| u64::from(x.abs_diff(*y)))
                .sum();
            let mean = diff as f64 / a.as_raw().len() as f64;
            println!(
                "{name},{:?},{:?},{mean:.2}",
                thumb.dimensions(),
                preview.dimensions()
            );
            assert!(mean < 45.0, "{name}: {mean}");
            compared += 1;
        }
        println!(
            "compared {compared}, no thumbnail {none}, files {}",
            files.len()
        );
    }
}
