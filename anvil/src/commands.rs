//! Every command of `anvil_core::commands` becomes a window action
//! (`win.edit.format-document`), and one key controller in the capture
//! phase feeds VS' keymap (with its chords) before any widget sees a key.

use std::rc::Rc;

use adw::prelude::*;
use anvil_core::commands::{Scope, COMMANDS};
use anvil_core::keymap::{Combo, Feed, Keymap};
use gtk::{gdk, gio, glib};

use crate::i18n::{tr, trf};
use crate::ui::window::App;

pub fn install(app: &Rc<App>) {
    for c in COMMANDS {
        let action = gio::SimpleAction::new(c.id, None);
        let id = c.id;
        action.connect_activate(move |_, _| crate::ui::actions::dispatch(id));
        app.window.add_action(&action);
    }
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    keys.connect_key_pressed(glib::clone!(
        #[weak(rename_to = window)]
        app.window,
        #[upgrade_or]
        glib::Propagation::Proceed,
        move |ctl, key, code, state| on_key(&window, ctl, key, code, state)
    ));
    app.window.add_controller(keys);
}

/// Loads the keymap with the user's overrides from `keymap.json`.
pub fn load_keymap() -> Keymap {
    let path = crate::config_dir().join("keymap.json");
    match std::fs::read_to_string(&path) {
        Ok(json) => Keymap::with_overrides(&json).0,
        Err(_) => Keymap::vs_default(),
    }
}

pub fn save_keymap(map: &Keymap) {
    let dir = crate::config_dir();
    let _ = std::fs::create_dir_all(&dir);
    let _ = anvil_core::text::write_atomic(&dir.join("keymap.json"), map.overrides_json().as_bytes());
}

fn key_name(key: gdk::Key) -> Option<String> {
    use gdk::Key as K;
    let named = match key {
        K::Return | K::KP_Enter | K::ISO_Enter => "Enter",
        K::Escape => "Escape",
        K::Tab | K::ISO_Left_Tab => "Tab",
        K::BackSpace => "Backspace",
        K::Delete | K::KP_Delete => "Delete",
        K::Insert | K::KP_Insert => "Insert",
        K::Up | K::KP_Up => "Up",
        K::Down | K::KP_Down => "Down",
        K::Left | K::KP_Left => "Left",
        K::Right | K::KP_Right => "Right",
        K::Home | K::KP_Home => "Home",
        K::End | K::KP_End => "End",
        K::Page_Up | K::KP_Page_Up => "PageUp",
        K::Page_Down | K::KP_Page_Down => "PageDown",
        K::Break | K::Pause => "Break",
        K::space => "Space",
        K::grave | K::dead_grave => "`",
        K::minus | K::KP_Subtract => "-",
        K::equal => "=",
        K::plus | K::KP_Add => "+",
        K::period | K::KP_Decimal => ".",
        K::comma => ",",
        K::semicolon => ";",
        K::slash | K::KP_Divide => "/",
        K::backslash => "\\",
        K::bracketleft => "[",
        K::bracketright => "]",
        K::apostrophe => "'",
        _ => "",
    };
    if !named.is_empty() {
        return Some(named.to_owned());
    }
    if let Some(n) = key.name() {
        let n = n.to_string();
        if n.len() > 1 && n.starts_with('F') && n[1..].parse::<u8>().is_ok() {
            return Some(n);
        }
    }
    key.to_unicode().filter(|c| c.is_alphanumeric()).map(|c| c.to_uppercase().to_string())
}

/// The key as VS sees it: the unshifted symbol of the physical key, so
/// `Ctrl+Shift+-` stays `-` and not `_`.
fn combo(window: &adw::ApplicationWindow, key: gdk::Key, code: u32, state: gdk::ModifierType) -> Option<Combo> {
    let base = gtk::prelude::WidgetExt::display(window)
        .translate_key(code, gdk::ModifierType::empty(), 0)
        .map(|(k, ..)| k)
        .filter(|k| key_name(*k).is_some())
        .unwrap_or(key);
    let name = key_name(base).or_else(|| key_name(key))?;
    if matches!(key, gdk::Key::Shift_L | gdk::Key::Shift_R | gdk::Key::Control_L | gdk::Key::Control_R | gdk::Key::Alt_L | gdk::Key::Alt_R | gdk::Key::Super_L | gdk::Key::Super_R) {
        return None;
    }
    Some(Combo {
        ctrl: state.contains(gdk::ModifierType::CONTROL_MASK),
        shift: state.contains(gdk::ModifierType::SHIFT_MASK),
        alt: state.contains(gdk::ModifierType::ALT_MASK),
        key: name,
    })
}

fn scope_of(window: &adw::ApplicationWindow) -> (Scope, bool) {
    let Some(focus) = GtkWindowExt::focus(window) else { return (Scope::Global, false) };
    let mut w = Some(focus.clone());
    while let Some(widget) = w {
        if widget.has_css_class("editor") {
            return (Scope::Editor, false);
        }
        if widget.has_css_class("explorer") {
            let typing = focus.is::<gtk::Text>();
            return (Scope::Explorer, typing);
        }
        if widget.has_css_class("designer") {
            // Class Details' fields are typing: Delete deletes a letter,
            // not the member's code.
            return (Scope::Designer, focus.is::<gtk::Text>());
        }
        // The terminal gets its keys (Ctrl+C, Ctrl+R, …) like any shell.
        if widget.is::<vte4::Terminal>() {
            return (Scope::Global, true);
        }
        w = widget.parent();
    }
    let typing = focus.is::<gtk::Text>() || focus.is::<gtk::TextView>();
    (Scope::Global, typing)
}

fn on_key(window: &adw::ApplicationWindow, _ctl: &gtk::EventControllerKey, key: gdk::Key, code: u32, state: gdk::ModifierType) -> glib::Propagation {
    let Some(app) = crate::ui::window::try_app() else { return glib::Propagation::Proceed };
    let Some(combo) = combo(window, key, code, state) else { return glib::Propagation::Proceed };
    let (scope, typing) = scope_of(window);
    // In an entry, plain keys belong to the entry.
    let pending = app.keymap.borrow().pending().is_some();
    if typing && !pending && !combo.ctrl && !combo.alt && !combo.key.starts_with('F') {
        return glib::Propagation::Proceed;
    }
    // Editor-owned keys first: Tab accepts ghost text, Escape dismisses.
    if scope == Scope::Editor && !pending && crate::ui::editor::handle_editor_key(&combo) {
        return glib::Propagation::Stop;
    }
    let feed = app.keymap.borrow_mut().feed(combo, scope);
    match feed {
        Feed::Command(id) => {
            app.status.set_chord(None);
            crate::ui::actions::dispatch(id);
            glib::Propagation::Stop
        }
        Feed::Pending(first) => {
            app.status.set_chord(Some(&trf("({}) was pressed. Waiting for second key of chord...", &[&first.label()])));
            glib::Propagation::Stop
        }
        Feed::NotACommand(chord) => {
            app.status.set_chord(None);
            app.status.flash(&trf("The key combination ({}) is not a command.", &[&chord.label()]));
            glib::Propagation::Stop
        }
        Feed::Unbound => glib::Propagation::Proceed,
    }
}

/// The title of a command in the UI language.
pub fn title(id: &str) -> String {
    anvil_core::commands::find(id).map(|c| tr(c.title)).unwrap_or_else(|| id.to_owned())
}

/// Tooltip text: title and shortcut.
pub fn tooltip(id: &str) -> String {
    let t = title(id);
    match crate::ui::window::try_app().and_then(|a| a.keymap.borrow().label(id)) {
        Some(k) => format!("{t} ({k})"),
        None => t,
    }
}
