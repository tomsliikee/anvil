//! UI density (Settings → Environment): compact or comfortable, as a CSS
//! class on the window that the stylesheet reads.

use adw::prelude::*;
use gtk::gio;

pub const KEY: &str = "density";
const CLASSES: [&str; 2] = ["density-compact", "density-comfortable"];

pub fn apply(window: &adw::ApplicationWindow, settings: &gio::Settings) {
    let wanted = if settings.string(KEY) == "compact" { CLASSES[0] } else { CLASSES[1] };
    for c in CLASSES {
        if c == wanted {
            window.add_css_class(c);
        } else {
            window.remove_css_class(c);
        }
    }
}
