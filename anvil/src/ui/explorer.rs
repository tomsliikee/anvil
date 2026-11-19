//! Solution Explorer: solution → projects → Dependencies, folders, files,
//! with VS' context menus. Files are what SDK-style globbing includes
//! (`**/*` minus `bin/`, `obj/` and `<Compile Remove>`).

use std::cell::RefCell;
use std::collections::BTreeSet;
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
}

fn is_hidden(name: &str) -> bool {
    name.starts_with('.') || name.eq_ignore_ascii_case("bin") || name.eq_ignore_ascii_case("obj")
}

fn children(d: &NodeData) -> Vec<NodeData> {
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
            let detail = gtk::Label::builder().xalign(0.0).css_classes(["count"]).build();
            row.append(&image);
            row.append(&name);
            row.append(&detail);
            let expander = gtk::TreeExpander::new();
            expander.set_child(Some(&row));
            item.set_child(Some(&expander));
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
            // Dragging a .cs file onto a diagram shows its types.
            let drag = gtk::DragSource::new();
            drag.set_actions(gdk::DragAction::COPY);
            let li = item.downgrade();
            drag.connect_prepare(move |_, _, _| {
                let li = li.upgrade()?;
                let row = li.item().and_downcast::<gtk::TreeListRow>()?;
                let node = row.item().and_downcast::<Node>()?;
                let d = node.data();
                (d.kind == Kind::File).then(|| gdk::ContentProvider::for_value(&d.path.to_string_lossy().to_value()))
            });
            expander.add_controller(drag);
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
            let detail = name.next_sibling().and_downcast::<gtk::Label>().expect("detail");
            image.set_icon_name(Some(icon(&d)));
            name.set_label(&d.name);
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
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 0);
        widget.add_css_class("explorer");
        widget.append(&scroller);
        Explorer { widget, list, selection, expanded: RefCell::default(), root: RefCell::default() }
    }

    pub fn connect(&self) {
        // Single click: preview tab, as VS' preview.
        self.selection.connect_selected_item_notify(|s| {
            let Some(node) = s.selected_item().and_downcast::<gtk::TreeListRow>().and_then(|r| r.item()).and_downcast::<Node>() else { return };
            let d = node.data();
            if d.kind == Kind::File {
                crate::ui::window::app().tabs.open(&d.path, true);
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
    }

    fn root_model(&self) -> Option<gtk::TreeListModel> {
        self.selection.model().and_downcast::<gtk::TreeListModel>()
    }

    fn remember_expanded(&self) {
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
        self.selection.set_model(Some(&model));
        let expanded = self.expanded.borrow().clone();
        let mut i = 0;
        while i < model.n_items() {
            if let Some(row) = model.item(i).and_downcast::<gtk::TreeListRow>()
                && let Some(n) = row.item().and_downcast::<Node>()
            {
                let d = n.data();
                let open = matches!(d.kind, Kind::Solution | Kind::SolutionFolder | Kind::Project) && expanded.is_empty() || expanded.contains(&d.path);
                if open {
                    row.set_expanded(true);
                }
            }
            i += 1;
        }
        self.selection.set_selected(gtk::INVALID_LIST_POSITION);
    }

    /// Selects a file's row when it is visible (tab switch).
    pub fn reveal(&self, path: &Path) {
        let Some(model) = self.root_model() else { return };
        for i in 0..model.n_items() {
            if let Some(n) = model.item(i).and_downcast::<gtk::TreeListRow>().and_then(|r| r.item()).and_downcast::<Node>()
                && n.data().path == path
            {
                if self.selection.selected() != i {
                    // Without opening it again as a preview.
                    let handler_free = self.selection.clone();
                    handler_free.set_selected(i);
                }
                return;
            }
        }
    }

    pub fn selected(&self) -> Option<NodeData> {
        self.selection.selected_item().and_downcast::<gtk::TreeListRow>().and_then(|r| r.item()).and_downcast::<Node>().map(|n| n.data())
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

pub fn rename_selected() {
    let app = crate::ui::window::app();
    let Some(d) = app.explorer.selected() else { return };
    if !matches!(d.kind, Kind::File | Kind::Folder) {
        return;
    }
    let old = d.path.clone();
    ask_name(&tr("Rename"), &d.name, move |n| {
        let new = old.with_file_name(&n);
        if new.exists() {
            window::show_error(&tr("Rename"), &trf("{} already exists.", &[&n]));
            return;
        }
        if let Err(e) = std::fs::rename(&old, &new) {
            window::show_error(&tr("Rename"), &e.to_string());
            return;
        }
        let app = crate::ui::window::app();
        if let Some((page, doc)) = app.tabs.find(&old) {
            if let crate::ui::editor::Doc::Code(c) = &*doc {
                app.lsp.did_close(&old);
                *c.path.borrow_mut() = new.clone();
                app.lsp.did_open(c);
            }
            page.set_title(&n);
        }
        app.explorer.reload();
    });
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
