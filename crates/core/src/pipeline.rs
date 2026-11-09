//! Single-file conversion (port of `converter.convert_file`).
//!
//! ```text
//! extract_embedded?
//!   yes: largest embedded JPEG
//!        resize == 0 -> write the JPEG bytes unchanged (EXIF/orientation fixed up)
//!        resize  > 0 -> decode -> orient -> resize -> encode(quality) -> EXIF
//!        failure     -> fall back to RAW development (like Python)
//!   no : develop -> RGB scale + Levels -> style -> resize -> quantise -> encode -> EXIF
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
use crate::error::{ConvertError, Result};
use crate::exif::{self as exifmod, ExifInfo};
use crate::settings::{ConflictPolicy, ConvertSettings};
use crate::{adjust, develop, encode, resize, style};

/// One file to convert, with its output location already planned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileTask {
    pub source: PathBuf,
    /// Directory the JPEG goes into (sub-folder structure preserved).
    pub out_dir: PathBuf,
    /// Output file stem (without `.jpg`).
    pub stem: String,
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
        self.out_dir.join(format!("{name}.jpg"))
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
    use std::collections::HashSet;
    let mut seen: HashSet<(PathBuf, String)> = HashSet::new();
    let mut out = Vec::with_capacity(files.len());
    for f in files {
        let mut t = FileTask::new(f, source_root, output_root);
        let key = |t: &FileTask| {
            (
                t.out_dir.clone(),
                t.primary_output()
                    .file_name()
                    .map(|n| n.to_string_lossy().to_lowercase())
                    .unwrap_or_default(),
            )
        };
        if seen.contains(&key(&t)) {
            t.batch_suffixed = true;
            // Same stem and extension differing only in case: add a counter.
            let base = t.stem.clone();
            let mut n = 1;
            while seen.contains(&key(&t)) {
                t.stem = format!("{base}_{n}");
                n += 1;
            }
        }
        seen.insert(key(&t));
        out.push(t);
    }
    out
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
    jpeg: Vec<u8>,
    width: u32,
    height: u32,
    method: Method,
}

/// Lazily obtained decoder + metadata for one RAW file.
struct RawCtx<'a> {
    source: &'a RawSource,
    decoder: Option<Box<dyn Decoder>>,
    decoder_err: Option<String>,
    metadata: Option<RawMetadata>,
}

impl<'a> RawCtx<'a> {
    fn new(source: &'a RawSource) -> Self {
        let (decoder, decoder_err) = match rawler::get_decoder(source) {
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

    fn decoder(&self) -> Result<&dyn Decoder> {
        self.decoder.as_deref().ok_or_else(|| {
            ConvertError::Decode(
                self.decoder_err
                    .clone()
                    .unwrap_or_else(|| "unsupported file".into()),
            )
        })
    }

    fn exif_info(&self) -> Option<ExifInfo> {
        self.metadata.as_ref().map(ExifInfo::from_rawler)
    }

    /// Orientation of the RAW (EXIF tag value 1..8), 1 if unknown.
    fn raw_orientation(&self) -> u16 {
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
        None => encode_developed(&ctx, settings, cancel)?,
    };
    check(cancel)?;

    fs::create_dir_all(&task.out_dir).map_err(|e| ConvertError::io(&task.out_dir, e))?;
    let output = match write_output(task, settings.on_conflict, &encoded.jpeg)? {
        Some(p) => p,
        None => {
            return Ok(ConvertOutcome::Skipped {
                output: task.primary_output(),
            })
        }
    };
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
        bytes: encoded.jpeg.len() as u64,
        fell_back,
    })
}

/// EXIF payload for re-encoded pixels that are already upright.
fn exif_for_upright(ctx: &RawCtx, embedded_jpeg: Option<&[u8]>) -> Option<Vec<u8>> {
    if let Some(mut tiff) = embedded_jpeg.and_then(exifmod::jpeg_exif) {
        exifmod::set_tiff_orientation(&mut tiff, 1);
        return Some(tiff);
    }
    let info = ctx.exif_info()?;
    match exifmod::build_exif(&info, 1) {
        Ok(t) => Some(t),
        Err(e) => {
            log::warn!("Could not build EXIF: {e}");
            None
        }
    }
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
            let orientation = own_orientation.unwrap_or(raw_orientation);

            if s.resize == 0 {
                // Lossless path: keep the camera's JPEG bytes.
                let jpeg = if s.preserve_exif {
                    if own_orientation.is_some() || (own_exif.is_some() && orientation == 1) {
                        data
                    } else {
                        // No usable EXIF in the preview: build one from RAW metadata.
                        let tiff = match ctx.exif_info() {
                            Some(info) => exifmod::build_exif(&info, orientation)?,
                            None => exifmod::orientation_only_exif(orientation)?,
                        };
                        exifmod::replace_jpeg_exif(&data, Some(&tiff))?
                    }
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
                    jpeg,
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
                exif_for_upright(ctx, Some(&data))
            } else {
                None
            };
            let jpeg = encode::encode_rgb8(&img, s.quality, exif.as_deref())?;
            Ok(Encoded {
                width: img.width(),
                height: img.height(),
                jpeg,
                method: Method::EmbeddedReencoded,
            })
        }
        EmbeddedPreview::Decoded(dynimg) => {
            let img = resize::apply_orientation(dynimg.into_rgb8(), raw_orientation);
            let img = resize::resize_long_edge(img, s.resize);
            check(cancel)?;
            let exif = if s.preserve_exif {
                exif_for_upright(ctx, None)
            } else {
                None
            };
            let jpeg = encode::encode_rgb8(&img, s.quality, exif.as_deref())?;
            Ok(Encoded {
                width: img.width(),
                height: img.height(),
                jpeg,
                method: Method::EmbeddedReencoded,
            })
        }
    }
}

fn encode_developed(ctx: &RawCtx, s: &ConvertSettings, cancel: &AtomicBool) -> Result<Encoded> {
    let decoder = ctx.decoder()?;
    let mut img = develop::develop(ctx.source, decoder, s.white_balance, s.brightness, cancel)?;
    check(cancel)?;
    adjust::apply(&mut img, s.rgb(), s.black_point, s.white_point);
    style::apply(&mut img, s.style);
    check(cancel)?;
    let img = resize::resize_long_edge(img, s.resize);
    let img8 = resize::quantize(&img);
    drop(img);
    check(cancel)?;
    let exif = if s.preserve_exif {
        let buf = ctx.source.buf();
        let embedded = embedded::largest_jpeg_in(buf).map(|c| c.bytes(buf).to_vec());
        exif_for_upright(ctx, embedded.as_deref())
    } else {
        None
    };
    let jpeg = encode::encode_rgb8(&img8, s.quality, exif.as_deref())?;
    Ok(Encoded {
        width: img8.width(),
        height: img8.height(),
        jpeg,
        method: Method::Developed,
    })
}

/// Write the JPEG according to the conflict policy. Returns `None` if skipped.
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
    fn cancelled_before_start() {
        let t = FileTask::new(Path::new("/x/a.ARW"), Path::new("/x"), Path::new("/y"));
        let cancel = AtomicBool::new(true);
        assert!(convert_one(&t, &ConvertSettings::default(), &cancel)
            .unwrap_err()
            .is_cancelled());
    }
}
