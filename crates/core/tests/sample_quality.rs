//! Manual measurements on real camera files in `tests/samples/` (gitignored).
//!
//! ```text
//! $env:CCOON_QUALITY_OUT = "...\out"; $env:CCOON_BASELINE_DIR = "...\v090_dev"
//! cargo test --release -p ccoonverter-core --test sample_quality -- --ignored --nocapture
//! ```
//! `compare_with_embedded` prints mean luminance against the camera JPEG (and
//! against a previous build's output in CCOON_BASELINE_DIR, when given) and
//! writes 1000 px JPEGs for visual comparison into CCOON_QUALITY_OUT.
//! `preview_timing` measures the adjustment chain at preview resolution.
//! `preview_matches_batch` compares the preview chain with the batch chain.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use ccoonverter_core::develop::LinearBase;
use ccoonverter_core::preview::{self, PREVIEW_LONG_EDGE};
use ccoonverter_core::render::{render, RenderParams};
use ccoonverter_core::{encode, resize, ConvertSettings, PictureStyle, WhiteBalance};
use image::RgbImage;

const VIEW_EDGE: u32 = 1000;

fn samples() -> Vec<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/samples");
    ccoonverter_core::scan::scan_folder(&dir).unwrap_or_default()
}

fn mean_luma(img: &RgbImage) -> f64 {
    let s: f64 = img
        .pixels()
        .map(|p| 0.299 * p.0[0] as f64 + 0.587 * p.0[1] as f64 + 0.114 * p.0[2] as f64)
        .sum();
    s / (img.width() * img.height()) as f64
}

fn full_base(p: &Path) -> LinearBase {
    let cancel = AtomicBool::new(false);
    preview::load_base(p, u32::MAX, &cancel).unwrap()
}

fn render8(base: LinearBase, params: &RenderParams, long_edge: u32) -> RgbImage {
    let cancel = AtomicBool::new(false);
    let img = render(base.image, &base.stats, params, long_edge, &cancel).unwrap();
    resize::quantize(&img)
}

#[test]
#[ignore]
fn compare_with_embedded() {
    let out = std::env::var_os("CCOON_QUALITY_OUT").map(PathBuf::from);
    let baseline = std::env::var_os("CCOON_BASELINE_DIR").map(PathBuf::from);
    if let Some(o) = &out {
        std::fs::create_dir_all(o).unwrap();
    }
    let params = RenderParams::from(&ConvertSettings::default());
    println!("sample,new_luma,embedded_luma,diff_new,diff_pct,old_luma,diff_old,diff_old_pct");
    for f in samples() {
        let stem = f.file_stem().unwrap().to_string_lossy().into_owned();
        let new_img = render8(full_base(&f), &params, VIEW_EDGE);
        let cancel = AtomicBool::new(false);
        let embedded = preview::load_embedded(&f, u32::MAX, &cancel)
            .unwrap()
            .map(|e| resize::resize_long_edge(e, VIEW_EDGE));
        if let Some(o) = &out {
            let dst = o.join(format!("{stem}_new.jpg"));
            std::fs::write(dst, encode::encode_rgb8(&new_img, 92, None).unwrap()).unwrap();
            if let Some(e) = &embedded {
                let dst = o.join(format!("{stem}_camera.jpg"));
                std::fs::write(dst, encode::encode_rgb8(e, 92, None).unwrap()).unwrap();
            }
        }
        let new_luma = mean_luma(&new_img);
        let old_luma = baseline.as_ref().and_then(|d| {
            image::open(d.join(format!("{stem}.jpg")))
                .ok()
                .map(|i| mean_luma(&i.into_rgb8()))
        });
        let (emb_luma, diff_new, pct_new) = match &embedded {
            Some(e) => {
                let v = mean_luma(e);
                (
                    format!("{v:.2}"),
                    format!("{:.2}", new_luma - v),
                    format!("{:.1}", 100.0 * (new_luma - v) / v),
                )
            }
            None => ("-".into(), "-".into(), "-".into()),
        };
        let (old_s, diff_old, pct_old) = match (old_luma, embedded.as_ref().map(mean_luma)) {
            (Some(o), Some(e)) => (
                format!("{o:.2}"),
                format!("{:.2}", o - e),
                format!("{:.1}", 100.0 * (o - e) / e),
            ),
            (Some(o), None) => (format!("{o:.2}"), "-".into(), "-".into()),
            _ => ("-".into(), "-".into(), "-".into()),
        };
        println!(
            "{stem},{new_luma:.2},{emb_luma},{diff_new},{pct_new},{old_s},{diff_old},{pct_old}"
        );
    }
}

#[test]
#[ignore]
fn preview_timing() {
    let cancel = AtomicBool::new(false);
    println!("sample,base_load_ms,update_ms_min,update_ms_mean,update_ms_max,updates");
    let variants: Vec<ConvertSettings> = vec![
        ConvertSettings::default(),
        ConvertSettings {
            exposure_ev: 0.7,
            ..Default::default()
        },
        ConvertSettings {
            white_balance: WhiteBalance::Auto,
            style: PictureStyle::Landscape,
            ..Default::default()
        },
        ConvertSettings {
            style: PictureStyle::Hdr,
            black_point: 8,
            white_point: 245,
            rgb_red: 1.1,
            ..Default::default()
        },
        ConvertSettings {
            tone_curve: false,
            style: PictureStyle::Dramatic,
            exposure_ev: -0.5,
            ..Default::default()
        },
    ];
    for f in samples() {
        let t0 = Instant::now();
        let base = preview::load_base(&f, PREVIEW_LONG_EDGE, &cancel).unwrap();
        let load_ms = t0.elapsed().as_secs_f64() * 1000.0;
        let mut times = Vec::new();
        for round in 0..6 {
            for s in &variants {
                let t = Instant::now();
                let img = render(
                    base.image.clone(),
                    &base.stats,
                    &RenderParams::from(s),
                    0,
                    &cancel,
                )
                .unwrap();
                let frame = resize::quantize(&img);
                std::hint::black_box(&frame);
                if round > 0 {
                    times.push(t.elapsed().as_secs_f64() * 1000.0);
                }
            }
        }
        let (mn, mx) = times
            .iter()
            .fold((f64::MAX, 0f64), |(a, b), v| (a.min(*v), b.max(*v)));
        let mean = times.iter().sum::<f64>() / times.len() as f64;
        println!(
            "{},{load_ms:.0},{mn:.1},{mean:.1},{mx:.1},{}",
            f.file_stem().unwrap().to_string_lossy(),
            times.len()
        );
    }
}

#[test]
#[ignore]
fn preview_matches_batch() {
    let cancel = AtomicBool::new(false);
    let params = RenderParams::from(&ConvertSettings {
        style: PictureStyle::Landscape,
        exposure_ev: 0.3,
        ..Default::default()
    });
    println!("sample,mean_abs_diff_8bit,mean_luma_preview,mean_luma_batch");
    for f in samples() {
        let preview_base = preview::load_base(&f, PREVIEW_LONG_EDGE, &cancel).unwrap();
        let preview_img = render8(preview_base, &params, 0);
        let batch = render8(full_base(&f), &params, PREVIEW_LONG_EDGE);
        let batch = if batch.dimensions() == preview_img.dimensions() {
            batch
        } else {
            image::imageops::resize(
                &batch,
                preview_img.width(),
                preview_img.height(),
                image::imageops::FilterType::Triangle,
            )
        };
        let sum: u64 = preview_img
            .as_raw()
            .iter()
            .zip(batch.as_raw())
            .map(|(a, b)| a.abs_diff(*b) as u64)
            .sum();
        println!(
            "{},{:.2},{:.2},{:.2}",
            f.file_stem().unwrap().to_string_lossy(),
            sum as f64 / preview_img.as_raw().len() as f64,
            mean_luma(&preview_img),
            mean_luma(&batch)
        );
    }
}
