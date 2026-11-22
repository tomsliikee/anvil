//! What a solution looks like when it is opened again: the open tabs in
//! both editor groups, the active one, caret and scroll per file, the dock
//! layout and the window size, kept in `.vs/anvil/state.json`. The last
//! layout is also kept in GSettings for solutions that have none yet.

use adw::prelude::*;
use anvil_core::session::{Layout, Session, WindowState};

use crate::ui::window::App;

pub const LAYOUT_KEY: &str = "dock-layout";

fn window_state(app: &App) -> WindowState {
    let w = &app.window;
    let (width, height) = if w.is_maximized() { w.default_size() } else { (w.width(), w.height()) };
    WindowState { width, height, maximized: w.is_maximized() }
}

/// The layout as it is on screen now.
pub fn current_layout(app: &App) -> Layout {
    let mut l = app.dock.layout();
    l.window = Some(window_state(app));
    l.split = app.tabs.split_position();
    l
}

/// Fills the session's view part from the window.
pub fn capture(app: &App, s: &mut Session) {
    s.open_files = app.tabs.group_paths(0);
    s.second_group = app.tabs.group_paths(1);
    s.active_file = app.tabs.group_current(0);
    s.second_active = app.tabs.group_current(1);
    s.views = app.tabs.file_views();
    let layout = current_layout(app);
    let _ = app.settings.set_string(LAYOUT_KEY, &layout.to_json());
    s.layout = Some(layout);
}

/// The layout a solution opens with: its own, else the last one used.
pub fn layout_for(app: &App, s: &Session) -> Layout {
    s.layout.clone().or_else(|| Layout::from_json(&app.settings.string(LAYOUT_KEY))).unwrap_or_default()
}

pub fn apply_window(app: &App, layout: &Layout) {
    let Some(w) = layout.window.filter(|w| w.width > 400 && w.height > 300) else { return };
    if w.maximized {
        app.window.maximize();
    } else {
        if app.window.is_maximized() {
            app.window.unmaximize();
        }
        app.window.set_default_size(w.width, w.height);
    }
}

/// Opens the session's tabs and puts carets and scroll back.
pub fn restore_tabs(app: &App, s: &Session) {
    for f in s.open_files.iter().filter(|f| f.exists()) {
        app.tabs.open(f, false);
    }
    if let Some(f) = s.active_file.as_ref().filter(|f| f.exists()) {
        app.tabs.open(f, false);
    }
    // Second group: the views of files already open are restored below.
    app.tabs.restore_second_group(&s.second_group, s.second_active.as_deref());
    for v in &s.views {
        app.tabs.restore_view(v);
    }
    app.tabs.focus_current();
}
