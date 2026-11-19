mod app;
mod commands;
mod host;
mod i18n;
mod motion;
mod palette;
mod runtime;
mod shots;
mod theme;
mod ui;

use std::path::PathBuf;

use adw::prelude::*;
use gtk::{gio, glib};

pub const APP_ID: &str = "io.github.tomsliikee.Anvil";

/// Installed data (language definitions, colour schemes): `/app/share/anvil`
/// in the Flatpak, `ANVIL_DATADIR` for a build run from the source tree.
pub fn data_dir() -> PathBuf {
    std::env::var_os("ANVIL_DATADIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/app/share/anvil"))
}

/// Anvil's own configuration folder (keymap overrides, user templates):
/// `ANVIL_CONFIG_DIR` keeps screenshot runs out of the real one.
pub fn config_dir() -> PathBuf {
    std::env::var_os("ANVIL_CONFIG_DIR").map(PathBuf::from).unwrap_or_else(|| glib::user_config_dir().join("anvil"))
}

fn main() -> glib::ExitCode {
    // The pinned language must be known before gettext binds; GSettings can
    // be read before GTK starts.
    let pinned = gio::SettingsSchemaSource::default()
        .and_then(|s| s.lookup(APP_ID, true))
        .map(|_| gio::Settings::new(APP_ID).string("language").to_string())
        .filter(|l| l != "system");
    i18n::init(pinned.as_deref());
    gio::resources_register_include!("anvil.gresource").expect("bundled resources");
    app::build().run()
}
