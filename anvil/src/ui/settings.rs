//! Settings: General, Editor, Keyboard (VS' Options → Keyboard: search by
//! command or key, conflicts, overrides), AI and Tools.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use anvil_core::commands::COMMANDS;
use anvil_core::keymap::{Chord, Combo};
use gtk::{gdk, glib};

use crate::i18n::{tr, trf};

fn switch(title: &str, key: &str) -> adw::SwitchRow {
    let r = adw::SwitchRow::new();
    r.set_title(title);
    crate::ui::window::app().settings.bind(key, &r, "active").build();
    r
}

fn choice(title: &str, key: &'static str, nicks: &'static [&'static str], labels: &[String]) -> adw::ComboRow {
    let r = adw::ComboRow::new();
    r.set_title(title);
    let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
    r.set_model(Some(&gtk::StringList::new(&refs)));
    let cur = crate::ui::window::app().settings.string(key);
    r.set_selected(nicks.iter().position(|n| *n == cur.as_str()).unwrap_or(0) as u32);
    r.connect_selected_notify(move |r| {
        let app = crate::ui::window::app();
        let _ = app.settings.set_string(key, nicks[r.selected() as usize]);
        if key == crate::theme::KEY {
            crate::theme::apply(&app.settings);
        }
    });
    r
}

fn general_page() -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder().title(tr("General")).icon_name("preferences-system-symbolic").build();
    let g = adw::PreferencesGroup::new();
    let language = choice(&tr("Language"), "language", &["system", "en", "de"], &[tr("System"), "English".into(), "Deutsch".into()]);
    language.set_subtitle(&tr("Takes effect after a restart."));
    g.add(&language);
    g.add(&choice(&tr("Appearance"), crate::theme::KEY, &["system", "light", "dark"], &[tr("Follow System"), tr("Light"), tr("Dark")]));
    g.add(&choice(&tr("Format of new solutions"), "new-solution-format", &["slnx", "sln"], &[".slnx (Visual Studio 2026)".into(), ".sln".into()]));
    page.add(&g);
    page
}

fn editor_page() -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder().title(tr("Editor")).icon_name("accessories-text-editor-symbolic").build();
    let g = adw::PreferencesGroup::new();
    g.add(&switch(&tr("Word wrap"), "word-wrap"));
    g.add(&switch(&tr("View white space"), "show-whitespace"));
    g.add(&switch(&tr("Smooth scrolling and caret"), crate::ui::smooth::KEY));
    g.add(&switch(&tr("Show StyleCop rules in the Error List"), "show-stylecop"));
    // CodeLens and Git blame have keys but no view yet; their switches
    // come back with the views.
    page.add(&g);
    page
}

fn ai_page() -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder().title(tr("AI")).icon_name("starred-symbolic").build();
    let g = adw::PreferencesGroup::new();
    g.set_description(Some(&tr("Local models through Ollama only. Nothing leaves this machine. Both parts are off by default; the master switch turns everything off for exams.")));
    g.add(&switch(&tr("AI features"), "ai-enabled"));
    g.add(&switch(&tr("Inline completion (Tab accepts)"), "ai-inline"));
    g.add(&switch(&tr("Chat panel"), "ai-chat"));
    let model = adw::EntryRow::new();
    model.set_title(&tr("Model for inline completion (empty: the first coder model)"));
    crate::ui::window::app().settings.bind("ai-inline-model", &model, "text").build();
    g.add(&model);
    let url = adw::EntryRow::new();
    url.set_title(&tr("Ollama address"));
    crate::ui::window::app().settings.bind("ollama-url", &url, "text").build();
    g.add(&url);
    page.add(&g);
    page
}

fn tools_page() -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder().title(tr("Tools")).icon_name("applications-engineering-symbolic").build();
    let g = adw::PreferencesGroup::new();
    let home = crate::host::home();
    let roslyn = adw::ActionRow::builder().title(tr("C# language server (Roslyn)")).subtitle(if anvil_lsp::roslyn::is_installed(&home, anvil_lsp::roslyn::VERSION) { trf("{} installed", &[anvil_lsp::roslyn::VERSION]) } else { tr("Not installed; downloaded on first use") }).build();
    let dbg = adw::ActionRow::builder().title(tr("Debugger (netcoredbg)")).subtitle(if anvil_dap::launch::binary(&home, anvil_dap::launch::NETCOREDBG_VERSION).exists() { trf("{} installed", &[anvil_dap::launch::NETCOREDBG_VERSION]) } else { tr("Not installed; downloaded on first F5") }).build();
    g.add(&roslyn);
    g.add(&dbg);
    let sdk = adw::ActionRow::builder().title(tr(".NET SDK")).subtitle(tr("Checking…")).build();
    g.add(&sdk);
    glib::spawn_future_local(glib::clone!(
        #[weak]
        sdk,
        async move {
            let out = crate::host::run(anvil_core::host::Command::dotnet().arg("--version"), None).await;
            sdk.set_subtitle(&match out {
                Ok(o) if o.ok() => o.stdout_text().trim().to_owned(),
                _ => tr("Not found on this computer. Install the .NET 10 SDK from your distribution."),
            });
        }
    ));
    page.add(&g);
    page
}

fn keyboard_page(dialog: &adw::PreferencesDialog) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder().title(tr("Keyboard")).icon_name("input-keyboard-symbolic").name("keyboard").build();
    let g = adw::PreferencesGroup::new();
    g.set_description(Some(&tr("Visual Studio's default scheme for C#. Click a shortcut and press new keys; a chord takes two presses. Backspace removes, Escape cancels.")));
    let search = gtk::SearchEntry::builder().placeholder_text(tr("Search commands or keys (e.g. Ctrl+K)")).build();
    let conflicts = gtk::Label::builder().xalign(0.0).wrap(true).css_classes(["hint"]).build();
    let list = gtk::ListBox::new();
    list.add_css_class("boxed-list");
    let reset = gtk::Button::with_label(&tr("Reset to Visual Studio defaults"));
    reset.add_css_class("quiet");
    let header = gtk::Box::new(gtk::Orientation::Vertical, 8);
    header.append(&search);
    header.append(&conflicts);
    header.append(&reset);
    g.add(&header);
    g.add(&list);
    page.add(&g);

    let rows: Rc<RefCell<Vec<(adw::ActionRow, &'static str)>>> = Rc::default();
    let refresh = {
        let rows = rows.clone();
        let conflicts = conflicts.clone();
        move || {
            let app = crate::ui::window::app();
            let map = app.keymap.borrow();
            for (row, id) in rows.borrow().iter() {
                let label = map.chords(id).iter().map(Chord::label).collect::<Vec<_>>().join("  ·  ");
                if let Some(b) = row.activatable_widget().and_downcast::<gtk::Button>() {
                    b.set_label(&if label.is_empty() { tr("(none)") } else { label });
                }
            }
            let c = map.conflicts();
            conflicts.set_label(&if c.is_empty() { tr("No conflicts.") } else { c.iter().map(|(k, a, b)| trf("{} is bound to {} and {}", &[k, &crate::commands::title(a), &crate::commands::title(b)])).collect::<Vec<_>>().join("\n") });
            crate::ui::menus::rebuild_menubar();
        }
    };
    for c in COMMANDS {
        let row = adw::ActionRow::builder().title(tr(c.title)).subtitle(c.vs_name).build();
        let b = gtk::Button::new();
        b.add_css_class("flat");
        b.set_valign(gtk::Align::Center);
        row.add_suffix(&b);
        row.set_activatable_widget(Some(&b));
        let id = c.id;
        let refresh = refresh.clone();
        let dialog = dialog.clone();
        b.connect_clicked(move |b| capture(&dialog, b, id, refresh.clone()));
        list.append(&row);
        rows.borrow_mut().push((row, id));
    }
    refresh();
    {
        let rows = rows.clone();
        search.connect_search_changed(move |e| {
            let q = e.text().to_lowercase();
            let map = crate::ui::window::app().keymap.borrow().clone();
            let by_key: Vec<&str> = map.find_by_key(&q);
            for (row, id) in rows.borrow().iter() {
                let hit = q.is_empty() || row.title().to_lowercase().contains(&q) || row.subtitle().is_some_and(|s| s.to_lowercase().contains(&q)) || by_key.contains(id);
                row.set_visible(hit);
            }
        });
    }
    reset.connect_clicked(move |_| {
        let app = crate::ui::window::app();
        *app.keymap.borrow_mut() = anvil_core::keymap::Keymap::vs_default();
        crate::commands::save_keymap(&app.keymap.borrow());
        refresh();
    });
    page
}

/// Records one binding: a key, or two for a chord (the second within the
/// same dialog).
fn capture(dialog: &adw::PreferencesDialog, button: &gtk::Button, id: &'static str, refresh: impl Fn() + Clone + 'static) {
    let label = gtk::Label::builder().label(tr("Press the new shortcut…")).css_classes(["statement-small"]).build();
    let pressed = gtk::Label::builder().css_classes(["mono"]).build();
    let b = gtk::Box::new(gtk::Orientation::Vertical, 12);
    b.set_margin_top(24);
    b.set_margin_bottom(24);
    b.set_margin_start(24);
    b.set_margin_end(24);
    b.append(&label);
    b.append(&pressed);
    let chord_dialog = adw::Dialog::builder().title(crate::commands::title(id)).child(&b).content_width(420).build();
    let first: Rc<RefCell<Option<Combo>>> = Rc::default();
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let d = chord_dialog.clone();
    keys.connect_key_pressed(move |_, key, _, state| {
        if matches!(key, gdk::Key::Shift_L | gdk::Key::Shift_R | gdk::Key::Control_L | gdk::Key::Control_R | gdk::Key::Alt_L | gdk::Key::Alt_R) {
            return glib::Propagation::Stop;
        }
        let app = crate::ui::window::app();
        if key == gdk::Key::Escape && first.borrow().is_none() {
            d.close();
            return glib::Propagation::Stop;
        }
        if key == gdk::Key::BackSpace && first.borrow().is_none() && state.is_empty() {
            app.keymap.borrow_mut().set(id, Vec::new());
            crate::commands::save_keymap(&app.keymap.borrow());
            refresh();
            d.close();
            return glib::Propagation::Stop;
        }
        let name = key.to_unicode().filter(|c| c.is_alphanumeric()).map(|c| c.to_uppercase().to_string()).or_else(|| key.name().map(|n| n.to_string()));
        let Some(name) = name.and_then(|n| anvil_core::keymap::normalise_key(&n)) else { return glib::Propagation::Stop };
        let combo = Combo { ctrl: state.contains(gdk::ModifierType::CONTROL_MASK), shift: state.contains(gdk::ModifierType::SHIFT_MASK), alt: state.contains(gdk::ModifierType::ALT_MASK), key: name };
        let prev = first.borrow_mut().take();
        let chord = match prev {
            None if combo.ctrl || combo.alt => {
                // Maybe a chord: wait 1.5 s for a second key.
                pressed.set_label(&format!("{}, …", combo.label()));
                *first.borrow_mut() = Some(combo.clone());
                let (f, d2, refresh2) = (first.clone(), d.clone(), refresh.clone());
                glib::timeout_add_local_once(std::time::Duration::from_millis(1500), move || {
                    if let Some(c) = f.borrow_mut().take() {
                        commit(id, Chord { first: c, second: None });
                        refresh2();
                        d2.close();
                    }
                });
                return glib::Propagation::Stop;
            }
            None => Chord { first: combo, second: None },
            Some(f) => Chord { first: f, second: Some(combo) },
        };
        commit(id, chord);
        refresh();
        d.close();
        glib::Propagation::Stop
    });
    chord_dialog.add_controller(keys);
    chord_dialog.present(Some(dialog));
    let _ = button;
}

fn commit(id: &'static str, chord: Chord) {
    let app = crate::ui::window::app();
    app.keymap.borrow_mut().set(id, vec![chord]);
    crate::commands::save_keymap(&app.keymap.borrow());
}

pub fn show(page: Option<&str>) {
    let app = crate::ui::window::app();
    let dialog = adw::PreferencesDialog::new();
    dialog.set_title(&tr("Settings"));
    dialog.set_search_enabled(true);
    dialog.add(&general_page());
    dialog.add(&editor_page());
    let kb = keyboard_page(&dialog);
    dialog.add(&kb);
    dialog.add(&ai_page());
    dialog.add(&tools_page());
    if page == Some("keyboard") {
        dialog.set_visible_page(&kb);
    }
    dialog.present(Some(&app.window));
}
