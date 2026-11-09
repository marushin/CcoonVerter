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
use crate::controller::BatchIssue;
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
        h.get_by_role_and_label(Role::CheckBox, t(Key::ExifKeep))
            .click();
        settle_steps(&mut h);
        h.get_by_role_and_label(Role::CheckBox, t(Key::KeepFileDate));
        assert!(app(&h).settings.preserve_exif);
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
        let shot = h.render().expect("offscreen render");
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
    let shot = h.render().expect("offscreen render");
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
    let mut h = harness(
        home,
        size,
        AppSettings {
            preview_open: true,
            ..with_source(no_update_check(), &samples_dir())
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

/// `CCOON_SCREENSHOT_OUT` = directory for the PNGs. Offscreen, nothing is shown.
#[test]
#[ignore]
fn screenshots() {
    let Some(out) = std::env::var_os("CCOON_SCREENSHOT_OUT").map(PathBuf::from) else {
        eprintln!("CCOON_SCREENSHOT_OUT not set");
        return;
    };
    std::fs::create_dir_all(&out).unwrap();
    let save = |h: &mut H, name: &str| {
        h.run_steps(4);
        let image = h.render().expect("offscreen render");
        image.save(out.join(name)).unwrap();
        println!("wrote {name}");
    };
    let wide = vec2(1240.0, 800.0);
    let narrow = vec2(600.0, 820.0);

    for (lang, code) in [(Lang::Korean, "ko"), (Lang::English, "en")] {
        i18n::set_language(lang);
        let home = tempfile::tempdir().unwrap();

        let mut h = harness(home.path(), wide, no_update_check(), 1.0);
        save(&mut h, &format!("empty_{code}.png"));

        let mut h = harness(home.path(), narrow, no_update_check(), 1.0);
        save(&mut h, &format!("empty_narrow_{code}.png"));

        let mut h = loaded(home.path(), wide, 1.0);
        select_camera_sample(&mut h);
        wait(&mut h, "thumbnails", |a| a.preview.has_real_thumbnails());
        save(&mut h, &format!("loaded_{code}.png"));

        app_mut(&mut h).preview.set_compare(true);
        save(&mut h, &format!("split_{code}.png"));

        h.get_by_role_and_label(Role::RadioButton, t(Key::ZoomActual))
            .click();
        h.run_steps(3);
        wait(&mut h, "the crop", |a| a.preview.has_crop());
        save(&mut h, &format!("zoom100_{code}.png"));

        let mut h = loaded(home.path(), wide, 1.0);
        let a = app_mut(&mut h);
        let mut report = BatchReport::new(
            &outcome_with_failures(),
            Path::new("D:/사진/Converted_JPG"),
            &a.paths.log_file(),
        );
        report.issues.push(BatchIssue {
            file: "broken_card.CR2".into(),
            reason: t(Key::FailUnreadable).to_string(),
        });
        a.report = Some(report);
        save(&mut h, &format!("result_{code}.png"));

        let mut h = loaded(home.path(), narrow, 1.0);
        wait(&mut h, "the developed preview", |a| {
            a.preview.has_developed()
        });
        save(&mut h, &format!("narrow_{code}.png"));

        app_mut(&mut h).settings.extract_embedded = true;
        save(&mut h, &format!("narrow_camera_{code}.png"));

        let mut h = loaded(home.path(), wide, 1.0);
        h.get_by_label(t(Key::Advanced)).click();
        h.run_steps(3);
        app_mut(&mut h).settings.brightness = 1.25;
        save(&mut h, &format!("advanced_{code}.png"));
    }
    i18n::set_language(Lang::English);
}
