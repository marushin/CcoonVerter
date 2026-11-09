//! Sample regression matrix on real camera files (`tests/regression/manifest.json`).
//!
//! ```text
//! cargo test --release -p ccoonverter-core --test regression_matrix -- --ignored --nocapture --test-threads=1
//! ```
//!
//! Files are downloaded once from raw.pixls.us into the gitignored
//! `tests/regression/cache/` (`CCOON_REGRESSION_CACHE` overrides it) and
//! verified against the manifest's sha256. Each file goes through decode,
//! development and embedded extraction, and through `run_batch` in both modes,
//! each stage behind `catch_unwind`, so a broken file is a row in the table
//! and never stops the run.
//!
//! The outcome is compared with `tests/regression/reference.json` and the
//! small PNGs in `tests/regression/reference/`. Run with
//! `CCOON_REGRESSION_BLESS=1` to rewrite them after an intended change.
//! A failing file listed as `known_unsupported` in the manifest is reported
//! but does not fail the test.
//!
//! Also here: `auto_exposure_holdout` (luminance against the camera JPEG,
//! before and after, per split) and `metadata_report` (tags in real output).

use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::panic::{self, catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use ccoonverter_core::develop::LinearBase;
use ccoonverter_core::preview;
use ccoonverter_core::render::{render, RenderParams};
use ccoonverter_core::{job, resize, ConvertSettings, FileStatus, FileTask};
use image::RgbImage;
use rawler::decoders::RawDecodeParams;
use rawler::rawsource::RawSource;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const VIEW_EDGE: u32 = 1000;
const THUMB_EDGE: u32 = 64;
/// Mean absolute difference (8-bit) allowed against the reference thumbnail.
const THUMB_TOLERANCE: f64 = 6.0;

#[derive(Debug, Deserialize)]
struct Manifest {
    files: Vec<Entry>,
}

#[derive(Debug, Deserialize)]
struct Entry {
    filename: String,
    url: String,
    sha256: String,
    make: String,
    model: String,
    format: String,
    #[serde(default)]
    mode: String,
    split: String,
    /// Why this file cannot be developed; expected failure.
    #[serde(default)]
    known_unsupported: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct Reference {
    width: u32,
    height: u32,
    mean_rgb: [f64; 3],
    embedded: Option<(u32, u32)>,
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn regression_dir() -> PathBuf {
    repo_root().join("tests/regression")
}

fn cache_dir() -> PathBuf {
    std::env::var_os("CCOON_REGRESSION_CACHE")
        .map(PathBuf::from)
        .unwrap_or_else(|| regression_dir().join("cache"))
}

fn load_manifest() -> Manifest {
    let text = fs::read_to_string(regression_dir().join("manifest.json")).expect("manifest");
    serde_json::from_str(&text).expect("valid manifest")
}

fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut f = fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

fn download(url: &str, dest: &Path) -> Result<(), String> {
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(600)))
        .user_agent("CcooNverter-regression-tests")
        .build();
    let agent = ureq::Agent::new_with_config(config);
    let mut body = agent
        .get(url)
        .call()
        .map_err(|e| format!("download failed: {e}"))?
        .into_body();
    let tmp = dest.with_extension("part");
    let mut file = fs::File::create(&tmp).map_err(|e| e.to_string())?;
    std::io::copy(&mut body.as_reader(), &mut file).map_err(|e| e.to_string())?;
    drop(file);
    fs::rename(&tmp, dest).map_err(|e| e.to_string())
}

/// The cached file, downloading it when missing. Every use verifies the hash.
fn ensure_file(entry: &Entry) -> Result<PathBuf, String> {
    let dir = cache_dir();
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(&entry.filename);
    if !path.exists() {
        println!("downloading {} ...", entry.filename);
        download(&entry.url, &path)?;
    }
    let hash = sha256_file(&path).map_err(|e| e.to_string())?;
    if hash != entry.sha256 {
        let _ = fs::remove_file(&path);
        return Err(format!(
            "sha256 mismatch for {}: got {hash}",
            entry.filename
        ));
    }
    Ok(path)
}

fn stage<T>(name: &str, f: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(e)) => Err(format!("{name}: {e}")),
        Err(p) => {
            let msg = p
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| p.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown panic".into());
            Err(format!("{name}: PANIC: {msg}"))
        }
    }
}

fn mean_luma(img: &RgbImage) -> f64 {
    let s: f64 = img
        .pixels()
        .map(|p| 0.299 * p.0[0] as f64 + 0.587 * p.0[1] as f64 + 0.114 * p.0[2] as f64)
        .sum();
    s / (img.width() as f64 * img.height() as f64)
}

fn luma_stddev(img: &RgbImage) -> f64 {
    let m = mean_luma(img);
    let s: f64 = img
        .pixels()
        .map(|p| {
            let l = 0.299 * p.0[0] as f64 + 0.587 * p.0[1] as f64 + 0.114 * p.0[2] as f64;
            (l - m) * (l - m)
        })
        .sum();
    (s / (img.width() as f64 * img.height() as f64)).sqrt()
}

fn mean_rgb(img: &RgbImage) -> [f64; 3] {
    let mut s = [0f64; 3];
    for p in img.pixels() {
        for (c, sum) in s.iter_mut().enumerate() {
            *sum += p.0[c] as f64;
        }
    }
    let n = img.width() as f64 * img.height() as f64;
    s.map(|v| v / n)
}

fn base(path: &Path) -> Result<LinearBase, String> {
    let cancel = AtomicBool::new(false);
    preview::load_base(path, u32::MAX, &cancel).map_err(|e| e.to_string())
}

fn render8(base: LinearBase, params: &RenderParams, long_edge: u32) -> Result<RgbImage, String> {
    let cancel = AtomicBool::new(false);
    let img =
        render(base.image, &base.stats, params, long_edge, &cancel).map_err(|e| e.to_string())?;
    Ok(resize::quantize(&img))
}

fn decode_stage(path: &Path) -> Result<String, String> {
    let source = RawSource::new(path).map_err(|e| e.to_string())?;
    let decoder = rawler::get_decoder(&source).map_err(|e| e.to_string())?;
    let params = RawDecodeParams::default();
    let raw = decoder
        .raw_image(&source, &params, false)
        .map_err(|e| e.to_string())?;
    Ok(format!("{}x{} cpp{}", raw.width, raw.height, raw.cpp))
}

/// One conversion through the batch runner, the path the app really takes.
fn batch_stage(path: &Path, settings: &ConvertSettings) -> Result<String, String> {
    let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
    let root = path.parent().unwrap();
    let task = FileTask::new(path, root, dir.path());
    let cancel = AtomicBool::new(false);
    let summary = job::run_batch(
        std::slice::from_ref(&task),
        settings,
        Some(4),
        None,
        &cancel,
    )
    .map_err(|e| e.to_string())?;
    let r = &summary.results[0];
    match r.status {
        FileStatus::Converted => Ok(format!(
            "{}x{} {:?}",
            r.width.unwrap_or(0),
            r.height.unwrap_or(0),
            r.method
        )),
        _ => Err(r.message.clone().unwrap_or_else(|| "failed".into())),
    }
}

struct RowResult {
    entry_format: String,
    filename: String,
    split: String,
    model: String,
    mode: String,
    status: String,
    decode: String,
    dims: String,
    luma: String,
    stddev: String,
    embedded: String,
    reference: String,
    note: String,
    failed: bool,
}

fn run_entry(entry: &Entry, refs: &BTreeMap<String, Reference>, bless: bool) -> RowResult {
    let mut row = RowResult {
        entry_format: entry.format.clone(),
        filename: entry.filename.clone(),
        split: entry.split.clone(),
        model: format!("{} {}", entry.make, entry.model),
        mode: entry.mode.clone(),
        status: String::new(),
        decode: "-".into(),
        dims: "-".into(),
        luma: "-".into(),
        stddev: "-".into(),
        embedded: "-".into(),
        reference: "-".into(),
        note: String::new(),
        failed: false,
    };
    let path = match stage("download", || ensure_file(entry)) {
        Ok(p) => p,
        Err(e) => {
            row.status = "FAIL".into();
            row.note = e;
            row.failed = true;
            return row;
        }
    };
    let mut problems = Vec::new();

    match stage("decode", || decode_stage(&path)) {
        Ok(s) => row.decode = s,
        Err(e) => {
            row.decode = "error".into();
            problems.push(e);
        }
    }

    let params = RenderParams::from(&ConvertSettings::default());
    let developed = stage("develop", || {
        let b = base(&path)?;
        render8(b, &params, VIEW_EDGE)
    });

    let embedded = stage("embedded", || {
        let cancel = AtomicBool::new(false);
        preview::load_embedded(&path, u32::MAX, &cancel).map_err(|e| e.to_string())
    });
    let embedded_dims = match &embedded {
        Ok(Some(img)) => {
            row.embedded = format!("{}x{}", img.width(), img.height());
            Some((img.width(), img.height()))
        }
        Ok(None) => {
            row.embedded = "none".into();
            None
        }
        Err(e) => {
            row.embedded = "error".into();
            problems.push(e.clone());
            None
        }
    };

    match developed {
        Ok(img) => {
            let (w, h) = img.dimensions();
            row.dims = format!("{w}x{h}");
            let m = mean_luma(&img);
            let sd = luma_stddev(&img);
            row.luma = format!("{m:.1}");
            row.stddev = format!("{sd:.1}");
            if w.max(h) != VIEW_EDGE || w.min(h) < 200 {
                problems.push(format!("implausible dimensions {w}x{h}"));
            }
            if !(1.0..=254.0).contains(&m) {
                problems.push(format!("degenerate mean luma {m:.1}"));
            }
            if sd < 2.0 {
                problems.push(format!("flat image, luma stddev {sd:.2}"));
            }
            let thumb = resize::resize_long_edge(img, THUMB_EDGE);
            let thumb_path = regression_dir()
                .join("reference")
                .join(format!("{}.png", &entry.sha256[..12]));
            if bless {
                fs::create_dir_all(thumb_path.parent().unwrap()).unwrap();
                thumb.save(&thumb_path).unwrap();
            } else {
                match image::open(&thumb_path) {
                    Ok(reference) => {
                        let reference = reference.into_rgb8();
                        if reference.dimensions() != thumb.dimensions() {
                            problems.push("reference thumbnail size differs".into());
                        } else {
                            let total: u64 = reference
                                .as_raw()
                                .iter()
                                .zip(thumb.as_raw())
                                .map(|(a, b)| a.abs_diff(*b) as u64)
                                .sum();
                            let mad = total as f64 / reference.as_raw().len() as f64;
                            row.reference = format!("{mad:.2}");
                            if mad > THUMB_TOLERANCE {
                                problems.push(format!(
                                    "differs from reference thumbnail by {mad:.2} (limit {THUMB_TOLERANCE})"
                                ));
                            }
                        }
                    }
                    Err(_) => problems.push("no reference thumbnail (run with BLESS)".into()),
                }
                if let Some(r) = refs.get(&entry.filename) {
                    if (r.width, r.height) != (w, h) {
                        problems.push(format!(
                            "dimensions changed: reference {}x{}",
                            r.width, r.height
                        ));
                    }
                    if r.embedded != embedded_dims {
                        problems.push(format!(
                            "embedded preview changed: reference {:?}",
                            r.embedded
                        ));
                    }
                }
            }
        }
        Err(e) => problems.push(e),
    }

    // The real entry points. Errors from these must arrive as a clean string.
    if let Err(e) = stage("batch_develop", || {
        batch_stage(&path, &ConvertSettings::default())
    }) {
        problems.push(e);
    }
    if let Err(e) = stage("batch_embedded", || {
        batch_stage(
            &path,
            &ConvertSettings {
                extract_embedded: true,
                preserve_exif: true,
                ..Default::default()
            },
        )
    }) {
        problems.push(e);
    }

    if problems.is_empty() {
        row.status = if entry.known_unsupported.is_some() {
            row.note = "works now: remove known_unsupported".into();
            "PASS (unexpected)".into()
        } else {
            "PASS".into()
        };
    } else if let Some(reason) = &entry.known_unsupported {
        row.status = "KNOWN".into();
        row.note = format!("{reason} [{}]", problems.join("; "));
    } else {
        row.status = "FAIL".into();
        row.failed = true;
        row.note = problems.join("; ");
    }
    row
}

fn reference_entry(entry: &Entry, path: &Path) -> Option<Reference> {
    let params = RenderParams::from(&ConvertSettings::default());
    let img = base(path)
        .ok()
        .and_then(|b| render8(b, &params, VIEW_EDGE).ok())?;
    let cancel = AtomicBool::new(false);
    let embedded = preview::load_embedded(path, u32::MAX, &cancel)
        .ok()
        .flatten()
        .map(|e| e.dimensions());
    let _ = entry;
    Some(Reference {
        width: img.width(),
        height: img.height(),
        mean_rgb: mean_rgb(&img),
        embedded,
    })
}

#[test]
#[ignore]
fn regression_matrix() {
    let bless = std::env::var_os("CCOON_REGRESSION_BLESS").is_some();
    let quiet = std::env::var_os("CCOON_REGRESSION_VERBOSE").is_none();
    if quiet {
        panic::set_hook(Box::new(|_| {}));
    }
    let manifest = load_manifest();
    let ref_path = regression_dir().join("reference.json");
    let refs: BTreeMap<String, Reference> = fs::read_to_string(&ref_path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();

    let mut rows = Vec::new();
    let mut new_refs: BTreeMap<String, Reference> = BTreeMap::new();
    for entry in &manifest.files {
        let row = run_entry(entry, &refs, bless);
        if bless && row.status == "PASS" {
            if let Ok(p) = ensure_file(entry) {
                if let Some(r) = reference_entry(entry, &p) {
                    new_refs.insert(entry.filename.clone(), r);
                }
            }
        }
        println!(
            "{:8} {:4} {:24} {:7} {:12} {:9} {:12} {:7} {:6} {:8} {}",
            row.status,
            row.entry_format,
            truncate(&row.model, 24),
            row.split,
            row.decode,
            row.dims,
            row.embedded,
            row.luma,
            row.stddev,
            row.reference,
            row.note
        );
        rows.push(row);
    }
    let _ = panic::take_hook();

    if bless {
        fs::write(
            &ref_path,
            serde_json::to_string_pretty(&new_refs).expect("serialise") + "\n",
        )
        .unwrap();
    }

    let report = render_report(&rows);
    let out = repo_root().join("target/regression");
    fs::create_dir_all(&out).unwrap();
    fs::write(out.join("report.md"), &report).unwrap();
    println!("\n{report}");

    let failed: Vec<&str> = rows
        .iter()
        .filter(|r| r.failed)
        .map(|r| r.filename.as_str())
        .collect();
    assert!(failed.is_empty(), "unexpected failures: {failed:?}");
}

/// Develop and measure mean luma with the given stats, then compare to the JPEG.
fn luma_with(
    base: &LinearBase,
    stats: ccoonverter_core::develop::BaseStats,
    params: &RenderParams,
) -> Result<f64, String> {
    let cancel = AtomicBool::new(false);
    let img = render(base.image.clone(), &stats, params, VIEW_EDGE, &cancel)
        .map_err(|e| e.to_string())?;
    Ok(mean_luma(&resize::quantize(&img)))
}

#[derive(Default)]
struct AeTally {
    n: usize,
    abs_pct: [f64; 3],
    abs_delta: [f64; 3],
    within_10: [usize; 3],
}

/// Luminance against the camera JPEG: the v0.10 1 % clip (`clip`), the
/// JPEG-free scene model alone (`scene`) and the shipped method (`final`).
#[test]
#[ignore]
fn auto_exposure_holdout() {
    use ccoonverter_core::develop::auto_gain;
    use ccoonverter_core::metering;
    let params = RenderParams::from(&ConvertSettings::default());
    let manifest = load_manifest();
    println!("split,format,file,camera_jpeg,clip,clip_pct,scene,scene_pct,final,final_pct,meter");
    let mut tally: BTreeMap<String, AeTally> = BTreeMap::new();
    for entry in &manifest.files {
        if entry.known_unsupported.is_some() {
            continue;
        }
        let path = match ensure_file(entry) {
            Ok(p) => p,
            Err(e) => {
                println!("{}: {e}", entry.filename);
                continue;
            }
        };
        let cancel = AtomicBool::new(false);
        let Some(reference) = preview::load_embedded(&path, u32::MAX, &cancel)
            .ok()
            .flatten()
            .map(|e| resize::resize_long_edge(e, VIEW_EDGE))
        else {
            println!(
                "{},{},{},none,,,,,,,",
                entry.split, entry.format, entry.filename
            );
            continue;
        };
        let camera = mean_luma(&reference);
        let Ok(base) = base(&path) else { continue };
        let mut clip_stats = base.stats;
        clip_stats.exposure_camera = auto_gain(base.stats.white_camera, 1.0);
        clip_stats.exposure_auto = auto_gain(base.stats.white_auto, 1.0);
        let scene_m = metering::meter(
            &base.image,
            base.stats.white_camera,
            base.stats.white_auto,
            base.stats.auto_gains,
            None,
        );
        let mut scene_stats = base.stats;
        scene_stats.exposure_camera = scene_m.gain_camera;
        scene_stats.exposure_auto = scene_m.gain_auto;
        let lumas = [
            luma_with(&base, clip_stats, &params).unwrap_or(0.0),
            luma_with(&base, scene_stats, &params).unwrap_or(0.0),
            luma_with(&base, base.stats, &params).unwrap_or(0.0),
        ];
        let pct = |v: f64| 100.0 * (v - camera) / camera;
        println!(
            "{},{},{},{camera:.1},{:.1},{:+.1},{:.1},{:+.1},{:.1},{:+.1},{:?}",
            entry.split,
            entry.format,
            entry.filename,
            lumas[0],
            pct(lumas[0]),
            lumas[1],
            pct(lumas[1]),
            lumas[2],
            pct(lumas[2]),
            base.stats.meter_source
        );
        let t = tally.entry(entry.split.clone()).or_default();
        t.n += 1;
        for (i, luma) in lumas.iter().enumerate() {
            t.abs_pct[i] += pct(*luma).abs();
            t.abs_delta[i] += (luma - camera).abs();
            if pct(*luma).abs() <= 10.0 {
                t.within_10[i] += 1;
            }
        }
    }
    println!("\nsplit,n,method,mean_abs_pct,mean_abs_delta_8bit,files_within_10pct");
    for (split, t) in &tally {
        for (i, name) in ["clip_v0.10", "scene_model", "final"].iter().enumerate() {
            println!(
                "{split},{},{name},{:.2},{:.2},{}",
                t.n,
                t.abs_pct[i] / t.n as f64,
                t.abs_delta[i] / t.n as f64,
                t.within_10[i]
            );
        }
    }
}

/// Tags present in real output per file. Tells what each format yields.
#[test]
#[ignore]
fn metadata_report() {
    use ::exif::{In, Reader, Tag};
    let manifest = load_manifest();
    println!("format,camera,mode,camera_tags,gps,lens_model,lens_spec,lens_serial,rating");
    for entry in &manifest.files {
        let Ok(path) = ensure_file(entry) else {
            continue;
        };
        for (label, settings) in [
            (
                "develop",
                ConvertSettings {
                    preserve_exif: true,
                    resize: 800,
                    ..Default::default()
                },
            ),
            (
                "embedded",
                ConvertSettings {
                    preserve_exif: true,
                    extract_embedded: true,
                    resize: 800,
                    ..Default::default()
                },
            ),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let task = FileTask::new(&path, path.parent().unwrap(), dir.path());
            let cancel = AtomicBool::new(false);
            let res = job::run_batch(
                std::slice::from_ref(&task),
                &settings,
                Some(4),
                None,
                &cancel,
            )
            .expect("batch");
            let r = &res.results[0];
            let Some(out) = r.output.as_ref() else {
                println!(
                    "{},{} {},{},failed: {}",
                    entry.format,
                    entry.make,
                    entry.model,
                    label,
                    r.message.clone().unwrap_or_default()
                );
                continue;
            };
            let bytes = fs::read(out).unwrap();
            let tiff = ccoonverter_core::exif::jpeg_exif(&bytes);
            let ex = tiff.and_then(|t| Reader::new().read_raw(t).ok());
            let has = |t: Tag| {
                ex.as_ref()
                    .is_some_and(|e| e.get_field(t, In::PRIMARY).is_some())
            };
            let n = ex.as_ref().map(|e| e.fields().count()).unwrap_or(0);
            let rating = ccoonverter_core::exif::find_xmp_rating(&bytes);
            println!(
                "{},{} {},{},{n},{},{},{},{},{:?}",
                entry.format,
                entry.make,
                entry.model,
                label,
                has(Tag::GPSLatitude),
                has(Tag::LensModel),
                has(Tag::LensSpecification),
                has(Tag::LensSerialNumber),
                rating,
            );
        }
    }
}

fn truncate(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

fn render_report(rows: &[RowResult]) -> String {
    let mut out = String::from(
        "| Status | Format | Camera | Mode | Split | Decoded | Output | Embedded | Mean luma | Luma SD | Ref MAD | Note |\n|---|---|---|---|---|---|---|---|---|---|---|---|\n",
    );
    for r in rows {
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
            r.status,
            r.entry_format,
            r.model,
            r.mode,
            r.split,
            r.decode,
            r.dims,
            r.embedded,
            r.luma,
            r.stddev,
            r.reference,
            r.note.replace('|', "/")
        ));
    }
    let pass = rows.iter().filter(|r| r.status.starts_with("PASS")).count();
    let known = rows.iter().filter(|r| r.status == "KNOWN").count();
    let fail = rows.iter().filter(|r| r.status == "FAIL").count();
    out.push_str(&format!(
        "\n{pass} passed, {known} known-unsupported, {fail} failed, {} total\n",
        rows.len()
    ));
    out
}
