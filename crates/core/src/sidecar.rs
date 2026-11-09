//! Star ratings in Adobe-compatible XMP sidecars: `<stem>.xmp` beside the RAW,
//! with the stars in `xmp:Rating`. This is what Bridge and Lightroom read.
//!
//! A sidecar that already exists may hold develop settings or anything else,
//! so only the rating is ever changed: the file is parsed just enough to find
//! that property and the new text is the old text with those bytes replaced,
//! nothing re-serialised. A file that cannot be parsed safely is left alone
//! and reported. RAW files are never opened for writing here.
//!
//! The RAW's own rating is looked up in a bounded read of the start of the
//! file, without the RAW decoder.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::exif;

const XAP_NS: &str = "http://ns.adobe.com/xap/1.0/";
const RDF_NS: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
/// Lightroom sidecars with long histories reach a few MB.
const MAX_SIDECAR_BYTES: u64 = 32 << 20;
const MAX_DEPTH: usize = 1024;
/// How much of a RAW is searched for an `xmp:Rating` packet.
pub const RAW_HEAD_BYTES: u64 = 1 << 20;

/// Why a rating could not be saved or read.
#[derive(Debug, thiserror::Error)]
pub enum SidecarError {
    #[error("{0} is not an XMP file that can be changed safely")]
    Unparsable(PathBuf),
    #[error("{0} is too large to be a sidecar")]
    TooLarge(PathBuf),
    #[error("{0} is read-only")]
    ReadOnly(PathBuf),
    #[error("{0}: {1}")]
    Io(PathBuf, io::Error),
}

/// `<stem>.xmp` beside `raw`, which may not exist. Two RAW files with the same
/// stem (`A.CR2`, `A.NEF`) share one sidecar, as in Adobe's convention.
pub fn sidecar_path(raw: &Path) -> PathBuf {
    raw.with_extension("xmp")
}

/// The sidecar of `raw` if one exists, in either case of the extension.
pub fn existing_sidecar(raw: &Path) -> Option<PathBuf> {
    ["xmp", "XMP", "Xmp"]
        .into_iter()
        .map(|ext| raw.with_extension(ext))
        .find(|p| p.is_file())
}

/// Sidecars of a list of files, found with one listing per folder so that a
/// 10,000-file list does not ask the file system about each file.
#[derive(Debug, Default)]
pub struct SidecarIndex {
    /// Per folder: lower-case stem to actual file name.
    folders: HashMap<PathBuf, HashMap<String, std::ffi::OsString>>,
}

impl SidecarIndex {
    pub fn of_files<'a>(files: impl IntoIterator<Item = &'a Path>) -> Self {
        let mut folders: HashMap<PathBuf, HashMap<String, std::ffi::OsString>> = HashMap::new();
        for file in files {
            let Some(parent) = file.parent() else {
                continue;
            };
            if folders.contains_key(parent) {
                continue;
            }
            let mut names = HashMap::new();
            if let Ok(entries) = fs::read_dir(parent) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    let is_xmp = path
                        .extension()
                        .is_some_and(|e| e.eq_ignore_ascii_case("xmp"));
                    if is_xmp {
                        if let Some(stem) = path.file_stem() {
                            names.insert(stem.to_string_lossy().to_lowercase(), entry.file_name());
                        }
                    }
                }
            }
            folders.insert(parent.to_path_buf(), names);
        }
        Self { folders }
    }

    /// The sidecar of `raw`, if the listing saw one.
    pub fn sidecar_for(&self, raw: &Path) -> Option<PathBuf> {
        let parent = raw.parent()?;
        let stem = raw.file_stem()?.to_string_lossy().to_lowercase();
        let name = self.folders.get(parent)?.get(&stem)?;
        Some(parent.join(name))
    }
}

// ----------------------------------------------------------------------------
// Parsing
// ----------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
struct Span {
    start: usize,
    end: usize,
}

/// One occurrence of `xmp:Rating`.
#[derive(Debug, Clone, Copy)]
enum Prop {
    Attr {
        /// The value between the quotes.
        value: Span,
        /// The whole attribute with the white space before it.
        whole: Span,
    },
    Element {
        /// The text between the tags.
        value: Span,
        /// From the start tag to the end of the end tag.
        whole: Span,
    },
}

impl Prop {
    fn value(self) -> Span {
        match self {
            Prop::Attr { value, .. } | Prop::Element { value, .. } => value,
        }
    }
    fn whole(self) -> Span {
        match self {
            Prop::Attr { whole, .. } | Prop::Element { whole, .. } => whole,
        }
    }
}

/// The first `rdf:Description`, where a new rating is added.
#[derive(Debug, Clone)]
struct FirstDescription {
    /// Just after the last attribute of its start tag.
    attr_end: usize,
    /// Prefix bound to the XMP namespace there, if any.
    xap_prefix: Option<String>,
    /// `xmp` is bound to something else there.
    xmp_prefix_taken: bool,
}

#[derive(Debug)]
struct Doc {
    props: Vec<Prop>,
    first: Option<FirstDescription>,
    /// Position of `</rdf:RDF>` and the prefix of that element.
    rdf_close: Option<(usize, String)>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind {
    Other,
    Rdf,
    Description,
    Rating { start: usize, text: usize },
}

struct Frame {
    name: Span,
    ns: Vec<(String, String)>,
    kind: Kind,
}

fn lookup<'a>(frames: &'a [Frame], own: &'a [(String, String)], prefix: &str) -> Option<&'a str> {
    own.iter()
        .rev()
        .chain(frames.iter().rev().flat_map(|f| f.ns.iter().rev()))
        .find(|(p, _)| p == prefix)
        .map(|(_, uri)| uri.as_str())
}

fn split_name(name: &str) -> Option<(&str, &str)> {
    match name.split_once(':') {
        Some((prefix, local)) if !local.contains(':') => Some((prefix, local)),
        Some(_) => None,
        None => Some(("", name)),
    }
}

fn is_ws(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\r' | b'\n')
}

/// The numeric value of a rating property, when it is one.
fn rating_value(text: &str) -> Option<i32> {
    let v: f64 = text.trim().parse().ok()?;
    (v.is_finite() && (-1.0..=5.0).contains(&v)).then(|| v.round() as i32)
}

impl Doc {
    /// Walk the document. `None` for anything not clearly well-formed XMP.
    fn parse(text: &str) -> Option<Doc> {
        let b = text.as_bytes();
        let mut doc = Doc {
            props: Vec::new(),
            first: None,
            rdf_close: None,
        };
        let mut frames: Vec<Frame> = Vec::new();
        let mut root_seen = false;
        let mut rdf_seen = false;
        let mut pos = 0usize;
        while pos < b.len() {
            if b[pos] != b'<' {
                let next = text.get(pos..)?.find('<').map_or(b.len(), |r| pos + r);
                // Only white space (or padding) is allowed outside the root.
                if frames.is_empty() {
                    let junk = text.get(pos..next)?;
                    if !junk.chars().all(|c| c.is_whitespace() || c == '\u{feff}') {
                        return None;
                    }
                }
                pos = next;
                continue;
            }
            let rest = text.get(pos..)?;
            if rest.starts_with("<!--") {
                if matches!(frames.last(), Some(f) if matches!(f.kind, Kind::Rating { .. })) {
                    return None;
                }
                pos += 4 + rest.get(4..)?.find("-->")? + 3;
            } else if rest.starts_with("<![CDATA[") {
                if matches!(frames.last(), Some(f) if matches!(f.kind, Kind::Rating { .. })) {
                    return None;
                }
                pos += rest.find("]]>")? + 3;
            } else if rest.starts_with("<?") {
                pos += rest.find("?>")? + 2;
            } else if rest.starts_with("<!") {
                return None; // DOCTYPE and friends have no place in XMP
            } else if rest.starts_with("</") {
                let end = pos + rest.find('>')?;
                let name = text.get(pos + 2..end)?.trim_end();
                let frame = frames.pop()?;
                if name != text.get(frame.name.start..frame.name.end)? {
                    return None;
                }
                match frame.kind {
                    Kind::Rating { start, text: t } => doc.props.push(Prop::Element {
                        value: Span { start: t, end: pos },
                        whole: Span {
                            start,
                            end: end + 1,
                        },
                    }),
                    Kind::Rdf if doc.rdf_close.is_none() => {
                        let rdf_name = text.get(frame.name.start..frame.name.end)?;
                        let prefix = split_name(rdf_name)?.0.to_owned();
                        doc.rdf_close = Some((pos, prefix));
                    }
                    _ => {}
                }
                pos = end + 1;
            } else {
                pos = Self::start_tag(text, pos, &mut frames, &mut doc, &mut rdf_seen)?;
                if frames.is_empty() && root_seen {
                    return None; // a second root element
                }
                root_seen = true;
            }
        }
        (frames.is_empty() && root_seen && rdf_seen).then_some(doc)
    }

    /// One start tag at `open`; returns the position after it.
    fn start_tag(
        text: &str,
        open: usize,
        frames: &mut Vec<Frame>,
        doc: &mut Doc,
        rdf_seen: &mut bool,
    ) -> Option<usize> {
        let b = text.as_bytes();
        if frames.len() >= MAX_DEPTH {
            return None;
        }
        if matches!(frames.last(), Some(f) if matches!(f.kind, Kind::Rating { .. })) {
            return None; // a structure where a number belongs
        }
        let mut pos = open + 1;
        let name_start = pos;
        while pos < b.len() && !is_ws(b[pos]) && b[pos] != b'/' && b[pos] != b'>' {
            pos += 1;
        }
        let name = Span {
            start: name_start,
            end: pos,
        };
        if name.start == name.end || b.get(pos).is_none() {
            return None;
        }

        struct RawAttr {
            name: Span,
            value: Span,
            whole: Span,
        }
        let mut attrs: Vec<RawAttr> = Vec::new();
        let mut attr_end = pos;
        let self_closed;
        loop {
            let ws_start = pos;
            while b.get(pos).is_some_and(|c| is_ws(*c)) {
                pos += 1;
            }
            match *b.get(pos)? {
                b'>' => {
                    self_closed = false;
                    pos += 1;
                    break;
                }
                b'/' => {
                    if *b.get(pos + 1)? != b'>' {
                        return None;
                    }
                    self_closed = true;
                    pos += 2;
                    break;
                }
                _ => {}
            }
            if pos == ws_start {
                return None; // attributes must be separated
            }
            let attr_start = pos;
            while b.get(pos).is_some_and(|c| !is_ws(*c) && *c != b'=') {
                if matches!(b[pos], b'<' | b'>' | b'"' | b'\'' | b'/') {
                    return None;
                }
                pos += 1;
            }
            let attr_name = Span {
                start: attr_start,
                end: pos,
            };
            if attr_name.start == attr_name.end {
                return None;
            }
            while b.get(pos).is_some_and(|c| is_ws(*c)) {
                pos += 1;
            }
            if *b.get(pos)? != b'=' {
                return None;
            }
            pos += 1;
            while b.get(pos).is_some_and(|c| is_ws(*c)) {
                pos += 1;
            }
            let quote = *b.get(pos)?;
            if quote != b'"' && quote != b'\'' {
                return None;
            }
            pos += 1;
            let value_start = pos;
            while *b.get(pos)? != quote {
                if b[pos] == b'<' {
                    return None;
                }
                pos += 1;
            }
            let value = Span {
                start: value_start,
                end: pos,
            };
            pos += 1;
            attr_end = pos;
            let this = text.get(attr_name.start..attr_name.end)?;
            for other in &attrs {
                if text.get(other.name.start..other.name.end)? == this {
                    return None; // a duplicate attribute
                }
            }
            attrs.push(RawAttr {
                name: attr_name,
                value,
                whole: Span {
                    start: ws_start,
                    end: pos,
                },
            });
        }

        // Namespace declarations of this element apply to itself.
        let mut own: Vec<(String, String)> = Vec::new();
        for a in &attrs {
            let attr = text.get(a.name.start..a.name.end)?;
            let value = text.get(a.value.start..a.value.end)?;
            if attr == "xmlns" {
                own.push((String::new(), value.to_owned()));
            } else if let Some(prefix) = attr.strip_prefix("xmlns:") {
                own.push((prefix.to_owned(), value.to_owned()));
            }
        }
        let element = text.get(name.start..name.end)?;
        let (prefix, local) = split_name(element)?;
        let uri = lookup(frames, &own, prefix);
        let parent = frames.last().map_or(Kind::Other, |f| f.kind);

        let mut kind = Kind::Other;
        if uri == Some(RDF_NS) && local == "RDF" {
            if self_closed {
                return None; // nowhere to add a rating
            }
            kind = Kind::Rdf;
            *rdf_seen = true;
        } else if uri == Some(RDF_NS) && local == "Description" && parent == Kind::Rdf {
            kind = Kind::Description;
            let is_first = doc.first.is_none();
            for a in &attrs {
                let attr = text.get(a.name.start..a.name.end)?;
                if attr == "xmlns" || attr.starts_with("xmlns:") {
                    continue;
                }
                let (p, l) = split_name(attr)?;
                if p.is_empty() || l != "Rating" {
                    continue;
                }
                if lookup(frames, &own, p) == Some(XAP_NS) {
                    doc.props.push(Prop::Attr {
                        value: a.value,
                        whole: a.whole,
                    });
                }
            }
            if is_first {
                let xap_prefix = own
                    .iter()
                    .rev()
                    .chain(frames.iter().rev().flat_map(|f| f.ns.iter().rev()))
                    .find(|(p, u)| {
                        u == XAP_NS && !p.is_empty() && lookup(frames, &own, p) == Some(XAP_NS)
                    })
                    .map(|(p, _)| p.clone());
                doc.first = Some(FirstDescription {
                    attr_end,
                    xap_prefix,
                    xmp_prefix_taken: lookup(frames, &own, "xmp").is_some_and(|u| u != XAP_NS),
                });
            }
        } else if uri == Some(XAP_NS) && local == "Rating" && parent == Kind::Description {
            if self_closed {
                return None;
            }
            kind = Kind::Rating {
                start: open,
                text: pos,
            };
        }

        if !self_closed {
            frames.push(Frame {
                name,
                ns: own,
                kind,
            });
        }
        Some(pos)
    }

    /// The stored rating: the first occurrence that is a number.
    fn stored(&self, text: &str) -> Option<i32> {
        self.props.iter().find_map(|p| {
            let v = p.value();
            rating_value(text.get(v.start..v.end)?)
        })
    }
}

/// The rating stored in XMP text. `Ok(None)` when it has no rating property;
/// `Err` when the text is not something that can be parsed safely.
pub fn parse_rating(xml: &[u8]) -> Result<Option<i32>, NotXmp> {
    let text = std::str::from_utf8(xml).map_err(|_| NotXmp)?;
    let doc = Doc::parse(text).ok_or(NotXmp)?;
    Ok(doc.stored(text))
}

/// The text is not XMP that this module can read safely.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotXmp;

/// `xml` with its rating set to `rating`: a number from 0 to 5, or removed
/// when `None`. Every other byte stays. `None` when the text is not safe to
/// change.
pub fn with_rating(xml: &[u8], rating: Option<u8>) -> Option<Vec<u8>> {
    let text = std::str::from_utf8(xml).ok()?;
    let doc = Doc::parse(text)?;
    let mut edits: Vec<(Span, String)> = Vec::new();
    match rating {
        Some(stars) => {
            let digit = stars.to_string();
            if doc.props.is_empty() {
                match &doc.first {
                    Some(first) => {
                        let new_attr = match &first.xap_prefix {
                            Some(prefix) => format!(" {prefix}:Rating=\"{digit}\""),
                            None if first.xmp_prefix_taken => return None,
                            None => {
                                format!(" xmlns:xmp=\"{XAP_NS}\" xmp:Rating=\"{digit}\"")
                            }
                        };
                        let at = first.attr_end;
                        edits.push((Span { start: at, end: at }, new_attr));
                    }
                    None => {
                        let (at, prefix) = doc.rdf_close.as_ref()?;
                        let p = if prefix.is_empty() {
                            String::new()
                        } else {
                            format!("{prefix}:")
                        };
                        let element = format!(
                            "<{p}Description {p}about=\"\" xmlns:xmp=\"{XAP_NS}\" \
                             xmp:Rating=\"{digit}\"/>"
                        );
                        edits.push((
                            Span {
                                start: *at,
                                end: *at,
                            },
                            element,
                        ));
                    }
                }
            } else {
                for prop in &doc.props {
                    edits.push((prop.value(), digit.clone()));
                }
            }
        }
        None => {
            for prop in &doc.props {
                edits.push((prop.whole(), String::new()));
            }
        }
    }
    edits.sort_by_key(|(span, _)| std::cmp::Reverse(span.start));
    let mut out = text.to_owned();
    for (span, replacement) in edits {
        out.replace_range(span.start..span.end, &replacement);
    }
    // What was written must read back as what was asked.
    let check = Doc::parse(&out)?;
    let wanted = rating.map(i32::from);
    (check.stored(&out) == wanted).then(|| out.into_bytes())
}

/// A new sidecar holding only the rating.
fn new_sidecar(stars: u8) -> String {
    format!(
        "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\" x:xmptk=\"CcooNverter\">\n \
         <rdf:RDF xmlns:rdf=\"{RDF_NS}\">\n  \
         <rdf:Description rdf:about=\"\"\n    \
         xmlns:xmp=\"{XAP_NS}\"\n   \
         xmp:Rating=\"{stars}\"/>\n \
         </rdf:RDF>\n</x:xmpmeta>\n"
    )
}

// ----------------------------------------------------------------------------
// Files
// ----------------------------------------------------------------------------

fn read_limited(path: &Path, limit: u64) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    File::open(path)?.take(limit).read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn read_sidecar(path: &Path) -> Result<Vec<u8>, SidecarError> {
    let size = fs::metadata(path)
        .map_err(|e| SidecarError::Io(path.to_owned(), e))?
        .len();
    if size > MAX_SIDECAR_BYTES {
        return Err(SidecarError::TooLarge(path.to_owned()));
    }
    read_limited(path, MAX_SIDECAR_BYTES).map_err(|e| SidecarError::Io(path.to_owned(), e))
}

/// The rating in an `xmp:Rating` packet at the start of the RAW (0 if none).
pub fn embedded_rating(raw: &Path) -> u8 {
    read_limited(raw, RAW_HEAD_BYTES)
        .ok()
        .and_then(|head| exif::find_xmp_rating(&head))
        .map_or(0, |r| r as u8)
}

/// What is known about the stars of one file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoadedRating {
    /// 0 to 5.
    pub stars: u8,
    /// A sidecar exists but could not be parsed; it will not be changed.
    pub sidecar_unsafe: bool,
}

/// Stars for `raw`: the sidecar first, then the RAW's own packet.
/// `sidecar` is the path found by [`SidecarIndex`], if any.
pub fn load_rating(raw: &Path, sidecar: Option<&Path>) -> LoadedRating {
    let mut sidecar_unsafe = false;
    if let Some(path) = sidecar {
        match read_sidecar(path).map(|b| parse_rating(&b)) {
            Ok(Ok(Some(value))) => {
                return LoadedRating {
                    stars: value.clamp(0, 5) as u8,
                    sidecar_unsafe: false,
                }
            }
            Ok(Ok(None)) => {}
            Ok(Err(NotXmp)) => {
                log::warn!("{}: not valid XMP, leaving it alone", path.display());
                sidecar_unsafe = true;
            }
            Err(e) => {
                log::warn!("Could not read {}: {e}", path.display());
                sidecar_unsafe = true;
            }
        }
    }
    LoadedRating {
        stars: embedded_rating(raw),
        sidecar_unsafe,
    }
}

/// The value a sidecar stores explicitly for `raw`, for the output file.
/// `None` when there is no sidecar, no rating in it, or it cannot be read.
pub fn stored_rating(raw: &Path) -> Option<u8> {
    let path = existing_sidecar(raw)?;
    let bytes = read_sidecar(&path).ok()?;
    match parse_rating(&bytes) {
        Ok(Some(value)) => Some(value.clamp(0, 5) as u8),
        _ => None,
    }
}

fn temp_path(target: &Path) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut name = std::ffi::OsString::from(".");
    name.push(target.file_name().unwrap_or_default());
    name.push(format!(".{}.{n}.tmp", std::process::id()));
    target.with_file_name(name)
}

/// Write `bytes` to a temporary file in the target's folder, then rename it.
fn write_atomic(target: &Path, bytes: &[u8]) -> Result<(), SidecarError> {
    let temp = temp_path(target);
    let io_err = |e| SidecarError::Io(target.to_owned(), e);
    let result = (|| -> io::Result<()> {
        let mut f = File::create(&temp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        fs::rename(&temp, target)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result.map_err(io_err)
}

/// Set the stars of `raw` (0 clears them). Only the sidecar is written, and
/// only when that changes something.
pub fn save_stars(raw: &Path, stars: u8) -> Result<(), SidecarError> {
    let stars = stars.min(5);
    let Some(path) = existing_sidecar(raw) else {
        if stars == 0 && embedded_rating(raw) == 0 {
            return Ok(()); // nothing to record: no sidecar is created
        }
        return write_atomic(&sidecar_path(raw), new_sidecar(stars).as_bytes());
    };
    let meta = fs::metadata(&path).map_err(|e| SidecarError::Io(path.clone(), e))?;
    if meta.permissions().readonly() {
        return Err(SidecarError::ReadOnly(path));
    }
    let bytes = read_sidecar(&path)?;
    // Clearing must not bring back the rating stored inside the RAW.
    let wanted = if stars > 0 || embedded_rating(raw) > 0 {
        Some(stars)
    } else {
        None
    };
    let updated =
        with_rating(&bytes, wanted).ok_or_else(|| SidecarError::Unparsable(path.clone()))?;
    if updated != bytes {
        write_atomic(&path, &updated)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A realistic Lightroom sidecar: develop settings, nested structures and
    /// other namespaces, including a colour label that is not ours.
    const LIGHTROOM: &str = "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n\
<x:xmpmeta xmlns:x=\"adobe:ns:meta/\" x:xmptk=\"Adobe XMP Core 7.0\">\n\
 <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n\
  <rdf:Description rdf:about=\"\"\n\
    xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\"\n\
    xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\"\n\
    xmlns:dc=\"http://purl.org/dc/elements/1.1/\"\n\
    xmlns:photoshop=\"http://ns.adobe.com/photoshop/1.0/\"\n\
   xmp:CreatorTool=\"Adobe Lightroom Classic 13.0\"\n\
   xmp:Label=\"Red\"\n\
   xmp:Rating=\"3\"\n\
   photoshop:SidecarForExtension=\"CR2\"\n\
   crs:Version=\"16.0\"\n\
   crs:WhiteBalance=\"Custom\"\n\
   crs:Exposure2012=\"+0.35\">\n\
   <crs:ToneCurvePV2012>\n\
    <rdf:Seq>\n\
     <rdf:li>0, 0</rdf:li>\n\
     <rdf:li>255, 255</rdf:li>\n\
    </rdf:Seq>\n\
   </crs:ToneCurvePV2012>\n\
   <dc:subject>\n\
    <rdf:Bag>\n\
     <rdf:li>제주 &amp; 바다</rdf:li>\n\
    </rdf:Bag>\n\
   </dc:subject>\n\
  </rdf:Description>\n\
 </rdf:RDF>\n\
</x:xmpmeta>\n\
<?xpacket end=\"w\"?>";

    fn rated(xml: &str) -> Option<i32> {
        parse_rating(xml.as_bytes()).unwrap()
    }

    #[test]
    fn reads_attribute_and_element_forms() {
        assert_eq!(rated(LIGHTROOM), Some(3));
        let element = "<x:xmpmeta xmlns:x='adobe:ns:meta/'><rdf:RDF \
            xmlns:rdf='http://www.w3.org/1999/02/22-rdf-syntax-ns#'><rdf:Description \
            xmlns:xmp='http://ns.adobe.com/xap/1.0/'><xmp:Rating>4</xmp:Rating>\
            </rdf:Description></rdf:RDF></x:xmpmeta>";
        assert_eq!(rated(element), Some(4));
    }

    #[test]
    fn another_prefix_for_the_namespace_is_the_same_property() {
        let xml = "<rdf:RDF xmlns:rdf='http://www.w3.org/1999/02/22-rdf-syntax-ns#'>\
            <rdf:Description xmlns:a='http://ns.adobe.com/xap/1.0/' a:Rating='2'/></rdf:RDF>";
        assert_eq!(rated(xml), Some(2));
        // Same local name in another namespace is someone else's property.
        let other = "<rdf:RDF xmlns:rdf='http://www.w3.org/1999/02/22-rdf-syntax-ns#'>\
            <rdf:Description xmlns:a='urn:other' a:Rating='2'/></rdf:RDF>";
        assert_eq!(rated(other), None);
    }

    #[test]
    fn changing_the_rating_leaves_every_other_byte_alone() {
        let out = with_rating(LIGHTROOM.as_bytes(), Some(5)).unwrap();
        let out = String::from_utf8(out).unwrap();
        assert_eq!(
            out,
            LIGHTROOM.replace("xmp:Rating=\"3\"", "xmp:Rating=\"5\"")
        );
    }

    #[test]
    fn removing_the_rating_removes_only_that_attribute() {
        let out = with_rating(LIGHTROOM.as_bytes(), None).unwrap();
        let out = String::from_utf8(out).unwrap();
        assert_eq!(out, LIGHTROOM.replace("\nxmp:Rating=\"3\"", ""));
        for kept in [
            "crs:Exposure2012=\"+0.35\"",
            "xmp:Label=\"Red\"",
            "제주 &amp; 바다",
        ] {
            assert!(out.contains(kept), "{kept}");
        }
        assert_eq!(rated(&out), None);
    }

    #[test]
    fn adding_a_rating_touches_only_the_description_tag() {
        let without = LIGHTROOM.replace("\nxmp:Rating=\"3\"", "");
        let out = with_rating(without.as_bytes(), Some(2)).unwrap();
        let out = String::from_utf8(out).unwrap();
        assert_eq!(
            out,
            without.replace(
                "crs:Exposure2012=\"+0.35\">",
                "crs:Exposure2012=\"+0.35\" xmp:Rating=\"2\">"
            )
        );
    }

    #[test]
    fn a_new_namespace_declaration_is_added_when_missing() {
        let xml = "<rdf:RDF xmlns:rdf='http://www.w3.org/1999/02/22-rdf-syntax-ns#'>\
            <rdf:Description rdf:about=''/></rdf:RDF>";
        let out = with_rating(xml.as_bytes(), Some(4)).unwrap();
        assert_eq!(parse_rating(&out).unwrap(), Some(4));
        assert!(String::from_utf8(out).unwrap().contains("xmlns:xmp="));
    }

    #[test]
    fn the_xmp_prefix_bound_elsewhere_is_refused() {
        let xml = "<rdf:RDF xmlns:rdf='http://www.w3.org/1999/02/22-rdf-syntax-ns#' \
            xmlns:xmp='urn:not-xmp'><rdf:Description rdf:about=''/></rdf:RDF>";
        assert!(with_rating(xml.as_bytes(), Some(1)).is_none());
    }

    #[test]
    fn a_rating_element_is_replaced_in_place() {
        let xml = "<rdf:RDF xmlns:rdf='http://www.w3.org/1999/02/22-rdf-syntax-ns#'>\
            <rdf:Description xmlns:xmp='http://ns.adobe.com/xap/1.0/'>\
            <xmp:Rating>1</xmp:Rating></rdf:Description></rdf:RDF>";
        let out = with_rating(xml.as_bytes(), Some(3)).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            xml.replace("<xmp:Rating>1<", "<xmp:Rating>3<")
        );
    }

    #[test]
    fn a_description_with_no_rating_yet_may_be_empty_rdf() {
        let xml = "<rdf:RDF xmlns:rdf='http://www.w3.org/1999/02/22-rdf-syntax-ns#'></rdf:RDF>";
        let out = with_rating(xml.as_bytes(), Some(1)).unwrap();
        assert_eq!(parse_rating(&out).unwrap(), Some(1));
    }

    #[test]
    fn broken_or_odd_sidecars_are_refused() {
        for bad in [
            "not xml at all",
            "<x:a><b></x:a>",
            "<a><b></b>",
            "<rdf:RDF xmlns:rdf='http://www.w3.org/1999/02/22-rdf-syntax-ns#'>",
            "<!DOCTYPE foo><rdf:RDF xmlns:rdf='http://www.w3.org/1999/02/22-rdf-syntax-ns#'></rdf:RDF>",
            "<a x='1' x='2'/>",
            "<a b=c/>",
            "<root/>", // well-formed but not XMP
            "<rdf:RDF xmlns:rdf='http://www.w3.org/1999/02/22-rdf-syntax-ns#'/>",
        ] {
            assert!(parse_rating(bad.as_bytes()).is_err(), "{bad}");
            assert!(with_rating(bad.as_bytes(), Some(2)).is_none(), "{bad}");
        }
        assert!(parse_rating(&[0xFF, 0xFE, 0x3C, 0x00]).is_err(), "UTF-16");
    }

    fn raw(dir: &Path, name: &str, body: &[u8]) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn creates_a_sidecar_only_when_a_rating_is_set() {
        let dir = tempfile::tempdir().unwrap();
        let file = raw(dir.path(), "a.CR2", b"RAWDATA");
        save_stars(&file, 0).unwrap();
        assert!(!sidecar_path(&file).exists(), "nothing to record");
        save_stars(&file, 4).unwrap();
        assert_eq!(
            load_rating(&file, existing_sidecar(&file).as_deref()).stars,
            4
        );
        assert_eq!(
            fs::read(dir.path().join("a.CR2")).unwrap(),
            b"RAWDATA",
            "the RAW is never touched"
        );
        let names: Vec<_> = fs::read_dir(dir.path()).unwrap().flatten().collect();
        assert_eq!(names.len(), 2, "no temporary files are left behind");
    }

    #[test]
    fn clearing_keeps_the_sidecar_and_every_other_mark() {
        let dir = tempfile::tempdir().unwrap();
        let file = raw(dir.path(), "a.NEF", b"RAW");
        let side = sidecar_path(&file);
        fs::write(&side, LIGHTROOM).unwrap();
        save_stars(&file, 0).unwrap();
        let text = fs::read_to_string(&side).unwrap();
        assert!(text.contains("crs:Exposure2012=\"+0.35\""));
        assert!(text.contains("xmp:Label=\"Red\""));
        assert_eq!(rated(&text), None);
        assert!(side.exists());
        // A sidecar this app made may end up empty of marks: still kept.
        let other = raw(dir.path(), "b.NEF", b"RAW");
        save_stars(&other, 3).unwrap();
        save_stars(&other, 0).unwrap();
        assert!(sidecar_path(&other).exists());
        assert_eq!(
            load_rating(&other, existing_sidecar(&other).as_deref()).stars,
            0
        );
    }

    #[test]
    fn an_unparsable_sidecar_is_refused_and_left_as_it_was() {
        let dir = tempfile::tempdir().unwrap();
        let file = raw(dir.path(), "a.ARW", b"RAW");
        let side = sidecar_path(&file);
        fs::write(&side, "<not really xmp").unwrap();
        assert!(matches!(
            save_stars(&file, 3),
            Err(SidecarError::Unparsable(_))
        ));
        assert_eq!(fs::read_to_string(&side).unwrap(), "<not really xmp");
        let loaded = load_rating(&file, existing_sidecar(&file).as_deref());
        assert!(loaded.sidecar_unsafe);
    }

    #[test]
    fn a_failed_write_leaves_the_old_file_and_no_temporary() {
        let dir = tempfile::tempdir().unwrap();
        let file = raw(dir.path(), "a.DNG", b"RAW");
        let side = sidecar_path(&file);
        fs::write(&side, LIGHTROOM).unwrap();
        let mut permissions = fs::metadata(&side).unwrap().permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&side, permissions.clone()).unwrap();
        assert!(matches!(
            save_stars(&file, 5),
            Err(SidecarError::ReadOnly(_))
        ));
        assert_eq!(fs::read_to_string(&side).unwrap(), LIGHTROOM);
        #[allow(clippy::permissions_set_readonly_false)]
        permissions.set_readonly(false);
        fs::set_permissions(&side, permissions).unwrap();

        // Target that cannot be replaced: the rename fails, nothing remains.
        let blocked = dir.path().join("sub").join("x.CR2");
        let err = save_stars(&blocked, 2).unwrap_err();
        assert!(matches!(err, SidecarError::Io(..)), "{err}");
        let leftovers = fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .count();
        assert_eq!(leftovers, 0);
    }

    #[test]
    fn write_atomic_failure_cleans_up_its_temporary() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("t.xmp");
        // A directory in the way makes the rename fail after the write.
        fs::create_dir(&target).unwrap();
        assert!(write_atomic(&target, b"x").is_err());
        let names: Vec<_> = fs::read_dir(dir.path()).unwrap().flatten().collect();
        assert_eq!(names.len(), 1, "only the blocking directory");
    }

    #[test]
    fn the_sidecar_wins_over_the_rating_inside_the_raw() {
        let dir = tempfile::tempdir().unwrap();
        let body = b"HEADER xmp:Rating=\"2\" TRAILER";
        let file = raw(dir.path(), "a.CR3", body);
        assert_eq!(embedded_rating(&file), 2);
        assert_eq!(load_rating(&file, None).stars, 2);
        fs::write(sidecar_path(&file), new_sidecar(5)).unwrap();
        assert_eq!(
            load_rating(&file, existing_sidecar(&file).as_deref()).stars,
            5
        );
    }

    #[test]
    fn clearing_a_rating_from_inside_the_raw_records_the_zero() {
        let dir = tempfile::tempdir().unwrap();
        let body = b"HEADER xmp:Rating=\"3\" TRAILER";
        let file = raw(dir.path(), "a.NEF", body);
        save_stars(&file, 0).unwrap();
        assert_eq!(
            load_rating(&file, existing_sidecar(&file).as_deref()).stars,
            0
        );
        assert_eq!(fs::read(&file).unwrap(), body);
        assert_eq!(stored_rating(&file), Some(0));
    }

    #[test]
    fn a_rejected_rating_reads_as_no_stars_and_is_kept_unless_changed() {
        let dir = tempfile::tempdir().unwrap();
        let file = raw(dir.path(), "a.ORF", b"RAW");
        let side = sidecar_path(&file);
        let xml = LIGHTROOM.replace("xmp:Rating=\"3\"", "xmp:Rating=\"-1\"");
        fs::write(&side, &xml).unwrap();
        assert_eq!(
            load_rating(&file, existing_sidecar(&file).as_deref()).stars,
            0
        );
        assert_eq!(fs::read_to_string(&side).unwrap(), xml);
    }

    #[test]
    fn one_listing_finds_sidecars_in_either_case() {
        let dir = tempfile::tempdir().unwrap();
        let a = raw(dir.path(), "a.CR2", b"RAW");
        let b = raw(dir.path(), "b.CR2", b"RAW");
        let c = raw(dir.path(), "c.CR2", b"RAW");
        fs::write(dir.path().join("a.xmp"), new_sidecar(1)).unwrap();
        fs::write(dir.path().join("b.XMP"), new_sidecar(2)).unwrap();
        let index = SidecarIndex::of_files([a.as_path(), b.as_path(), c.as_path()]);
        assert_eq!(index.sidecar_for(&a), Some(dir.path().join("a.xmp")));
        assert_eq!(index.sidecar_for(&b), Some(dir.path().join("b.XMP")));
        assert_eq!(index.sidecar_for(&c), None);
    }

    #[test]
    fn two_raws_with_one_stem_share_a_sidecar() {
        let dir = tempfile::tempdir().unwrap();
        let cr2 = raw(dir.path(), "shot.CR2", b"A");
        let nef = raw(dir.path(), "shot.NEF", b"B");
        save_stars(&cr2, 4).unwrap();
        assert_eq!(
            load_rating(&nef, existing_sidecar(&nef).as_deref()).stars,
            4
        );
    }

    #[test]
    fn only_a_bounded_head_of_the_raw_is_searched() {
        let dir = tempfile::tempdir().unwrap();
        let mut body = vec![0x11u8; RAW_HEAD_BYTES as usize + 16];
        body.extend_from_slice(b"xmp:Rating=\"4\"");
        let file = raw(dir.path(), "a.CR2", &body);
        assert_eq!(embedded_rating(&file), 0, "the packet is past the bound");
    }

    #[test]
    fn a_sidecar_at_the_end_of_the_size_limit_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let file = raw(dir.path(), "a.CR2", b"RAW");
        let f = File::create(sidecar_path(&file)).unwrap();
        f.set_len(MAX_SIDECAR_BYTES + 1).unwrap();
        drop(f);
        assert!(matches!(
            save_stars(&file, 1),
            Err(SidecarError::TooLarge(_))
        ));
    }
}
