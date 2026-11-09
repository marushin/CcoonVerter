//! Single-file conversion (port of `converter.convert_file`).
//!
//! ```text
//! extract_embedded?
//!   yes: largest embedded JPEG
//!        resize == 0 -> write the JPEG bytes unchanged (EXIF/orientation fixed up)
//!        resize  > 0 -> decode -> orient -> resize -> encode(quality) -> EXIF
//!        failure     -> fall back to RAW development (like Python)
//!   no : develop_linear -> render (see `render.rs`) -> encode in the output
//!        format with ICC profile and EXIF (16-bit TIFF goes straight from f32)
//! ```
//! The cancel flag is checked between all expensive stages.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use rawler::decoders::{Decoder, RawDecodeParams, RawMetadata};
use rawler::rawsource::RawSource;
use serde::Serialize;

use crate::embedded::{self, EmbeddedPreview};
use crate::error::{classify_unsupported, ConvertError, Result};
use crate::exif::{self as exifmod, ExifInfo};
use crate::render::{self, RenderParams};
use crate::settings::{ColorSpace, ConflictPolicy, ConvertSettings, OutputFormat};
use crate::{color, develop, encode, resize};

/// One file to convert, with its output location already planned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileTask {
    pub source: PathBuf,
    /// Directory the output goes into (sub-folder structure preserved).
    pub out_dir: PathBuf,
    /// Output file stem (without the extension).
    pub stem: String,
    /// Output extension without the dot (set from the output format).
    pub output_ext: &'static str,
    /// Lower-case source extension, used for conflict suffixes.
    pub ext: String,
    /// True if the stem already carries the `_<ext>` suffix because another
    /// file of the same batch maps to the same name.
    pub batch_suffixed: bool,
}

impl FileTask {
    /// Plan the output of `source` (found below `source_root`) inside `output_root`.
    pub fn new(source: &Path, source_root: &Path, output_root: &Path) -> Self {
        let rel_dir = source
            .parent()
            .and_then(|p| p.strip_prefix(source_root).ok())
            .map(Path::to_path_buf)
            .unwrap_or_default();
        let stem = source
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "image".into());
        let ext = source
            .extension()
            .map(|s| s.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        Self {
            source: source.to_path_buf(),
            out_dir: output_root.join(rel_dir),
            stem,
            output_ext: "jpg",
            ext,
            batch_suffixed: false,
        }
    }

    /// Candidate output paths in order of preference.
    pub fn candidate(&self, attempt: u32) -> PathBuf {
        let with_ext = if self.ext.is_empty() {
            format!("{}_raw", self.stem)
        } else {
            format!("{}_{}", self.stem, self.ext)
        };
        let name = match (self.batch_suffixed, attempt) {
            (false, 0) => self.stem.clone(),
            (false, 1) | (true, 0) => with_ext,
            (false, n) => format!("{with_ext}_{}", n - 1),
            (true, n) => format!("{with_ext}_{n}"),
        };
        self.out_dir.join(format!("{name}.{}", self.output_ext))
    }

    /// The preferred output path.
    pub fn primary_output(&self) -> PathBuf {
        self.candidate(0)
    }
}

/// Plan all tasks of a batch. Files of the same folder that would produce the
/// same JPEG name (e.g. `IMG_1.ARW` and `IMG_1.DNG`) are disambiguated: the
/// first in sorted order keeps the plain name, the others get `_<ext>`.
pub fn plan_tasks(files: &[PathBuf], source_root: &Path, output_root: &Path) -> Vec<FileTask> {
    let mut out: Vec<FileTask> = files
        .iter()
        .map(|f| FileTask::new(f, source_root, output_root))
        .collect();
    disambiguate(&mut out);
    out
}

/// Give every task of a batch a unique output, in task order: the first keeps
/// its name, later ones get `_<ext>`, and a counter if that is taken too.
/// Linear in the batch size even when every name is the same.
pub(crate) fn disambiguate(tasks: &mut [FileTask]) {
    use std::collections::{HashMap, HashSet};
    type Key = (PathBuf, String);
    let key = |t: &FileTask| -> Key {
        (
            t.out_dir.clone(),
            t.primary_output()
                .file_name()
                .map(|n| n.to_string_lossy().to_lowercase())
                .unwrap_or_default(),
        )
    };
    let mut seen: HashSet<Key> = HashSet::new();
    let mut counters: HashMap<Key, u32> = HashMap::new();
    for t in tasks.iter_mut() {
        if seen.contains(&key(t)) {
            t.batch_suffixed = true;
            let base = t.stem.clone();
            if seen.contains(&key(t)) {
                let slot = (t.out_dir.clone(), base.to_lowercase());
                let n = counters.entry(slot).or_insert(1);
                loop {
                    t.stem = format!("{base}_{n}");
                    *n += 1;
                    if !seen.contains(&key(t)) {
                        break;
                    }
                }
            }
        }
        seen.insert(key(t));
    }
}

/// How the output was produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Method {
    /// Embedded JPEG written byte-for-byte (resize = 0).
    EmbeddedCopy,
    /// Embedded JPEG decoded, resized and re-encoded.
    EmbeddedReencoded,
    /// RAW developed with rawler.
    Developed,
}

impl Method {
    pub fn as_str(self) -> &'static str {
        match self {
            Method::EmbeddedCopy => "embedded_copy",
            Method::EmbeddedReencoded => "embedded_reencoded",
            Method::Developed => "developed",
        }
    }
}

/// Successful result of [`convert_one`].
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ConvertOutcome {
    Converted {
        output: PathBuf,
        method: Method,
        width: u32,
        height: u32,
        bytes: u64,
        /// True if embedded extraction was requested but failed and the RAW was developed.
        fell_back: bool,
    },
    /// Output exists and the policy is `Skip`.
    Skipped { output: PathBuf },
}

fn check(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        Err(ConvertError::Cancelled)
    } else {
        Ok(())
    }
}

struct Encoded {
    data: Vec<u8>,
    width: u32,
    height: u32,
    method: Method,
}

/// Lazily obtained decoder + metadata for one RAW file.
pub(crate) struct RawCtx<'a> {
    pub(crate) source: &'a RawSource,
    decoder: Option<Box<dyn Decoder>>,
    decoder_err: Option<String>,
    metadata: Option<RawMetadata>,
}

impl<'a> RawCtx<'a> {
    pub(crate) fn new(source: &'a RawSource) -> Self {
        let (decoder, decoder_err) = match crate::rawguard::get_decoder(source) {
            Ok(d) => (Some(d), None),
            Err(e) => (None, Some(e.to_string())),
        };
        let metadata = decoder.as_ref().and_then(|d| {
            d.raw_metadata(source, &RawDecodeParams::default())
                .map_err(|e| log::debug!("metadata unavailable: {e}"))
                .ok()
        });
        Self {
            source,
            decoder,
            decoder_err,
            metadata,
        }
    }

    pub(crate) fn decoder(&self) -> Result<&dyn Decoder> {
        self.decoder.as_deref().ok_or_else(|| {
            ConvertError::Decode(
                self.decoder_err
                    .clone()
                    .unwrap_or_else(|| "unsupported file".into()),
            )
        })
    }

    /// RAW metadata plus the rating from the file's XMP packet, if any.
    fn exif_info(&self) -> Option<ExifInfo> {
        let mut info = self.metadata.as_ref().map(ExifInfo::from_rawler)?;
        if info.rating.is_none() {
            info.rating = exifmod::find_xmp_rating(self.source.buf());
        }
        Some(info)
    }

    /// Orientation of the RAW (EXIF tag value 1..8), 1 if unknown.
    pub(crate) fn raw_orientation(&self) -> u16 {
        if let Some(o) = self
            .metadata
            .as_ref()
            .and_then(|m| m.exif.orientation)
            .filter(|o| (1..=8).contains(o))
        {
            return o;
        }
        // Some decoders only expose orientation on the RawImage; dummy decode is cheap.
        self.decoder
            .as_ref()
            .and_then(|d| {
                d.raw_image(self.source, &RawDecodeParams::default(), true)
                    .ok()
            })
            .map(|r| r.orientation.to_u16())
            .filter(|o| (1..=8).contains(o))
            .unwrap_or(1)
    }
}

/// Convert one RAW file. Never panics on bad input except inside rawler;
/// callers (see `job.rs`) wrap this in `catch_unwind`.
pub fn convert_one(
    task: &FileTask,
    settings: &ConvertSettings,
    cancel: &AtomicBool,
) -> Result<ConvertOutcome> {
    check(cancel)?;
    let start = Instant::now();
    let name = task
        .source
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();

    if settings.on_conflict == ConflictPolicy::Skip && task.primary_output().exists() {
        log::info!("{name}: output exists, skipped");
        return Ok(ConvertOutcome::Skipped {
            output: task.primary_output(),
        });
    }

    log::info!("Converting {name}...");
    let source = RawSource::new(&task.source).map_err(|e| ConvertError::io(&task.source, e))?;
    let ctx = RawCtx::new(&source);
    check(cancel)?;

    let mut fell_back = false;
    let mut encoded = None;
    if settings.extract_embedded {
        match encode_embedded(&ctx, settings, cancel) {
            Ok(e) => encoded = Some(e),
            Err(ConvertError::Cancelled) => return Err(ConvertError::Cancelled),
            Err(e) => {
                log::warn!(
                    "{name}: embedded JPEG unavailable ({e}). Falling back to RAW development."
                );
                fell_back = true;
            }
        }
    }
    let encoded = match encoded {
        Some(e) => e,
        None => encode_developed(&ctx, settings, cancel)
            .map_err(|e| explain_unsupported(e, source.buf()))?,
    };
    check(cancel)?;

    fs::create_dir_all(&task.out_dir).map_err(|e| ConvertError::io(&task.out_dir, e))?;
    let output = match write_output(task, settings.on_conflict, &encoded.data)? {
        Some(p) => p,
        None => {
            return Ok(ConvertOutcome::Skipped {
                output: task.primary_output(),
            })
        }
    };
    if settings.keep_source_date {
        if let Err(e) = copy_modified_time(&task.source, &output) {
            log::warn!("{name}: could not copy the source file date: {e}");
        }
    }
    log::info!(
        "Successfully converted {name} to {} ({:?}, {}x{}, {:.2}s)",
        output.display(),
        encoded.method,
        encoded.width,
        encoded.height,
        start.elapsed().as_secs_f32()
    );
    Ok(ConvertOutcome::Converted {
        output,
        method: encoded.method,
        width: encoded.width,
        height: encoded.height,
        bytes: encoded.data.len() as u64,
        fell_back,
    })
}

/// Turn a decoder message about an unsupported camera or compression into an
/// error that says whether camera JPEG mode can still get something out of
/// the file. Nothing is converted here: the user decides.
fn explain_unsupported(e: ConvertError, buf: &[u8]) -> ConvertError {
    let ConvertError::Decode(detail) = &e else {
        return e;
    };
    match classify_unsupported(detail) {
        Some(kind) => ConvertError::UnsupportedRaw {
            detail: detail.clone(),
            kind,
            has_camera_jpeg: embedded::largest_jpeg_in(buf).is_some(),
        },
        None => e,
    }
}

/// Give `dst` the modification time of `src`.
fn copy_modified_time(src: &Path, dst: &Path) -> std::io::Result<()> {
    let modified = fs::metadata(src)?.modified()?;
    let f = OpenOptions::new().write(true).open(dst)?;
    f.set_modified(modified)
}

/// EXIF payload for re-encoded pixels that are already upright: the
/// embedded JPEG's EXIF, enriched with tags only the RAW has, else one built
/// from the RAW metadata. `space` is the colour space of developed pixels;
/// `None` leaves the colour tags alone (camera JPEG pixels).
fn exif_for_upright(
    ctx: &RawCtx,
    embedded_jpeg: Option<&[u8]>,
    space: Option<ColorSpace>,
) -> Option<Vec<u8>> {
    let mut info = ctx.exif_info();
    if let (Some(i), Some(s)) = (info.as_mut(), space) {
        i.output_color_space = Some(exifmod::exif_color_space(s));
    }
    if let Some(mut tiff) = embedded_jpeg.and_then(exifmod::jpeg_exif) {
        if let Some(info) = &info {
            if exifmod::lacks_fields(&tiff, info) {
                match exifmod::merge_exif(&tiff, info, 1) {
                    Ok(mut merged) => {
                        if let Some(s) = space {
                            exifmod::set_tiff_color_space(&mut merged, s);
                        }
                        return Some(merged);
                    }
                    Err(e) => log::warn!("Could not merge EXIF: {e}"),
                }
            }
        }
        exifmod::set_tiff_orientation(&mut tiff, 1);
        if let Some(s) = space {
            exifmod::set_tiff_color_space(&mut tiff, s);
        }
        return Some(tiff);
    }
    match exifmod::build_exif(&info?, 1) {
        Ok(t) => Some(t),
        Err(e) => {
            log::warn!("Could not build EXIF: {e}");
            None
        }
    }
}

/// Add the XMP rating to a finished JPEG when the RAW has one.
fn with_rating(ctx: &RawCtx, preserve: bool, jpeg: Vec<u8>) -> Vec<u8> {
    if !preserve {
        return jpeg;
    }
    match ctx.exif_info().and_then(|i| i.rating) {
        Some(r) => exifmod::add_xmp_rating(&jpeg, r).unwrap_or_else(|e| {
            log::warn!("Could not write the XMP rating: {e}");
            jpeg
        }),
        None => jpeg,
    }
}

/// Orientation for an embedded JPEG stream: its own EXIF value when present,
/// otherwise the RAW's.
pub(crate) fn embedded_orientation(data: &[u8], raw_orientation: u16) -> u16 {
    exifmod::jpeg_exif(data)
        .as_deref()
        .and_then(exifmod::tiff_orientation)
        .unwrap_or(raw_orientation)
}

fn encode_embedded(ctx: &RawCtx, s: &ConvertSettings, cancel: &AtomicBool) -> Result<Encoded> {
    let preview = embedded::extract(ctx.source, ctx.decoder.as_deref())
        .ok_or(ConvertError::NoEmbeddedJpeg)?;
    check(cancel)?;
    let raw_orientation = ctx.raw_orientation();

    match preview {
        EmbeddedPreview::Jpeg {
            data,
            width,
            height,
        } => {
            let own_exif = exifmod::jpeg_exif(&data);
            let own_orientation = own_exif.as_deref().and_then(exifmod::tiff_orientation);
            let orientation = embedded_orientation(&data, raw_orientation);

            if s.resize == 0 {
                // Lossless path: keep the camera's JPEG bytes.
                let jpeg = if s.preserve_exif {
                    let info = ctx.exif_info();
                    let keep_payload =
                        own_orientation.is_some() || (own_exif.is_some() && orientation == 1);
                    let merged = match (&own_exif, &info) {
                        (Some(tiff), Some(i)) if keep_payload && exifmod::lacks_fields(tiff, i) => {
                            exifmod::merge_exif(tiff, i, orientation).ok()
                        }
                        _ => None,
                    };
                    let jpeg = if let Some(tiff) = merged {
                        exifmod::replace_jpeg_exif(&data, Some(&tiff))?
                    } else if keep_payload {
                        data
                    } else {
                        // No usable EXIF in the preview: build one from RAW metadata.
                        let tiff = match &info {
                            Some(info) => exifmod::build_exif(info, orientation)?,
                            None => exifmod::orientation_only_exif(orientation)?,
                        };
                        exifmod::replace_jpeg_exif(&data, Some(&tiff))?
                    };
                    with_rating(ctx, true, jpeg)
                } else if orientation != 1 {
                    let tiff = exifmod::orientation_only_exif(orientation)?;
                    exifmod::replace_jpeg_exif(&data, Some(&tiff))?
                } else {
                    exifmod::replace_jpeg_exif(&data, None)?
                };
                let (w, h) = if matches!(orientation, 5..=8) {
                    (height, width)
                } else {
                    (width, height)
                };
                return Ok(Encoded {
                    data: jpeg,
                    width: w,
                    height: h,
                    method: Method::EmbeddedCopy,
                });
            }

            let img =
                image::load_from_memory_with_format(&data, image::ImageFormat::Jpeg)?.into_rgb8();
            check(cancel)?;
            let img = resize::apply_orientation(img, orientation);
            let img = resize::resize_long_edge(img, s.resize);
            check(cancel)?;
            let exif = if s.preserve_exif {
                exif_for_upright(ctx, Some(&data), None)
            } else {
                None
            };
            let jpeg = encode::encode_rgb8(&img, s.quality, exif.as_deref())?;
            let jpeg = with_rating(ctx, s.preserve_exif, jpeg);
            Ok(Encoded {
                width: img.width(),
                height: img.height(),
                data: jpeg,
                method: Method::EmbeddedReencoded,
            })
        }
        EmbeddedPreview::Decoded(dynimg) => {
            let img = resize::apply_orientation(dynimg.into_rgb8(), raw_orientation);
            let img = resize::resize_long_edge(img, s.resize);
            check(cancel)?;
            let exif = if s.preserve_exif {
                exif_for_upright(ctx, None, None)
            } else {
                None
            };
            let jpeg = encode::encode_rgb8(&img, s.quality, exif.as_deref())?;
            let jpeg = with_rating(ctx, s.preserve_exif, jpeg);
            Ok(Encoded {
                width: img.width(),
                height: img.height(),
                data: jpeg,
                method: Method::EmbeddedReencoded,
            })
        }
    }
}

fn encode_developed(ctx: &RawCtx, s: &ConvertSettings, cancel: &AtomicBool) -> Result<Encoded> {
    let decoder = ctx.decoder()?;
    let base = develop::develop_linear(ctx.source, decoder, ctx.raw_orientation(), cancel)?;
    check(cancel)?;
    let img = render::render(
        base.image,
        &base.stats,
        &RenderParams::from(s),
        s.resize,
        cancel,
    )?;
    check(cancel)?;
    let format = s.effective_format();
    let space = s.effective_color_space();
    let exif = if s.preserve_exif {
        let buf = ctx.source.buf();
        let embedded = embedded::largest_jpeg_in(buf).map(|c| c.bytes(buf).to_vec());
        exif_for_upright(ctx, embedded.as_deref(), Some(space))
    } else {
        None
    };
    let rating = if s.preserve_exif {
        ctx.exif_info().and_then(|i| i.rating)
    } else {
        None
    };
    let meta = encode::OutputMeta {
        exif: exif.as_deref(),
        icc: Some(color::icc_profile(space)),
        rating,
    };
    let (width, height) = img.dimensions();
    let data = if format == OutputFormat::Tiff16 {
        // Straight from f32: this path never goes through 8 bits.
        encode::encode_tiff16(&img, &meta)?
    } else {
        let img8 = resize::quantize(&img);
        drop(img);
        check(cancel)?;
        match format {
            OutputFormat::Png => encode::encode_png(&img8, &meta)?,
            OutputFormat::Webp => encode::encode_webp(&img8, s.quality, &meta)?,
            OutputFormat::Jpeg | OutputFormat::Tiff16 => {
                encode::encode_jpeg(&img8, s.quality, &meta)?
            }
        }
    };
    Ok(Encoded {
        width,
        height,
        data,
        method: Method::Developed,
    })
}

/// Write the output according to the conflict policy. Returns `None` if skipped.
pub fn write_output(
    task: &FileTask,
    policy: ConflictPolicy,
    data: &[u8],
) -> Result<Option<PathBuf>> {
    let write = |path: &Path, create_new: bool| -> std::io::Result<()> {
        let mut opts = OpenOptions::new();
        opts.write(true);
        if create_new {
            opts.create_new(true);
        } else {
            opts.create(true).truncate(true);
        }
        let mut f = opts.open(path)?;
        if let Err(e) = f.write_all(data).and_then(|_| f.flush()) {
            drop(f);
            let _ = fs::remove_file(path);
            return Err(e);
        }
        Ok(())
    };
    match policy {
        ConflictPolicy::Overwrite => {
            let p = task.primary_output();
            write(&p, false).map_err(|e| ConvertError::io(&p, e))?;
            Ok(Some(p))
        }
        ConflictPolicy::Skip => {
            let p = task.primary_output();
            match write(&p, true) {
                Ok(()) => Ok(Some(p)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(None),
                Err(e) => Err(ConvertError::io(&p, e)),
            }
        }
        ConflictPolicy::Suffix => {
            for attempt in 0..10_000 {
                let p = task.candidate(attempt);
                match write(&p, true) {
                    Ok(()) => return Ok(Some(p)),
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(e) => return Err(ConvertError::io(&p, e)),
                }
            }
            Err(ConvertError::io(
                task.primary_output(),
                std::io::Error::other("too many existing files with the same name"),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::FailureKind;

    #[test]
    fn output_path_preserves_subfolders() {
        let t = FileTask::new(
            Path::new("/src/2026/한글/IMG_1.ARW"),
            Path::new("/src"),
            Path::new("/out"),
        );
        assert_eq!(t.out_dir, Path::new("/out/2026/한글"));
        assert_eq!(t.primary_output(), Path::new("/out/2026/한글/IMG_1.jpg"));
        assert_eq!(t.candidate(1), Path::new("/out/2026/한글/IMG_1_arw.jpg"));
        assert_eq!(t.candidate(2), Path::new("/out/2026/한글/IMG_1_arw_1.jpg"));
        let top = FileTask::new(Path::new("/src/a.dng"), Path::new("/src"), Path::new("/o"));
        assert_eq!(top.primary_output(), Path::new("/o/a.jpg"));
    }

    #[test]
    fn plan_disambiguates_same_stem_in_batch() {
        let files: Vec<PathBuf> = [
            "/s/IMG_1.ARW",
            "/s/IMG_1.DNG",
            "/s/sub/IMG_1.ARW",
            "/s/IMG_2.NEF",
        ]
        .iter()
        .map(PathBuf::from)
        .collect();
        let tasks = plan_tasks(&files, Path::new("/s"), Path::new("/o"));
        let outs: Vec<PathBuf> = tasks.iter().map(|t| t.primary_output()).collect();
        assert_eq!(
            outs,
            vec![
                PathBuf::from("/o/IMG_1.jpg"),
                PathBuf::from("/o/IMG_1_dng.jpg"),
                PathBuf::from("/o/sub/IMG_1.jpg"),
                PathBuf::from("/o/IMG_2.jpg"),
            ]
        );
        assert_eq!(tasks[1].candidate(1), PathBuf::from("/o/IMG_1_dng_1.jpg"));
    }

    #[test]
    fn conflict_policies() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        let out = dir.path().join("out");
        fs::create_dir_all(&out).unwrap();
        let t = FileTask::new(&src.join("A.CR3"), &src, &out);
        fs::write(out.join("A.jpg"), b"old").unwrap();

        // Skip
        assert_eq!(
            write_output(&t, ConflictPolicy::Skip, b"new").unwrap(),
            None
        );
        assert_eq!(fs::read(out.join("A.jpg")).unwrap(), b"old");
        // Suffix
        let p = write_output(&t, ConflictPolicy::Suffix, b"s1")
            .unwrap()
            .unwrap();
        assert_eq!(p, out.join("A_cr3.jpg"));
        let p = write_output(&t, ConflictPolicy::Suffix, b"s2")
            .unwrap()
            .unwrap();
        assert_eq!(p, out.join("A_cr3_1.jpg"));
        assert_eq!(fs::read(out.join("A.jpg")).unwrap(), b"old");
        // Overwrite
        let p = write_output(&t, ConflictPolicy::Overwrite, b"new")
            .unwrap()
            .unwrap();
        assert_eq!(p, out.join("A.jpg"));
        assert_eq!(fs::read(out.join("A.jpg")).unwrap(), b"new");
        // Skip writes when free
        let t2 = FileTask::new(&src.join("B.NEF"), &src, &out);
        assert_eq!(
            write_output(&t2, ConflictPolicy::Skip, b"b").unwrap(),
            Some(out.join("B.jpg"))
        );
    }

    #[test]
    fn convert_one_reports_decode_error_for_garbage() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("bad.ARW");
        fs::write(&src, vec![0u8; 4096]).unwrap();
        let t = FileTask::new(&src, dir.path(), &dir.path().join("out"));
        let cancel = AtomicBool::new(false);
        let err = convert_one(&t, &ConvertSettings::default(), &cancel).unwrap_err();
        assert!(matches!(err, ConvertError::Decode(_)), "{err:?}");
        // Embedded mode also falls back and then fails cleanly.
        let s = ConvertSettings {
            extract_embedded: true,
            ..Default::default()
        };
        assert!(convert_one(&t, &s, &cancel).is_err());
    }

    #[test]
    fn embedded_copy_from_fake_raw() {
        // A "RAW" that rawler cannot decode but that contains a JPEG: the
        // scanner still finds it and the bytes are copied unchanged.
        let dir = tempfile::tempdir().unwrap();
        let jpg = crate::embedded::tests::tiny_jpeg(48, 32, [200, 100, 50]);
        let mut blob = vec![0x42u8; 300];
        blob.extend_from_slice(&jpg);
        blob.extend_from_slice(&[0u8; 100]);
        let src = dir.path().join("fake.NEF");
        fs::write(&src, &blob).unwrap();
        let out = dir.path().join("out");
        let t = FileTask::new(&src, dir.path(), &out);
        let cancel = AtomicBool::new(false);
        let s = ConvertSettings {
            extract_embedded: true,
            preserve_exif: false,
            ..Default::default()
        };
        let res = convert_one(&t, &s, &cancel).unwrap();
        match res {
            ConvertOutcome::Converted {
                output,
                method,
                width,
                height,
                ..
            } => {
                assert_eq!(method, Method::EmbeddedCopy);
                assert_eq!((width, height), (48, 32));
                assert_eq!(fs::read(&output).unwrap(), jpg);
            }
            other => panic!("{other:?}"),
        }
        // Resize path re-encodes.
        let s2 = ConvertSettings {
            extract_embedded: true,
            resize: 24,
            on_conflict: ConflictPolicy::Overwrite,
            ..Default::default()
        };
        match convert_one(&t, &s2, &cancel).unwrap() {
            ConvertOutcome::Converted {
                method,
                width,
                height,
                output,
                ..
            } => {
                assert_eq!(method, Method::EmbeddedReencoded);
                assert_eq!((width, height), (24, 16));
                let img = image::open(output).unwrap();
                assert_eq!(img.width(), 24);
            }
            other => panic!("{other:?}"),
        }
        // Skip policy now skips.
        let s3 = ConvertSettings {
            on_conflict: ConflictPolicy::Skip,
            ..s
        };
        assert!(matches!(
            convert_one(&t, &s3, &cancel).unwrap(),
            ConvertOutcome::Skipped { .. }
        ));
    }

    #[test]
    fn keep_source_date_copies_mtime_when_enabled() {
        use crate::testutil::{synthetic_dng, DngSpec};
        use std::time::{Duration, SystemTime};
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("a.DNG");
        fs::write(&src, synthetic_dng(&DngSpec::default())).unwrap();
        // An old date, clearly not "now".
        let old = SystemTime::UNIX_EPOCH + Duration::from_secs(1_500_000_000);
        fs::File::options()
            .write(true)
            .open(&src)
            .unwrap()
            .set_modified(old)
            .unwrap();
        let out = dir.path().join("out");
        let t = FileTask::new(&src, dir.path(), &out);
        let cancel = AtomicBool::new(false);
        let ConvertOutcome::Converted { output, .. } = convert_one(
            &t,
            &ConvertSettings {
                on_conflict: ConflictPolicy::Overwrite,
                ..Default::default()
            },
            &cancel,
        )
        .unwrap() else {
            panic!("expected conversion");
        };
        let now_date = fs::metadata(&output).unwrap().modified().unwrap();
        assert!(now_date.duration_since(old).unwrap() > Duration::from_secs(86_400));

        let ConvertOutcome::Converted { output, .. } = convert_one(
            &t,
            &ConvertSettings {
                on_conflict: ConflictPolicy::Overwrite,
                keep_source_date: true,
                ..Default::default()
            },
            &cancel,
        )
        .unwrap() else {
            panic!("expected conversion");
        };
        let kept = fs::metadata(&output).unwrap().modified().unwrap();
        assert_eq!(kept, old);
    }

    fn developed(
        s: &ConvertSettings,
        dng: &crate::testutil::DngSpec,
    ) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("a.DNG");
        fs::write(&src, crate::testutil::synthetic_dng(dng)).unwrap();
        let tasks = plan_tasks(&[src], dir.path(), &dir.path().join("out"));
        let cancel = AtomicBool::new(false);
        let planned = crate::naming::plan_output_names(&tasks, s, &cancel).unwrap();
        let ConvertOutcome::Converted { output, .. } =
            convert_one(&planned[0], s, &cancel).unwrap()
        else {
            panic!("expected conversion");
        };
        (dir, output)
    }

    #[test]
    fn every_format_writes_its_extension_and_decodes() {
        for (format, ext) in [
            (OutputFormat::Jpeg, "jpg"),
            (OutputFormat::Png, "png"),
            (OutputFormat::Tiff16, "tif"),
            (OutputFormat::Webp, "webp"),
        ] {
            for space in [ColorSpace::Srgb, ColorSpace::AdobeRgb] {
                let s = ConvertSettings {
                    output_format: format,
                    color_space: space,
                    preserve_exif: true,
                    ..Default::default()
                };
                let (_d, out) = developed(&s, &crate::testutil::DngSpec::default());
                assert_eq!(out.extension().unwrap(), ext);
                let img = image::open(&out).unwrap();
                assert_eq!((img.width(), img.height()), (64, 48), "{ext}");
                // PNG compresses its profile, the other formats store it raw.
                if format != OutputFormat::Png {
                    let bytes = fs::read(&out).unwrap();
                    let name = match space {
                        ColorSpace::Srgb => &b"sRGB-compatible"[..],
                        ColorSpace::AdobeRgb => &b"AdobeRGB-compatible"[..],
                    };
                    assert!(
                        bytes.windows(name.len()).any(|w| w == name),
                        "{ext} {space:?} has no profile"
                    );
                }
            }
        }
    }

    #[test]
    fn sixteen_bit_output_is_not_upscaled_from_eight_bits() {
        // A wide ramp: an 8-bit path would give at most 256 levels, all exact
        // multiples of 257.
        let spec = crate::testutil::DngSpec {
            width: 1024,
            height: 16,
            preview: None,
            ..Default::default()
        };
        let s = ConvertSettings {
            output_format: OutputFormat::Tiff16,
            ..Default::default()
        };
        let (_d, out) = developed(&s, &spec);
        let img = image::open(&out).unwrap();
        assert!(
            matches!(img, image::DynamicImage::ImageRgb16(_)),
            "{:?}",
            img.color()
        );
        let img = img.into_rgb16();
        let values: std::collections::HashSet<u16> = img.pixels().map(|p| p.0[0]).collect();
        let off_grid = values.iter().filter(|v| **v % 257 != 0).count();
        assert!(values.len() > 300, "only {} levels", values.len());
        assert!(
            off_grid > values.len() / 2,
            "{off_grid} of {}",
            values.len()
        );
    }

    #[test]
    fn exif_survives_in_every_format_and_describes_the_colour_space() {
        use ::exif::{In, Tag};
        for format in OutputFormat::ALL {
            let s = ConvertSettings {
                output_format: format,
                color_space: ColorSpace::AdobeRgb,
                preserve_exif: true,
                ..Default::default()
            };
            let spec = crate::testutil::DngSpec {
                preview: None,
                ..Default::default()
            };
            let (_d, out) = developed(&s, &spec);
            let bytes = fs::read(&out).unwrap();
            let tiff = match format {
                OutputFormat::Tiff16 => bytes.clone(),
                OutputFormat::Jpeg => exifmod::jpeg_exif(&bytes).unwrap(),
                OutputFormat::Png => {
                    let r = png::Decoder::new(std::io::Cursor::new(bytes.as_slice()))
                        .read_info()
                        .unwrap();
                    r.info().exif_metadata.as_ref().unwrap().to_vec()
                }
                OutputFormat::Webp => {
                    let p = bytes.windows(4).position(|w| w == b"EXIF").unwrap();
                    let n = u32::from_le_bytes(bytes[p + 4..p + 8].try_into().unwrap()) as usize;
                    bytes[p + 8..p + 8 + n].to_vec()
                }
            };
            let ex = ::exif::Reader::new().read_raw(tiff).unwrap();
            let f = |t: Tag| ex.get_field(t, In::PRIMARY);
            assert_eq!(
                f(Tag::Make).unwrap().display_value().to_string(),
                "\"CcooNverter\""
            );
            assert_eq!(f(Tag::Orientation).unwrap().value.get_uint(0), Some(1));
            assert_eq!(
                f(Tag::ColorSpace).unwrap().value.get_uint(0),
                Some(0xFFFF),
                "{format:?}"
            );
            assert!(f(Tag::DateTimeOriginal).is_some());
        }
    }

    #[test]
    fn camera_jpeg_mode_ignores_output_format() {
        let s = ConvertSettings {
            extract_embedded: true,
            output_format: OutputFormat::Webp,
            color_space: ColorSpace::AdobeRgb,
            ..Default::default()
        };
        let (_d, out) = developed(&s, &crate::testutil::DngSpec::default());
        assert_eq!(out.extension().unwrap(), "jpg");
        let b = fs::read(&out).unwrap();
        assert_eq!(&b[..3], [0xFF, 0xD8, 0xFF]);
        assert!(!b.windows(11).any(|w| w == b"ICC_PROFILE"));
    }

    #[test]
    fn unsupported_errors_offer_camera_jpeg_only_when_one_exists() {
        let jpg = crate::embedded::tests::tiny_jpeg(48, 32, [200, 100, 50]);
        let msg = "Error: Unknown camera, model 'ILCE-7M5', make: 'SONY', mode: 'arw6'";
        let e = explain_unsupported(ConvertError::Decode(msg.into()), &jpg);
        assert!(e.camera_jpeg_available());
        assert!(e.to_string().contains("Camera JPEG mode"), "{e}");
        assert_eq!(e.failure_kind(), FailureKind::UnsupportedCamera);

        let he = "Failed to decode image, possibly corrupt image: \
                  NEF compression Some(HighEfficency) is not supported";
        let e = explain_unsupported(ConvertError::Decode(he.into()), &[0u8; 8]);
        assert_eq!(e.failure_kind(), FailureKind::UnsupportedCompression);
        assert!(!e.camera_jpeg_available());
        assert!(!e.to_string().contains("Camera JPEG mode"), "{e}");

        let other = explain_unsupported(ConvertError::Decode("bad data".into()), &jpg);
        assert!(matches!(other, ConvertError::Decode(_)));
    }

    /// The promise in the user documents: a conversion never changes the RAW,
    /// even with the output in the same folder and every overwriting option on.
    #[test]
    fn the_raw_file_is_left_untouched() {
        use crate::testutil::{synthetic_dng, DngSpec};
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("a.DNG");
        fs::write(&src, synthetic_dng(&DngSpec::default())).unwrap();
        let bytes = fs::read(&src).unwrap();
        let modified = fs::metadata(&src).unwrap().modified().unwrap();
        let cancel = AtomicBool::new(false);
        for format in OutputFormat::ALL {
            for extract_embedded in [false, true] {
                let s = ConvertSettings {
                    output_format: format,
                    extract_embedded,
                    keep_source_date: true,
                    preserve_exif: true,
                    on_conflict: ConflictPolicy::Overwrite,
                    filename_template: "{name}".into(),
                    ..Default::default()
                };
                let tasks = plan_tasks(std::slice::from_ref(&src), dir.path(), dir.path());
                let planned = crate::naming::plan_output_names(&tasks, &s, &cancel).unwrap();
                convert_one(&planned[0], &s, &cancel).unwrap();
                assert_eq!(fs::read(&src).unwrap(), bytes, "{format:?}");
                assert_eq!(fs::metadata(&src).unwrap().modified().unwrap(), modified);
            }
        }
    }

    #[test]
    fn cancelled_before_start() {
        let t = FileTask::new(Path::new("/x/a.ARW"), Path::new("/x"), Path::new("/y"));
        let cancel = AtomicBool::new(true);
        assert!(convert_one(&t, &ConvertSettings::default(), &cancel)
            .unwrap_err()
            .is_cancelled());
    }
}
