//! The window: toolbar, editor area, bottom panel, side panel, status line,
//! and the one `App` every part reaches through [`app`].

use std::cell::{Cell, OnceCell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use adw::prelude::*;
use anvil_core::keymap::Keymap;
use anvil_core::session::Session;
use anvil_core::solution::Solution;
use gtk::{gio, glib};

use crate::i18n::{tr, trf};
use crate::ui::dock::{Dock, DockArea};
use crate::ui::{build, chat, debugger, editor, errorlist, explorer, find, git, lsp, menus, output, start, status, tabswitch, terminal, tests};
use crate::APP_ID;

pub struct App {
    pub window: adw::ApplicationWindow,
    pub settings: gio::Settings,
    pub keymap: RefCell<Keymap>,
    pub solution: RefCell<Option<Solution>>,
    pub session: RefCell<Session>,
    pub center: gtk::Stack,
    pub tabs: Rc<editor::Tabs>,
    pub explorer: Rc<explorer::Explorer>,
    /// The tool windows, docked left, right or at the bottom.
    pub dock: Rc<Dock>,
    pub switcher: tabswitch::Switcher,
    pub errors: Rc<errorlist::ErrorList>,
    pub output: Rc<output::Output>,
    pub terminal: Rc<terminal::Terminal>,
    pub find_results: Rc<find::FindResults>,
    pub status: status::StatusBar,
    pub lsp: Rc<lsp::Lsp>,
    pub builder: Rc<build::Builder>,
    pub debugger: Rc<debugger::Debugger>,
    pub tests: Rc<tests::TestExplorer>,
    pub git: Rc<git::GitPanel>,
    pub chat: Rc<chat::Chat>,
    pub start: start::StartPage,
    pub toolbar: Toolbar,
    pub toast: adw::ToastOverlay,
    pub watcher: crate::ui::watch::Watcher,
    /// Bumped whenever the solution changes, so late async results for an
    /// old solution are dropped.
    pub generation: Cell<u64>,
}

pub struct Toolbar {
    pub widget: gtk::Box,
    pub startup: gtk::DropDown,
    /// Set while Anvil fills the dropdown itself, so its `selected`
    /// notification does not count as the user's choice.
    pub filling_startup: Cell<bool>,
    pub start: gtk::Button,
    pub start_label: gtk::Label,
    busy: gtk::Spinner,
    start_icon: gtk::Image,
    pub debug_controls: gtk::Box,
    pub ai: gtk::ToggleButton,
}

impl Toolbar {
    /// A spinner in Start while it builds, downloads or attaches.
    pub fn set_busy(&self, on: bool) {
        self.busy.set_visible(on);
        self.busy.set_spinning(on);
        self.start_icon.set_visible(!on);
    }
}

thread_local! {
    static APP: OnceCell<Rc<App>> = const { OnceCell::new() };
}

/// The application state. Panics before the window exists, which would
/// be a bug in the start-up order.
pub fn app() -> Rc<App> {
    try_app().expect("the window exists")
}

pub fn try_app() -> Option<Rc<App>> {
    APP.with(|a| a.get().cloned())
}

fn icon_button(icon: &str, command: &str) -> gtk::Button {
    let b = gtk::Button::from_icon_name(icon);
    b.set_action_name(Some(&format!("win.{command}")));
    b.update_property(&[gtk::accessible::Property::Label(&crate::commands::title(command))]);
    b
}

fn build_toolbar() -> Toolbar {
    // Only what runs the program; Save, Undo and Redo stay in the menus
    // and on the keyboard.
    let widget = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    widget.add_css_class("run-bar");
    widget.set_valign(gtk::Align::Center);

    let build = icon_button("system-run-symbolic", "build.solution");

    let startup = gtk::DropDown::from_strings(&[]);
    startup.set_tooltip_text(Some(&tr("Startup Project")));
    startup.set_visible(false);

    // Start (F5), labelled with the startup project as VS does, with VS'
    // green triangle.
    let start_label = gtk::Label::new(Some(&tr("Start")));
    let start_icon = gtk::Image::from_icon_name("media-playback-start-symbolic");
    start_icon.add_css_class("run-icon");
    let busy = gtk::Spinner::new();
    busy.set_visible(false);
    let start_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    start_box.append(&start_icon);
    start_box.append(&busy);
    start_box.append(&start_label);
    let start = gtk::Button::builder().child(&start_box).css_classes(["start"]).valign(gtk::Align::Center).action_name("win.debug.start").build();

    let run = icon_button("media-skip-forward-symbolic", "debug.start-without");
    let hot = icon_button("power-profile-performance-symbolic", "debug.hot-reload");

    let debug_controls = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    for (icon, cmd) in [
        ("media-playback-pause-symbolic", "debug.break-all"),
        ("media-playback-stop-symbolic", "debug.stop"),
        ("view-refresh-symbolic", "debug.restart"),
        ("go-next-symbolic", "debug.step-over"),
        ("go-down-symbolic", "debug.step-into"),
        ("go-up-symbolic", "debug.step-out"),
    ] {
        debug_controls.append(&icon_button(icon, cmd));
    }
    debug_controls.set_visible(false);

    let ai = gtk::ToggleButton::builder().icon_name("starred-symbolic").valign(gtk::Align::Center).build();
    ai.add_css_class("header-pill");
    ai.set_tooltip_text(Some(&tr("AI")));

    for w in [build.upcast_ref::<gtk::Widget>(), startup.upcast_ref(), start.upcast_ref(), run.upcast_ref(), hot.upcast_ref(), debug_controls.upcast_ref()] {
        widget.append(w);
    }
    Toolbar { widget, startup, filling_startup: Cell::new(false), start, start_label, busy, start_icon, debug_controls, ai }
}

pub fn build(application: &adw::Application) -> Rc<App> {
    let settings = gio::Settings::new(APP_ID);
    let window = adw::ApplicationWindow::builder()
        .application(application)
        .title("Anvil")
        .default_width(settings.int("window-width"))
        .default_height(settings.int("window-height"))
        .maximized(settings.boolean("window-maximized"))
        .build();
    window.add_css_class("anvil");

    let toolbar = build_toolbar();
    let header = adw::HeaderBar::new();
    header.set_title_widget(Some(&toolbar.widget));
    let menubar = menus::menubar();
    header.pack_start(&menubar);
    let theme_button = gtk::Button::from_icon_name(if adw::StyleManager::default().is_dark() { "weather-clear-symbolic" } else { "weather-clear-night-symbolic" });
    theme_button.set_action_name(Some("win.view.toggle-theme"));
    theme_button.add_css_class("header-pill");
    theme_button.set_valign(gtk::Align::Center);
    theme_button.set_tooltip_text(Some(&tr("Light or Dark")));
    header.pack_end(&theme_button);
    header.pack_end(&toolbar.ai);
    header.pack_end(&crate::ui::palette_search::header_button());
    adw::StyleManager::default().connect_dark_notify(glib::clone!(
        #[weak]
        theme_button,
        move |sm| theme_button.set_icon_name(if sm.is_dark() { "weather-clear-symbolic" } else { "weather-clear-night-symbolic" })
    ));

    // Centre: start page or editor tabs.
    let tabs = Rc::new(editor::Tabs::new());
    let start = start::StartPage::new();
    let center = gtk::Stack::new();
    center.set_transition_type(gtk::StackTransitionType::Crossfade);
    center.set_transition_duration(crate::motion::STATE_MS);
    center.add_named(&start.widget, Some("start"));
    center.add_named(&tabs.widget, Some("work"));
    center.set_vexpand(true);

    // Tool windows: VS' places by default, the user's from the layout.
    let errors = Rc::new(errorlist::ErrorList::new());
    let output = Rc::new(output::Output::new());
    let terminal = Rc::new(terminal::Terminal::new());
    let find_results = Rc::new(find::FindResults::new());
    let tests = Rc::new(tests::TestExplorer::new());
    let debugger = Rc::new(debugger::Debugger::new());
    let explorer = Rc::new(explorer::Explorer::new());
    let git = Rc::new(git::GitPanel::new());
    let chat = Rc::new(chat::Chat::new());
    let dock = Dock::new(&center);
    dock.add(&errors.widget, "errors", &tr("Error List"), DockArea::Bottom);
    dock.add(&output.widget, "output", &tr("Output"), DockArea::Bottom);
    dock.add(&terminal.widget, "terminal", &tr("Terminal"), DockArea::Bottom);
    dock.add(&tests.widget, "tests", &tr("Test Explorer"), DockArea::Bottom);
    dock.add(&find_results.widget, "find", &tr("Find Results"), DockArea::Bottom);
    dock.add(&crate::ui::ide::references_widget(), "references", &tr("Find All References"), DockArea::Bottom);
    debugger.add_pages(&dock);
    dock.add(&explorer.widget, "explorer", &tr("Solution Explorer"), DockArea::Right);
    dock.add(&git.widget, "git", &tr("Git Changes"), DockArea::Right);
    dock.add(&chat.widget, "chat", &tr("AI Chat"), DockArea::Right);
    dock.set_available("chat", settings.boolean("ai-enabled") && settings.boolean("ai-chat"));
    let switcher = tabswitch::Switcher::new();

    let status = status::StatusBar::new();
    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&header);
    content.append(&dock.widget);
    content.append(&status.widget);
    // The Ctrl+Tab switcher floats over everything.
    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&content));
    overlay.add_overlay(&switcher.widget);
    let toast = adw::ToastOverlay::new();
    toast.set_child(Some(&overlay));
    window.set_content(Some(&toast));

    let app = Rc::new(App {
        window: window.clone(),
        settings: settings.clone(),
        keymap: RefCell::new(crate::commands::load_keymap()),
        solution: RefCell::new(None),
        session: RefCell::new(Session::default()),
        center,
        tabs,
        explorer,
        dock,
        switcher,
        errors,
        output,
        terminal,
        find_results,
        status,
        lsp: Rc::new(lsp::Lsp::new()),
        builder: Rc::new(build::Builder::new()),
        debugger,
        tests,
        git,
        chat,
        start,
        toolbar,
        toast,
        watcher: crate::ui::watch::Watcher::new(),
        generation: Cell::new(0),
    });
    APP.with(|a| {
        let _ = a.set(app.clone());
    });

    crate::commands::install(&app);
    app.toolbar.widget.set_tooltip_text(None);
    app.toolbar.start.set_tooltip_text(Some(&crate::commands::tooltip("debug.start")));
    app.tabs.connect();
    app.switcher.connect(&app.window);
    crate::ui::density::apply(&app.window, &app.settings);
    app.explorer.connect();
    app.errors.connect();
    app.start.connect();
    app.chat.connect();
    app.git.connect();
    app.tests.connect();
    app.debugger.connect();
    app.toolbar.startup.connect_selected_notify(|dd| {
        let app = crate::ui::window::app();
        if app.toolbar.filling_startup.get() {
            return;
        }
        let Some(name) = dd.selected_item().and_downcast::<gtk::StringObject>().map(|s| s.string().to_string()) else { return };
        let changed = app.session.borrow().startup_project.as_deref() != Some(name.as_str());
        if changed {
            set_startup_project(&name);
        }
    });
    app.settings.bind("ai-enabled", &app.toolbar.ai, "active").build();
    app.settings.connect_changed(None, |s, key| {
        if key.starts_with("ai-") {
            let app = crate::ui::window::app();
            let on = s.boolean("ai-enabled");
            app.dock.set_available("chat", on && s.boolean("ai-chat"));
        }
        if key == crate::ui::density::KEY {
            let app = crate::ui::window::app();
            crate::ui::density::apply(&app.window, s);
        }
    });

    window.connect_close_request(
        move |w| {
            let s = gio::Settings::new(APP_ID);
            let (width, height) = (w.width(), w.height());
            if !w.is_maximized() {
                let _ = s.set_int("window-width", width);
                let _ = s.set_int("window-height", height);
            }
            let _ = s.set_boolean("window-maximized", w.is_maximized());
            if !confirm_close() {
                return glib::Propagation::Stop;
            }
            save_session();
            crate::ui::window::app().lsp.stop();
            glib::Propagation::Proceed
        }
    );
    app.center.set_visible_child_name("start");
    if let Some(l) = anvil_core::session::Layout::from_json(&app.settings.string(crate::ui::session::LAYOUT_KEY)) {
        app.dock.apply(&l);
    }
    app.start.refresh();
    set_workspace(false);
    app
}

/// The start page stands alone, as VS' start window does; the panels and
/// the run buttons come with a solution or a file.
pub fn set_workspace(on: bool) {
    let app = crate::ui::window::app();
    app.toolbar.widget.set_visible(on);
    app.dock.set_workspace(on);
}

/// Unsaved files: VS asks once for all of them.
fn confirm_close() -> bool {
    let app = crate::ui::window::app();
    let dirty = app.tabs.dirty_names();
    if dirty.is_empty() {
        return true;
    }
    let dialog = adw::AlertDialog::new(Some(&tr("Save changes?")), Some(&dirty.join("\n")));
    dialog.add_responses(&[("cancel", &tr("Cancel")), ("discard", &tr("Don't Save")), ("save", &tr("Save"))]);
    dialog.set_response_appearance("discard", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("save"));
    dialog.choose(Some(&app.window), gio::Cancellable::NONE, |r| {
        let app = crate::ui::window::app();
        match r.as_str() {
            "save" => {
                if app.tabs.save_all() {
                    app.tabs.mark_all_clean();
                    app.window.close();
                }
            }
            "discard" => {
                app.tabs.mark_all_clean();
                app.window.close();
            }
            _ => {}
        }
    });
    false
}

pub fn save_session() {
    let app = crate::ui::window::app();
    let Some(dir) = app.solution.borrow().as_ref().map(|s| s.dir.clone()) else { return };
    {
        let mut s = app.session.borrow_mut();
        crate::ui::session::capture(&app, &mut s);
        s.breakpoints = app.debugger.breakpoints();
    }
    if let Err(e) = app.session.borrow().save(&dir) {
        app.output.append("Build", &format!("{}: {e}\n", tr("Could not save the session state")));
    }
}

/// Opens a solution, a folder with one, a project or a single file.
pub fn open_path(path: &Path) {
    let app = crate::ui::window::app();
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    if path.is_dir() {
        match Solution::find_in(path) {
            Some(s) => open_solution(&s),
            None => app.status.flash(&tr("This folder holds no .slnx or .sln.")),
        }
    } else if matches!(ext.as_str(), "slnx" | "sln") {
        open_solution(path);
    } else if ext == "csproj" {
        // VS opens a project in a solution of its own; Anvil looks for one
        // next to it or above.
        let mut dir = path.parent();
        while let Some(d) = dir {
            if let Some(s) = Solution::find_in(d) {
                open_solution(&s);
                return;
            }
            dir = d.parent();
        }
        app.status.flash(&tr("No solution found for this project."));
    } else {
        app.center.set_visible_child_name("work");
        set_workspace(true);
        app.tabs.open(path, false);
    }
}

pub fn open_solution(path: &Path) {
    crate::ui::jobs::trace("open solution: start");
    let app = crate::ui::window::app();
    if app.solution.borrow().is_some() {
        save_session();
        // Nothing typed is lost when switching solutions: a file that
        // cannot be saved keeps the old solution open.
        if !app.tabs.save_all() {
            return;
        }
        app.tabs.close_all();
        app.lsp.stop();
        app.debugger.stop();
        app.errors.clear_all();
    }
    let solution = match Solution::open(path) {
        Ok(s) => s,
        Err(e) => {
            show_error(&tr("The solution could not be opened"), &e.to_string());
            return;
        }
    };
    app.generation.set(app.generation.get() + 1);
    remember_recent(path);
    crate::ui::jobs::trace("open solution: parsed");
    let session = Session::load(&solution.dir);
    app.window.set_title(Some(&format!("{} - Anvil", solution.name)));
    *app.session.borrow_mut() = session.clone();
    *app.solution.borrow_mut() = Some(solution);
    crate::ui::jobs::trace("open solution: session loaded");
    app.explorer.reload();
    refresh_startup_dropdown();
    app.debugger.load_breakpoints(&session.breakpoints);
    crate::ui::jobs::trace("open solution: explorer + breakpoints done");
    app.center.set_visible_child_name("work");
    let layout = crate::ui::session::layout_for(&app, &session);
    crate::ui::session::apply_window(&app, &layout);
    app.dock.apply(&layout);
    set_workspace(true);
    crate::ui::session::restore_tabs(&app, &session);
    crate::ui::jobs::trace("open solution: open files done");
    app.watcher.watch(solution_dir().as_deref());
    app.lsp.reset_restarts();
    app.lsp.start_for_solution();
    crate::ui::jobs::trace("open solution: watcher + lsp start done");
    app.git.refresh();
    crate::ui::jobs::trace("open solution: git done");
    app.tests.discover();
    crate::ui::jobs::trace("open solution: tests discover done");
    warn_startup_mismatch();
    crate::ui::jobs::trace("open solution: done (main thread free)");
    // Anvil does not install the SDK; it says so plainly.
    glib::spawn_future_local(async {
        if !crate::host::has("dotnet").await {
            crate::ui::window::show_error(&tr("The .NET SDK was not found"), &tr("Anvil builds, runs and analyses with the host's dotnet. Install the .NET 10 SDK from your distribution (for example `sudo dnf install dotnet-sdk-10.0`)."));
        }
    });
}

pub fn close_solution() {
    let app = crate::ui::window::app();
    if app.solution.borrow().is_none() {
        return;
    }
    save_session();
    // As when switching: nothing typed is lost.
    if !app.tabs.save_all() {
        return;
    }
    app.tabs.close_all();
    app.lsp.stop();
    app.debugger.stop();
    app.watcher.watch(None);
    *app.solution.borrow_mut() = None;
    app.generation.set(app.generation.get() + 1);
    app.explorer.reload();
    app.errors.clear_all();
    app.window.set_title(Some("Anvil"));
    app.center.set_visible_child_name("start");
    set_workspace(false);
    app.start.refresh();
    refresh_startup_dropdown();
}

fn remember_recent(path: &Path) {
    let app = crate::ui::window::app();
    let p = path.to_string_lossy().to_string();
    let mut list: Vec<String> = app.settings.strv("recent-solutions").iter().map(|s| s.to_string()).filter(|s| *s != p).collect();
    list.insert(0, p);
    list.truncate(12);
    let refs: Vec<&str> = list.iter().map(String::as_str).collect();
    let _ = app.settings.set_strv("recent-solutions", refs.as_slice());
}

pub fn refresh_startup_dropdown() {
    let app = crate::ui::window::app();
    let names: Vec<String> = app.solution.borrow().as_ref().map(|s| s.projects.iter().map(|p| p.name.clone()).collect()).unwrap_or_default();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let model = gtk::StringList::new(&refs);
    // `set_model` selects row 0 and notifies; that is not a choice.
    app.toolbar.filling_startup.set(true);
    app.toolbar.startup.set_model(Some(&model));
    let startup = startup_project_name();
    if let Some(i) = startup.and_then(|n| names.iter().position(|x| *x == n)) {
        app.toolbar.startup.set_selected(i as u32);
    }
    app.toolbar.filling_startup.set(false);
    // The project is named on Start already; the choice is only shown
    // when there is one to make.
    app.toolbar.startup.set_visible(names.len() > 1);
    app.toolbar.start_label.set_label(&startup_project_name().unwrap_or_else(|| tr("Start")));
}

/// The startup project: the user's choice, else the first project that
/// can run (VS would take the first even when it is a class library).
pub fn startup_project_name() -> Option<String> {
    let app = crate::ui::window::app();
    let sol = app.solution.borrow();
    let sol = sol.as_ref()?;
    app.session.borrow().startup_project.clone().filter(|n| sol.project_named(n).is_some()).or_else(|| sol.default_startup().map(|p| p.name.clone()))
}

pub fn set_startup_project(name: &str) {
    let app = crate::ui::window::app();
    app.session.borrow_mut().startup_project = Some(name.to_owned());
    // The dropdown follows a choice made in the Solution Explorer; while
    // debugging, Start keeps saying Continue.
    let index = app.solution.borrow().as_ref().and_then(|s| s.projects.iter().position(|p| p.name == name));
    if let Some(i) = index
        && app.toolbar.startup.selected() != i as u32
    {
        app.toolbar.filling_startup.set(true);
        app.toolbar.startup.set_selected(i as u32);
        app.toolbar.filling_startup.set(false);
    }
    if !app.debugger.is_active() {
        app.toolbar.start_label.set_label(name);
    }
    save_session();
    app.explorer.reload();
    warn_startup_mismatch();
}

/// VS keeps the startup project in its `.suo`, which Anvil cannot write:
/// on the lecturer's PC VS starts the first project of the solution.
fn warn_startup_mismatch() {
    let app = crate::ui::window::app();
    let first = app.solution.borrow().as_ref().and_then(|s| s.first_project().map(|p| p.name.clone()));
    let chosen = app.session.borrow().startup_project.clone();
    if let (Some(first), Some(chosen)) = (first, chosen)
        && first != chosen
    {
        let toast = adw::Toast::new(&trf("Visual Studio will start {} (the first project). Move {} first in the solution?", &[&first, &chosen]));
        toast.set_button_label(Some(&tr("Move First")));
        toast.set_timeout(10);
        toast.connect_button_clicked(move |_| build::move_project_first(&chosen));
        app.toast.add_toast(toast);
    }
}

/// A failure, without a modal dialog: a toast, details in Output › General
/// (see [`crate::ui::jobs::error`]).
pub fn show_error(title: &str, detail: &str) {
    crate::ui::jobs::error("General", title, detail);
}

pub fn toast(text: &str) {
    crate::ui::jobs::info(text);
}

/// Shows a tool window wherever it docks and opens its area. The names
/// are kept from when there was one bottom and one side panel.
pub fn show_bottom(name: &str) {
    crate::ui::window::app().dock.show(name);
}

pub fn show_side(name: &str) {
    crate::ui::window::app().dock.show(name);
}

pub fn solution_dir() -> Option<PathBuf> {
    crate::ui::window::app().solution.borrow().as_ref().map(|s| s.dir.clone())
}
