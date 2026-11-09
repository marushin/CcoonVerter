//! CcooNverter desktop GUI (eframe/egui).
//!
//! Start-up: resolve OS directories, initialise logging, import legacy Python
//! settings once, load settings + presets, then open the window. All
//! conversion work happens on a worker thread (see [`controller`]).

// No console window for release builds on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod controller;
mod dialogs;
mod fonts;
mod i18n;
mod preview;
mod preview_panel;
mod theme;
mod titlebar;
mod widgets;

use std::sync::Arc;

use ccoonverter_core::config;
use ccoonverter_core::logging::{self, LogOptions};
use ccoonverter_core::paths::AppPaths;
use ccoonverter_core::presets::PresetStore;
use ccoonverter_core::AppSettings;
use eframe::egui;

/// 256 px copy of `assets/icon.png` (the 2048 px original would add 3 MB to the exe).
const ICON_PNG: &[u8] = include_bytes!("../../../assets/icon_256.png");

fn load_icon() -> Option<egui::IconData> {
    match image::load_from_memory_with_format(ICON_PNG, image::ImageFormat::Png) {
        Ok(img) => {
            let rgba = img.into_rgba8();
            let (width, height) = rgba.dimensions();
            Some(egui::IconData {
                rgba: rgba.into_raw(),
                width,
                height,
            })
        }
        Err(e) => {
            log::warn!("Could not decode window icon: {e}");
            None
        }
    }
}

/// Shown when the window cannot be created; points to the other renderer.
#[cfg(not(feature = "wgpu"))]
const RENDERER_HINT: i18n::Key = i18n::Key::RendererHintGlow;
#[cfg(feature = "wgpu")]
const RENDERER_HINT: i18n::Key = i18n::Key::RendererHintWgpu;

fn window_title() -> String {
    format!("{} v{}", config::APP_NAME, env!("CARGO_PKG_VERSION"))
}

fn main() -> eframe::Result {
    let paths = AppPaths::resolve();
    if let Err(e) = logging::init(&LogOptions::new(Some(paths.log_file()), false)) {
        eprintln!("Logging disabled: {e}");
    }
    log::info!("{}", "=".repeat(60));
    log::info!("Starting {}", window_title());
    log::info!("Config dir: {}", paths.config_dir.display());

    for f in paths.migrate_legacy() {
        log::info!("Migrated legacy file {}", f.display());
    }
    let settings = AppSettings::load(&paths.settings_file());
    i18n::apply(settings.language);
    log::info!(
        "UI language: setting {}, system locale {:?}, using {:?}",
        settings.language.as_str(),
        i18n::system_locale(),
        i18n::language()
    );
    let presets = PresetStore::load(&paths.presets_file());

    let mut viewport = egui::ViewportBuilder::default()
        .with_title(window_title())
        .with_app_id("ccoonverter")
        .with_inner_size([600.0, 820.0])
        .with_min_inner_size([520.0, 700.0]);
    if let Some(icon) = load_icon() {
        viewport = viewport.with_icon(Arc::new(icon));
    }

    let mut options = eframe::NativeOptions {
        viewport,
        centered: true,
        ..Default::default()
    };
    // Default build: OpenGL (glow) only. `--features wgpu` switches to wgpu.
    #[cfg(feature = "wgpu")]
    {
        options.renderer = eframe::Renderer::Wgpu;
    }
    #[cfg(not(feature = "wgpu"))]
    {
        options.renderer = eframe::Renderer::Glow;
    }

    let result = eframe::run_native(
        config::APP_NAME,
        options,
        Box::new(move |cc| {
            theme::apply(&cc.egui_ctx);
            fonts::install(&cc.egui_ctx);
            let ctx = cc.egui_ctx.clone();
            Ok(Box::new(app::CcooNverterApp::new(
                ctx, paths, settings, presets,
            )))
        }),
    );
    if let Err(e) = &result {
        log::error!("Failed to start the window: {e}");
        rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Error)
            .set_title(config::APP_NAME)
            .set_description(i18n::tf(
                i18n::Key::WindowOpenFailed,
                &[&e, &i18n::t(RENDERER_HINT)],
            ))
            .show();
    }
    log::info!("{}", "=".repeat(60));
    result
}
