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
mod theme;
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
    let presets = PresetStore::load(&paths.presets_file());

    let mut viewport = egui::ViewportBuilder::default()
        .with_title(window_title())
        .with_app_id("ccoonverter")
        .with_inner_size([600.0, 820.0])
        .with_min_inner_size([520.0, 700.0]);
    if let Some(icon) = load_icon() {
        viewport = viewport.with_icon(Arc::new(icon));
    }

    #[allow(unused_mut)]
    let mut options = eframe::NativeOptions {
        viewport,
        centered: true,
        ..Default::default()
    };
    #[cfg(feature = "glow")]
    {
        options.renderer = eframe::Renderer::Glow;
    }

    let result = eframe::run_native(
        config::APP_NAME,
        options,
        Box::new(move |cc| {
            theme::apply(&cc.egui_ctx);
            fonts::install(&cc.egui_ctx);
            Ok(Box::new(app::CcooNverterApp::new(paths, settings, presets)))
        }),
    );
    if let Err(e) = &result {
        log::error!("Failed to start the window: {e}");
        rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Error)
            .set_title(config::APP_NAME)
            .set_description(format!(
                "Could not open the application window:\n{e}\n\nIf this persists (old GPU, remote desktop or VM), use a build with the `glow` renderer."
            ))
            .show();
    }
    log::info!("{}", "=".repeat(60));
    result
}
