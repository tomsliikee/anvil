//! Solution Explorer: solution → projects → Dependencies, folders, files,
//! with VS' context menus. Files are what SDK-style globbing includes
//! (`**/*` minus `bin/`, `obj/` and `<Compile Remove>`).

use std::cell::{Cell, RefCell};
use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use adw::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, gio, glib};

use crate::i18n::{tr, trf};
use crate::ui::menus::Item;
use crate::ui::window::{self};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    Solution,
    SolutionFolder,
    Project,
    Dependencies,
    DepGroup(String),
    Dependency,
    Folder,
    File,
}

#[derive(Clone, Debug)]
pub struct NodeData {
    pub kind: Kind,
    pub name: String,
    pub path: PathBuf,
    /// The owning project's name (empty for the solution).
    pub project: String,
    pub detail: String,
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct Node {
        pub data: RefCell<Option<NodeData>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Node {
        const NAME: &'static str = "AnvilExplorerNode";
        type Type = super::Node;
    }

    impl ObjectImpl for Node {}
}

glib::wrapper! {
    pub struct Node(ObjectSubclass<imp::Node>);
}

impl Node {
    fn new(d: NodeData) -> Node {
        let n: Node = glib::Object::new();
        *n.imp().data.borrow_mut() = Some(d);
        n
    }

    pub fn data(&self) -> NodeData {
        self.imp().data.borrow().clone().expect("set at creation")
    }
}

pub struct Explorer {
    pub widget: gtk::Box,
    list: gtk::ListView,
    selection: gtk::SingleSelection,
    expanded: RefCell<BTreeSet<PathBuf>>,
    /// The solution the expanded set belongs to.
    root: RefCell<PathBuf>,
    pub search: gtk::SearchEntry,
    /// VS' "Track Active Item": the current tab's file is selected and
    /// revealed.
    pub track: gtk::ToggleButton,
    no_match: gtk::Label,
    /// While filtering: the paths that stay (matches and their parents).
    keep: RefCell<Option<HashSet<PathBuf>>>,
    query: RefCell<String>,
    /// The path whose label is being edited inline (F2).
    editing: RefCell<Option<PathBuf>>,
    /// Set while Anvil selects a row itself, so no preview tab opens.
    quiet: Cell<bool>,
    prompt: gtk::Revealer,
    prompt_label: gtk::Label,
    prompt_yes: gtk::Button,
    prompt_no: gtk::Button,
    answer: RefCell<Option<Box<dyn FnOnce(bool)>>>,
}

fn is_hidden(name: &str) -> bool {
    name.starts_with('.') || name.eq_ignore_ascii_case("bin") || name.eq_ignore_ascii_case("obj")
}

fn children(d: &NodeData) -> Vec<NodeData> {
    let mut out = all_children(d);
    let app = crate::ui::window::app();
    if let Some(keep) = app.explorer.keep.borrow().as_ref() {
        out.retain(|c| keep.contains(&c.path));
    }
    out
}

fn all_children(d: &NodeData) -> Vec<NodeData> {
    let app = crate::ui::window::app();
    let sol = app.solution.borrow();
    let Some(sol) = sol.as_ref() else { return Vec::new() };
    match &d.kind {
        Kind::Solution => {
            let mut out = Vec::new();
            for f in &sol.folders {
                let name = f.trim_matches('/').rsplit('/').next().unwrap_or(f).to_owned();
                out.push(NodeData { kind: Kind::SolutionFolder, name, path: sol.dir.join(f.trim_matches('/')), project: String::new(), detail: f.clone() });
            }
            for p in sol.projects.iter().filter(|p| p.folder.is_none()) {
                out.push(project_node(p));
            }
            out
        }
        Kind::SolutionFolder => sol.projects.iter().filter(|p| p.folder.as_deref() == Some(d.detail.as_str())).map(project_node).collect(),
        Kind::Project => {
            let mut out = vec![NodeData { kind: Kind::Dependencies, name: tr("Dependencies"), path: d.path.join(".deps"), project: d.name.clone(), detail: String::new() }];
            out.extend(dir_children(&d.path, &d.name, sol.project_named(&d.name)));
            out
        }
        Kind::Dependencies => {
            let Some(p) = sol.project_named(&d.project) else { return Vec::new() };
            let mut out = vec![NodeData { kind: Kind::DepGroup("frameworks".into()), name: tr("Frameworks"), path: d.path.join("f"), project: d.project.clone(), detail: String::new() }];
            if p.csproj.as_ref().is_some_and(|c| !c.package_refs().is_empty()) {
                out.push(NodeData { kind: Kind::DepGroup("packages".into()), name: tr("Packages"), path: d.path.join("p"), project: d.project.clone(), detail: String::new() });
            }
            if p.csproj.as_ref().is_some_and(|c| !c.project_refs().is_empty()) {
                out.push(NodeData { kind: Kind::DepGroup("projects".into()), name: tr("Projects"), path: d.path.join("r"), project: d.project.clone(), detail: String::new() });
            }
            out
        }
        Kind::DepGroup(g) => {
            let Some(p) = sol.project_named(&d.project) else { return Vec::new() };
            let Some(c) = p.csproj.as_ref() else { return Vec::new() };
            match g.as_str() {
                "packages" => c.package_refs().into_iter().map(|r| NodeData { kind: Kind::Dependency, name: format!("{} ({})", r.id, r.version.unwrap_or_default()), path: d.path.join(&r.id), project: d.project.clone(), detail: r.id }).collect(),
                "projects" => c.project_refs().into_iter().map(|r| NodeData { kind: Kind::Dependency, name: anvil_core::slnx::stem(&r), path: d.path.join(&r), project: d.project.clone(), detail: r }).collect(),
                _ => vec![NodeData { kind: Kind::Dependency, name: "Microsoft.NETCore.App".into(), path: d.path.join("netcore"), project: d.project.clone(), detail: c.target_framework().unwrap_or_default() }],
            }
        }
        Kind::Folder => dir_children(&d.path, &d.project, sol.project_named(&d.project)),
        _ => Vec::new(),
    }
}

fn project_node(p: &anvil_core::solution::Project) -> NodeData {
    NodeData { kind: Kind::Project, name: p.name.clone(), path: p.dir.clone(), project: p.name.clone(), detail: p.path.to_string_lossy().into_owned() }
}

fn dir_children(dir: &Path, project: &str, p: Option<&anvil_core::solution::Project>) -> Vec<NodeData> {
    let removes: Vec<String> = p.and_then(|p| p.csproj.as_ref()).map(|c| c.compile_removes()).unwrap_or_default();
    let root = p.map(|p| p.dir.clone()).unwrap_or_default();
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut dirs = Vec::new();
    let mut files = Vec::new();
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let path = e.path();
        if is_hidden(&name) || name.ends_with(".anvil-tmp") {
            continue;
        }
        let rel = path.strip_prefix(&root).map(|r| r.to_string_lossy().replace('\\', "/")).unwrap_or_default();
        if removes.iter().any(|r| r.eq_ignore_ascii_case(&rel)) {
            continue;
        }
        if path.is_dir() {
            dirs.push(NodeData { kind: Kind::Folder, name, path, project: project.to_owned(), detail: String::new() });
        } else if !name.ends_with(".csproj") && !name.ends_with(".user") {
            files.push(NodeData { kind: Kind::File, name, path, project: project.to_owned(), detail: String::new() });
        }
    }
    dirs.sort_by_key(|d| d.name.to_lowercase());
    files.sort_by_key(|d| d.name.to_lowercase());
    dirs.extend(files);
    dirs
}

fn icon(d: &NodeData) -> &'static str {
    match &d.kind {
        Kind::Solution => "view-grid-symbolic",
        Kind::SolutionFolder | Kind::Folder => "folder-symbolic",
        Kind::Project => "applications-engineering-symbolic",
        Kind::Dependencies | Kind::DepGroup(_) => "package-x-generic-symbolic",
        Kind::Dependency => "package-x-generic-symbolic",
        Kind::File => {
            if d.name.ends_with(".cd") {
                "view-app-grid-symbolic"
            } else if d.name.ends_with(".json") || d.name.ends_with(".xml") || d.name.ends_with(".config") {
                "text-x-generic-symbolic"
            } else {
                "text-x-script-symbolic"
            }
        }
    }
}

impl Explorer {
    pub fn new() -> Explorer {
        let selection = gtk::SingleSelection::new(None::<gio::ListModel>);
        selection.set_autoselect(false);
        selection.set_can_unselect(true);
        let factory = gtk::SignalListItemFactory::new();
        factory.connect_setup(|_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().expect("list item");
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            let image = gtk::Image::new();
            image.add_css_class("tree-icon");
            let name = gtk::Label::builder().xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).css_classes(["name"]).build();
            // Inline rename (F2): the label gives way to an entry.
            let entry = gtk::Entry::builder().visible(false).hexpand(true).css_classes(["rename"]).build();
            let detail = gtk::Label::builder().xalign(0.0).css_classes(["count"]).build();
            row.append(&image);
            row.append(&name);
            row.append(&entry);
            row.append(&detail);
            let expander = gtk::TreeExpander::new();
            expander.set_child(Some(&row));
            item.set_child(Some(&expander));
            entry.connect_activate(|e| finish_rename(e, true));
            let esc = gtk::EventControllerKey::new();
            esc.connect_key_pressed(|k, key, _, _| {
                if key == gdk::Key::Escape
                    && let Some(e) = k.widget().and_downcast::<gtk::Entry>()
                {
                    finish_rename(&e, false);
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Proceed
            });
            entry.add_controller(esc);
            // Clicking elsewhere commits, as in VS.
            let focus = gtk::EventControllerFocus::new();
            focus.connect_leave(|f| {
                if let Some(e) = f.widget().and_downcast::<gtk::Entry>() {
                    finish_rename(&e, true);
                }
            });
            entry.add_controller(focus);
            // Right-click: select the row, then VS' menu for its kind.
            let click = gtk::GestureClick::new();
            click.set_button(gdk::BUTTON_SECONDARY);
            let li = item.downgrade();
            click.connect_pressed(move |g, _, x, y| {
                let Some(li) = li.upgrade() else { return };
                let app = crate::ui::window::app();
                app.explorer.selection.set_selected(li.position());
                if let Some(w) = g.widget() {
                    context_menu(&w, x, y);
                }
            });
            expander.add_controller(click);
            // Files and folders drag: onto a diagram (types of a .cs file,
            // copy) or onto another folder or project (move).
            let drag = gtk::DragSource::new();
            drag.set_actions(gdk::DragAction::COPY | gdk::DragAction::MOVE);
            let li = item.downgrade();
            drag.connect_prepare(move |_, _, _| {
                let li = li.upgrade()?;
                let row = li.item().and_downcast::<gtk::TreeListRow>()?;
                let node = row.item().and_downcast::<Node>()?;
                let d = node.data();
                matches!(d.kind, Kind::File | Kind::Folder).then(|| gdk::ContentProvider::for_value(&d.path.to_string_lossy().to_value()))
            });
            expander.add_controller(drag);
            let drop = gtk::DropTarget::new(glib::types::Type::STRING, gdk::DragAction::MOVE);
            drop.connect_drop(|t, v, _, _| {
                let Some(target) = t.widget().and_downcast::<gtk::TreeExpander>().and_then(|e| e.item()).and_downcast::<Node>() else { return false };
                let Ok(src) = v.get::<String>() else { return false };
                let Some((from, to)) = drop_plan(Path::new(&src), &target.data()) else { return false };
                // After the drop has finished, not inside GTK's handler.
                glib::idle_add_local_once(move || move_by_user(from, to));
                true
            });
            expander.add_controller(drop);
        });
        factory.connect_bind(|_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().expect("list item");
            let Some(row) = item.item().and_downcast::<gtk::TreeListRow>() else { return };
            let Some(expander) = item.child().and_downcast::<gtk::TreeExpander>() else { return };
            expander.set_list_row(Some(&row));
            let Some(node) = row.item().and_downcast::<Node>() else { return };
            let d = node.data();
            let b = expander.child().and_downcast::<gtk::Box>().expect("row box");
            let image = b.first_child().and_downcast::<gtk::Image>().expect("icon");
            let name = image.next_sibling().and_downcast::<gtk::Label>().expect("name");
            let entry = name.next_sibling().and_downcast::<gtk::Entry>().expect("entry");
            let detail = entry.next_sibling().and_downcast::<gtk::Label>().expect("detail");
            image.set_icon_name(Some(icon(&d)));
            let app = crate::ui::window::app();
            match highlight(&d.name, &app.explorer.query.borrow()) {
                Some(markup) => name.set_markup(&markup),
                None => name.set_text(&d.name),
            }
            // A recycled row must not show another row's rename.
            let editing = app.explorer.editing.borrow().as_ref() == Some(&d.path);
            entry.set_visible(editing);
            name.set_visible(!editing);
            let startup = crate::ui::window::startup_project_name();
            name.remove_css_class("project-name");
            if d.kind == Kind::Project && startup.as_deref() == Some(d.name.as_str()) {
                name.add_css_class("project-name");
            }
            detail.set_label(match &d.kind {
                Kind::Solution => &d.detail,
                Kind::Dependency if !d.detail.starts_with("net") => "",
                Kind::Dependency => &d.detail,
                _ => "",
            });
        });
        let list = gtk::ListView::new(Some(selection.clone()), Some(factory));
        list.add_css_class("tree");
        list.add_css_class("explorer");
        list.set_single_click_activate(false);
        let scroller = gtk::ScrolledWindow::builder().child(&list).vexpand(true).hscrollbar_policy(gtk::PolicyType::Never).build();

        let search = gtk::SearchEntry::builder().hexpand(true).placeholder_text(tr("Search Solution Explorer (Ctrl+;)")).build();
        let track = gtk::ToggleButton::builder().icon_name("find-location-symbolic").active(true).tooltip_text(tr("Track Active Item")).css_classes(["flat"]).build();
        let bar = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        bar.add_css_class("explorer-bar");
        bar.set_margin_start(6);
        bar.set_margin_end(6);
        bar.set_margin_top(6);
        bar.set_margin_bottom(4);
        bar.append(&search);
        bar.append(&track);

        // A small question under the search field instead of a dialog.
        let prompt_label = gtk::Label::builder().wrap(true).xalign(0.0).build();
        let prompt_yes = gtk::Button::new();
        let prompt_no = gtk::Button::new();
        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        buttons.set_halign(gtk::Align::End);
        buttons.append(&prompt_no);
        buttons.append(&prompt_yes);
        let prompt_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
        prompt_box.add_css_class("explorer-prompt");
        prompt_box.append(&prompt_label);
        prompt_box.append(&buttons);
        let prompt = gtk::Revealer::builder().child(&prompt_box).reveal_child(false).transition_type(gtk::RevealerTransitionType::SlideDown).transition_duration(150).build();
        prompt_yes.connect_clicked(|_| answer_prompt(true));
        prompt_no.connect_clicked(|_| answer_prompt(false));
        let esc = gtk::EventControllerKey::new();
        esc.connect_key_pressed(|_, key, _, _| {
            if key == gdk::Key::Escape {
                answer_prompt(false);
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        prompt_box.add_controller(esc);

        let no_match = gtk::Label::builder().label(tr("No matches")).visible(false).css_classes(["count"]).margin_top(12).build();
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 0);
        widget.add_css_class("explorer");
        widget.append(&bar);
        widget.append(&prompt);
        widget.append(&no_match);
        widget.append(&scroller);
        Explorer {
            widget,
            list,
            selection,
            expanded: RefCell::default(),
            root: RefCell::default(),
            search,
            track,
            no_match,
            keep: RefCell::default(),
            query: RefCell::default(),
            editing: RefCell::default(),
            quiet: Cell::new(false),
            prompt,
            prompt_label,
            prompt_yes,
            prompt_no,
            answer: RefCell::default(),
        }
    }

    pub fn connect(&self) {
        // Single click: preview tab, as VS' preview.
        self.selection.connect_selected_item_notify(|s| {
            let app = crate::ui::window::app();
            if app.explorer.quiet.get() {
                return;
            }
            let Some(node) = s.selected_item().and_downcast::<gtk::TreeListRow>().and_then(|r| r.item()).and_downcast::<Node>() else { return };
            let d = node.data();
            if d.kind == Kind::File {
                app.tabs.open(&d.path, true);
            }
        });
        self.list.connect_activate(|_, pos| {
            let app = crate::ui::window::app();
            let Some(row) = app.explorer.selection.item(pos).and_downcast::<gtk::TreeListRow>() else { return };
            let Some(node) = row.item().and_downcast::<Node>() else { return };
            let d = node.data();
            match d.kind {
                Kind::File => {
                    app.tabs.open(&d.path, false);
                }
                _ => row.set_expanded(!row.is_expanded()),
            }
        });
        // SearchEntry waits 150 ms after the last key by itself.
        self.search.connect_search_changed(|e| crate::ui::window::app().explorer.set_filter(e.text().trim()));
        self.search.connect_stop_search(|e| {
            e.set_text("");
            crate::ui::window::app().explorer.list.grab_focus();
        });
        // Enter or Down goes from the search field into the results.
        self.search.connect_activate(|_| {
            let app = crate::ui::window::app();
            app.explorer.list.grab_focus();
        });
        // After the tab is registered: the first tab of an empty view is
        // selected while it is appended, before the tabs know its file.
        crate::ui::window::app().tabs.view.connect_selected_page_notify(|_| {
            glib::idle_add_local_once(|| {
                let app = crate::ui::window::app();
                if let Some(p) = app.tabs.current_path() {
                    app.explorer.reveal(&p);
                }
            });
        });
        self.track.connect_toggled(|t| {
            if t.is_active()
                && let Some(p) = crate::ui::window::app().tabs.current_path()
            {
                crate::ui::window::app().explorer.reveal(&p);
            }
        });
    }

    fn root_model(&self) -> Option<gtk::TreeListModel> {
        self.selection.model().and_downcast::<gtk::TreeListModel>()
    }

    fn remember_expanded(&self) {
        // The filtered tree is all open; that is not the user's state.
        if self.keep.borrow().is_some() {
            return;
        }
        let Some(model) = self.root_model() else { return };
        let mut set = BTreeSet::new();
        for i in 0..model.n_items() {
            if let Some(row) = model.item(i).and_downcast::<gtk::TreeListRow>()
                && row.is_expanded()
                && let Some(n) = row.item().and_downcast::<Node>()
            {
                set.insert(n.data().path);
            }
        }
        if !set.is_empty() {
            *self.expanded.borrow_mut() = set;
        }
    }

    /// Rebuilds the tree from disk, keeping what was expanded.
    pub fn reload(&self) {
        self.remember_expanded();
        let app = crate::ui::window::app();
        let sol = app.solution.borrow().clone();
        let Some(sol) = sol else {
            self.selection.set_model(None::<&gio::ListModel>);
            return;
        };
        // Another solution starts with solution and projects open, as VS.
        if *self.root.borrow() != sol.path {
            self.expanded.borrow_mut().clear();
            *self.root.borrow_mut() = sol.path.clone();
        }
        if !self.query.borrow().is_empty() {
            *self.keep.borrow_mut() = Some(filter_keep(&sol, &self.query.borrow()));
        }
        let root = gio::ListStore::new::<Node>();
        let count = sol.projects.len();
        root.append(&Node::new(NodeData {
            kind: Kind::Solution,
            name: trf("Solution '{}'", &[&sol.name]),
            path: sol.dir.clone(),
            project: String::new(),
            detail: crate::i18n::ntr("({} of {} project)", "({} of {} projects)", count as u32).replacen("{}", &count.to_string(), 2),
        }));
        let model = gtk::TreeListModel::new(root, false, false, |item| {
            let node = item.downcast_ref::<Node>()?;
            let kids = children(&node.data());
            if kids.is_empty() {
                return None;
            }
            let store = gio::ListStore::new::<Node>();
            for k in kids {
                store.append(&Node::new(k));
            }
            Some(store.upcast())
        });
        self.quiet.set(true);
        self.selection.set_model(Some(&model));
        let filtering = self.keep.borrow().is_some();
        let expanded = self.expanded.borrow().clone();
        let mut i = 0;
        while i < model.n_items() {
            if let Some(row) = model.item(i).and_downcast::<gtk::TreeListRow>()
                && let Some(n) = row.item().and_downcast::<Node>()
            {
                let d = n.data();
                let open = filtering && d.kind != Kind::Dependencies || matches!(d.kind, Kind::Solution | Kind::SolutionFolder | Kind::Project) && expanded.is_empty() || expanded.contains(&d.path);
                if open {
                    row.set_expanded(true);
                }
            }
            i += 1;
        }
        self.no_match.set_visible(filtering && model.n_items() <= 1);
        self.selection.set_selected(gtk::INVALID_LIST_POSITION);
        self.quiet.set(false);
    }

    /// Filters the tree to names containing `q` (case-insensitive), with
    /// their parents; an empty `q` gives the tree back as it was.
    pub fn set_filter(&self, q: &str) {
        if *self.query.borrow() == q {
            return;
        }
        if q.is_empty() {
            *self.query.borrow_mut() = String::new();
            *self.keep.borrow_mut() = None;
        } else {
            // The user's expanded rows, before the filter opens everything.
            self.remember_expanded();
            *self.query.borrow_mut() = q.to_owned();
            *self.keep.borrow_mut() = Some(HashSet::new());
        }
        self.reload();
        if q.is_empty()
            && let Some(p) = crate::ui::window::app().tabs.current_path()
        {
            self.reveal(&p);
        }
    }

    /// Ctrl+; : shows the explorer and puts the caret into its search.
    pub fn focus_search(&self) {
        window::show_side("explorer");
        self.search.grab_focus();
        self.search.select_region(0, -1);
    }

    /// Track Active Item: selects a file's row, opening the folders above
    /// it, and scrolls it into view. Without tracking nothing happens.
    pub fn reveal(&self, path: &Path) {
        if self.track.is_active() {
            self.select_path(path);
        }
    }

    fn select_path(&self, path: &Path) {
        let Some(model) = self.root_model() else { return };
        let app = crate::ui::window::app();
        let ancestors: HashSet<PathBuf> = {
            let sol = app.solution.borrow();
            let Some(sol) = sol.as_ref() else { return };
            let mut set: HashSet<PathBuf> = path.ancestors().skip(1).take_while(|a| a.starts_with(&sol.dir)).map(Path::to_path_buf).collect();
            if let Some(f) = sol.project_of(path).and_then(|p| p.folder.as_ref()) {
                set.insert(sol.dir.join(f.trim_matches('/')));
            }
            set
        };
        let mut found = None;
        let mut i = 0;
        // Children follow their parent, so one pass opens the whole chain.
        while i < model.n_items() {
            if let Some(row) = model.item(i).and_downcast::<gtk::TreeListRow>()
                && let Some(n) = row.item().and_downcast::<Node>()
            {
                let d = n.data();
                if d.path == path && d.kind == Kind::File || d.path == path && found.is_none() && d.kind == Kind::Folder {
                    found = Some(i);
                    if d.kind == Kind::File {
                        break;
                    }
                } else if ancestors.contains(&d.path) && matches!(d.kind, Kind::Solution | Kind::SolutionFolder | Kind::Project | Kind::Folder) {
                    row.set_expanded(true);
                }
            }
            i += 1;
        }
        let Some(i) = found else { return };
        if self.selection.selected() != i {
            self.quiet.set(true);
            self.selection.set_selected(i);
            self.quiet.set(false);
        }
        self.list.scroll_to(i, gtk::ListScrollFlags::NONE, None);
    }

    pub fn selected(&self) -> Option<NodeData> {
        self.selection.selected_item().and_downcast::<gtk::TreeListRow>().and_then(|r| r.item()).and_downcast::<Node>().map(|n| n.data())
    }

    /// Asks a yes/no question inline; Escape and clicking "no" answer no.
    /// An open question is answered no when the next one comes.
    fn ask(&self, text: &str, yes: &str, no: &str, default_yes: bool, done: impl FnOnce(bool) + 'static) {
        answer_prompt(false);
        self.prompt_label.set_text(text);
        self.prompt_yes.set_label(yes);
        self.prompt_no.set_label(no);
        self.prompt_yes.remove_css_class("suggested-action");
        self.prompt_no.remove_css_class("suggested-action");
        let default = if default_yes { &self.prompt_yes } else { &self.prompt_no };
        default.add_css_class("suggested-action");
        *self.answer.borrow_mut() = Some(Box::new(done));
        self.prompt.set_reveal_child(true);
        default.grab_focus();
    }

    /// Moves the focus to the tree, which ends an unchanged inline rename.
    pub fn list_grab_focus(&self) {
        self.list.grab_focus();
    }

    fn rename_entry(&self, path: &Path) -> Option<gtk::Entry> {
        fn walk(w: &gtk::Widget, path: &Path) -> Option<gtk::Entry> {
            if let Some(e) = w.downcast_ref::<gtk::TreeExpander>() {
                let hit = e.item().and_downcast::<Node>().is_some_and(|n| n.data().path == path);
                let b = e.child()?;
                return if hit { b.first_child()?.next_sibling()?.next_sibling().and_downcast::<gtk::Entry>() } else { None };
            }
            let mut c = w.first_child();
            while let Some(ch) = c {
                if let Some(found) = walk(&ch, path) {
                    return Some(found);
                }
                c = ch.next_sibling();
            }
            None
        }
        walk(self.list.upcast_ref(), path)
    }
}

/// `name` with the first match of `q` in bold on a faint fill, as Pango
/// markup; `None` when there is nothing to mark.
fn highlight(name: &str, q: &str) -> Option<String> {
    if q.is_empty() {
        return None;
    }
    let lower = name.to_lowercase();
    let at = lower.find(&q.to_lowercase())?;
    // Lower-casing can change byte lengths; only mark when it did not.
    if lower.len() != name.len() || !name.is_char_boundary(at) || !name.is_char_boundary(at + q.len()) {
        return None;
    }
    let c = crate::palette::current().line;
    let bg = format!("#{:02x}{:02x}{:02x}", (c.red() * 255.0) as u8, (c.green() * 255.0) as u8, (c.blue() * 255.0) as u8);
    let esc = |s: &str| glib::markup_escape_text(s).to_string();
    Some(format!("{}<span weight=\"bold\" bgcolor=\"{bg}\">{}</span>{}", esc(&name[..at]), esc(&name[at..at + q.len()]), esc(&name[at + q.len()..])))
}

/// Every path the filtered tree shows: projects, folders and files whose
/// name contains `q`, and everything above them.
fn filter_keep(sol: &anvil_core::solution::Solution, q: &str) -> HashSet<PathBuf> {
    let q = q.to_lowercase();
    let mut keep = HashSet::new();
    let add_chain = |keep: &mut HashSet<PathBuf>, p: &Path, project: &anvil_core::solution::Project| {
        for a in p.ancestors().take_while(|a| a.starts_with(&project.dir)) {
            keep.insert(a.to_path_buf());
        }
        if let Some(f) = &project.folder {
            keep.insert(sol.dir.join(f.trim_matches('/')));
        }
    };
    // A guard against a project folder with a huge tree under it.
    let mut budget = 50_000usize;
    for p in &sol.projects {
        if p.name.to_lowercase().contains(&q) {
            add_chain(&mut keep, &p.dir, p);
        }
        let mut stack = vec![NodeData { kind: Kind::Folder, name: p.name.clone(), path: p.dir.clone(), project: p.name.clone(), detail: String::new() }];
        while let Some(d) = stack.pop() {
            for c in dir_children(&d.path, &p.name, Some(p)) {
                budget = budget.saturating_sub(1);
                if c.name.to_lowercase().contains(&q) {
                    add_chain(&mut keep, &c.path, p);
                }
                if c.kind == Kind::Folder && budget > 0 {
                    stack.push(c);
                }
            }
        }
    }
    keep
}

fn answer_prompt(yes: bool) {
    let app = crate::ui::window::app();
    let done = app.explorer.answer.borrow_mut().take();
    app.explorer.prompt.set_reveal_child(false);
    if let Some(done) = done {
        done(yes);
    }
}

/// The project of the selected node (or of the current file).
pub fn selected_project() -> Option<String> {
    let app = crate::ui::window::app();
    app.explorer.selected().map(|d| d.project).filter(|p| !p.is_empty())
}

/// The folder new items go into: the selected folder, the project folder,
/// or the current file's folder.
pub fn target_folder() -> Option<(String, PathBuf)> {
    let app = crate::ui::window::app();
    if let Some(d) = app.explorer.selected()
        && !d.project.is_empty()
    {
        let dir = match d.kind {
            Kind::Folder | Kind::Project => d.path,
            Kind::File => d.path.parent().map(Path::to_path_buf).unwrap_or(d.path),
            _ => app.solution.borrow().as_ref().and_then(|s| s.project_named(&d.project).map(|p| p.dir.clone()))?,
        };
        return Some((d.project, dir));
    }
    let current = app.tabs.current_path()?;
    let sol = app.solution.borrow();
    let p = sol.as_ref()?.project_of(&current)?;
    Some((p.name.clone(), current.parent()?.to_path_buf()))
}

fn context_menu(w: &gtk::Widget, x: f64, y: f64) {
    use Item::{Command as C, Separator as S};
    let app = crate::ui::window::app();
    let Some(d) = app.explorer.selected() else { return };
    let add = Item::Submenu(tr("Add"), vec![C("project.add-new-item"), C("project.add-existing-item"), C("project.add-class"), C("project.add-folder"), C("project.add-class-diagram")]);
    let items: Vec<Item> = match d.kind {
        Kind::Solution => vec![C("build.solution"), C("build.rebuild-solution"), C("build.clean-solution"), S, Item::Submenu(tr("Add"), vec![C("project.new-project-in-solution")]), S, C("git.init"), S, C("explorer.open-folder"), C("explorer.open-terminal"), S, C("file.handin")],
        Kind::Project => vec![
            C("build.project"),
            S,
            add,
            C("project.view-class-diagram"),
            C("project.add-reference"),
            C("view.nuget"),
            S,
            C("project.set-startup"),
            Item::Submenu(tr("Debug"), vec![C("debug.start"), C("debug.start-without")]),
            S,
            C("explorer.open-folder"),
            C("explorer.open-terminal"),
            S,
            C("project.properties"),
        ],
        Kind::Folder => vec![add, S, C("explorer.exclude"), C("explorer.delete"), C("explorer.rename"), S, C("explorer.open-folder"), C("explorer.open-terminal")],
        Kind::File => {
            let path = d.path.clone();
            let open = Item::Run(tr("Open"), Rc::new(move || {
                crate::ui::window::app().tabs.open(&path, false);
            }));
            let mut v = vec![open, S];
            if d.name.ends_with(".cs") {
                v.push(C("project.view-class-diagram"));
            }
            v.extend([C("explorer.exclude"), C("explorer.delete"), C("explorer.rename"), S, C("explorer.open-folder")]);
            v
        }
        Kind::Dependencies | Kind::DepGroup(_) | Kind::Dependency => vec![C("project.add-reference"), C("view.nuget")],
        Kind::SolutionFolder => vec![Item::Submenu(tr("Add"), vec![C("project.new-project-in-solution")])],
    };
    crate::ui::menus::popup(w, x, y, &items);
}

pub fn new_folder() {
    let Some((_, dir)) = target_folder() else { return };
    let name = (1..).map(|n| if n == 1 { tr("NewFolder") } else { format!("{}{n}", tr("NewFolder")) }).find(|n| !dir.join(n).exists()).unwrap_or_default();
    ask_name(&tr("New Folder"), &name, move |n| {
        if let Err(e) = std::fs::create_dir(dir.join(&n)) {
            window::show_error(&tr("New Folder"), &e.to_string());
        }
        crate::ui::window::app().explorer.reload();
    });
}

fn ask_name(title: &str, initial: &str, done: impl Fn(String) + 'static) {
    let app = crate::ui::window::app();
    let dialog = adw::AlertDialog::new(Some(title), None);
    let entry = gtk::Entry::builder().text(initial).activates_default(true).build();
    dialog.set_extra_child(Some(&entry));
    dialog.add_responses(&[("cancel", &tr("Cancel")), ("ok", &tr("OK"))]);
    dialog.set_default_response(Some("ok"));
    // Select the name without its extension, as VS does on F2.
    let stem_len = initial.rfind('.').filter(|i| *i > 0).unwrap_or(initial.len());
    entry.connect_map(move |e| e.select_region(0, stem_len as i32));
    dialog.choose(Some(&app.window), gio::Cancellable::NONE, move |r| {
        let t = entry.text().trim().to_owned();
        if r == "ok" && !t.is_empty() && !t.contains('/') {
            done(t);
        }
    });
}

/// F2: the selected file or folder's label becomes an entry; Enter (or
/// clicking elsewhere) renames, Escape cancels.
pub fn rename_selected() {
    let app = crate::ui::window::app();
    let Some(d) = app.explorer.selected() else { return };
    if !matches!(d.kind, Kind::File | Kind::Folder) || app.explorer.editing.borrow().is_some() {
        return;
    }
    let pos = app.explorer.selection.selected();
    app.explorer.list.scroll_to(pos, gtk::ListScrollFlags::NONE, None);
    *app.explorer.editing.borrow_mut() = Some(d.path.clone());
    let Some(entry) = app.explorer.rename_entry(&d.path) else {
        app.explorer.editing.borrow_mut().take();
        return;
    };
    if let Some(label) = entry.prev_sibling() {
        label.set_visible(false);
    }
    entry.set_text(&d.name);
    entry.set_visible(true);
    entry.grab_focus();
    // The name without its extension, as VS selects it.
    let stem_len = if d.kind == Kind::File { d.name.rfind('.').filter(|i| *i > 0).unwrap_or(d.name.len()) } else { d.name.len() };
    entry.select_region(0, d.name[..stem_len].chars().count() as i32);
}

fn finish_rename(entry: &gtk::Entry, commit: bool) {
    let app = crate::ui::window::app();
    let Some(old) = app.explorer.editing.borrow_mut().take() else { return };
    entry.set_visible(false);
    if let Some(label) = entry.prev_sibling() {
        label.set_visible(true);
    }
    app.explorer.list.grab_focus();
    let name = entry.text().trim().to_owned();
    let unchanged = old.file_name().is_some_and(|n| n.to_string_lossy() == name);
    if !commit || name.is_empty() || unchanged {
        return;
    }
    if name.contains('/') || name == "." || name == ".." {
        window::toast(&trf("'{}' is not a valid name.", &[&name]));
        return;
    }
    let new = old.with_file_name(&name);
    let old_stem = old.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let new_stem = new.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let is_cs = old.is_file() && old.extension().is_some_and(|e| e == "cs") && new.extension().is_some_and(|e| e == "cs");
    let type_pos = if is_cs && old_stem != new_stem && app.lsp.client().is_some() { type_position(&old, &old_stem) } else { None };
    let Some(pos) = type_pos else {
        run_move(&old, &new);
        return;
    };
    // VS asks the same after renaming a file that holds a type of its name.
    app.explorer.ask(&trf("Also rename the type '{}' to '{}' and all its references?", &[&old_stem, &new_stem]), &tr("Rename Type"), &tr("File Only"), true, move |yes| {
        if !yes {
            run_move(&old, &new);
            return;
        }
        rename_type(old, new, pos, new_stem);
    });
}

/// The LSP position of the type named `name` in a `.cs` file.
fn type_position(path: &Path, name: &str) -> Option<lsp_types::Position> {
    let app = crate::ui::window::app();
    let text = app.tabs.code_doc(path).map(|d| d.text()).or_else(|| anvil_core::text::TextFile::read(path).ok().map(|f| f.text))?;
    let s = anvil_core::csharp::parse(&text);
    let start = s.all_types().into_iter().find(|(_, t)| t.name == name).map(|(_, t)| t.name_range.start)?;
    Some(anvil_lsp::position::to_position(&text, start))
}

/// Renames the type through Roslyn while the file still has its old name
/// (Roslyn knows it there), then the file.
fn rename_type(old: PathBuf, new: PathBuf, pos: lsp_types::Position, new_name: String) {
    let app = crate::ui::window::app();
    let Some(client) = app.lsp.client() else {
        run_move(&old, &new);
        return;
    };
    let params = serde_json::json!({"textDocument": {"uri": anvil_lsp::roslyn::file_uri(&old)}, "position": pos, "newName": new_name});
    glib::spawn_future_local(async move {
        match client.request_raw("textDocument/rename", params).await {
            Ok(v) => match serde_json::from_value::<lsp_types::WorkspaceEdit>(v) {
                Ok(edit) => {
                    if !crate::ui::lsp::apply_workspace_edit(&edit) {
                        window::toast(&tr("Some files could not be changed by the type rename."));
                    }
                }
                Err(_) => window::toast(&tr("The language server did not rename the type.")),
            },
            Err(e) => window::toast(&trf("The type was not renamed: {}", &[&e.to_string()])),
        }
        run_move(&old, &new);
    });
}

/// Where a drop of `src` onto `target` moves it; `None` when the drop
/// makes no sense (its own folder, into itself, outside the solution).
fn drop_plan(src: &Path, target: &NodeData) -> Option<(PathBuf, PathBuf)> {
    let dir = match target.kind {
        Kind::Folder | Kind::Project => target.path.clone(),
        Kind::File => target.path.parent()?.to_path_buf(),
        _ => return None,
    };
    let app = crate::ui::window::app();
    let sol = app.solution.borrow();
    let sol = sol.as_ref()?;
    sol.project_of(src)?;
    sol.project_of(&dir)?;
    if !src.exists() || src.parent() == Some(dir.as_path()) || dir.starts_with(src) {
        return None;
    }
    Some((src.to_path_buf(), dir.join(src.file_name()?)))
}

fn run_move(from: &Path, to: &Path) {
    if let Err(e) = move_path(from, to) {
        window::toast(&e);
    }
}

/// A drag and drop: moves, then offers (default no) to change the
/// namespaces of moved `.cs` files when they went to another project.
pub fn move_by_user(from: PathBuf, to: PathBuf) {
    let (src_project, dest) = {
        let app = crate::ui::window::app();
        let sol = app.solution.borrow();
        let Some(sol) = sol.as_ref() else { return };
        (sol.project_of(&from).map(|p| p.name.clone()), sol.project_of(&to).cloned())
    };
    if let Err(e) = move_path(&from, &to) {
        window::toast(&e);
        return;
    }
    let Some(dest) = dest else { return };
    if src_project.as_deref() == Some(dest.name.as_str()) {
        return;
    }
    let mut files = Vec::new();
    if to.is_dir() {
        anvil_core::solution::walk(&to, &to, &mut |rel, abs| {
            if rel.ends_with(".cs") {
                files.push(abs.to_owned());
            }
        });
    } else if to.extension().is_some_and(|e| e == "cs") {
        files.push(to.clone());
    }
    let plan: Vec<(PathBuf, String)> = files
        .into_iter()
        .filter_map(|f| {
            let rel_dir = anvil_core::moves::rel(&dest.dir, f.parent()?)?;
            Some((f, anvil_core::templates::namespace_for(&dest.root_namespace(), &rel_dir)))
        })
        .collect();
    if plan.is_empty() {
        return;
    }
    let text = if plan.len() == 1 { trf("Change the namespace to '{}'?", &[&plan[0].1]) } else { trf("Change the namespaces of {} files to match '{}'?", &[&plan.len().to_string(), &dest.name]) };
    let app = crate::ui::window::app();
    app.explorer.ask(&text, &tr("Change Namespace"), &tr("Keep"), false, move |yes| {
        if yes {
            update_namespaces(&plan);
        }
    });
}

fn update_namespaces(plan: &[(PathBuf, String)]) {
    let app = crate::ui::window::app();
    let mut skipped = 0;
    for (path, ns) in plan {
        if let Some(doc) = app.tabs.code_doc(path) {
            match anvil_core::moves::namespace_edit(&doc.text(), ns) {
                Some(e) => doc.apply_edits(&[e]),
                None => skipped += 1,
            }
            continue;
        }
        let Ok(mut f) = anvil_core::text::TextFile::read(path) else {
            skipped += 1;
            continue;
        };
        match anvil_core::moves::namespace_edit(&f.text, ns) {
            Some(e) => {
                f.text = anvil_core::edit::apply(&f.text, &[e]);
                if f.write(path).is_err() {
                    skipped += 1;
                }
            }
            None => skipped += 1,
        }
    }
    if skipped > 0 {
        window::toast(&crate::i18n::ntr("{} file kept its namespace (none or several declared, or already right).", "{} files kept their namespace (none or several declared, or already right).", skipped as u32).replacen("{}", &skipped.to_string(), 1));
    }
}

/// Moves (or renames) a file or folder on disk and in the project files:
/// explicit items follow losslessly, open tabs follow the file.
pub fn move_path(from: &Path, to: &Path) -> Result<(), String> {
    let title = from.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if to.exists() && !(from.to_string_lossy().eq_ignore_ascii_case(&to.to_string_lossy())) {
        return Err(trf("{} already exists.", &[&to.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()]));
    }
    let app = crate::ui::window::app();
    let (src, dest) = {
        let sol = app.solution.borrow();
        let sol = sol.as_ref().ok_or_else(|| tr("No solution is open."))?;
        (sol.project_of(from).cloned(), sol.project_of(to).cloned())
    };
    let is_dir = from.is_dir();
    // The project files first in memory: a broken one stops the move.
    let mut writes: Vec<(PathBuf, anvil_core::text::TextFile)> = Vec::new();
    if let Some(src) = &src {
        let read = |p: &Path| anvil_core::text::TextFile::read(p).map_err(|e| trf("{} could not be read: {}", &[&p.display().to_string(), &e.to_string()]));
        let sf = read(&src.path)?;
        let other = dest.as_ref().filter(|d| d.path != src.path);
        let df = match other {
            Some(d) => Some(read(&d.path)?),
            None => None,
        };
        let (s_new, d_new) = anvil_core::moves::project_changes(&src.dir, &sf.text, other.zip(df.as_ref()).map(|(d, f)| (d.dir.as_path(), f.text.as_str())), from, to, is_dir).map_err(|e| e.to_string())?;
        if let Some(t) = s_new {
            writes.push((src.path.clone(), anvil_core::text::TextFile { text: t, ..sf }));
        }
        if let (Some(t), Some(d), Some(f)) = (d_new, other, df) {
            writes.push((d.path.clone(), anvil_core::text::TextFile { text: t, ..f }));
        }
    }
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::rename(from, to).map_err(|e| trf("{} could not be moved: {}", &[&title, &e.to_string()]))?;
    for (p, f) in &writes {
        if let Err(e) = f.write(p) {
            window::toast(&trf("{} could not be saved", &[&p.display().to_string()]));
            eprintln!("anvil: {}: {e}", p.display());
        }
    }
    follow_tabs(from, to);
    if writes.is_empty() {
        app.explorer.reload();
    } else {
        reload_solution_model();
    }
    app.explorer.select_path(to);
    Ok(())
}

/// Open tabs of a moved file (or of files in a moved folder) take the new
/// path; the language server sees a close and an open.
fn follow_tabs(from: &Path, to: &Path) {
    let app = crate::ui::window::app();
    for old in app.tabs.open_paths() {
        let Ok(rest) = old.strip_prefix(from) else { continue };
        let new = if rest.as_os_str().is_empty() { to.to_path_buf() } else { to.join(rest) };
        let Some((_, doc)) = app.tabs.find(&old) else { continue };
        match &*doc {
            crate::ui::editor::Doc::Code(c) => {
                app.lsp.did_close(&old);
                *c.path.borrow_mut() = new;
                app.lsp.did_open(c);
            }
            crate::ui::editor::Doc::Diagram(d) => *d.path.borrow_mut() = new,
        }
    }
    app.tabs.refresh_titles();
}

pub fn delete_selected() {
    let app = crate::ui::window::app();
    let Some(d) = app.explorer.selected() else { return };
    if !matches!(d.kind, Kind::File | Kind::Folder) {
        return;
    }
    let dialog = adw::AlertDialog::new(Some(&trf("'{}' will be deleted.", &[&d.name])), Some(&tr("It is moved to the trash.")));
    dialog.add_responses(&[("cancel", &tr("Cancel")), ("delete", &tr("Delete"))]);
    dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
    dialog.choose(Some(&app.window), gio::Cancellable::NONE, move |r| {
        if r != "delete" {
            return;
        }
        let app = crate::ui::window::app();
        if let Some((page, _)) = app.tabs.find(&d.path) {
            app.tabs.view.close_page(&page);
        }
        if let Err(e) = gio::File::for_path(&d.path).trash(gio::Cancellable::NONE) {
            window::show_error(&tr("Delete"), &e.to_string());
        }
        app.explorer.reload();
    });
}

/// Exclude From Project: `<Compile Remove>` (or `<None Remove>`) in the
/// project file, written losslessly.
pub fn exclude_selected() {
    let app = crate::ui::window::app();
    let Some(d) = app.explorer.selected() else { return };
    let sol = app.solution.borrow().clone();
    let Some(p) = sol.as_ref().and_then(|s| s.project_named(&d.project)) else { return };
    let Ok(rel) = d.path.strip_prefix(&p.dir) else { return };
    let mut rel = rel.to_string_lossy().replace('\\', "/");
    if d.kind == Kind::Folder {
        rel.push_str("/**");
    }
    let Ok(mut file) = anvil_core::text::TextFile::read(&p.path) else { return };
    let Ok(c) = anvil_core::csproj::Csproj::parse(&file.text) else { return };
    let edit = c.exclude_compile(&rel);
    file.text = anvil_core::edit::apply(&file.text, &[edit]);
    if let Err(e) = file.write(&p.path) {
        window::show_error(&tr("Exclude From Project"), &e.to_string());
    }
    reload_solution_model();
}

/// Re-reads the solution and project files after Anvil changed them.
pub fn reload_solution_model() {
    let app = crate::ui::window::app();
    let path = app.solution.borrow().as_ref().map(|s| s.path.clone());
    if let Some(path) = path
        && let Ok(s) = anvil_core::solution::Solution::open(&path)
    {
        *app.solution.borrow_mut() = Some(s);
    }
    app.explorer.reload();
    window::refresh_startup_dropdown();
}

pub fn open_folder_of_selected() {
    let app = crate::ui::window::app();
    let path = app.explorer.selected().map(|d| d.path).or_else(window::solution_dir);
    let Some(path) = path else { return };
    let folder = if path.is_dir() { path } else { path.parent().map(Path::to_path_buf).unwrap_or(path) };
    gtk::FileLauncher::new(Some(&gio::File::for_path(&folder))).launch(Some(&app.window), gio::Cancellable::NONE, |_| {});
}

pub fn open_terminal_at_selected() {
    let app = crate::ui::window::app();
    let path = app.explorer.selected().map(|d| d.path).or_else(window::solution_dir);
    let Some(path) = path else { return };
    let folder = if path.is_dir() { path } else { path.parent().map(Path::to_path_buf).unwrap_or(path) };
    window::show_bottom("terminal");
    app.terminal.shell_in(&folder);
}
