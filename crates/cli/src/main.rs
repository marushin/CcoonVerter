//! `ccoonverter-cli`: headless front-end for `ccoonverter-core`.
//!
//! ```text
//! ccoonverter-cli convert <SOURCE_DIR> [--out DIR] [--resize N] [--quality Q] [--brightness F]
//!                         [--wb camera|auto] [--style NAME] [--rgb R,G,B] [--levels BP,WP]
//!                         [--embedded] [--exif] [--threads N] [--on-conflict skip|overwrite|suffix]
//!                         [--preset NAME] [--json]
//! ccoonverter-cli scan <DIR> [--json]
//! ccoonverter-cli info <RAW_FILE> [--json]
//! ```
//! Exit codes: 0 success, 1 at least one file failed, 2 usage / fatal error.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc;

use anyhow::{bail, Context, Result};
use ccoonverter_core::config;
use ccoonverter_core::job::{self, FileStatus};
use ccoonverter_core::logging::{self, LogOptions};
use ccoonverter_core::paths::AppPaths;
use ccoonverter_core::presets::PresetStore;
use ccoonverter_core::{ConflictPolicy, ConvertSettings, PictureStyle, WhiteBalance};
use clap::{Args, Parser, Subcommand, ValueEnum};
use serde::Serialize;

#[derive(Parser, Debug)]
#[command(
    name = "ccoonverter-cli",
    version,
    about = "CcooNverter command line: batch RAW (ARW/CR2/CR3/DNG/NEF) to JPEG"
)]
struct Cli {
    /// Verbose logging to stderr (debug level).
    #[arg(short, long, global = true)]
    verbose: bool,
    /// Also write the log to this file.
    #[arg(long, global = true, value_name = "PATH")]
    log_file: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Convert every RAW file below SOURCE_DIR (sub-folders preserved).
    Convert(ConvertArgs),
    /// List the RAW files that `convert` would process.
    Scan {
        dir: PathBuf,
        /// Print JSON instead of one path per line.
        #[arg(long)]
        json: bool,
    },
    /// Show decoder, size, embedded JPEG and EXIF information for one RAW file.
    Info {
        file: PathBuf,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum WbArg {
    Camera,
    Auto,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum ConflictArg {
    Skip,
    Overwrite,
    Suffix,
}

#[derive(Args, Debug)]
struct ConvertArgs {
    /// Folder containing RAW files (searched recursively).
    source: PathBuf,
    /// Output folder (default: <SOURCE_DIR>/Converted_JPG).
    #[arg(short, long, value_name = "DIR")]
    out: Option<PathBuf>,
    /// Start from a preset saved by the GUI (other flags override it).
    #[arg(long, value_name = "NAME")]
    preset: Option<String>,
    /// Long edge in pixels, 0 = original size.
    #[arg(long, value_name = "N")]
    resize: Option<u32>,
    /// JPEG quality 1-100 (default 90).
    #[arg(short, long, value_name = "Q")]
    quality: Option<u8>,
    /// Brightness multiplier 0.0-2.0 (default 1.0).
    #[arg(long, value_name = "F")]
    brightness: Option<f32>,
    /// White balance.
    #[arg(long, value_enum)]
    wb: Option<WbArg>,
    /// Picture style: Standard, Portrait, Landscape, Neutral, Vivid, Monochrome,
    /// Sepia, Cinematic, HDR, Soft, Dramatic, "Black & White" (case-insensitive; bw ok).
    #[arg(long, value_name = "NAME", value_parser = parse_style)]
    style: Option<PictureStyle>,
    /// RGB channel multipliers, each 0.0-2.0, e.g. 1.0,1.05,0.95.
    #[arg(long, value_name = "R,G,B", value_parser = parse_rgb)]
    rgb: Option<[f32; 3]>,
    /// Levels black and white point on the 8-bit scale, e.g. 10,245.
    #[arg(long, value_name = "BP,WP", value_parser = parse_levels)]
    levels: Option<(u8, u8)>,
    /// Use the camera's embedded JPEG instead of developing the RAW (fast).
    #[arg(long)]
    embedded: bool,
    /// Copy EXIF metadata into the JPEG.
    #[arg(long)]
    exif: bool,
    /// What to do when the JPEG already exists (default: suffix).
    #[arg(long, value_enum)]
    on_conflict: Option<ConflictArg>,
    /// Worker threads (default: all cores for --embedded, at most 4 otherwise).
    #[arg(short = 'j', long, value_name = "N")]
    threads: Option<usize>,
    /// Print the summary as JSON on stdout.
    #[arg(long)]
    json: bool,
}

fn parse_style(s: &str) -> std::result::Result<PictureStyle, String> {
    s.parse()
}

fn parse_floats(s: &str) -> std::result::Result<Vec<f32>, String> {
    s.split(',')
        .map(|p| {
            p.trim()
                .parse::<f32>()
                .map_err(|_| format!("'{p}' is not a number"))
        })
        .collect()
}

fn parse_rgb(s: &str) -> std::result::Result<[f32; 3], String> {
    let v = parse_floats(s)?;
    match v.as_slice() {
        [r, g, b] => Ok([*r, *g, *b]),
        _ => Err("expected three comma-separated values R,G,B".into()),
    }
}

fn parse_levels(s: &str) -> std::result::Result<(u8, u8), String> {
    let parts: Vec<&str> = s.split(',').map(str::trim).collect();
    match parts.as_slice() {
        [bp, wp] => Ok((
            bp.parse().map_err(|_| format!("'{bp}' is not 0-255"))?,
            wp.parse().map_err(|_| format!("'{wp}' is not 0-255"))?,
        )),
        _ => Err("expected BP,WP".into()),
    }
}

impl ConvertArgs {
    fn settings(&self) -> Result<ConvertSettings> {
        let mut s = match &self.preset {
            Some(name) => {
                let paths = AppPaths::resolve();
                let store = PresetStore::load(&paths.presets_file());
                store.get(name).cloned().with_context(|| {
                    format!(
                        "preset '{name}' not found in {} (available: {})",
                        paths.presets_file().display(),
                        store.names().join(", ")
                    )
                })?
            }
            None => ConvertSettings::default(),
        };
        if let Some(v) = self.resize {
            s.resize = v;
        }
        if let Some(v) = self.quality {
            s.quality = v;
        }
        if let Some(v) = self.brightness {
            s.brightness = v;
        }
        if let Some(v) = self.wb {
            s.white_balance = match v {
                WbArg::Camera => WhiteBalance::CameraDefault,
                WbArg::Auto => WhiteBalance::Auto,
            };
        }
        if let Some(v) = self.style {
            s.style = v;
        }
        if let Some([r, g, b]) = self.rgb {
            s.rgb_red = r;
            s.rgb_green = g;
            s.rgb_blue = b;
        }
        if let Some((bp, wp)) = self.levels {
            s.black_point = bp;
            s.white_point = wp;
        }
        if self.embedded {
            s.extract_embedded = true;
        }
        if self.exif {
            s.preserve_exif = true;
        }
        if let Some(v) = self.on_conflict {
            s.on_conflict = match v {
                ConflictArg::Skip => ConflictPolicy::Skip,
                ConflictArg::Overwrite => ConflictPolicy::Overwrite,
                ConflictArg::Suffix => ConflictPolicy::Suffix,
            };
        }
        if let Err(e) = s.validate() {
            bail!("invalid settings: {e}");
        }
        Ok(s)
    }
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| p.display().to_string())
}

fn cmd_convert(args: &ConvertArgs) -> Result<ExitCode> {
    let settings = args.settings()?;
    if !args.source.is_dir() {
        bail!("source folder not found: {}", args.source.display());
    }
    let (tasks, out_root) = job::prepare(&args.source, args.out.as_deref())
        .with_context(|| format!("cannot scan {}", args.source.display()))?;
    if tasks.is_empty() {
        if args.json {
            println!(
                "{}",
                serde_json::json!({"total": 0, "converted": 0, "skipped": 0, "failed": 0,
                    "cancelled": 0, "results": []})
            );
        } else {
            eprintln!("No RAW files found in {}", args.source.display());
        }
        return Ok(ExitCode::SUCCESS);
    }
    if !args.json {
        eprintln!(
            "Converting {} file(s) -> {} [{}, style {}, wb {}, quality {}, resize {}]",
            tasks.len(),
            out_root.display(),
            if settings.extract_embedded {
                "embedded JPEG"
            } else {
                "RAW develop"
            },
            settings.style,
            settings.white_balance,
            settings.quality,
            settings.resize
        );
    }

    let (tx, rx) = mpsc::channel::<job::Progress>();
    let quiet = args.json;
    let printer = std::thread::spawn(move || {
        for p in rx {
            if quiet {
                continue;
            }
            let r = &p.last;
            let what = match r.status {
                FileStatus::Converted => format!(
                    "OK    {} -> {} ({}x{}{})",
                    file_name(&r.source),
                    r.output.as_deref().map(file_name).unwrap_or_default(),
                    r.width.unwrap_or(0),
                    r.height.unwrap_or(0),
                    r.method
                        .map(|m| format!(", {}", m.as_str()))
                        .unwrap_or_default()
                ),
                FileStatus::Skipped => format!("SKIP  {} (output exists)", file_name(&r.source)),
                FileStatus::Failed => format!(
                    "FAIL  {}: {}",
                    file_name(&r.source),
                    r.message.as_deref().unwrap_or("error")
                ),
                FileStatus::Cancelled => format!("STOP  {}", file_name(&r.source)),
            };
            let eta = p
                .eta()
                .map(|d| format!(" ETA {:02}:{:02}", d.as_secs() / 60, d.as_secs() % 60))
                .unwrap_or_default();
            eprintln!("[{}/{}]{eta} {what}", p.done, p.total);
        }
    });

    let cancel = AtomicBool::new(false);
    let summary = job::run_batch(&tasks, &settings, args.threads, Some(tx), &cancel)?;
    let _ = printer.join();

    if args.json {
        println!("{}", serde_json::to_string_pretty(&summary)?);
    } else {
        println!(
            "Done in {:.1}s ({} threads): {} converted, {} skipped, {} failed",
            summary.elapsed_secs,
            summary.threads,
            summary.converted,
            summary.skipped,
            summary.failed
        );
        println!("Output: {}", out_root.display());
    }
    Ok(if summary.failed > 0 {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    })
}

fn cmd_scan(dir: &Path, json: bool) -> Result<ExitCode> {
    let files = ccoonverter_core::scan::scan_folder(dir)
        .with_context(|| format!("cannot scan {}", dir.display()))?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "dir": dir, "count": files.len(), "files": files
            }))?
        );
    } else {
        for f in &files {
            println!("{}", f.display());
        }
        eprintln!("{} RAW file(s)", files.len());
    }
    Ok(ExitCode::SUCCESS)
}

#[derive(Serialize)]
struct EmbeddedInfo {
    offset: usize,
    bytes: usize,
    width: u32,
    height: u32,
    has_exif: bool,
    exif_orientation: Option<u16>,
}

#[derive(Serialize)]
struct RawInfo {
    file: PathBuf,
    file_size: u64,
    decoder: Option<String>,
    decoder_error: Option<String>,
    make: Option<String>,
    model: Option<String>,
    raw_width: Option<usize>,
    raw_height: Option<usize>,
    components: Option<usize>,
    bits_per_sample: Option<usize>,
    orientation: Option<u16>,
    wb_coeffs: Option<[f32; 4]>,
    exif: Option<ccoonverter_core::exif::ExifInfo>,
    embedded_jpegs: Vec<EmbeddedInfo>,
}

fn cmd_info(file: &Path, json: bool) -> Result<ExitCode> {
    use rawler_api::*;
    let source = RawSource::new(file).with_context(|| format!("cannot open {}", file.display()))?;
    let buf = source.buf();
    let mut info = RawInfo {
        file: file.to_path_buf(),
        file_size: buf.len() as u64,
        decoder: None,
        decoder_error: None,
        make: None,
        model: None,
        raw_width: None,
        raw_height: None,
        components: None,
        bits_per_sample: None,
        orientation: None,
        wb_coeffs: None,
        exif: None,
        embedded_jpegs: ccoonverter_core::embedded::find_jpegs(buf)
            .into_iter()
            .map(|c| {
                let exif = ccoonverter_core::exif::jpeg_exif(c.bytes(buf));
                EmbeddedInfo {
                    offset: c.offset,
                    bytes: c.len,
                    width: c.width,
                    height: c.height,
                    has_exif: exif.is_some(),
                    exif_orientation: exif
                        .as_deref()
                        .and_then(ccoonverter_core::exif::tiff_orientation),
                }
            })
            .collect(),
    };
    match get_decoder(&source) {
        Ok(dec) => {
            info.decoder = Some(format!("{:?}", dec.format_hint()));
            let params = RawDecodeParams::default();
            match dec.raw_image(&source, &params, true) {
                Ok(raw) => {
                    info.make = Some(raw.clean_make.clone());
                    info.model = Some(raw.clean_model.clone());
                    info.raw_width = Some(raw.width);
                    info.raw_height = Some(raw.height);
                    info.components = Some(raw.cpp);
                    info.bits_per_sample = Some(raw.bps);
                    info.orientation = Some(raw.orientation.to_u16());
                    info.wb_coeffs = Some(raw.wb_coeffs);
                }
                Err(e) => info.decoder_error = Some(e.to_string()),
            }
            match dec.raw_metadata(&source, &params) {
                Ok(md) => info.exif = Some(ccoonverter_core::exif::ExifInfo::from_rawler(&md)),
                Err(e) => {
                    info.decoder_error.get_or_insert_with(|| e.to_string());
                }
            }
        }
        Err(e) => info.decoder_error = Some(e.to_string()),
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&info)?);
    } else {
        let opt = |v: &Option<String>| v.clone().unwrap_or_else(|| "-".into());
        println!(
            "File:        {} ({} bytes)",
            info.file.display(),
            info.file_size
        );
        println!("Decoder:     {}", opt(&info.decoder));
        if let Some(e) = &info.decoder_error {
            println!("Decode note: {e}");
        }
        println!("Camera:      {} {}", opt(&info.make), opt(&info.model));
        if let (Some(w), Some(h)) = (info.raw_width, info.raw_height) {
            println!(
                "Raw size:    {w}x{h}, {} component(s), {} bps",
                info.components.unwrap_or(0),
                info.bits_per_sample.unwrap_or(0)
            );
        }
        if let Some(o) = info.orientation {
            println!("Orientation: {o}");
        }
        if let Some(wb) = info.wb_coeffs {
            println!("WB coeffs:   {wb:?}");
        }
        if let Some(e) = &info.exif {
            let rat = |r: Option<(u32, u32)>| {
                r.map(|(n, d)| format!("{n}/{d}"))
                    .unwrap_or_else(|| "-".into())
            };
            println!("Date:        {}", opt(&e.date_time_original));
            println!(
                "Exposure:    {} s, f/{}, ISO {}, {} mm",
                rat(e.exposure_time),
                e.fnumber
                    .map(|(n, d)| format!("{:.1}", n as f64 / d as f64))
                    .unwrap_or_else(|| "-".into()),
                e.iso.map(|v| v.to_string()).unwrap_or_else(|| "-".into()),
                e.focal_length
                    .map(|(n, d)| format!("{:.0}", n as f64 / d as f64))
                    .unwrap_or_else(|| "-".into())
            );
            println!("Lens:        {}", opt(&e.lens_model));
        }
        if info.embedded_jpegs.is_empty() {
            println!("Embedded:    none found by scanner");
        }
        for (i, j) in info.embedded_jpegs.iter().enumerate() {
            println!(
                "Embedded #{i}: {}x{} JPEG, {} bytes at offset {}{}",
                j.width,
                j.height,
                j.bytes,
                j.offset,
                if j.has_exif { ", has EXIF" } else { "" }
            );
        }
    }
    Ok(
        if info.decoder.is_some() || !info.embedded_jpegs.is_empty() {
            ExitCode::SUCCESS
        } else {
            ExitCode::from(1)
        },
    )
}

/// The handful of rawler items `info` needs, re-exported via the core crate's dependency.
mod rawler_api {
    pub use ccoonverter_core::rawler::decoders::RawDecodeParams;
    pub use ccoonverter_core::rawler::get_decoder;
    pub use ccoonverter_core::rawler::rawsource::RawSource;
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let opts = LogOptions::new(cli.log_file.clone(), cli.verbose);
    if let Err(e) = logging::init(&opts) {
        eprintln!("warning: logging disabled: {e}");
    }
    log::debug!("{} {}", config::APP_NAME, config::APP_VERSION);
    let res = match &cli.command {
        Command::Convert(a) => cmd_convert(a),
        Command::Scan { dir, json } => cmd_scan(dir, *json),
        Command::Info { file, json } => cmd_info(file, *json),
    };
    match res {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::from(2)
        }
    }
}
