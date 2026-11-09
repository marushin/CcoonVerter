//! In-process UI tests. Everything here goes through egui's own event queue
//! inside this process (egui_kittest); no real mouse or keyboard is involved,
//! and no native dialog can open (`dialogs` is inert under `cfg(test)`).
//!
//! `screenshots` is `#[ignore]`d: with `CCOON_SCREENSHOT_OUT` set it renders
//! the window offscreen and writes PNGs there.

use std::time::{Duration, Instant};

use ccoonverter_core::job::{BatchSummary, FileStatus};
use ccoonverter_core::testutil::{synthetic_dng, DngSpec};
use ccoonverter_core::{ConvertSettings, FileResult};
use eframe::egui::{Event, FontId, Modifiers, PointerButton, Pos2, Role};
use egui_kittest::kittest::{NodeT, Queryable};
use egui_kittest::Harness;

use super::*;
use crate::controller::{BatchIssue, DropSelection};
use crate::i18n::Lang;
use crate::preview_panel::Zoom;

type H = Harness<'static, Option<CcooNverterApp>>;

fn no_update_check() -> AppSettings {
    AppSettings {
        check_updates: false,
        ..Default::default()
    }
}

fn harness(dir: &Path, size: Vec2, settings: AppSettings, ppp: f32) -> H {
    let dir = dir.to_path_buf();
    let mut h = Harness::builder()
        .with_size(size)
        .with_pixels_per_point(ppp)
        .with_max_steps(500)
        .with_step_dt(0.02)
        .build_ui_state(
            move |ui, state: &mut Option<CcooNverterApp>| {
                let ctx = ui.ctx().clone();
                let app = state.get_or_insert_with(|| {
                    crate::fonts::install(&ctx);
                    theme::apply(&ctx);
                    CcooNverterApp::new(
                        ctx.clone(),
                        AppPaths::with_root(dir.clone()),
                        settings.clone(),
                        PresetStore::default(),
                    )
                });
                // The harness insets its Ui; the real window does not.
                let full = ctx.content_rect();
                let mut root = ui.new_child(egui::UiBuilder::new().max_rect(full));
                root.set_clip_rect(full);
                app.run_frame(&mut root, None);
            },
            None,
        );
    h.run_steps(4);
    h
}

/// Offscreen wgpu renders from several tests at once crash the test process
/// on exit (seen with DirectX 12; the shipped build uses OpenGL), so one at a time.
fn render(h: &mut H) -> image::RgbaImage {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    h.render().expect("offscreen render")
}

fn app(h: &H) -> &CcooNverterApp {
    h.state().as_ref().unwrap()
}

fn app_mut(h: &mut H) -> &mut CcooNverterApp {
    h.state_mut().as_mut().unwrap()
}

fn settle_steps(h: &mut H) {
    h.run_steps(4);
}

/// One click at `pos`, spread over real frames as a user's would be.
fn click_at(h: &mut H, pos: Pos2) {
    h.event(Event::PointerMoved(pos));
    h.step();
    press(h, pos, true);
    h.step();
    press(h, pos, false);
    h.step();
}

fn press(h: &H, pos: Pos2, pressed: bool) {
    h.event(Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::default(),
    });
}

/// Drive frames until `done` holds, while workers finish their part.
fn wait(h: &mut H, what: &str, mut done: impl FnMut(&CcooNverterApp) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(120);
    while !done(app(h)) {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        h.step();
        std::thread::sleep(Duration::from_millis(15));
    }
    settle_steps(h);
}

fn samples_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/samples")
}

fn with_source(settings: AppSettings, source: &Path) -> AppSettings {
    AppSettings {
        last_source: Some(source.to_path_buf()),
        ..settings
    }
}

const SIZES: [(f32, f32); 5] = [
    (600.0, 820.0),
    (520.0, 700.0),
    (900.0, 700.0),
    (1240.0, 800.0),
    (1920.0, 1080.0),
];

#[test]
fn run_bar_stays_in_the_window_at_every_size_in_both_languages() {
    for lang in [Lang::English, Lang::Korean] {
        i18n::set_language(lang);
        for (w, h_) in SIZES {
            let dir = tempfile::tempdir().unwrap();
            let mut h = harness(dir.path(), vec2(w, h_), no_update_check(), 1.0);
            let window = Rect::from_min_size(Pos2::ZERO, vec2(w, h_));
            let button = h.get_by_label(t(Key::StartButton)).rect();
            assert!(
                window.contains_rect(button),
                "{lang:?} {w}x{h_}: {button:?}"
            );
            assert!(button.bottom() <= h_ - 2.0, "{lang:?} {w}x{h_}: {button:?}");
            // The bar does not move while a batch runs, either.
            app_mut(&mut h).progress = ProgressView::starting(3);
            settle_steps(&mut h);
            let button = h.get_by_label(t(Key::StartButton)).rect();
            assert!(window.contains_rect(button), "{lang:?} {w}x{h_}");
        }
    }
    i18n::set_language(Lang::English);
}

#[test]
fn run_bar_is_not_part_of_the_scrolling_column() {
    i18n::set_language(Lang::English);
    let dir = tempfile::tempdir().unwrap();
    let mut h = harness(dir.path(), vec2(600.0, 820.0), no_update_check(), 1.0);
    let before = h.get_by_label(t(Key::StartButton)).rect();
    // Open Advanced so the column has to scroll, then scroll down.
    h.get_by_label(t(Key::Advanced)).click();
    settle_steps(&mut h);
    let mid = h.get_by_label(t(Key::SectionPresets)).rect().center();
    h.hover_at(mid);
    for _ in 0..12 {
        h.event(Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: vec2(0.0, -200.0),
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Wheel,
            modifiers: Modifiers::default(),
        });
        h.step();
    }
    settle_steps(&mut h);
    assert_eq!(before, h.get_by_label(t(Key::StartButton)).rect());
}

#[test]
fn mode_toggle_switches_mode_and_hides_what_does_not_apply() {
    i18n::set_language(Lang::English);
    let dir = tempfile::tempdir().unwrap();
    let mut h = harness(dir.path(), vec2(1240.0, 800.0), no_update_check(), 1.0);
    assert!(!app(&h).settings.extract_embedded);
    assert!(h
        .query_by_role_and_label(Role::Slider, t(Key::ExposureLabel))
        .is_some());
    h.get_by_role_and_label(Role::RadioButton, t(Key::ModeCamera))
        .click();
    settle_steps(&mut h);
    assert!(app(&h).settings.extract_embedded);
    for key in [
        Key::ExposureLabel,
        Key::StyleStandard,
        Key::Advanced,
        Key::ToneCurve,
    ] {
        assert!(h.query_by_label(t(key)).is_none(), "{key:?} still shown");
    }
    h.get_by_role_and_label(Role::RadioButton, t(Key::ModeRaw))
        .click();
    settle_steps(&mut h);
    assert!(!app(&h).settings.extract_embedded);
}

#[test]
fn quality_is_disabled_when_a_camera_jpeg_would_not_be_re_encoded() {
    i18n::set_language(Lang::English);
    let dir = tempfile::tempdir().unwrap();
    let mut h = harness(dir.path(), vec2(1240.0, 800.0), no_update_check(), 1.0);
    app_mut(&mut h).settings.extract_embedded = true;
    settle_steps(&mut h);
    let disabled = |h: &H| {
        h.get_by_role_and_label(Role::Slider, t(Key::QualityLabel))
            .accesskit_node()
            .is_disabled()
    };
    assert!(disabled(&h));
    app_mut(&mut h).settings.resize_text = "1200".into();
    settle_steps(&mut h);
    assert!(!disabled(&h));
}

#[test]
fn slider_double_click_returns_to_default() {
    i18n::set_language(Lang::English);
    let dir = tempfile::tempdir().unwrap();
    let mut h = harness(dir.path(), vec2(1240.0, 800.0), no_update_check(), 1.0);
    let rect = h
        .get_by_role_and_label(Role::Slider, t(Key::ExposureLabel))
        .rect();
    let at = rect.left_center() + vec2(rect.width() * 0.8, 0.0);
    // One click sets the value at the pointer...
    click_at(&mut h, at);
    assert!(app(&h).settings.exposure_ev > 1.0);
    h.run_steps(20); // long enough that the next two clicks are a double click
                     // ... and a double click anywhere on the slider resets it.
    click_at(&mut h, at);
    click_at(&mut h, at);
    settle_steps(&mut h);
    assert_eq!(app(&h).settings.exposure_ev, 0.0);

    // Same for a plain, non-bipolar slider: quality returns to 90, not to the centre.
    let rect = h
        .get_by_role_and_label(Role::Slider, t(Key::QualityLabel))
        .rect();
    let at = rect.center();
    h.run_steps(20);
    click_at(&mut h, at);
    click_at(&mut h, at);
    settle_steps(&mut h);
    assert_eq!(app(&h).settings.quality, 90.0);
}

#[test]
fn dragging_a_slider_changes_the_value_and_shows_the_accent_on_it() {
    i18n::set_language(Lang::English);
    let dir = tempfile::tempdir().unwrap();
    let mut h = harness(dir.path(), vec2(1240.0, 800.0), no_update_check(), 1.0);
    let rect = h
        .get_by_role_and_label(Role::Slider, t(Key::ExposureLabel))
        .rect();
    h.hover_at(rect.center());
    h.step();
    h.drag_at(rect.center());
    h.step();
    for i in 1..=5 {
        h.hover_at(rect.center() + vec2(i as f32 * 12.0, 0.0));
        h.step();
    }
    assert!(app(&h).settings.exposure_ev > 0.4);
    h.drop_at(rect.center() + vec2(60.0, 0.0));
    settle_steps(&mut h);
    assert!(app(&h).settings.exposure_ev > 0.4);
}

#[test]
fn advanced_section_opens_and_brightness_appears_only_when_set() {
    i18n::set_language(Lang::English);
    let dir = tempfile::tempdir().unwrap();
    let mut h = harness(dir.path(), vec2(1240.0, 800.0), no_update_check(), 1.0);
    assert!(h
        .query_by_role_and_label(Role::Slider, "RGB balance R")
        .is_none());
    h.get_by_label(t(Key::Advanced)).click();
    settle_steps(&mut h);
    assert!(h
        .query_by_role_and_label(Role::Slider, "RGB balance R")
        .is_some());
    assert!(h
        .query_by_role_and_label(Role::Slider, t(Key::LevelsWhite))
        .is_some());
    assert!(
        h.query_by_role_and_label(Role::Slider, t(Key::BrightnessLabel))
            .is_none(),
        "default brightness stays hidden"
    );

    // A value from an old preset shows up, keeps its value and can be reset.
    let preset = ConvertSettings {
        brightness: 1.25,
        ..Default::default()
    };
    app_mut(&mut h).settings.apply_preset(&preset);
    settle_steps(&mut h);
    assert!(h
        .query_by_role_and_label(Role::Slider, t(Key::BrightnessLabel))
        .is_some());
    let render = ccoonverter_core::render::RenderParams::from(&app(&h).settings.to_convert());
    assert_eq!(
        render.brightness, 1.25,
        "the preset still renders as before"
    );
    h.get_all_by_label(t(Key::Reset)).last().unwrap().click();
    settle_steps(&mut h);
    assert_eq!(app(&h).settings.brightness, 1.0);
    assert!(h
        .query_by_role_and_label(Role::Slider, t(Key::BrightnessLabel))
        .is_none());
}

#[test]
fn collapsed_advanced_header_names_an_active_brightness() {
    i18n::set_language(Lang::English);
    let dir = tempfile::tempdir().unwrap();
    let mut h = harness(dir.path(), vec2(1240.0, 800.0), no_update_check(), 1.0);
    app_mut(&mut h).settings.brightness = 1.4;
    settle_steps(&mut h);
    h.get_by_label_contains("Brightness 1.40");
}

#[test]
fn clicking_a_style_thumbnail_selects_it() {
    i18n::set_language(Lang::English);
    let dir = tempfile::tempdir().unwrap();
    let mut h = harness(dir.path(), vec2(1240.0, 800.0), no_update_check(), 1.0);
    for style in [PictureStyle::Vivid, PictureStyle::Sepia] {
        h.get_by_role_and_label(Role::RadioButton, t(style_name(style)))
            .click();
        settle_steps(&mut h);
        assert_eq!(app(&h).settings.style, style);
    }
    // All twelve are there, as placeholders until a photo is loaded.
    for style in PictureStyle::ALL {
        h.get_by_role_and_label(Role::RadioButton, t(style_name(style)));
    }
}

fn failed_result(name: &str) -> FileResult {
    FileResult {
        source: PathBuf::from(name),
        status: FileStatus::Failed,
        output: None,
        method: None,
        width: None,
        height: None,
        message: Some("Unsupported camera model 'X100'".into()),
        failure: Some(ccoonverter_core::error::FailureKind::UnsupportedCamera),
        camera_jpeg_available: false,
        duration_ms: 10,
    }
}

fn outcome_with_failures() -> BatchOutcome {
    BatchOutcome::Finished(BatchSummary {
        total: 14,
        converted: 12,
        skipped: 1,
        failed: 2,
        cancelled: 0,
        was_cancelled: false,
        threads: 4,
        elapsed_secs: 61.5,
        results: vec![failed_result("DSC_0001.RAF"), failed_result("DSC_0002.RAF")],
    })
}

#[test]
fn result_card_shows_counts_and_reasons_and_closes() {
    for lang in [Lang::English, Lang::Korean] {
        i18n::set_language(lang);
        for size in [vec2(1240.0, 800.0), vec2(600.0, 820.0)] {
            let dir = tempfile::tempdir().unwrap();
            let mut h = harness(dir.path(), size, no_update_check(), 1.0);
            let a = app_mut(&mut h);
            a.report = Some(BatchReport::new(
                &outcome_with_failures(),
                Path::new("D:/사진/out"),
                &a.paths.log_file(),
            ));
            settle_steps(&mut h);
            h.get_by_label(t(Key::ResultPartialTitle));
            h.get_by_label("DSC_0001.RAF");
            h.get_by_label("DSC_0002.RAF");
            // A readable reason, not an internal error.
            let reason = tf(Key::FailUnsupportedCamera, &[&"X100"]);
            assert_eq!(h.get_all_by_label(&reason).count(), 2);
            for count in ["12", "1", "2"] {
                h.get_by_label(count);
            }
            h.get_by_label(t(Key::ResultOpenFolder));
            h.get_by_label(t(Key::Close)).click();
            settle_steps(&mut h);
            assert!(app(&h).report.is_none());
            assert!(h.query_by_label(t(Key::ResultPartialTitle)).is_none());
        }
    }
    i18n::set_language(Lang::English);
}

#[test]
fn real_batch_from_the_button_ends_in_the_result_card() {
    i18n::set_language(Lang::English);
    let home = tempfile::tempdir().unwrap();
    let src = tempfile::tempdir().unwrap();
    std::fs::write(
        src.path().join("good.DNG"),
        synthetic_dng(&DngSpec::default()),
    )
    .unwrap();
    std::fs::write(src.path().join("broken.NEF"), [0u8; 64]).unwrap();
    let mut h = harness(
        home.path(),
        vec2(1240.0, 800.0),
        with_source(no_update_check(), src.path()),
        1.0,
    );
    h.get_by_label(t(Key::StartButton)).click();
    settle_steps(&mut h);
    wait(&mut h, "the batch to finish", |a| a.report.is_some());
    let report = app(&h).report.as_ref().unwrap();
    assert_eq!(report.converted, 1);
    assert_eq!(report.failed, 1);
    h.get_by_label("broken.NEF");
    h.get_by_label(t(Key::ResultOpenFolder)).click();
    settle_steps(&mut h);
    assert!(app(&h).report.is_some(), "open folder keeps the card");
}

#[test]
fn starting_without_a_folder_explains_itself_in_the_app() {
    i18n::set_language(Lang::English);
    let dir = tempfile::tempdir().unwrap();
    let mut h = harness(dir.path(), vec2(1240.0, 800.0), no_update_check(), 1.0);
    h.get_by_label(t(Key::StartButton)).click();
    settle_steps(&mut h);
    assert!(app(&h).job.is_none());
    let notice = app(&h).notice.as_ref().map(|(n, _)| n.as_str());
    assert_eq!(notice, Some(t(Key::InvalidSourceFolder)));
}

#[test]
fn cancel_appears_while_converting_and_asks_first() {
    i18n::set_language(Lang::English);
    let home = tempfile::tempdir().unwrap();
    let src = tempfile::tempdir().unwrap();
    let dng = synthetic_dng(&DngSpec {
        width: 600,
        height: 400,
        ..Default::default()
    });
    for i in 0..30 {
        std::fs::write(src.path().join(format!("f{i}.DNG")), &dng).unwrap();
    }
    let mut h = harness(
        home.path(),
        vec2(1240.0, 800.0),
        with_source(no_update_check(), src.path()),
        1.0,
    );
    assert!(h.query_by_label(t(Key::CancelButton)).is_none());
    h.get_by_label(t(Key::StartButton)).click();
    settle_steps(&mut h);
    h.get_by_label(t(Key::Converting));
    crate::dialogs::TEST_ANSWER.with(|a| a.set(true));
    h.get_by_label(t(Key::CancelButton)).click();
    settle_steps(&mut h);
    wait(&mut h, "the batch to stop", |a| a.report.is_some());
    assert_eq!(app(&h).report.as_ref().unwrap().kind, ReportKind::Cancelled);
}

#[test]
fn language_change_updates_the_window() {
    i18n::set_language(Lang::English);
    let dir = tempfile::tempdir().unwrap();
    let mut settings = no_update_check();
    settings.language = UiLanguage::English;
    // Tall enough that the language row is on screen without scrolling.
    let mut h = harness(dir.path(), vec2(1240.0, 1300.0), settings, 1.0);
    h.get_by_label("Let's Get Cooking!");
    h.get_by_value(t(Key::LanguageEnglish)).click();
    settle_steps(&mut h);
    h.get_all_by_label(t(Key::LanguageKorean))
        .last()
        .unwrap()
        .click();
    settle_steps(&mut h);
    assert_eq!(app(&h).language, UiLanguage::Korean);
    assert_eq!(i18n::language(), Lang::Korean);
    h.get_by_label("맛있게 구워볼까요!");
    h.get_by_label("RAW 폴더를 여기에 놓으세요");
    i18n::set_language(Lang::English);
}

#[test]
fn exif_and_file_date_options_have_their_own_labels() {
    for lang in [Lang::English, Lang::Korean] {
        i18n::set_language(lang);
        let dir = tempfile::tempdir().unwrap();
        let mut h = harness(dir.path(), vec2(1240.0, 800.0), no_update_check(), 1.0);
        assert!(app(&h).settings.preserve_exif, "EXIF is kept by default");
        h.get_by_role_and_label(Role::CheckBox, t(Key::ExifKeep))
            .click();
        settle_steps(&mut h);
        h.get_by_role_and_label(Role::CheckBox, t(Key::KeepFileDate));
        assert!(!app(&h).settings.preserve_exif);
        assert!(!app(&h).settings.keep_source_date);
    }
    i18n::set_language(Lang::English);
}

#[test]
fn empty_state_is_the_drop_target_hint() {
    for lang in [Lang::English, Lang::Korean] {
        i18n::set_language(lang);
        for size in [vec2(1240.0, 800.0), vec2(600.0, 820.0)] {
            let dir = tempfile::tempdir().unwrap();
            let h = harness(dir.path(), size, no_update_check(), 1.0);
            h.get_by_label(t(Key::DropRawFolderHere));
        }
    }
    i18n::set_language(Lang::English);
}

/// A 1:1 crop at a HiDPI scale: alternating image columns must come out as
/// alternating physical columns, with no interpolation.
#[test]
fn hundred_percent_view_shows_real_pixels_on_physical_pixels() {
    for ppp in [1.0f32, 1.5, 2.0] {
        i18n::set_language(Lang::English);
        let dir = tempfile::tempdir().unwrap();
        let mut h = harness(dir.path(), vec2(1240.0, 800.0), no_update_check(), ppp);
        app_mut(&mut h).preview.detach_worker();
        // The harness renderer accepts textures up to 2048 px.
        let (w, h_px) = (2000u32, 1500u32);
        let pixels: Vec<Color32> = (0..h_px)
            .flat_map(|_| {
                (0..w).map(|x| {
                    if x.is_multiple_of(2) {
                        Color32::BLACK
                    } else {
                        Color32::WHITE
                    }
                })
            })
            .collect();
        let image = egui::ColorImage {
            size: [w as usize, h_px as usize],
            source_size: vec2(w as f32, h_px as f32),
            pixels,
        };
        let region = ccoonverter_core::render::Region {
            x: 500,
            y: 250,
            w,
            h: h_px,
        };
        app_mut(&mut h).preview.inject_crop(
            PathBuf::from("checker.DNG"),
            [3000, 2000],
            region,
            image,
        );
        h.run_steps(4);
        let shot = render(&mut h);
        // Sample a row through the middle of the photo area.
        let y = (shot.height() as f32 * 0.55) as u32;
        let start = (shot.width() as f32 * 0.55) as u32;
        let mut alternations = 0;
        for x in start..start + 200 {
            let a = shot.get_pixel(x, y).0[0];
            let b = shot.get_pixel(x + 1, y).0[0];
            assert!(
                !(10..=245).contains(&a),
                "ppp {ppp}: blurred value {a} at x={x}"
            );
            if a.abs_diff(b) > 200 {
                alternations += 1;
            }
        }
        assert_eq!(alternations, 200, "ppp {ppp}");
    }
}

/// The rendered photo area is grey with R = G = B, and the chrome is not.
#[test]
fn rendered_photo_area_is_neutral_and_the_chrome_is_warm() {
    i18n::set_language(Lang::English);
    let dir = tempfile::tempdir().unwrap();
    let mut h = harness(dir.path(), vec2(1240.0, 800.0), no_update_check(), 1.0);
    let shot = render(&mut h);
    let pixel = |x: u32, y: u32| shot.get_pixel(x, y).0;
    // A grid over the photo area, away from the text in the middle.
    for y in (100..700).step_by(29) {
        for x in [370, 500, 1100, 1220] {
            let [r, g, b, _] = pixel(x, y);
            assert_eq!((r, g), (g, b), "photo area at {x},{y}");
        }
    }
    let [r, _, b, _] = pixel(5, 3);
    assert!(r > b, "chrome should be warm: {r} {b}");
}

#[test]
fn window_shrinks_to_small_screens_and_keeps_its_minimum() {
    assert_eq!(
        fitted_window_size(DEFAULT_WINDOW, vec2(1920.0, 1080.0)),
        DEFAULT_WINDOW
    );
    let [w, h] = fitted_window_size(DEFAULT_WINDOW, vec2(1280.0, 720.0));
    assert!(w <= 1280.0);
    assert_eq!(h, MIN_WINDOW[1], "the minimum wins on a very small screen");
    let [_, h] = fitted_window_size(DEFAULT_WINDOW, vec2(1280.0, 800.0));
    assert!(h <= 704.0);
    assert_eq!(
        fitted_window_size(DEFAULT_WINDOW, vec2(400.0, 400.0)),
        MIN_WINDOW
    );
}

#[test]
fn output_options_modal_fits_the_narrow_window() {
    for lang in [Lang::English, Lang::Korean] {
        i18n::set_language(lang);
        let dir = tempfile::tempdir().unwrap();
        let mut h = harness(dir.path(), vec2(600.0, 820.0), no_update_check(), 1.0);
        app_mut(&mut h).output_options_open = true;
        app_mut(&mut h).settings.color_space = ColorSpace::AdobeRgb;
        h.run_steps(4);
        let area = h
            .ctx
            .memory(|m| m.area_rect(Id::unique("output_options_modal")))
            .expect("modal area");
        assert!(area.height() <= 820.0, "{lang:?}: {}", area.height());
        assert!(area.width() <= 600.0, "{lang:?}: {}", area.width());
    }
    i18n::set_language(Lang::English);
}

/// The update check is on for a new user; it does not depend on the settings
/// these tests use.
#[test]
fn update_check_is_on_by_default_for_a_new_install() {
    let dir = tempfile::tempdir().unwrap();
    let settings = AppSettings::load(&dir.path().join("settings.json"));
    assert!(settings.check_updates);
    let app = CcooNverterApp::new(
        egui::Context::default(),
        AppPaths::with_root(dir.path().to_path_buf()),
        settings,
        PresetStore::default(),
    );
    assert!(app.check_updates);
    assert!(app.update_rx.is_some(), "the background check was started");
}

fn text_width(ctx: &egui::Context, text: &str, font: FontId) -> f32 {
    ctx.fonts_mut(|f| f.layout_no_wrap(text.to_owned(), font, TEXT).size().x)
}

fn font_context() -> egui::Context {
    let ctx = egui::Context::default();
    crate::fonts::install(&ctx);
    theme::apply(&ctx);
    let mut out = ctx.run_ui(Default::default(), |_| {});
    out.textures_delta.clear();
    ctx
}

#[test]
fn labels_fit_their_cells_in_both_languages() {
    let ctx = font_context();
    for lang in [Lang::English, Lang::Korean] {
        i18n::set_language(lang);
        let cols = Columns::SETTINGS;
        for key in [
            Key::ExposureLabel,
            Key::WhiteBalanceLabel,
            Key::ResizeLabel,
            Key::QualityLabel,
            Key::BrightnessLabel,
            Key::LevelsBlack,
            Key::LevelsWhite,
            Key::IfOutputExists,
        ] {
            let w = text_width(&ctx, t(key), theme::font_body());
            assert!(w <= cols.label, "{key:?} {lang:?}: {w}");
        }
        for key in [Key::SourceFolder, Key::OutputFolderOptional] {
            let w = text_width(&ctx, t(key), theme::font_body());
            assert!(w <= PATH_LABEL_WIDTH, "{key:?} {lang:?}: {w}");
        }
        for (key, width, font) in [
            (Key::StartButton, 190.0, theme::font_header()),
            (Key::Converting, 190.0, theme::font_header()),
            (Key::CancelButton, 80.0, theme::font_body_bold()),
            (Key::Browse, 76.0, theme::font_body()),
            (Key::Save, 58.0, theme::font_body()),
            (Key::Delete, 58.0, theme::font_body()),
            (Key::Reset, 68.0, theme::font_small()),
            (Key::ShowPreview, 120.0, theme::font_small()),
            (Key::HidePreview, 120.0, theme::font_small()),
            (Key::Clear, 48.0, theme::font_small()),
            (Key::ZoomFit, 62.0, theme::font_body_bold()),
            (Key::ZoomActual, 62.0, theme::font_body_bold()),
            (Key::Compare, 72.0, theme::font_small_bold()),
            (Key::Histogram, 84.0, theme::font_small_bold()),
            (Key::ThumbsToggle, 72.0, theme::font_small_bold()),
            (Key::ResultOpenFolder, 120.0, theme::font_body_bold()),
            (Key::Close, 90.0, theme::font_body()),
            (Key::Download, 120.0, theme::font_body_bold()),
            (Key::Later, 100.0, theme::font_body()),
            (Key::FormatLabel, MODAL_LABEL_WIDTH, theme::font_body()),
            (Key::ColorSpaceLabel, MODAL_LABEL_WIDTH, theme::font_body()),
            (
                Key::NameTemplateLabel,
                MODAL_LABEL_WIDTH,
                theme::font_body(),
            ),
        ] {
            let w = text_width(&ctx, t(key), font);
            assert!(w <= width - 4.0, "{key:?} {lang:?}: {w} > {width}");
        }
        // Style names fit the narrowest tile (window at its minimum width).
        let narrow = (MIN_WINDOW[0] - 2.0 * PAD_LARGE - 6.0 - 3.0 * STYLE_TILE_GAP) / 4.0;
        for s in PictureStyle::ALL {
            let w = text_width(&ctx, t(style_name(s)), theme::font_small());
            assert!(w <= narrow, "{s:?} {lang:?}: {w} > {narrow}");
        }
        // Mode switch segments.
        let segment = (ADJUSTMENTS_WIDTH - 24.0) / 2.0 - 8.0;
        for key in [Key::ModeRaw, Key::ModeCamera] {
            let w = text_width(&ctx, t(key), theme::font_body_bold());
            assert!(w <= segment, "{key:?} {lang:?}: {w}");
        }
        // The widest format summary fits the output button.
        for text in [
            tf(Key::OutputButton, &[&"TIFF 16-bit  ·  AdobeRGB"]),
            tf(Key::OutputButton, &[&t(Key::CameraJpegOutput)]),
        ] {
            let w = text_width(&ctx, &text, theme::font_body());
            assert!(w <= 300.0, "output button {lang:?}: {w}");
        }
    }
    i18n::set_language(Lang::English);
}

#[test]
fn issue_rows_hold_the_longest_reason_in_two_lines() {
    let ctx = font_context();
    for lang in [Lang::English, Lang::Korean] {
        i18n::set_language(lang);
        let reason = tf(Key::FailUnsupportedCamera, &[&"Canon EOS R5 Mark II"]);
        let wrapped = ctx.fonts_mut(|f| {
            f.layout(reason.clone(), theme::font_small(), TEXT, 280.0)
                .rows
                .len()
        });
        assert!(wrapped <= 2, "{lang:?}: {wrapped} rows");
    }
    i18n::set_language(Lang::English);
}

/// Load the real samples into a window and wait for the first developed frame.
fn loaded(home: &Path, size: Vec2, ppp: f32) -> H {
    loaded_folder(home, &samples_dir(), size, ppp)
}

/// `folder` loaded into a window, with its first preview developed.
fn loaded_folder(home: &Path, folder: &Path, size: Vec2, ppp: f32) -> H {
    let mut h = harness(
        home,
        size,
        AppSettings {
            preview_open: true,
            ..with_source(no_update_check(), folder)
        },
        ppp,
    );
    wait(&mut h, "scan", |a| a.preview.file_count() > 0);
    wait(&mut h, "the developed preview", |a| {
        a.preview.has_developed()
    });
    h
}

/// Hardest case on these samples: the Canon 7D II, which has a camera JPEG.
fn select_camera_sample(h: &mut H) {
    let p = |h: &H| app(h).preview.current_index();
    let target = 1; // scan order: 5D III DNG, 7D II CR2, ...
    while p(h) != target {
        h.get_by_label(">").click();
        h.run_steps(3);
    }
    wait(h, "the developed preview", |a| {
        a.preview.has_developed() && a.preview.has_camera()
    });
}

#[test]
#[ignore]
fn real_sample_zoom_pans_and_compares() {
    if !samples_dir().is_dir() {
        return;
    }
    i18n::set_language(Lang::English);
    let home = tempfile::tempdir().unwrap();
    let mut h = loaded(home.path(), vec2(1240.0, 800.0), 1.5);
    select_camera_sample(&mut h);
    wait(&mut h, "thumbnails", |a| a.preview.has_real_thumbnails());

    h.get_by_role_and_label(Role::RadioButton, t(Key::ZoomActual))
        .click();
    h.run_steps(3);
    assert_eq!(app(&h).preview.zoom(), Zoom::Actual);
    wait(&mut h, "the crop", |a| a.preview.has_crop());
    let [vw, _] = app(&h).preview.view_px();
    let r = app(&h).preview.crop_region().unwrap();
    assert!(r.w >= vw, "crop {} covers view {vw}", r.w);

    // Drag the photo: the view moves, and new pixels arrive.
    let start = app(&h).preview.center();
    let at = Pos2::new(800.0, 400.0);
    h.hover_at(at);
    h.step();
    h.drag_at(at);
    h.step();
    h.hover_at(at + vec2(-120.0, -80.0));
    h.step();
    h.drop_at(at + vec2(-120.0, -80.0));
    h.run_steps(3);
    let moved = app(&h).preview.center();
    assert!(moved[0] > start[0] && moved[1] > start[1], "{moved:?}");

    // Double-click returns to the whole photo.
    click_at(&mut h, at);
    click_at(&mut h, at);
    h.run_steps(3);
    assert_eq!(app(&h).preview.zoom(), Zoom::Fit);
    assert!(!app(&h).preview.compare_on());
}

fn synthetic_source(names: &[&str]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let dng = synthetic_dng(&DngSpec {
        width: 320,
        height: 240,
        ..Default::default()
    });
    for name in names {
        std::fs::write(dir.path().join(name), &dng).unwrap();
    }
    dir
}

fn synthetic_loaded(home: &Path, src: &Path) -> H {
    let mut h = harness(
        home,
        vec2(1240.0, 800.0),
        with_source(no_update_check(), src),
        1.0,
    );
    wait(&mut h, "scan", |a| a.preview.file_count() > 0);
    wait(&mut h, "the developed preview", |a| {
        a.preview.has_developed()
    });
    h
}

fn open_hundred_percent(h: &mut H) {
    h.get_by_role_and_label(Role::RadioButton, t(Key::ZoomActual))
        .click();
    h.run_steps(3);
    wait(h, "the crop", |a| a.preview.has_crop());
}

/// A full-resolution image is hundreds of megabytes: it must not stay behind
/// when the folder changes to one where the 100 % view has nothing to show.
#[test]
fn changing_the_folder_leaves_the_100_percent_view_and_frees_its_memory() {
    i18n::set_language(Lang::English);
    let home = tempfile::tempdir().unwrap();
    let src = synthetic_source(&["a.DNG"]);
    let empty = tempfile::tempdir().unwrap();
    let mut h = synthetic_loaded(home.path(), src.path());
    open_hundred_percent(&mut h);
    let before = app(&h).preview.full_releases();

    app_mut(&mut h).source = empty.path().to_string_lossy().into_owned();
    wait(&mut h, "the empty folder", |a| a.preview.file_count() == 0);
    assert_eq!(app(&h).preview.zoom(), Zoom::Fit);
    wait(&mut h, "the release", |a| {
        a.preview.full_releases() > before
    });
}

#[test]
fn hiding_the_preview_leaves_the_100_percent_view_and_frees_its_memory() {
    i18n::set_language(Lang::English);
    let home = tempfile::tempdir().unwrap();
    let src = synthetic_source(&["a.DNG"]);
    let mut h = synthetic_loaded(home.path(), src.path());
    open_hundred_percent(&mut h);
    let before = app(&h).preview.full_releases();

    // Narrow window with the preview collapsed: nothing shows the crop.
    app_mut(&mut h).preview.open = false;
    h.set_size(vec2(600.0, 820.0));
    h.run_steps(4);
    assert_eq!(app(&h).preview.zoom(), Zoom::Fit);
    wait(&mut h, "the release", |a| {
        a.preview.full_releases() > before
    });
}

/// A batch started from the 100 % view frees the full-resolution image, and
/// the view comes back when the batch is over.
#[test]
fn batch_while_zoomed_frees_the_image_and_the_view_recovers() {
    i18n::set_language(Lang::English);
    let home = tempfile::tempdir().unwrap();
    let src = synthetic_source(&["a.DNG"]);
    let mut h = synthetic_loaded(home.path(), src.path());
    open_hundred_percent(&mut h);
    let before = app(&h).preview.full_releases();

    h.get_by_label(t(Key::StartButton)).click();
    h.run_steps(3);
    wait(&mut h, "the batch", |a| a.report.is_some());
    assert_eq!(app(&h).report.as_ref().unwrap().converted, 1);
    assert!(app(&h).preview.full_releases() > before, "image released");
    assert_eq!(app(&h).preview.zoom(), Zoom::Actual);
    wait(&mut h, "the crop again", |a| a.preview.has_crop());
}

/// A file whose camera JPEG can be shown but that cannot be developed.
fn undevelopable_raw(dir: &Path) -> PathBuf {
    let img = image::RgbImage::from_pixel(64, 48, image::Rgb([180, 90, 40]));
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new(&mut jpeg)
        .encode_image(&img)
        .unwrap();
    let mut blob = vec![0x42u8; 300];
    blob.extend_from_slice(&jpeg);
    blob.extend_from_slice(&[0u8; 100]);
    let path = dir.join("unsupported.NEF");
    std::fs::write(&path, blob).unwrap();
    path
}

/// Regression: on a RAW that cannot be developed, the 100 % button promised
/// "Developing at full resolution..." forever.
#[test]
fn hundred_percent_on_an_undevelopable_file_returns_to_the_whole_photo() {
    i18n::set_language(Lang::English);
    let home = tempfile::tempdir().unwrap();
    let src = tempfile::tempdir().unwrap();
    undevelopable_raw(src.path());
    let mut h = harness(
        home.path(),
        vec2(1240.0, 800.0),
        with_source(no_update_check(), src.path()),
        1.0,
    );
    wait(&mut h, "the camera JPEG", |a| a.preview.has_camera());
    wait(&mut h, "the failure", |a| {
        !a.preview.has_developed() && a.preview.shown_status().contains("Can't develop")
    });
    h.get_by_role_and_label(Role::RadioButton, t(Key::ZoomActual))
        .click();
    h.run_steps(3);
    wait(&mut h, "the way back", |a| a.preview.zoom() == Zoom::Fit);
    assert!(app(&h).preview.has_camera(), "the camera JPEG stays");
    assert!(app(&h).preview.shown_status().contains("Can't develop"));
}

/// Regression: two drops of the same size from one folder kept showing the
/// first file, while the batch converted the second.
#[test]
fn a_new_drop_of_the_same_size_changes_the_preview() {
    i18n::set_language(Lang::English);
    let home = tempfile::tempdir().unwrap();
    let src = synthetic_source(&["a.DNG", "b.DNG"]);
    let mut h = harness(home.path(), vec2(1240.0, 800.0), no_update_check(), 1.0);
    let staged = |name: &str| DropSelection {
        root: src.path().to_path_buf(),
        files: vec![src.path().join(name)],
    };
    let a = app_mut(&mut h);
    a.source = src.path().to_string_lossy().into_owned();
    a.selection = Some(staged("a.DNG"));
    wait(&mut h, "the first drop", |a| a.preview.has_developed());
    app_mut(&mut h).selection = Some(staged("b.DNG"));
    h.run_steps(3);
    assert_eq!(
        app(&h).preview.current_file(),
        Some(&src.path().join("b.DNG"))
    );
}

/// Regression: in camera JPEG mode nothing is developed, but "Developing..."
/// (or the file count) stayed on the photo.
#[test]
fn camera_jpeg_mode_shows_no_development_status() {
    i18n::set_language(Lang::English);
    let home = tempfile::tempdir().unwrap();
    let src = synthetic_source(&["a.DNG", "b.DNG"]);
    let settings = AppSettings {
        convert: ConvertSettings {
            extract_embedded: true,
            ..Default::default()
        },
        ..with_source(no_update_check(), src.path())
    };
    let mut h = harness(home.path(), vec2(1240.0, 800.0), settings, 1.0);
    wait(&mut h, "the camera JPEG", |a| a.preview.has_camera());
    h.get_by_label(">").click();
    h.run_steps(3);
    wait(&mut h, "the next camera JPEG", |a| a.preview.has_camera());
    assert_eq!(app(&h).preview.shown_status(), "");
}

/// A typo in the file name template is explained at once, in the user's
/// language, instead of ending in an "unexpected error" card.
#[test]
fn an_invalid_file_name_is_refused_before_the_batch_starts() {
    i18n::set_language(Lang::Korean);
    let home = tempfile::tempdir().unwrap();
    let src = synthetic_source(&["a.DNG"]);
    let mut h = harness(
        home.path(),
        vec2(1240.0, 800.0),
        with_source(no_update_check(), src.path()),
        1.0,
    );
    app_mut(&mut h).settings.filename_template = "{oops}".into();
    settle_steps(&mut h);
    h.get_by_label(t(Key::StartButton)).click();
    settle_steps(&mut h);
    assert!(app(&h).job.is_none());
    assert!(app(&h).report.is_none());
    let notice = app(&h).notice.as_ref().map(|(n, _)| n.clone()).unwrap();
    assert_eq!(notice, tf(Key::TemplateUnknownToken, &[&"oops"]));
    i18n::set_language(Lang::English);
}

#[test]
fn zero_typed_in_the_long_edge_field_reads_as_original_size() {
    i18n::set_language(Lang::English);
    let home = tempfile::tempdir().unwrap();
    let mut h = harness(home.path(), vec2(1240.0, 800.0), no_update_check(), 1.0);
    assert_eq!(app(&h).settings.resize_text, "");
    app_mut(&mut h).settings.resize_text = "0".into();
    settle_steps(&mut h);
    // Fields top to bottom: RAW folder, save folder, long edge.
    let field = h
        .get_all_by_role(Role::TextInput)
        .nth(2)
        .expect("the long edge field");
    field.focus();
    h.run_steps(3);
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    assert_eq!(app(&h).settings.resize_text, "");
    assert_eq!(app(&h).settings.to_convert().resize, 0);
}

// ----------------------------------------------------------------------------
// Thumbnail panel
// ----------------------------------------------------------------------------

/// A list of files that need not exist, in a window with the preview open.
fn with_list(size: Vec2, n: usize) -> H {
    let dir = tempfile::tempdir().unwrap().keep();
    let settings = AppSettings {
        preview_open: true,
        ..no_update_check()
    };
    let mut h = harness(&dir, size, settings, 1.0);
    let files = (0..n).map(|i| PathBuf::from(format!("D:/fake/f{i:05}.CR2")));
    app_mut(&mut h).preview.set_file_list(files.collect());
    settle_steps(&mut h);
    wait(&mut h, "the ratings", |a| a.preview.marks_loaded());
    h
}

fn name(i: usize) -> String {
    format!("f{i:05}.CR2")
}

fn dng(dir: &Path, name: &str, preview: bool) {
    let mut spec = DngSpec {
        width: 320,
        height: 240,
        ..Default::default()
    };
    if !preview {
        spec.preview = None;
    }
    std::fs::write(dir.join(name), synthetic_dng(&spec)).unwrap();
}

/// A window that loaded `src` and has answers for what it shows.
fn loaded_thumbs(home: &Path, src: &Path, size: Vec2) -> H {
    let mut h = harness(
        home,
        size,
        with_source(
            AppSettings {
                preview_open: true,
                ..no_update_check()
            },
            src,
        ),
        1.0,
    );
    wait(&mut h, "the scan", |a| a.preview.file_count() > 0);
    wait(&mut h, "the thumbnails", |a| {
        a.preview.browser().wanted().is_empty() && a.preview.browser().cached() > 0
    });
    h
}

fn select_rect(h: &H) -> Rect {
    app(h).preview.image_rect()
}

#[test]
fn the_panel_needs_three_columns_and_a_strip_takes_over_below() {
    i18n::set_language(Lang::English);
    let at = |size: Vec2| {
        let h = with_list(size, 30);
        app(&h).preview.browser().columns()
    };
    assert_eq!(at(vec2(1240.0, 800.0)), Some(2), "two columns by default");
    assert_eq!(at(vec2(999.0, 800.0)), None, "strip at 999");
    assert!(at(vec2(1000.0, 800.0)).is_some(), "panel at 1000");
    assert_eq!(at(vec2(600.0, 820.0)), None);
    assert_eq!(at(vec2(520.0, 700.0)), None);
}

#[test]
fn run_bar_and_layout_hold_with_thumbnails_at_every_size_in_both_languages() {
    for lang in [Lang::English, Lang::Korean] {
        i18n::set_language(lang);
        for (w, h_) in SIZES {
            let mut h = with_list(vec2(w, h_), 40);
            let window = Rect::from_min_size(Pos2::ZERO, vec2(w, h_));
            let button = h.get_by_label(t(Key::StartButton)).rect();
            assert!(window.contains_rect(button), "{lang:?} {w}x{h_}");
            assert!(button.bottom() <= h_ - 2.0, "{lang:?} {w}x{h_}");
            let photo = select_rect(&h);
            assert!(window.contains_rect(photo), "{lang:?} {w}x{h_}: {photo:?}");
            assert!(photo.bottom() <= button.top(), "{lang:?} {w}x{h_}");
            // The first cell is on screen, whichever arrangement is used.
            let cell = h.get_by_label(&name(0)).rect();
            assert!(window.intersects(cell), "{lang:?} {w}x{h_}");
            if w >= 1000.0 {
                assert!(
                    photo.width() >= MIN_PREVIEW_WIDTH - PAD_MEDIUM * 2.0 - 1.0,
                    "{lang:?} {w}x{h_}: photo {}",
                    photo.width()
                );
                assert!(cell.left() > photo.right(), "panel right of the photo");
            }
            let panel = app(&h).preview.browser().columns().is_some();
            assert_eq!(panel, w >= 1000.0, "{lang:?} {w}x{h_}");
            app_mut(&mut h).progress = ProgressView::starting(3);
            settle_steps(&mut h);
            let button = h.get_by_label(t(Key::StartButton)).rect();
            assert!(window.contains_rect(button), "{lang:?} {w}x{h_}");
        }
    }
    i18n::set_language(Lang::English);
}

#[test]
fn at_the_default_size_the_photo_is_clearly_the_largest_thing() {
    i18n::set_language(Lang::English);
    let h = with_list(vec2(1240.0, 800.0), 30);
    let photo = select_rect(&h);
    let panel_w = app(&h).thumb_panel_width;
    assert!(
        photo.width() > panel_w * 2.0,
        "photo {} panel {panel_w}",
        photo.width()
    );
    assert!(photo.width() > ADJUSTMENTS_WIDTH + 200.0);
}

#[test]
fn clicking_a_thumbnail_shows_that_file() {
    i18n::set_language(Lang::English);
    let mut h = with_list(vec2(1240.0, 800.0), 30);
    assert_eq!(app(&h).preview.current_index(), 0);
    h.get_by_label(&name(3)).click();
    settle_steps(&mut h);
    assert_eq!(app(&h).preview.current_index(), 3);
    let f = app(&h).preview.current_file().unwrap().clone();
    assert!(f.to_string_lossy().ends_with(&name(3)));
}

#[test]
fn buttons_and_arrow_keys_move_the_marker_and_scroll_it_into_view() {
    i18n::set_language(Lang::English);
    let mut h = with_list(vec2(1240.0, 800.0), 60);
    let window = Rect::from_min_size(Pos2::ZERO, vec2(1240.0, 800.0));
    for _ in 0..26 {
        h.get_by_label(">").click();
        settle_steps(&mut h);
    }
    let i = app(&h).preview.current_index();
    assert_eq!(i, 26);
    assert!(app(&h).preview.browser().visible().contains(&i));
    let cell = h.get_by_label(&name(i)).rect();
    assert!(window.contains_rect(cell), "{cell:?}");

    h.key_press(egui::Key::ArrowLeft);
    settle_steps(&mut h);
    assert_eq!(app(&h).preview.current_index(), 25);
    h.key_press(egui::Key::ArrowRight);
    h.key_press(egui::Key::ArrowRight);
    settle_steps(&mut h);
    assert_eq!(app(&h).preview.current_index(), 27);

    // Up and Down move by a row: two columns here.
    h.key_press(egui::Key::ArrowDown);
    settle_steps(&mut h);
    assert_eq!(app(&h).preview.current_index(), 29);
    h.key_press(egui::Key::ArrowUp);
    h.key_press(egui::Key::ArrowUp);
    settle_steps(&mut h);
    assert_eq!(app(&h).preview.current_index(), 25);
    let cell = h.get_by_label(&name(25)).rect();
    assert!(window.contains_rect(cell), "{cell:?}");
}

#[test]
fn a_far_jump_scrolls_the_strip_too() {
    i18n::set_language(Lang::English);
    let mut h = with_list(vec2(600.0, 820.0), 200);
    assert_eq!(app(&h).preview.browser().columns(), None);
    for _ in 0..40 {
        h.key_press(egui::Key::ArrowRight);
        h.step();
    }
    settle_steps(&mut h);
    let i = app(&h).preview.current_index();
    assert_eq!(i, 40);
    assert!(app(&h).preview.browser().visible().contains(&i));
    let cell = h.get_by_label(&name(i)).rect();
    assert!(
        cell.right() <= 600.0 && cell.left() >= 0.0,
        "{cell:?} not on screen"
    );
}

#[test]
fn arrow_keys_leave_a_focused_text_field_alone() {
    i18n::set_language(Lang::English);
    let mut h = with_list(vec2(1240.0, 800.0), 30);
    let field = h.get_all_by_role(Role::TextInput).next().unwrap();
    field.focus();
    h.run_steps(3);
    h.key_press(egui::Key::ArrowRight);
    settle_steps(&mut h);
    assert_eq!(app(&h).preview.current_index(), 0, "text field had focus");
}

#[test]
fn the_size_slider_changes_the_number_of_columns() {
    i18n::set_language(Lang::English);
    let mut h = with_list(vec2(1240.0, 800.0), 30);
    assert_eq!(app(&h).preview.browser().columns(), Some(2));
    let rect = h
        .get_by_role_and_label(Role::Slider, t(Key::ThumbSizeLabel))
        .rect();
    click_at(&mut h, rect.left_center() + vec2(9.0, 0.0));
    settle_steps(&mut h);
    let small = app(&h).preview.thumb_size;
    assert!(small <= 68.0, "{small}");
    let cols_small = app(&h).preview.browser().columns().unwrap();
    assert!(cols_small >= 3, "{cols_small}");
    click_at(&mut h, rect.right_center() - vec2(9.0, 0.0));
    settle_steps(&mut h);
    let cols_big = app(&h).preview.browser().columns().unwrap();
    assert_eq!(cols_big, 1, "at the largest size one column fits");
}

#[test]
fn dragging_the_panel_edge_respects_its_limits() {
    i18n::set_language(Lang::English);
    let (w, h_) = (1240.0, 800.0);
    let mut h = with_list(vec2(w, h_), 30);
    let drag = |h: &mut H, to: f32| {
        let edge = w - app(h).thumb_panel_width;
        let start = Pos2::new(edge, 400.0);
        h.hover_at(start);
        h.step();
        h.drag_at(start);
        h.step();
        h.hover_at(Pos2::new(to, 400.0));
        h.step();
        h.drop_at(Pos2::new(to, 400.0));
        h.run_steps(4);
    };
    // Left as far as it goes: the preview keeps its minimum width.
    drag(&mut h, 0.0);
    let widest = app(&h).thumb_panel_width;
    assert!(
        (widest - (w - ADJUSTMENTS_WIDTH - MIN_PREVIEW_WIDTH)).abs() <= 1.0,
        "widest {widest}"
    );
    assert!(select_rect(&h).width() >= MIN_PREVIEW_WIDTH - PAD_MEDIUM * 2.0 - 1.0);
    // Right as far as it goes: the panel keeps its minimum width.
    drag(&mut h, w);
    let narrowest = app(&h).thumb_panel_width;
    assert!(
        (narrowest - PANEL_MIN_WIDTH).abs() <= 1.0,
        "narrowest {narrowest}"
    );
    assert_ne!(widest, narrowest);
}

#[test]
fn the_toggle_hides_and_shows_the_panel_and_settings_remember_it() {
    i18n::set_language(Lang::English);
    let mut h = with_list(vec2(1240.0, 800.0), 30);
    let photo_with_panel = select_rect(&h).width();
    assert!(h.query_by_label(&name(0)).is_some());
    h.get_by_label(t(Key::ThumbsToggle)).click();
    settle_steps(&mut h);
    assert!(!app(&h).preview.thumbs_open);
    assert!(h.query_by_label(&name(0)).is_none(), "panel hidden");
    assert!(select_rect(&h).width() > photo_with_panel, "photo grows");
    assert_eq!(app(&h).preview.browser().columns(), None);
    let saved = app(&h).app_settings();
    assert!(!saved.thumbs_open);
    let shown = app(&h).thumb_panel_width;
    assert_eq!(saved.thumb_panel_width, shown);
    h.get_by_label(t(Key::ThumbsToggle)).click();
    settle_steps(&mut h);
    assert!(h.query_by_label(&name(0)).is_some());
}

#[test]
fn settings_restore_the_panel_width_size_and_visibility() {
    i18n::set_language(Lang::English);
    let dir = tempfile::tempdir().unwrap();
    let settings = AppSettings {
        thumb_panel_width: 300.0,
        thumb_size: 72.0,
        thumbs_open: false,
        ..no_update_check()
    };
    let mut h = harness(dir.path(), vec2(1240.0, 800.0), settings, 1.0);
    let files = (0..10).map(|i| PathBuf::from(format!("D:/fake/{i}.CR2")));
    app_mut(&mut h).preview.set_file_list(files.collect());
    settle_steps(&mut h);
    assert!(!app(&h).preview.thumbs_open);
    assert_eq!(app(&h).preview.thumb_size, 72.0);
    app_mut(&mut h).preview.thumbs_open = true;
    settle_steps(&mut h);
    assert!((app(&h).thumb_panel_width - 300.0).abs() <= 1.0);
}

#[test]
fn the_panel_and_strip_vanish_with_the_preview_and_while_a_result_is_shown() {
    i18n::set_language(Lang::English);
    let mut h = with_list(vec2(600.0, 820.0), 20);
    assert!(h.query_by_label(&name(0)).is_some(), "strip shown");
    h.get_by_label(t(Key::HidePreview)).click();
    settle_steps(&mut h);
    assert!(
        h.query_by_label(&name(0)).is_none(),
        "gone with the preview"
    );

    let mut h = with_list(vec2(1240.0, 800.0), 20);
    let a = app_mut(&mut h);
    a.report = Some(BatchReport::new(
        &outcome_with_failures(),
        Path::new("D:/out"),
        &a.paths.log_file(),
    ));
    settle_steps(&mut h);
    assert!(h.query_by_label(&name(0)).is_none(), "gone with the photo");
}

#[test]
fn the_panel_is_neutral_and_only_the_current_outline_is_butter() {
    i18n::set_language(Lang::English);
    let mut h = with_list(vec2(1240.0, 800.0), 8);
    let tile = h.get_by_label(&name(1)).rect();
    let current = h.get_by_label(&name(0)).rect();
    let shot = render(&mut h);
    let at = |p: Pos2| shot.get_pixel(p.x as u32, p.y as u32).0;
    // Tile and panel are grey, and the tile is the lighter of the two.
    let [r, g, b, _] = at(tile.left_top() + vec2(20.0, 20.0));
    assert!(r == g && g == b, "tile {r} {g} {b}");
    let [pr, pg, pb, _] = at(Pos2::new(tile.right() + 4.0, tile.top() + 20.0));
    assert!(pr == pg && pg == pb, "panel {pr} {pg} {pb}");
    assert!(r > pr, "tile is lighter than the panel");
    // Butter around the current tile, nowhere around the others.
    let butter = |p: [u8; 4]| p[0] > 240 && p[1] > 190 && p[2] < 110;
    let current_edge = at(Pos2::new(current.center().x, current.top() + 1.0));
    assert!(butter(current_edge), "{current_edge:?}");
    let other_edge = at(Pos2::new(tile.center().x, tile.top() + 1.0));
    assert!(!butter(other_edge), "{other_edge:?}");
}

#[test]
fn the_status_row_counts_the_files() {
    for lang in [Lang::English, Lang::Korean] {
        i18n::set_language(lang);
        let h = with_list(vec2(1240.0, 800.0), 38);
        h.get_by_label(&tf(Key::ThumbsCount, &[&38]));
    }
    i18n::set_language(Lang::English);
}

/// Real worker, synthetic files: the camera JPEG appears, and a file without
/// one gets a placeholder, then its developed picture once it has been shown.
#[test]
fn a_file_without_a_camera_jpeg_shows_a_placeholder_until_it_is_developed() {
    i18n::set_language(Lang::English);
    let home = tempfile::tempdir().unwrap();
    let src = tempfile::tempdir().unwrap();
    dng(src.path(), "a.DNG", true);
    dng(src.path(), "b.DNG", false);
    dng(src.path(), "c.DNG", true);
    let mut h = loaded_thumbs(home.path(), src.path(), vec2(1240.0, 800.0));
    use crate::thumbs::ThumbState::{Camera, Developed, Placeholder};
    assert_eq!(app(&h).preview.browser().state(0), Camera);
    assert_eq!(app(&h).preview.browser().state(1), Placeholder);
    assert_eq!(app(&h).preview.browser().state(2), Camera);

    h.get_by_label("b.DNG").click();
    h.run_steps(3);
    wait(&mut h, "the development", |a| a.preview.has_developed());
    assert_eq!(app(&h).preview.browser().state(1), Developed);
}

#[test]
fn a_damaged_file_gets_a_placeholder_and_the_app_keeps_running() {
    i18n::set_language(Lang::English);
    let home = tempfile::tempdir().unwrap();
    let src = tempfile::tempdir().unwrap();
    dng(src.path(), "a.DNG", true);
    // Zero-size box in a CR3 and a garbage ARW.
    let mut cr3 = vec![0u8, 0, 0, 24];
    cr3.extend_from_slice(b"ftypcrx ");
    cr3.resize(24, 0);
    cr3.extend_from_slice(&[0, 0, 0, 16]);
    cr3.extend_from_slice(b"moov");
    cr3.extend_from_slice(&[0, 0, 0, 0]);
    cr3.extend_from_slice(b"trak");
    std::fs::write(src.path().join("b.CR3"), cr3).unwrap();
    std::fs::write(src.path().join("c.ARW"), [0xFFu8; 4096]).unwrap();
    let mut h = loaded_thumbs(home.path(), src.path(), vec2(1240.0, 800.0));
    use crate::thumbs::ThumbState::Placeholder;
    assert_eq!(app(&h).preview.browser().state(1), Placeholder);
    assert_eq!(app(&h).preview.browser().state(2), Placeholder);
    // Still responsive, and clicking one fails only for that file.
    h.get_by_label("c.ARW").click();
    h.run_steps(5);
    assert_eq!(app(&h).preview.current_index(), 2);
}

/// Regression guard for the class of bug fixed for scans in v1.0.0: nothing
/// from the previous folder may show up in the new list.
#[test]
fn results_for_an_old_folder_are_discarded_after_the_folder_changes() {
    i18n::set_language(Lang::English);
    let mut h = with_list(vec2(1240.0, 800.0), 10);
    let old = app(&h).preview.browser().generation();
    let files = (0..10).map(|i| PathBuf::from(format!("D:/other/{i}.NEF")));
    app_mut(&mut h).preview.set_file_list(files.collect());
    settle_steps(&mut h);
    let ctx = h.ctx.clone();
    app_mut(&mut h).preview.browser_mut().accept_for_test(
        &ctx,
        crate::thumbs::ThumbResult {
            generation: old,
            index: 0,
            image: Some(egui::ColorImage::from_rgb([1, 1], &[1, 2, 3])),
        },
    );
    settle_steps(&mut h);
    assert_eq!(
        app(&h).preview.browser().state(0),
        crate::thumbs::ThumbState::Waiting
    );
    assert_eq!(app(&h).preview.browser().cached(), 0);
}

fn star_label(file: &str, k: u8) -> String {
    tf(Key::StarButton, &[&file, &k])
}

#[test]
fn clicking_stars_sets_and_clears_them() {
    i18n::set_language(Lang::English);
    let mut h = with_list(vec2(1240.0, 800.0), 12);
    let labelled = star_label(&name(4), 4);
    assert_eq!(h.get_all_by_label(&labelled).count(), 1);
    h.get_by_label(&labelled).click();
    settle_steps(&mut h);
    assert_eq!(app(&h).preview.stars_of(4), 4);
    h.get_by_label(&labelled).click();
    settle_steps(&mut h);
    assert_eq!(app(&h).preview.stars_of(4), 0, "same star clears");
    // Under the photo, for the current file (also under its tile).
    h.get_all_by_label(&star_label(&name(0), 3))
        .next()
        .unwrap()
        .click();
    settle_steps(&mut h);
    assert_eq!(app(&h).preview.stars_of(0), 3);
}

#[test]
fn number_keys_set_stars_only_when_no_text_field_has_focus() {
    i18n::set_language(Lang::English);
    let mut h = with_list(vec2(1240.0, 800.0), 12);
    h.key_press(egui::Key::Num2);
    settle_steps(&mut h);
    assert_eq!(app(&h).preview.stars_of(0), 2);
    h.key_press(egui::Key::Num5);
    settle_steps(&mut h);
    assert_eq!(app(&h).preview.stars_of(0), 5);
    h.key_press(egui::Key::Num0);
    settle_steps(&mut h);
    assert_eq!(app(&h).preview.stars_of(0), 0);

    let field = h.get_all_by_role(Role::TextInput).next().unwrap();
    field.focus();
    h.run_steps(3);
    h.key_press(egui::Key::Num3);
    settle_steps(&mut h);
    assert_eq!(app(&h).preview.stars_of(0), 0, "the text field had focus");
}

#[test]
fn the_filter_narrows_the_grid_navigation_and_the_count() {
    use crate::preview_panel::Filter;
    i18n::set_language(Lang::English);
    let mut h = with_list(vec2(1240.0, 800.0), 12);
    for (i, stars) in [(1usize, 2u8), (5, 4), (9, 5)] {
        app_mut(&mut h).preview.set_stars(i, stars);
    }
    settle_steps(&mut h);
    app_mut(&mut h).preview.set_filter(Filter::Rated(4));
    settle_steps(&mut h);
    assert_eq!(app(&h).preview.shown_count(), 2);
    assert!(h.query_by_label(&name(1)).is_none(), "filtered out");
    assert!(h.query_by_label(&name(5)).is_some());
    h.get_by_label("2 / 12");
    // The current file was filtered out: the nearest one is shown.
    assert_eq!(app(&h).preview.current_index(), 5);
    h.get_by_label(">").click();
    settle_steps(&mut h);
    assert_eq!(app(&h).preview.current_index(), 9);
    h.get_by_label(">").click();
    settle_steps(&mut h);
    assert_eq!(
        app(&h).preview.current_index(),
        5,
        "wraps inside the filter"
    );
    h.key_press(egui::Key::ArrowLeft);
    settle_steps(&mut h);
    assert_eq!(app(&h).preview.current_index(), 9);

    // Setting the filter from the header row.
    h.get_by_role_and_label(Role::RadioButton, t(Key::FilterAll))
        .click();
    settle_steps(&mut h);
    assert_eq!(app(&h).preview.shown_count(), 12);
    h.get_by_role_and_label(Role::RadioButton, t(Key::FilterRated))
        .click();
    settle_steps(&mut h);
    assert_eq!(app(&h).preview.shown_count(), 3, "Rated starts at one star");
    h.get_by_label(&star_label(t(Key::MinStarsLabel), 5))
        .click();
    settle_steps(&mut h);
    assert_eq!(app(&h).preview.shown_count(), 1);
}

#[test]
fn an_empty_filter_explains_itself_and_disables_the_batch() {
    use crate::preview_panel::Filter;
    i18n::set_language(Lang::English);
    let mut h = with_list(vec2(1240.0, 800.0), 6);
    wait(&mut h, "the ratings", |a| a.preview.marks_loaded());
    app_mut(&mut h).preview.set_filter(Filter::Rated(5));
    settle_steps(&mut h);
    h.get_by_label(t(Key::FilterEmpty));
    let zero = tf(Key::StartFiltered, &[&0]);
    let button = h.get_by_label(&zero);
    assert!(button.accesskit_node().is_disabled());
    assert_eq!(app(&h).preview.current_index(), 0);
    h.get_all_by_label(t(Key::FilterClear))
        .next()
        .unwrap()
        .click();
    settle_steps(&mut h);
    assert_eq!(app(&h).preview.shown_count(), 6);
    h.get_by_label(t(Key::StartButton));
}

#[test]
fn the_run_bar_says_what_a_filter_will_convert() {
    use crate::preview_panel::Filter;
    for lang in [Lang::English, Lang::Korean] {
        i18n::set_language(lang);
        for size in [vec2(1240.0, 800.0), vec2(520.0, 700.0)] {
            let mut h = with_list(size, 38);
            wait(&mut h, "the ratings", |a| a.preview.marks_loaded());
            for i in 0..12 {
                app_mut(&mut h).preview.set_stars(i, 3);
            }
            app_mut(&mut h).preview.set_filter(Filter::Rated(1));
            settle_steps(&mut h);
            let note = tf(Key::FilterBatchNote, &[&12, &38]);
            h.get_by_label(&note);
            let start = tf(Key::StartFiltered, &[&12]);
            let button = h.get_by_label(&start);
            let window = Rect::from_min_size(Pos2::ZERO, size);
            assert!(window.contains_rect(button.rect()), "{lang:?} {size:?}");
            // Fits its cell rather than being cut off.
            let ctx = h.ctx.clone();
            let text_w = text_width(&ctx, &note, theme::font_body_bold());
            let room = (size.x - 2.0 * PAD_LARGE - (size.x * 0.34).clamp(190.0, 280.0) - PAD_LARGE)
                - RUN_BAR_CLEAR
                - 6.0;
            assert!(text_w <= room, "{lang:?} {size:?}: {text_w} > {room}");
            app_mut(&mut h).preview.clear_filter();
            settle_steps(&mut h);
            h.get_by_label(t(Key::StartButton));
        }
    }
    i18n::set_language(Lang::English);
}

#[test]
fn with_a_filter_on_only_the_files_shown_are_converted() {
    use crate::preview_panel::Filter;
    i18n::set_language(Lang::English);
    let home = tempfile::tempdir().unwrap();
    let src = synthetic_source(&["a.DNG", "b.DNG", "c.DNG"]);
    let out = src.path().join("Converted_JPG");
    let mut h = harness(
        home.path(),
        vec2(1240.0, 800.0),
        with_source(no_update_check(), src.path()),
        1.0,
    );
    wait(&mut h, "the scan", |a| a.preview.file_count() == 3);
    wait(&mut h, "the ratings", |a| a.preview.marks_loaded());
    app_mut(&mut h).preview.set_stars(1, 4);
    app_mut(&mut h).preview.set_filter(Filter::Rated(1));
    settle_steps(&mut h);
    h.get_by_label(&tf(Key::FilterBatchNote, &[&1, &3]));
    h.get_by_label(&tf(Key::StartFiltered, &[&1])).click();
    settle_steps(&mut h);
    wait(&mut h, "the batch", |a| a.report.is_some());
    let report = app(&h).report.as_ref().unwrap();
    assert_eq!(report.converted, 1);
    let note = report.filter_note.clone().unwrap();
    assert_eq!(note, tf(Key::ResultFilterNote, &[&1, &3]));
    h.get_by_label(&note);
    assert!(out.join("b.jpg").exists());
    assert!(!out.join("a.jpg").exists());
    assert!(!out.join("c.jpg").exists());
    // The rating went to the sidecar, beside the untouched RAW.
    assert!(src.path().join("b.xmp").exists());
}

#[test]
fn a_mark_that_cannot_be_saved_shows_a_notice_and_stays() {
    i18n::set_language(Lang::English);
    let mut h = with_list(vec2(1240.0, 800.0), 6);
    app_mut(&mut h).preview.set_stars(2, 3);
    wait(&mut h, "the notice", |a| {
        a.notice.as_ref().is_some_and(|(n, _)| n.contains(&name(2)))
    });
    assert_eq!(app(&h).preview.stars_of(2), 3);
}

#[test]
fn stars_already_in_sidecars_are_shown() {
    i18n::set_language(Lang::English);
    let home = tempfile::tempdir().unwrap();
    let src = synthetic_source(&["a.DNG", "b.DNG"]);
    ccoonverter_core::sidecar::save_stars(&src.path().join("b.DNG"), 4).unwrap();
    let mut h = harness(
        home.path(),
        vec2(1240.0, 800.0),
        with_source(no_update_check(), src.path()),
        1.0,
    );
    wait(&mut h, "the scan", |a| a.preview.file_count() == 2);
    wait(&mut h, "the ratings", |a| a.preview.marks_loaded());
    assert_eq!(app(&h).preview.stars_of(0), 0);
    assert_eq!(app(&h).preview.stars_of(1), 4);
}

/// 10,000 small files, each with a sidecar: how long the ratings take to read,
/// and frame time with a filter on. In release:
/// `cargo test --release -p ccoonverter ratings_on_ten_thousand -- --ignored --nocapture --test-threads=1`
#[test]
#[ignore]
fn ratings_on_ten_thousand_files() {
    use crate::preview_panel::Filter;
    i18n::set_language(Lang::English);
    let dir = tempfile::tempdir().unwrap();
    let dng = synthetic_dng(&DngSpec {
        width: 32,
        height: 24,
        ..Default::default()
    });
    let t0 = Instant::now();
    for i in 0..10_000 {
        let raw = dir.path().join(format!("IMG_{i:05}.DNG"));
        std::fs::write(&raw, &dng).unwrap();
        if i % 2 == 0 {
            ccoonverter_core::sidecar::save_stars(&raw, (i % 5) as u8 + 1).unwrap();
        }
    }
    println!(
        "created 10,000 RAWs and 5,000 sidecars in {:.1} s",
        t0.elapsed().as_secs_f64()
    );
    let home = tempfile::tempdir().unwrap();
    let mut h = harness(
        home.path(),
        vec2(1920.0, 1080.0),
        with_source(no_update_check(), dir.path()),
        1.0,
    );
    let t1 = Instant::now();
    wait(&mut h, "the scan", |a| a.preview.file_count() == 10_000);
    let scanned = t1.elapsed().as_secs_f64();
    let t2 = Instant::now();
    wait(&mut h, "the ratings", |a| a.preview.marks_loaded());
    println!(
        "scan {scanned:.2} s; all 10,000 ratings read in {:.2} s (preview gate included, first file developing)",
        t2.elapsed().as_secs_f64()
    );
    let frames = |h: &mut H, n: usize| {
        let mut times = Vec::with_capacity(n);
        for _ in 0..n {
            let t = Instant::now();
            h.step();
            times.push(t.elapsed().as_secs_f64() * 1000.0);
        }
        times.sort_by(f64::total_cmp);
        (times.iter().sum::<f64>() / n as f64, times[n * 95 / 100])
    };
    let (no_filter, no_filter_p95) = frames(&mut h, 120);
    app_mut(&mut h).preview.set_filter(Filter::Rated(3));
    h.step();
    let (filtered, filtered_p95) = frames(&mut h, 120);
    println!(
        "frame ms: no filter mean {no_filter:.3} p95 {no_filter_p95:.3}; filter on mean {filtered:.3} p95 {filtered_p95:.3}; shown {}",
        app(&h).preview.shown_count()
    );
    assert_eq!(app(&h).preview.shown_count(), 3000);
}

/// Measurements on a 10,000-entry list. In release:
/// `cargo test --release -p ccoonverter thumbnail_panel_at_ten_thousand -- --ignored --nocapture --test-threads=1`
#[test]
#[ignore]
fn thumbnail_panel_at_ten_thousand_files() {
    use sysinfo::{Pid, ProcessesToUpdate, System};
    i18n::set_language(Lang::English);
    let samples = ccoonverter_core::scan::scan_folder(&samples_dir()).unwrap();
    let list =
        |n: usize| -> Vec<PathBuf> { (0..n).map(|i| samples[i % samples.len()].clone()).collect() };
    let mut sys = System::new();
    let pid = Pid::from_u32(std::process::id());
    let mut rss_mb = || {
        sys.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
        sys.process(pid).map_or(0, |p| p.memory()) as f64 / 1_048_576.0
    };
    type EachFrame = Box<dyn FnMut(&mut H, usize)>;
    let frames = |h: &mut H, n: usize, mut each: EachFrame| {
        let mut times = Vec::with_capacity(n);
        for i in 0..n {
            each(h, i);
            let t = Instant::now();
            h.step();
            times.push(t.elapsed().as_secs_f64() * 1000.0);
        }
        times.sort_by(f64::total_cmp);
        (times.iter().sum::<f64>() / n as f64, times[n * 95 / 100])
    };
    println!(
        "window,files,arrangement,idle_mean_ms,idle_p95_ms,scrolling_mean_ms,scrolling_p95_ms"
    );
    for &(w, h_) in &[(1240.0, 800.0), (600.0, 820.0)] {
        for n in [100usize, 1_000, 10_000] {
            let dir = tempfile::tempdir().unwrap();
            let settings = AppSettings {
                preview_open: true,
                ..no_update_check()
            };
            let mut h = harness(dir.path(), vec2(w, h_), settings, 1.0);
            app_mut(&mut h).preview.set_file_list(list(n));
            settle_steps(&mut h);
            wait(&mut h, "the first preview", |a| a.preview.has_developed());
            let (idle, idle95) = frames(&mut h, 120, Box::new(|_, _| {}));
            let (scroll, scroll95) = frames(
                &mut h,
                200,
                Box::new(move |h: &mut H, i| {
                    let at = app(h).preview.current_index();
                    app_mut(h)
                        .preview
                        .browser_mut()
                        .accept_view_jump((at + i * 3) % n);
                }),
            );
            let columns = app(&h).preview.browser().columns();
            println!(
                "{w}x{h_},{n},{:?},{idle:.3},{idle95:.3},{scroll:.3},{scroll95:.3}",
                columns
            );
        }
    }

    // Memory and throughput across a 10,000 list: jump through it for a minute.
    let dir = tempfile::tempdir().unwrap();
    let mut h = harness(
        dir.path(),
        vec2(1920.0, 1080.0),
        AppSettings {
            preview_open: true,
            ..no_update_check()
        },
        1.0,
    );
    let start_rss = rss_mb();
    app_mut(&mut h).preview.set_file_list(list(10_000));
    h.step();
    let t0 = Instant::now();
    wait(&mut h, "the first screen of thumbnails", |a| {
        a.preview.browser().wanted().is_empty() && a.preview.browser().cached() > 0
    });
    let cached_first = app(&h).preview.browser().cached();
    println!(
        "first screen: {cached_first} thumbnails in {:.2} s ({:.1}/s through the panel, gated)",
        t0.elapsed().as_secs_f64(),
        cached_first as f64 / t0.elapsed().as_secs_f64()
    );
    let (mut peak_rss, mut peak_cached) = (0f64, 0usize);
    let (mut jumps, t1) = (0usize, Instant::now());
    while t1.elapsed() < Duration::from_secs(60) {
        jumps += 1;
        let target = (jumps * 37) % 10_000;
        app_mut(&mut h)
            .preview
            .browser_mut()
            .accept_view_jump(target);
        let until = Instant::now() + Duration::from_millis(250);
        while Instant::now() < until {
            h.step();
            std::thread::sleep(Duration::from_millis(8));
        }
        peak_cached = peak_cached.max(app(&h).preview.browser().cached());
        peak_rss = peak_rss.max(rss_mb());
    }
    println!(
        "{jumps} jumps; cache peak {peak_cached} (limit {}); working set start {start_rss:.0} MB, peak {peak_rss:.0} MB",
        crate::thumbs::CACHE_CAPACITY
    );
    assert!(peak_cached <= crate::thumbs::CACHE_CAPACITY);
}

/// Folder shown in the documentation: the samples live in the repository,
/// which is no business of a reader.
fn demo_folder(code: &str) -> &'static str {
    if code == "ko" {
        r"D:\사진\2026-11 제주\RAW"
    } else {
        r"D:\Photos\2026-11 Jeju\RAW"
    }
}

/// Stop the worker, so the source text can change without a new scan, and
/// show `demo_folder`. Call last: nothing new loads afterwards.
fn show_demo_folder(h: &mut H, code: &str) {
    let a = app_mut(h);
    a.preview.detach_worker();
    a.source = demo_folder(code).to_string();
}

/// `CCOON_SCREENSHOT_OUT` = directory for the PNGs. Offscreen, nothing is shown.
#[test]
#[ignore]
fn screenshots() {
    let Some(out) = std::env::var_os("CCOON_SCREENSHOT_OUT").map(PathBuf::from) else {
        eprintln!("CCOON_SCREENSHOT_OUT not set");
        return;
    };
    std::fs::create_dir_all(&out).unwrap();
    let save = |h: &mut H, name: String| {
        // No pointer drawn on the picture and no button left in its hover state.
        h.set_render_cursor(false);
        h.remove_cursor();
        h.run_steps(4);
        let image = render(h);
        image.save(out.join(&name)).unwrap();
        println!("wrote {name}");
    };
    let wide = vec2(1240.0, 800.0);
    let narrow = vec2(600.0, 820.0);
    // Only to compare with a capture of the real window at its display scale.
    let ppp: f32 = std::env::var("CCOON_SCREENSHOT_PPP")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1.0);

    for (lang, code) in [(Lang::Korean, "ko"), (Lang::English, "en")] {
        i18n::set_language(lang);
        let home = tempfile::tempdir().unwrap();
        let main_name = if code == "ko" {
            "gui_windows_ko.png".to_string()
        } else {
            "gui_windows.png".to_string()
        };
        // A realistic folder for the thumbnail panel: the five samples under
        // neutral camera-style names, repeated (hard links, nothing copied).
        let shoot = tempfile::tempdir().unwrap();
        let samples = ccoonverter_core::scan::scan_folder(&samples_dir()).unwrap();
        for i in 0..24 {
            let from = &samples[i % samples.len()];
            let ext = from.extension().unwrap().to_string_lossy().into_owned();
            let to = shoot.path().join(format!("IMG_{:04}.{ext}", 3101 + i));
            // Hard links do not cross drives; a copy is fine then.
            if std::fs::hard_link(from, &to).is_err() {
                std::fs::copy(from, &to).unwrap();
            }
        }
        let settle_thumbnails = |h: &mut H| {
            wait(h, "file thumbnails", |a| {
                a.preview.browser().wanted().is_empty() && a.preview.browser().cached() > 0
            });
        };
        let shoot_window = |size: Vec2| {
            let mut h = loaded_folder(home.path(), shoot.path(), size, ppp);
            select_camera_sample(&mut h);
            wait(&mut h, "thumbnails", |a| a.preview.has_real_thumbnails());
            settle_thumbnails(&mut h);
            h
        };
        let loaded_camera_sample = || {
            let mut h = loaded(home.path(), wide, ppp);
            select_camera_sample(&mut h);
            wait(&mut h, "thumbnails", |a| a.preview.has_real_thumbnails());
            settle_thumbnails(&mut h);
            h
        };

        let mut h = harness(home.path(), wide, no_update_check(), 1.0);
        save(&mut h, format!("empty_{code}.png"));

        let mut h = shoot_window(wide);
        // A few rated photos, so the stars and the filter can be seen.
        for (i, stars) in [(0, 5u8), (1, 4), (3, 3), (6, 5), (8, 2), (13, 4)] {
            app_mut(&mut h).preview.set_stars(i, stars);
        }
        wait(&mut h, "the ratings", |a| a.preview.marks_loaded());
        settle_steps(&mut h);
        show_demo_folder(&mut h, code);
        save(&mut h, main_name);
        app_mut(&mut h)
            .preview
            .set_filter(crate::preview_panel::Filter::Rated(4));
        h.run_steps(2);
        settle_thumbnails(&mut h);
        save(&mut h, format!("thumbs_filter_{code}.png"));
        app_mut(&mut h)
            .preview
            .set_filter(crate::preview_panel::Filter::All);
        h.run_steps(2);
        app_mut(&mut h).preview.set_compare(true);
        save(&mut h, format!("split_{code}.png"));

        // The panel hidden, and the strip that replaces it in a narrow window.
        let mut h = shoot_window(wide);
        show_demo_folder(&mut h, code);
        app_mut(&mut h).preview.thumbs_open = false;
        save(&mut h, format!("thumbs_hidden_{code}.png"));
        let mut h = shoot_window(vec2(900.0, 700.0));
        show_demo_folder(&mut h, code);
        save(&mut h, format!("thumbs_strip_{code}.png"));

        let mut h = loaded_camera_sample();
        app_mut(&mut h).preview.set_compare(true);
        h.get_by_role_and_label(Role::RadioButton, t(Key::ZoomActual))
            .click();
        h.run_steps(3);
        wait(&mut h, "the crop", |a| a.preview.has_crop());
        show_demo_folder(&mut h, code);
        save(&mut h, format!("zoom100_{code}.png"));

        let mut h = loaded_camera_sample();
        show_demo_folder(&mut h, code);
        let a = app_mut(&mut h);
        let mut report = BatchReport::new(
            &outcome_with_failures(),
            Path::new(demo_folder(code)).join("Converted_JPG").as_path(),
            Path::new(r"C:\Users\me\AppData\Local\ccoon\CcooNverter\data\logs\ccoonverter.log"),
        );
        report.issues.push(BatchIssue {
            file: "broken_card.CR2".into(),
            reason: t(Key::FailUnreadable).to_string(),
        });
        a.report = Some(report);
        save(&mut h, format!("result_{code}.png"));

        let mut h = loaded_camera_sample();
        show_demo_folder(&mut h, code);
        let a = app_mut(&mut h);
        a.output_options_open = true;
        a.settings.filename_template = "{date}_{name}".into();
        save(&mut h, format!("output_{code}.png"));

        let mut h = loaded_folder(home.path(), shoot.path(), narrow, 1.0);
        select_camera_sample(&mut h);
        wait(&mut h, "file thumbnails", |a| {
            a.preview.browser().wanted().is_empty() && a.preview.browser().cached() > 0
        });
        show_demo_folder(&mut h, code);
        save(&mut h, format!("narrow_{code}.png"));
        app_mut(&mut h).settings.extract_embedded = true;
        save(&mut h, format!("narrow_camera_{code}.png"));

        let mut h = loaded_camera_sample();
        show_demo_folder(&mut h, code);
        h.get_by_label(t(Key::Advanced)).click();
        h.run_steps(3);
        app_mut(&mut h).settings.brightness = 1.25;
        save(&mut h, format!("advanced_{code}.png"));
    }
    i18n::set_language(Lang::English);
}
