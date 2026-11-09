//! Black-box tests of the `ccoonverter-cli` binary on synthetic DNG files.

use std::path::Path;
use std::process::Command;

use ccoonverter_core::testutil::{synthetic_dng, DngSpec};

fn cli() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_ccoonverter-cli"));
    // Never touch the real user directories from tests.
    c.env(
        "CCOONVERTER_HOME",
        std::env::temp_dir().join("ccoonverter-cli-test-home"),
    );
    c
}

fn make_source(dir: &Path, with_broken: bool) {
    std::fs::create_dir_all(dir.join("sub 폴더")).unwrap();
    std::fs::write(dir.join("one.DNG"), synthetic_dng(&DngSpec::default())).unwrap();
    std::fs::write(
        dir.join("sub 폴더").join("two.dng"),
        synthetic_dng(&DngSpec {
            orientation: 8,
            ..Default::default()
        }),
    )
    .unwrap();
    if with_broken {
        std::fs::write(dir.join("broken.NEF"), [0u8; 512]).unwrap();
    }
}

#[test]
fn convert_json_success_and_structure() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    make_source(&src, false);
    let out = tmp.path().join("out");
    let o = cli()
        .args(["convert"])
        .arg(&src)
        .arg("--out")
        .arg(&out)
        .args([
            "--style",
            "black & white",
            "--resize",
            "32",
            "--exif",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["total"], 2);
    assert_eq!(v["converted"], 2);
    assert_eq!(v["failed"], 0);
    assert!(out.join("one.jpg").is_file());
    let two = image::open(out.join("sub 폴더").join("two.jpg")).unwrap();
    // orientation 8 -> rotated, long edge 32
    assert_eq!((two.width(), two.height()), (24, 32));

    // Second run with default suffix policy creates *_dng.jpg.
    let o = cli()
        .arg("convert")
        .arg(&src)
        .arg("--out")
        .arg(&out)
        .args(["--embedded", "--json"])
        .output()
        .unwrap();
    assert!(o.status.success());
    assert!(out.join("one_dng.jpg").is_file());

    // Skip policy leaves everything alone.
    let o = cli()
        .arg("convert")
        .arg(&src)
        .arg("--out")
        .arg(&out)
        .args(["--on-conflict", "skip", "--json"])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["skipped"], 2);
}

#[test]
fn convert_exit_code_nonzero_on_failure() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    make_source(&src, true);
    let o = cli()
        .arg("convert")
        .arg(&src)
        .args([
            "--wb",
            "auto",
            "--style",
            "Vivid",
            "--rgb",
            "1.1,1,0.9",
            "--levels",
            "5,250",
        ])
        .output()
        .unwrap();
    assert_eq!(
        o.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    let stdout = String::from_utf8_lossy(&o.stdout);
    assert!(stdout.contains("2 converted"), "{stdout}");
    assert!(stdout.contains("1 failed"), "{stdout}");
    // Default output folder.
    assert!(src.join("Converted_JPG").join("one.jpg").is_file());
}

#[test]
fn invalid_arguments_exit_2() {
    let tmp = tempfile::tempdir().unwrap();
    let o = cli()
        .arg("convert")
        .arg(tmp.path())
        .args(["--quality", "0"])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&o.stderr).contains("Quality"));
    let o = cli()
        .arg("convert")
        .arg(tmp.path().join("missing"))
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(2));
}

#[test]
fn scan_and_info() {
    let tmp = tempfile::tempdir().unwrap();
    make_source(tmp.path(), true);
    let o = cli()
        .arg("scan")
        .arg(tmp.path())
        .arg("--json")
        .output()
        .unwrap();
    assert!(o.status.success());
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["count"], 3);

    let o = cli()
        .arg("info")
        .arg(tmp.path().join("one.DNG"))
        .arg("--json")
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["decoder"], "DNG");
    assert_eq!(v["raw_width"], 64);
    assert_eq!(v["exif"]["iso"], 200);
    assert_eq!(v["embedded_jpegs"][0]["width"], 32);

    let o = cli()
        .arg("info")
        .arg(tmp.path().join("one.DNG"))
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&o.stdout);
    assert!(text.contains("Synthetic DNG"), "{text}");
}

#[test]
fn format_extension_template_and_camera_jpeg_restriction() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    make_source(&src, false);
    let out = tmp.path().join("out");
    let o = cli()
        .args(["convert"])
        .arg(&src)
        .arg("--out")
        .arg(&out)
        .args([
            "--format",
            "png",
            "--color-space",
            "adobergb",
            "--name-template",
            "x_{seq}",
            "-j",
            "1",
        ])
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let img = image::open(out.join("x_0001.png")).unwrap();
    assert!(img.width() > 0);
    assert!(!out.join("one.jpg").exists());

    let o = cli()
        .args(["convert"])
        .arg(&src)
        .arg("--out")
        .arg(&out)
        .args(["--embedded", "--format", "png"])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&o.stderr).contains("--embedded"));

    let o = cli()
        .args(["convert"])
        .arg(&src)
        .arg("--out")
        .arg(&out)
        .args(["--name-template", "../evil"])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&o.stderr).contains("template"));
}
