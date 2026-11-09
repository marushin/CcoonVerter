//! End-to-end tests on a synthetic DNG written by `testutil::synthetic_dng`.
//! These exercise the real rawler decode + develop path without camera files.

use std::path::Path;
use std::sync::atomic::AtomicBool;

use ccoonverter_core::testutil::{synthetic_dng, DngSpec};
use ccoonverter_core::{
    convert_one, ConflictPolicy, ConvertOutcome, ConvertSettings, FileTask, Method, PictureStyle,
    WhiteBalance,
};

fn write_dng(dir: &Path, name: &str, spec: &DngSpec) -> std::path::PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, synthetic_dng(spec)).unwrap();
    p
}

fn run(src: &Path, root: &Path, s: &ConvertSettings) -> (std::path::PathBuf, Method, u32, u32) {
    let t = FileTask::new(src, root, &root.join("out"));
    let cancel = AtomicBool::new(false);
    match convert_one(&t, s, &cancel).unwrap() {
        ConvertOutcome::Converted {
            output,
            method,
            width,
            height,
            ..
        } => (output, method, width, height),
        o => panic!("{o:?}"),
    }
}

fn mean_rgb(img: &image::RgbImage, x0: u32, x1: u32, y0: u32, y1: u32) -> [f64; 3] {
    let mut s = [0f64; 3];
    let mut n = 0f64;
    for y in y0..y1 {
        for x in x0..x1 {
            let p = img.get_pixel(x, y).0;
            for c in 0..3 {
                s[c] += p[c] as f64;
            }
            n += 1.0;
        }
    }
    s.map(|v| v / n)
}

#[test]
fn develop_path_produces_plausible_colours() {
    let dir = tempfile::tempdir().unwrap();
    let src = write_dng(dir.path(), "a.DNG", &DngSpec::default());
    let s = ConvertSettings {
        on_conflict: ConflictPolicy::Overwrite,
        ..Default::default()
    };
    let (out, method, w, h) = run(&src, dir.path(), &s);
    assert_eq!(method, Method::Developed);
    assert_eq!((w, h), (64, 48));
    let img = image::open(&out).unwrap().into_rgb8();
    // Red patch (top right) must be red, blue patch (bottom right) blue.
    let red = mean_rgb(&img, 40, 60, 4, 20);
    let blue = mean_rgb(&img, 40, 60, 28, 44);
    assert!(
        red[0] > red[1] + 60.0 && red[0] > red[2] + 60.0,
        "red {red:?}"
    );
    assert!(
        blue[2] > blue[0] + 60.0 && blue[2] > blue[1] + 30.0,
        "blue {blue:?}"
    );
    // Left gradient is roughly neutral (camera WB applied).
    let grey = mean_rgb(&img, 20, 28, 10, 38);
    assert!(
        (grey[0] - grey[1]).abs() < 25.0 && (grey[2] - grey[1]).abs() < 25.0,
        "grey {grey:?}"
    );
    // Auto-exposure: the frame is not dark.
    let all = mean_rgb(&img, 0, 64, 0, 48);
    assert!(all.iter().sum::<f64>() / 3.0 > 60.0, "mean {all:?}");
}

#[test]
fn every_style_auto_wb_resize_and_exif() {
    let dir = tempfile::tempdir().unwrap();
    let src = write_dng(dir.path(), "b.DNG", &DngSpec::default());
    let mut means = Vec::new();
    for style in PictureStyle::ALL {
        let s = ConvertSettings {
            style,
            white_balance: WhiteBalance::Auto,
            resize: 32,
            preserve_exif: true,
            on_conflict: ConflictPolicy::Overwrite,
            ..Default::default()
        };
        let (out, _, w, h) = run(&src, dir.path(), &s);
        assert_eq!((w, h), (32, 24), "{style}");
        let bytes = std::fs::read(&out).unwrap();
        let tiff = ccoonverter_core::exif::jpeg_exif(&bytes).expect("exif written");
        let ex = exif::Reader::new().read_raw(tiff).unwrap();
        let make = ex.get_field(exif::Tag::Make, exif::In::PRIMARY).unwrap();
        assert!(make.display_value().to_string().contains("CcooNverter"));
        let iso = ex
            .get_field(exif::Tag::PhotographicSensitivity, exif::In::PRIMARY)
            .unwrap();
        assert_eq!(iso.value.get_uint(0), Some(200));
        let img = image::load_from_memory(&bytes).unwrap().into_rgb8();
        means.push((style, mean_rgb(&img, 0, 32, 0, 24)));
    }
    // Grayscale styles give neutral pixels, Sepia is warm.
    for (style, m) in &means {
        match style {
            PictureStyle::BlackAndWhite | PictureStyle::Monochrome => {
                assert!((m[0] - m[2]).abs() < 3.0, "{style} {m:?}")
            }
            PictureStyle::Sepia => assert!(m[0] > m[2] + 30.0, "{style} {m:?}"),
            _ => {}
        }
    }
}

#[test]
fn orientation_is_applied_in_both_paths() {
    let dir = tempfile::tempdir().unwrap();
    let spec = DngSpec {
        orientation: 6,
        ..Default::default()
    };
    let src = write_dng(dir.path(), "r.DNG", &spec);
    // Develop: pixels rotated, EXIF orientation 1.
    let s = ConvertSettings {
        preserve_exif: true,
        on_conflict: ConflictPolicy::Overwrite,
        ..Default::default()
    };
    let (out, _, w, h) = run(&src, dir.path(), &s);
    assert_eq!((w, h), (48, 64));
    let tiff = ccoonverter_core::exif::jpeg_exif(&std::fs::read(&out).unwrap()).unwrap();
    assert_eq!(ccoonverter_core::exif::tiff_orientation(&tiff), Some(1));

    // Embedded copy: bytes kept, orientation 6 written into EXIF.
    let s = ConvertSettings {
        extract_embedded: true,
        preserve_exif: false,
        on_conflict: ConflictPolicy::Overwrite,
        ..Default::default()
    };
    let (out, method, w, h) = run(&src, dir.path(), &s);
    assert_eq!(method, Method::EmbeddedCopy);
    assert_eq!((w, h), (24, 32));
    let tiff = ccoonverter_core::exif::jpeg_exif(&std::fs::read(&out).unwrap()).unwrap();
    assert_eq!(ccoonverter_core::exif::tiff_orientation(&tiff), Some(6));

    // Embedded re-encode: pixels rotated.
    let s = ConvertSettings {
        extract_embedded: true,
        resize: 16,
        on_conflict: ConflictPolicy::Overwrite,
        ..Default::default()
    };
    let (out, method, w, h) = run(&src, dir.path(), &s);
    assert_eq!(method, Method::EmbeddedReencoded);
    assert_eq!((w, h), (12, 16));
    let img = image::open(out).unwrap();
    assert_eq!((img.width(), img.height()), (12, 16));
}

#[test]
fn brightness_and_levels_change_output() {
    let dir = tempfile::tempdir().unwrap();
    let src = write_dng(dir.path(), "c.DNG", &DngSpec::default());
    let base = ConvertSettings {
        on_conflict: ConflictPolicy::Overwrite,
        ..Default::default()
    };
    let lum = |s: &ConvertSettings| {
        let (out, ..) = run(&src, dir.path(), s);
        let img = image::open(out).unwrap().into_rgb8();
        mean_rgb(&img, 0, 64, 0, 48).iter().sum::<f64>() / 3.0
    };
    let normal = lum(&base);
    let dark = lum(&ConvertSettings {
        brightness: 0.5,
        ..base.clone()
    });
    let crushed = lum(&ConvertSettings {
        black_point: 80,
        white_point: 160,
        ..base.clone()
    });
    assert!(dark < normal - 20.0, "{dark} vs {normal}");
    assert!((crushed - normal).abs() > 5.0, "{crushed} vs {normal}");
}

/// Real camera files: put them in `<repo>/tests/samples/` (gitignored).
/// Skips cleanly when the folder is missing or empty.
#[test]
fn real_samples_if_present() {
    let samples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/samples");
    let files = match ccoonverter_core::scan::scan_folder(&samples) {
        Ok(f) if !f.is_empty() => f,
        _ => {
            eprintln!("tests/samples not present: skipping real RAW tests");
            return;
        }
    };
    let out = tempfile::tempdir().unwrap();
    for embedded in [true, false] {
        let s = ConvertSettings {
            extract_embedded: embedded,
            resize: 800,
            preserve_exif: true,
            style: PictureStyle::Vivid,
            on_conflict: ConflictPolicy::Overwrite,
            ..Default::default()
        };
        let tasks = ccoonverter_core::plan_tasks(&files, &samples, out.path());
        let cancel = AtomicBool::new(false);
        let summary = ccoonverter_core::run_batch(&tasks, &s, None, None, &cancel).unwrap();
        for r in &summary.results {
            eprintln!(
                "{:?} embedded={embedded}: {:?} {:?}",
                r.source, r.status, r.message
            );
        }
        assert_eq!(summary.failed, 0, "embedded={embedded}");
    }
}
