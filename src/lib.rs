//! Generic settings window.
//!
//! The `settings` binary calls [`run`] with no host callback. Another program
//! can link this crate, call [`set_on_change`], then [`run`].

mod app;
mod config;
mod hooks;
mod i18n;
mod migrate;
mod palette;
mod schema;
mod single_instance;
mod theme;
mod validation;

pub use hooks::{set_legacy_color_resolver, set_on_change, Change, Preview};

use std::path::PathBuf;

/// Open the settings window and block until it closes.
pub fn run() -> eframe::Result {
    let config_path = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(default_config_path);

    if !single_instance::acquire(&config_path) {
        return Ok(());
    }

    let schema = schema::load().expect("Failed to parse schema.toml");
    let mut config = config::ConfigStore::load(&config_path)
        .unwrap_or_else(|e| panic!("Failed to load config {config_path:?}: {e}"));

    if let Err(e) = migrate::run(&schema, &mut config) {
        eprintln!("warning: config migration failed: {e}");
    }

    for field in schema.tabs.iter().flat_map(|tab| tab.fields.iter().flatten()) {
        if let Some(key) = &field.record_id_key {
            config.ensure_record_ids(&field.key, key);
        }
    }
    if config.is_dirty() {
        if let Err(e) = config.save() {
            eprintln!("warning: could not save record identities: {e}");
        }
    }

    let parent_lang = schema
        .lang_key
        .as_deref()
        .and_then(|key| config.get_str(key))
        .map(str::to_owned);
    i18n::init(
        i18n::Lang::resolve(&schema.lang, parent_lang.as_deref()),
        &schema.ui_strings,
    );

    let viewport = egui::ViewportBuilder::default()
        .with_title(i18n::t().window_title)
        .with_inner_size(app::compute_window_size(&schema));
    #[cfg(not(target_os = "macos"))]
    let viewport = viewport.with_icon(std::sync::Arc::new(
        eframe::icon_data::from_png_bytes(include_bytes!("../assets/appicon.png"))
            .expect("failed to load app icon"),
    ));

    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "settings",
        options,
        Box::new(|cc| Ok(Box::new(app::SettingsApp::new(cc, schema, config)))),
    )
}

fn default_config_path() -> PathBuf {
    if let Some(bundled) = bundled_config_path() {
        return bundled;
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("demo/config.toml")
}

#[cfg(target_os = "macos")]
fn bundled_config_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let macos_dir = exe.parent()?;
    if macos_dir.file_name().is_some_and(|n| n == "MacOS") {
        return Some(macos_dir.parent()?.join("Resources/config.toml"));
    }
    None
}

#[cfg(not(target_os = "macos"))]
fn bundled_config_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let path = exe.parent()?.join("config.toml");
    path.exists().then_some(path)
}
