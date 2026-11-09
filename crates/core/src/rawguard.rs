//! Checks in front of rawler for what its parsers cannot survive.
//!
//! rawler 0.8.0 walks the nested boxes of a CR3 (ISO BMFF) file with loops
//! that only end when the read position passes the end of the parent box. A
//! child box of size 0 sends the reader back to the same box, so a damaged
//! file spins forever, and in the loops that collect vendor boxes it grows a
//! `Vec` until the allocation aborts the whole process. A worker's
//! `catch_unwind` cannot contain that, so such files are refused here, before
//! the decoder sees them.
//!
//! Its TIFF reader allocates `count * element size` of an IFD entry before
//! checking the file is that long, up to 32 GiB for one damaged entry, which
//! aborts on a machine that cannot provide it. The directories rawler always
//! reads (IFD chain, SubIFDs, Exif, GPS) are checked here for such entries;
//! maker notes read later through other offsets are not.
//!
//! Both found by `crates/core/tests/fuzz_parsers.rs`.

use rawler::decoders::Decoder;
use rawler::rawsource::RawSource;
use rawler::RawlerError;

const MAX_DEPTH: u32 = 8;
/// Larger sizes make rawler's `offset + size` overflow and move backwards.
const MAX_BOX_SIZE: u64 = 1 << 62;

/// Bytes between a box header and the first child box, for the boxes rawler
/// reads as containers.
fn child_offset(typ: &[u8; 4], uuid: Option<&[u8]>) -> Option<usize> {
    const CR3_DESC_UUID: [u8; 16] = [
        0x85, 0xC0, 0xB6, 0x87, 0x82, 0x0F, 0x11, 0xE0, 0x81, 0x11, 0xF4, 0xCE, 0x46, 0x2B, 0x6A,
        0x48,
    ];
    match typ {
        b"moov" | b"trak" | b"mdia" | b"minf" | b"stbl" => Some(0),
        b"stsd" => Some(8),
        b"CRAW" => Some(82),
        b"CDI1" => Some(4),
        b"CCTP" => Some(12),
        b"uuid" if uuid == Some(&CR3_DESC_UUID[..]) => Some(0),
        _ => None,
    }
}

fn be32(buf: &[u8], pos: usize) -> Option<u32> {
    let b = buf.get(pos..pos.checked_add(4)?)?;
    Some(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

fn be64(buf: &[u8], pos: usize) -> Option<u64> {
    let b = buf.get(pos..pos.checked_add(8)?)?;
    Some(u64::from_be_bytes([
        b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
    ]))
}

/// False when a box between `start` and `end` would stall rawler. Top level
/// is lenient: most files are not BMFF at all and rawler handles them there.
fn boxes_ok(buf: &[u8], start: usize, end: usize, depth: u32) -> bool {
    let mut pos = start;
    while pos < end {
        let Some(size32) = be32(buf, pos) else {
            return true; // truncated: rawler stops on the read error
        };
        let Some(typ) = buf.get(pos + 4..pos + 8) else {
            return true;
        };
        let typ: [u8; 4] = [typ[0], typ[1], typ[2], typ[3]];
        let mut header = 8usize;
        let mut size = u64::from(size32);
        if size32 == 1 {
            let Some(large) = be64(buf, pos + 8) else {
                return true;
            };
            size = large;
            header = 16;
        }
        let uuid = if &typ == b"uuid" {
            let u = buf.get(pos + header..pos + header + 16);
            if u.is_none() {
                return true;
            }
            header += 16;
            u
        } else {
            None
        };
        if size >= MAX_BOX_SIZE {
            return false;
        }
        if size < header as u64 {
            // Inside a container this is the stall; at the top rawler has
            // its own check.
            return depth == 0;
        }
        let child_end = pos.saturating_add(size as usize).min(end);
        if depth < MAX_DEPTH {
            if let Some(skip) = child_offset(&typ, uuid) {
                if !boxes_ok(buf, pos + header + skip, child_end, depth + 1) {
                    return false;
                }
            }
        }
        pos = pos.saturating_add(size as usize);
    }
    true
}

/// True when rawler's BMFF parser can walk `buf` without stalling.
pub fn bmff_structure_ok(buf: &[u8]) -> bool {
    boxes_ok(buf, 0, buf.len(), 0)
}

/// An entry may claim up to this many bytes even if the file is smaller:
/// harmless to allocate, and damaged entries of that size are left to rawler.
const MAX_HARMLESS_ENTRY: u64 = 1 << 30;
/// rawler's own limits.
const MAX_IFD_ENTRIES: usize = 4096;
const MAX_IFDS: usize = 256;

struct Tiff<'a> {
    buf: &'a [u8],
    le: bool,
}

impl Tiff<'_> {
    fn u16(&self, pos: usize) -> Option<u16> {
        let b = self.buf.get(pos..pos.checked_add(2)?)?;
        Some(if self.le {
            u16::from_le_bytes([b[0], b[1]])
        } else {
            u16::from_be_bytes([b[0], b[1]])
        })
    }

    fn u32(&self, pos: usize) -> Option<u32> {
        let b = self.buf.get(pos..pos.checked_add(4)?)?;
        let a = [b[0], b[1], b[2], b[3]];
        Some(if self.le {
            u32::from_le_bytes(a)
        } else {
            u32::from_be_bytes(a)
        })
    }

    /// Values of an offset-holding entry (SubIFDs and friends).
    fn offsets(&self, entry: usize, typ: u16, count: usize) -> Vec<usize> {
        if count == 0 || !matches!(typ, 3 | 4 | 13) || count > 64 {
            return Vec::new();
        }
        let size = if typ == 3 { 2 } else { 4 };
        let start = if count * size <= 4 {
            entry + 8
        } else {
            match self.u32(entry + 8) {
                Some(p) => p as usize,
                None => return Vec::new(),
            }
        };
        (0..count)
            .filter_map(|i| {
                let p = start.checked_add(i * size)?;
                if size == 2 {
                    self.u16(p).map(usize::from)
                } else {
                    self.u32(p).map(|v| v as usize)
                }
            })
            .collect()
    }

    /// False when an IFD entry at `ifd` claims a size the file cannot have.
    fn ifd_ok(&self, ifd: usize, seen: &mut Vec<usize>) -> bool {
        if seen.len() >= MAX_IFDS || seen.contains(&ifd) {
            return true;
        }
        seen.push(ifd);
        let Some(n) = self.u16(ifd) else {
            return true;
        };
        let n = usize::from(n).min(MAX_IFD_ENTRIES);
        let limit = (self.buf.len() as u64).max(MAX_HARMLESS_ENTRY);
        let mut children = Vec::new();
        for i in 0..n {
            let e = ifd + 2 + i * 12;
            let (Some(tag), Some(typ), Some(count)) =
                (self.u16(e), self.u16(e + 2), self.u32(e + 4))
            else {
                return true;
            };
            let size: u64 = match typ {
                1 | 2 | 6 | 7 => 1,
                3 | 8 => 2,
                4 | 9 | 11 | 13 => 4,
                5 | 10 | 12 => 8,
                _ => continue,
            };
            if u64::from(count) * size > limit {
                return false;
            }
            if matches!(tag, 330 | 0x8769 | 0x8825) {
                children.extend(self.offsets(e, typ, count as usize));
            }
        }
        // The next IFD of the chain, then the sub-directories.
        if let Some(next) = self.u32(ifd + 2 + n * 12) {
            children.push(next as usize);
        }
        children
            .into_iter()
            .filter(|c| *c != 0)
            .all(|c| self.ifd_ok(c, seen))
    }
}

/// False when a TIFF-based file has an entry claiming more than the file can
/// hold and more than is harmless to allocate.
pub fn tiff_entries_ok(buf: &[u8]) -> bool {
    let le = match buf.get(..2) {
        Some(b"II") => true,
        Some(b"MM") => false,
        _ => return true,
    };
    let t = Tiff { buf, le };
    match t.u32(4) {
        Some(ifd) => t.ifd_ok(ifd as usize, &mut Vec::new()),
        None => true,
    }
}

/// `rawler::get_decoder` behind [`bmff_structure_ok`].
pub fn get_decoder(source: &RawSource) -> Result<Box<dyn Decoder>, RawlerError> {
    if !bmff_structure_ok(source.buf()) {
        return Err(RawlerError::DecoderFailed(
            "damaged CR3 structure: a box has an invalid size".into(),
        ));
    }
    if !tiff_entries_ok(source.buf()) {
        return Err(RawlerError::DecoderFailed(
            "damaged TIFF structure: a tag claims more data than the file holds".into(),
        ));
    }
    rawler::get_decoder(source)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn boxed(typ: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut v = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        v.extend_from_slice(typ);
        v.extend_from_slice(body);
        v
    }

    #[test]
    fn valid_nested_boxes_pass() {
        let leaf = boxed(b"mdhd", &[0; 16]);
        let mdia = boxed(b"mdia", &leaf);
        let trak = boxed(b"trak", &mdia);
        let mut file = boxed(b"ftyp", b"crx \0\0\0\0");
        file.extend(boxed(b"moov", &trak));
        assert!(bmff_structure_ok(&file));
    }

    /// The exact shape that hung and then aborted the process in rawler.
    #[test]
    fn zero_size_box_in_a_container_is_refused() {
        let mut stall = 0u32.to_be_bytes().to_vec();
        stall.extend_from_slice(b"abcd");
        for container in [b"moov", b"trak", b"mdia", b"minf", b"stbl"] {
            let mut file = boxed(b"ftyp", b"crx \0\0\0\0");
            file.extend(boxed(container, &stall));
            assert!(!bmff_structure_ok(&file), "{container:?}");
        }
        // stsd children start after version/flags and entry count.
        let mut body = vec![0u8; 8];
        body.extend_from_slice(&stall);
        let stbl = boxed(b"stbl", &boxed(b"stsd", &body));
        assert!(!bmff_structure_ok(&boxed(b"moov", &stbl)));
    }

    #[test]
    fn overflowing_large_size_is_refused() {
        let mut body = 1u32.to_be_bytes().to_vec();
        body.extend_from_slice(b"abcd");
        body.extend_from_slice(&u64::MAX.to_be_bytes());
        assert!(!bmff_structure_ok(&boxed(b"moov", &body)));
    }

    #[test]
    fn non_bmff_and_truncated_files_are_left_to_rawler() {
        assert!(bmff_structure_ok(b""));
        assert!(bmff_structure_ok(b"II*\0\x08\0\0\0"));
        assert!(bmff_structure_ok(&[0u8; 64]));
        let moov = boxed(b"moov", &[0u8; 3]);
        assert!(bmff_structure_ok(&moov));
    }

    fn tiff_with_entry(tag: u16, typ: u16, count: u32) -> Vec<u8> {
        let mut v = b"II*\0\x08\0\0\0\x01\0".to_vec();
        v.extend_from_slice(&tag.to_le_bytes());
        v.extend_from_slice(&typ.to_le_bytes());
        v.extend_from_slice(&count.to_le_bytes());
        v.extend_from_slice(&[0; 8]);
        v
    }

    /// One damaged entry used to ask rawler for up to 32 GiB.
    #[test]
    fn impossible_tiff_entry_is_refused() {
        assert!(!tiff_entries_ok(&tiff_with_entry(0x0100, 12, 0x8000_0000)));
        assert!(!tiff_entries_ok(&tiff_with_entry(0x0100, 4, u32::MAX)));
        assert!(tiff_entries_ok(&tiff_with_entry(0x0100, 4, 1)));
        // Too big for the file but cheap to allocate: rawler reports it itself.
        assert!(tiff_entries_ok(&tiff_with_entry(0x0100, 1, 1 << 24)));
        // Unknown type: rawler reads no array for it.
        assert!(tiff_entries_ok(&tiff_with_entry(0x0100, 99, u32::MAX)));
    }

    #[test]
    fn sub_ifds_are_checked_and_loops_end() {
        // IFD0 at 8 points its Exif entry at IFD at 26, which has a bad entry.
        let mut v = b"II*\0\x08\0\0\0\x01\0".to_vec();
        v.extend_from_slice(&0x8769u16.to_le_bytes());
        v.extend_from_slice(&4u16.to_le_bytes());
        v.extend_from_slice(&1u32.to_le_bytes());
        v.extend_from_slice(&26u32.to_le_bytes());
        v.extend_from_slice(&0u32.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&0x0100u16.to_le_bytes());
        v.extend_from_slice(&12u16.to_le_bytes());
        v.extend_from_slice(&u32::MAX.to_le_bytes());
        v.extend_from_slice(&[0; 8]);
        assert!(!tiff_entries_ok(&v));

        // A next-IFD pointer back to itself ends.
        let mut c = tiff_with_entry(0x0100, 4, 1);
        c[22..26].copy_from_slice(&8u32.to_le_bytes());
        assert!(tiff_entries_ok(&c));
    }

    #[test]
    fn refused_file_is_a_decode_error() {
        let mut stall = 0u32.to_be_bytes().to_vec();
        stall.extend_from_slice(b"abcd");
        let mut file = boxed(b"ftyp", b"crx \0\0\0\0");
        file.extend(boxed(b"moov", &stall));
        let source = RawSource::new_from_slice(&file);
        let e = get_decoder(&source).err().unwrap();
        assert!(e.to_string().contains("damaged CR3"), "{e}");
    }
}
