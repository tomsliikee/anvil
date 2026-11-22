//! The application object: one window, opened files from the command line.

use adw::prelude::*;
use gtk::gio;

use crate::{APP_ID, shots, theme, ui};

pub fn build() -> adw::Application {
    let app = adw::Application::builder()
        .application_id(APP_ID)
        // A screenshot run is its own process, never a second window of a
        // running Anvil.
        .flags(if shots::requested().is_some() { gio::ApplicationFlags::HANDLES_OPEN | gio::ApplicationFlags::NON_UNIQUE } else { gio::ApplicationFlags::HANDLES_OPEN })
        // libadwaita loads style.css from here by itself.
        .resource_base_path("/io/github/tomsliikee/Anvil")
        .build();

    app.connect_startup(|_| {
        gtk::Window::set_default_icon_name(APP_ID);
        theme::apply(&gio::Settings::new(APP_ID));
        ui::editor::register_sourceview_paths();
    });

    app.connect_activate(|app| {
        if let Some(window) = app.active_window() {
            window.present();
            return;
        }
        let win = ui::window::build(app);
        win.window.present();
        crate::frame_log::watch_window(&win.window);
        if let Some(dir) = shots::requested() {
            shots::run(dir);
        } else if let Ok(path) = std::env::var("ANVIL_OPEN") {
            ui::window::open_path(std::path::Path::new(&path));
        }
    });

    app.connect_shutdown(|_| crate::frame_log::report());

    app.connect_open(|app, files, _| {
        app.activate();
        if let Some(path) = files.first().and_then(|f| f.path()) {
            ui::window::open_path(&path);
        }
    });

    app
}
