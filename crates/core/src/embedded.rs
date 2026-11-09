//! Extraction of the camera-rendered JPEG embedded in RAW files.
//!
//! Primary method: scan the whole file for JPEG streams (`FF D8 FF`), walk
//! their marker segments properly (so nested EXIF thumbnails do not end a
//! stream early), find the matching `FF D9`, and keep the stream with the
//! most pixels. Only baseline / extended / progressive Huffman frames
//! (SOF0/1/2) are accepted, which rules out the lossless-JPEG (SOF3) raw data
//! of CR2/DNG files. Every candidate is double-checked by parsing its header
//! with the `image` crate.
//!
//! Fallback (when no stream is found): rawler's `full_image` / `preview_image`,
//! which yield decoded pixels instead of bytes.

use std::io::Cursor;

use image::{DynamicImage, ImageFormat, ImageReader};
use rawler::decoders::{Decoder, RawDecodeParams};
use rawler::rawsource::RawSource;

/// Location and size of one JPEG stream inside a buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JpegCandidate {
    pub offset: usize,
    pub len: usize,
    pub width: u32,
    pub height: u32,
    /// SOF marker (0xC0, 0xC1 or 0xC2).
    pub sof: u8,
}

impl JpegCandidate {
    pub fn pixels(&self) -> u64 {
        self.width as u64 * self.height as u64
    }

    pub fn bytes<'a>(&self, buf: &'a [u8]) -> &'a [u8] {
        &buf[self.offset..self.offset + self.len]
    }
}

fn be16(buf: &[u8], pos: usize) -> Option<usize> {
    Some(((*buf.get(pos)? as usize) << 8) | *buf.get(pos + 1)? as usize)
}

/// Parse a JPEG stream that starts with SOI at `start`. Returns the candidate
/// if the stream is complete (EOI found) and uses a supported frame type.
/// Every byte walked is charged to `budget`; an empty budget ends the walk.
fn parse_stream(buf: &[u8], start: usize, budget: &mut usize) -> Option<JpegCandidate> {
    let n = buf.len();
    let mut pos = start + 2; // after SOI
    let mut frame: Option<(u8, u32, u32)> = None;
    loop {
        *budget = budget.checked_sub(4)?;
        // Expect a marker; skip fill bytes.
        if *buf.get(pos)? != 0xFF {
            return None;
        }
        while pos < n && buf[pos] == 0xFF {
            pos += 1;
            *budget = budget.checked_sub(1)?;
        }
        let marker = *buf.get(pos)?;
        pos += 1;
        match marker {
            0xD9 => {
                // EOI
                let (sof, width, height) = frame?;
                return Some(JpegCandidate {
                    offset: start,
                    len: pos - start,
                    width,
                    height,
                    sof,
                });
            }
            0xD8 => return None, // nested SOI at marker position: corrupt
            0x01 | 0xD0..=0xD7 => continue,
            _ => {}
        }
        let len = be16(buf, pos)?;
        if len < 2 || pos + len > n {
            return None;
        }
        let seg = &buf[pos + 2..pos + len];
        match marker {
            0xC0..=0xCF if marker != 0xC4 && marker != 0xC8 && marker != 0xCC => {
                if !matches!(marker, 0xC0..=0xC2) || seg.len() < 6 {
                    // Lossless / arithmetic / hierarchical: not a viewable preview.
                    return None;
                }
                let height = be16(seg, 1)? as u32;
                let width = be16(seg, 3)? as u32;
                let comps = seg[5];
                if width == 0 || height == 0 || !(comps == 1 || comps == 3) {
                    return None;
                }
                if frame.is_none() {
                    frame = Some((marker, width, height));
                }
            }
            _ => {}
        }
        pos += len;
        if marker == 0xDA {
            // Entropy-coded data follows: scan to the next real marker.
            frame?;
            loop {
                let rel = buf.get(pos..)?.iter().position(|&b| b == 0xFF)?;
                *budget = budget.checked_sub(rel + 2)?;
                pos += rel;
                let next = *buf.get(pos + 1)?;
                match next {
                    0x00 | 0xD0..=0xD7 => pos += 2,
                    0xFF => pos += 1,
                    _ => break, // a marker: handled by the outer loop
                }
            }
        }
    }
}

/// Streams kept from one file. Cameras store a handful.
const MAX_CANDIDATES: usize = 4096;

/// Work allowed for walking streams: this many passes over the file, plus a
/// floor for small files. A file made of unfinished streams would otherwise
/// walk to its end from every `FF D8 FF`, quadratic in the file size.
const SCAN_PASSES: usize = 32;
const SCAN_FLOOR: usize = 1 << 20;

/// Find every complete, supported JPEG stream in `buf`.
pub fn find_jpegs(buf: &[u8]) -> Vec<JpegCandidate> {
    let mut out = Vec::new();
    let mut budget = buf
        .len()
        .saturating_mul(SCAN_PASSES)
        .saturating_add(SCAN_FLOOR);
    let mut i = 0usize;
    while i + 3 <= buf.len() && out.len() < MAX_CANDIDATES && budget > 0 {
        let rel = match buf[i..].iter().position(|&b| b == 0xFF) {
            Some(r) => r,
            None => break,
        };
        i += rel;
        if i + 3 > buf.len() {
            break;
        }
        if buf[i + 1] == 0xD8 && buf[i + 2] == 0xFF {
            if let Some(c) = parse_stream(buf, i, &mut budget) {
                if header_ok(c.bytes(buf), c.width, c.height) {
                    out.push(c);
                }
            }
        }
        i += 1;
    }
    out
}

/// Confirm the stream header with the `image` crate's decoder.
fn header_ok(bytes: &[u8], width: u32, height: u32) -> bool {
    match ImageReader::with_format(Cursor::new(bytes), ImageFormat::Jpeg).into_dimensions() {
        Ok((w, h)) => w == width && h == height,
        Err(e) => {
            log::debug!("embedded candidate rejected by decoder: {e}");
            false
        }
    }
}

/// The largest (by pixel count, then byte size) JPEG stream in `buf`.
pub fn largest_jpeg_in(buf: &[u8]) -> Option<JpegCandidate> {
    find_jpegs(buf)
        .into_iter()
        .max_by_key(|c| (c.pixels(), c.len))
}

/// Result of [`extract`]: either the raw JPEG bytes or decoded pixels.
#[derive(Debug)]
pub enum EmbeddedPreview {
    /// JPEG bytes found by the scanner.
    Jpeg {
        data: Vec<u8>,
        width: u32,
        height: u32,
    },
    /// Decoded image from rawler's preview API (no bytes available).
    Decoded(DynamicImage),
}

/// Extract the largest embedded preview from an open RAW source.
///
/// `decoder` is optional because the scanner also works on files rawler cannot
/// decode. Returns `None` when nothing usable is found.
pub fn extract(source: &RawSource, decoder: Option<&dyn Decoder>) -> Option<EmbeddedPreview> {
    let buf = source.buf();
    if let Some(c) = largest_jpeg_in(buf) {
        return Some(EmbeddedPreview::Jpeg {
            data: c.bytes(buf).to_vec(),
            width: c.width,
            height: c.height,
        });
    }
    let decoder = decoder?;
    let params = RawDecodeParams::default();
    for (name, res) in [
        ("full_image", decoder.full_image(source, &params)),
        ("preview_image", decoder.preview_image(source, &params)),
    ] {
        match res {
            Ok(Some(img)) => {
                log::debug!(
                    "embedded: using rawler {name} ({}x{})",
                    img.width(),
                    img.height()
                );
                return Some(EmbeddedPreview::Decoded(img));
            }
            Ok(None) => {}
            Err(e) => log::debug!("embedded: rawler {name} failed: {e}"),
        }
    }
    None
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use image::{Rgb, RgbImage};

    /// Encode a small solid JPEG for fixtures.
    pub fn tiny_jpeg(w: u32, h: u32, color: [u8; 3]) -> Vec<u8> {
        let img = RgbImage::from_pixel(w, h, Rgb(color));
        crate::encode::encode_rgb8(&img, 90, None).unwrap()
    }

    /// A JPEG with an EXIF APP1 that itself contains a JPEG thumbnail
    /// (nested SOI/EOI), to make sure the walker does not stop early.
    fn jpeg_with_nested_thumb(w: u32, h: u32) -> Vec<u8> {
        let thumb = tiny_jpeg(8, 8, [1, 2, 3]);
        let mut tiff = b"II*\0\x08\0\0\0\0\0\0\0\0\0".to_vec();
        tiff.extend_from_slice(&thumb);
        let img = RgbImage::from_pixel(w, h, Rgb([10, 200, 30]));
        crate::encode::encode_rgb8(&img, 90, Some(&tiff)).unwrap()
    }

    #[test]
    fn finds_largest_of_two_embedded_jpegs() {
        let small = tiny_jpeg(16, 12, [255, 0, 0]);
        let large = tiny_jpeg(64, 48, [0, 0, 255]);
        let mut blob = vec![0u8; 1000];
        blob.extend_from_slice(&[0xFF, 0xD8, 0xFF, 0x00, 0x12]); // false positive
        blob.extend_from_slice(&small);
        blob.extend_from_slice(&[0xFF; 37]);
        let large_off = blob.len();
        blob.extend_from_slice(&large);
        blob.extend_from_slice(&[0xAB; 500]);

        let all = find_jpegs(&blob);
        assert_eq!(all.len(), 2, "{all:?}");
        let best = largest_jpeg_in(&blob).unwrap();
        assert_eq!((best.width, best.height), (64, 48));
        assert_eq!(best.offset, large_off);
        assert_eq!(best.bytes(&blob), &large[..]);
        // The extracted bytes decode.
        let img = image::load_from_memory(best.bytes(&blob)).unwrap();
        assert_eq!(img.width(), 64);
    }

    #[test]
    fn nested_exif_thumbnail_does_not_truncate_stream() {
        let j = jpeg_with_nested_thumb(40, 30);
        let mut blob = b"RAWHEADER".to_vec();
        blob.extend_from_slice(&j);
        let best = largest_jpeg_in(&blob).unwrap();
        assert_eq!((best.width, best.height), (40, 30));
        assert_eq!(best.len, j.len());
    }

    /// Unfinished streams one after another used to cost a walk to the end of
    /// the file from each of them.
    #[test]
    fn unfinished_streams_cannot_make_the_scan_quadratic() {
        let sof = [
            0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x0B, 0x08, 0x00, 0x10, 0x00, 0x10, 0x01, 0x01, 0x11,
            0x00, 0xFF, 0xDA, 0x00, 0x02,
        ];
        let blob: Vec<u8> = sof.iter().copied().cycle().take(32 << 20).collect();
        let start = std::time::Instant::now();
        assert!(find_jpegs(&blob).is_empty());
        assert!(
            start.elapsed().as_secs() < 30,
            "scan took {:?}",
            start.elapsed()
        );
    }

    #[test]
    fn scan_budget_does_not_hide_real_previews() {
        // Many false starts first, a real JPEG after them: still found.
        let mut blob = vec![0x11u8; 4096];
        for _ in 0..2000 {
            blob.extend_from_slice(&[0xFF, 0xD8, 0xFF, 0x00, 0x12]);
        }
        blob.extend_from_slice(&tiny_jpeg(64, 48, [9, 9, 200]));
        let best = largest_jpeg_in(&blob).unwrap();
        assert_eq!((best.width, best.height), (64, 48));
    }

    #[test]
    fn truncated_stream_is_ignored() {
        let j = tiny_jpeg(32, 32, [9, 9, 9]);
        let blob = j[..j.len() / 2].to_vec();
        assert!(largest_jpeg_in(&blob).is_none());
    }

    #[test]
    fn lossless_sof3_is_rejected() {
        // SOI, SOF3 (len 11), EOI
        let blob = [
            0xFF, 0xD8, 0xFF, 0xC3, 0x00, 0x0B, 0x0E, 0x00, 0x10, 0x00, 0x10, 0x01, 0x01, 0x11,
            0x00, 0xFF, 0xD9,
        ];
        assert!(find_jpegs(&blob).is_empty());
    }
}
