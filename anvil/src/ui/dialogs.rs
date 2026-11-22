//! Open, New Project, Go To All, Insert Snippet, Add Existing Item and Add
//! Project Reference.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use adw::prelude::*;
use anvil_core::host::Command;
use gtk::{gio, glib};

use crate::host;
use crate::i18n::{tr, trf};
use crate::ui::build::dotnet_on_file;
use crate::ui::window::{self};

fn filter(name: &str, patterns: &[&str]) -> gtk::FileFilter {
    let f = gtk::FileFilter::new();
    f.set_name(Some(name));
    for p in patterns {
        f.add_pattern(p);
    }
    f
}

pub fn open_solution() {
    let filters = gio::ListStore::new::<gtk::FileFilter>();
    filters.append(&filter(&tr("Solution files"), &["*.slnx", "*.sln"]));
    filters.append(&filter(&tr("Project files"), &["*.csproj"]));
    let fd = gtk::FileDialog::builder().title(tr("Open Project or Solution")).filters(&filters).build();
    fd.open(Some(&crate::ui::window::app().window), gio::Cancellable::NONE, |r| {
        if let Ok(f) = r
            && let Some(p) = f.path()
        {
            window::open_path(&p);
        }
    });
}

pub fn open_folder() {
    let fd = gtk::FileDialog::builder().title(tr("Open Folder")).build();
    fd.select_folder(Some(&crate::ui::window::app().window), gio::Cancellable::NONE, |r| {
        if let Ok(f) = r
            && let Some(p) = f.path()
        {
            window::open_path(&p);
        }
    });
}

pub fn open_file() {
    let fd = gtk::FileDialog::builder().title(tr("Open File")).build();
    fd.open(Some(&crate::ui::window::app().window), gio::Cancellable::NONE, |r| {
        if let Ok(f) = r
            && let Some(p) = f.path()
        {
            window::open_path(&p);
        }
    });
}

// ---- New Project ----------------------------------------------------------

#[derive(Clone)]
pub struct ProjectTemplate {
    title: String,
    detail: String,
    short_name: &'static str,
    extra: Vec<&'static str>,
    stylecop: bool,
}

pub fn project_templates() -> Vec<ProjectTemplate> {
    vec![
        ProjectTemplate { title: tr("Console App"), detail: tr("A command-line application (.NET 10, Program.Main as in Visual Studio with \"Do not use top-level statements\")"), short_name: "console", extra: vec!["--use-program-main"], stylecop: false },
        ProjectTemplate { title: tr("Console App with StyleCop"), detail: tr("As the course uses it: StyleCop.Analyzers 1.1.118 with default rules"), short_name: "console", extra: vec!["--use-program-main"], stylecop: true },
        ProjectTemplate { title: tr("Class Library"), detail: tr("A library of classes for other projects"), short_name: "classlib", extra: vec![], stylecop: false },
        ProjectTemplate { title: tr("MSTest Test Project"), detail: tr("Unit tests with MSTest"), short_name: "mstest", extra: vec![], stylecop: false },
        ProjectTemplate { title: tr("xUnit Test Project"), detail: tr("Unit tests with xUnit"), short_name: "xunit", extra: vec![], stylecop: false },
        ProjectTemplate { title: tr("NUnit Test Project"), detail: tr("Unit tests with NUnit"), short_name: "nunit", extra: vec![], stylecop: false },
    ]
}

/// New Project; with `into` the project is added to that solution.
pub fn new_project(into: Option<PathBuf>) {
    let app = crate::ui::window::app();
    let templates = project_templates();
    let list = gtk::ListBox::new();
    list.add_css_class("templates");
    for t in &templates {
        let b = gtk::Box::new(gtk::Orientation::Vertical, 2);
        b.append(&gtk::Label::builder().label(&t.title).xalign(0.0).css_classes(["template-title"]).build());
        b.append(&gtk::Label::builder().label(&t.detail).xalign(0.0).wrap(true).css_classes(["hint"]).build());
        list.append(&b);
    }
    list.select_row(list.row_at_index(if into.is_some() { 0 } else { 1 }).as_ref());
    let name = gtk::Entry::builder().text("ConsoleApp1").hexpand(true).build();
    let location = gtk::Entry::builder().text(glib::home_dir().join("source/repos").to_string_lossy()).hexpand(true).build();
    let browse = gtk::Button::from_icon_name("folder-open-symbolic");
    let same_dir = gtk::CheckButton::with_label(&tr("Place solution and project in the same directory"));
    let grid = gtk::Grid::builder().row_spacing(8).column_spacing(8).build();
    grid.attach(&gtk::Label::builder().label(tr("Project name")).xalign(0.0).build(), 0, 0, 1, 1);
    grid.attach(&name, 1, 0, 2, 1);
    if into.is_none() {
        grid.attach(&gtk::Label::builder().label(tr("Location")).xalign(0.0).build(), 0, 1, 1, 1);
        grid.attach(&location, 1, 1, 1, 1);
        grid.attach(&browse, 2, 1, 1, 1);
        grid.attach(&same_dir, 1, 2, 2, 1);
    }
    browse.connect_clicked(glib::clone!(
        #[weak]
        location,
        move |_| {
            let fd = gtk::FileDialog::builder().title(tr("Location")).build();
            fd.select_folder(Some(&crate::ui::window::app().window), gio::Cancellable::NONE, move |r| {
                if let Ok(f) = r
                    && let Some(p) = f.path()
                {
                    location.set_text(&p.to_string_lossy());
                }
            });
        }
    ));
    let b = gtk::Box::new(gtk::Orientation::Vertical, 12);
    b.append(&gtk::ScrolledWindow::builder().child(&list).min_content_height(300).min_content_width(520).css_classes(["templates-scroll"]).overflow(gtk::Overflow::Hidden).hscrollbar_policy(gtk::PolicyType::Never).build());
    b.append(&grid);
    let dialog = adw::AlertDialog::new(Some(&if into.is_some() { tr("Add a new project") } else { tr("Create a new project") }), None);
    dialog.set_extra_child(Some(&b));
    dialog.add_responses(&[("cancel", &tr("Cancel")), ("create", &tr("Create"))]);
    dialog.set_response_appearance("create", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("create"));
    dialog.choose(Some(&app.window), gio::Cancellable::NONE, move |r| {
        if r != "create" {
            return;
        }
        let Some(t) = list.selected_row().map(|r| templates[r.index() as usize].clone()) else { return };
        let project = anvil_core::templates::safe_identifier(name.text().trim());
        let into = into.clone();
        let location = PathBuf::from(location.text().trim());
        let same = same_dir.is_active();
        glib::spawn_future_local(async move {
            if let Err(e) = create_project(t, &project, &location, same, into).await {
                window::show_error(&tr("The project could not be created"), &e);
            }
        });
    });
}

async fn run_ok(cmd: Command) -> Result<String, String> {
    let out = host::run(cmd, None).await.map_err(|e| e.to_string())?;
    crate::ui::window::app().output.append("Build", &out.stdout_text());
    if out.ok() { Ok(out.stdout_text()) } else { Err(format!("{}{}", out.stdout_text(), out.stderr_text())) }
}

/// Text files `dotnet new` wrote: CRLF like VS, BOM as the template has it.
fn crlf_tree(dir: &Path) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            let n = e.file_name();
            if n != "bin" && n != "obj" {
                crlf_tree(&p);
            }
        } else if matches!(p.extension().and_then(|e| e.to_str()), Some("cs" | "csproj" | "slnx" | "sln" | "json" | "props" | "editorconfig"))
            && let Ok(bytes) = std::fs::read(&p)
        {
            let f = anvil_core::text::TextFile::decode(&bytes);
            let fixed = anvil_core::text::restyle(&bytes, f.encoding, anvil_core::text::Eol::Crlf);
            if fixed != bytes {
                let _ = anvil_core::text::write_atomic(&p, &fixed);
            }
        }
    }
}

pub async fn create_project(t: ProjectTemplate, project: &str, location: &Path, same_dir: bool, into: Option<PathBuf>) -> Result<(), String> {
    let app = crate::ui::window::app();
    let (sln_dir, proj_dir, sln_path) = match &into {
        Some(sln) => {
            let d = sln.parent().map(Path::to_path_buf).unwrap_or_default();
            (d.clone(), d.join(project), sln.clone())
        }
        None => {
            let sd = location.join(project);
            let pd = if same_dir { sd.clone() } else { sd.join(project) };
            let ext = if app.settings.string("new-solution-format") == "sln" { "sln" } else { "slnx" };
            (sd.clone(), pd, sd.join(format!("{project}.{ext}")))
        }
    };
    if proj_dir.join(format!("{project}.csproj")).exists() {
        return Err(trf("{} already exists.", &[&proj_dir.display().to_string()]));
    }
    std::fs::create_dir_all(&proj_dir).map_err(|e| e.to_string())?;
    app.output.show("Build");
    run_ok(Command::dotnet().args(["new", t.short_name, "-n", project, "-o"]).arg(proj_dir.to_string_lossy()).args(t.extra.iter().copied()).cwd(&sln_dir)).await?;
    if into.is_none() {
        let mut cmd = Command::dotnet().args(["new", "sln", "-n", project, "-o"]).arg(sln_dir.to_string_lossy()).cwd(&sln_dir);
        if sln_path.extension().is_some_and(|e| e == "sln") {
            cmd = cmd.args(["--format", "sln"]);
        }
        run_ok(cmd).await?;
    }
    let csproj = proj_dir.join(format!("{project}.csproj"));
    if t.stylecop
        && let Ok(mut f) = anvil_core::text::TextFile::read(&csproj)
        && let Ok(c) = anvil_core::csproj::Csproj::parse(&f.text)
    {
        f.text = anvil_core::edit::apply(&f.text, &[c.add_stylecop()]);
        f.write(&csproj).map_err(|e| e.to_string())?;
    }
    if t.short_name == "console" {
        let ns = anvil_core::templates::safe_identifier_path(project);
        anvil_core::text::write_atomic(&proj_dir.join("Program.cs"), &anvil_core::templates::console_program(&ns)).map_err(|e| e.to_string())?;
    }
    crlf_tree(&proj_dir);
    dotnet_on_file(&sln_path, vec!["sln".into(), sln_path.to_string_lossy().into_owned(), "add".into(), csproj.to_string_lossy().into_owned()]).await?;
    if into.is_none() {
        crlf_tree(&sln_dir);
        window::open_solution(&sln_path);
    } else {
        crate::ui::explorer::reload_solution_model();
    }
    if let Some(program) = [proj_dir.join("Program.cs"), proj_dir.join("UnitTest1.cs"), proj_dir.join("Class1.cs")].into_iter().find(|p| p.exists()) {
        app.tabs.open(&program, false);
    }
    Ok(())
}

// ---- Go To All --------------------------------------------------------------

struct Target {
    label: String,
    detail: String,
    path: PathBuf,
    line: u32,
}

fn all_targets() -> Vec<Target> {
    let app = crate::ui::window::app();
    let Some(sol) = app.solution.borrow().clone() else { return Vec::new() };
    let mut out = Vec::new();
    for p in &sol.projects {
        for f in p.source_files() {
            let text = app.tabs.code_doc(&f).map(|d| d.text()).or_else(|| anvil_core::text::TextFile::read(&f).ok().map(|t| t.text)).unwrap_or_default();
            let file_name = f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            out.push(Target { label: file_name.clone(), detail: tr("file"), path: f.clone(), line: 1 });
            let st = anvil_core::csharp::parse(&text);
            let line_of = |byte: usize| text[..byte.min(text.len())].matches('\n').count() as u32 + 1;
            for (outer, t) in st.all_types() {
                out.push(Target { label: t.name.clone(), detail: format!("{} {} · {file_name}", t.kind.keyword(), t.full_name(&outer)), path: f.clone(), line: line_of(t.name_range.start) });
                for m in &t.members {
                    out.push(Target { label: m.name.clone(), detail: format!("{} · {}", m.display(anvil_core::csharp::MembersFormat::FullSignature), t.name), path: f.clone(), line: line_of(m.name_range.start) });
                }
            }
        }
    }
    out
}

/// Ctrl+T: types, members and files by name.
pub fn goto_all() {
    let app = crate::ui::window::app();
    let targets = Rc::new(all_targets());
    let entry = gtk::SearchEntry::new();
    entry.add_css_class("search-entry");
    let list = gtk::ListBox::new();
    list.add_css_class("results");
    let scroller = gtk::ScrolledWindow::builder().child(&list).min_content_height(360).min_content_width(560).build();
    let b = gtk::Box::new(gtk::Orientation::Vertical, 8);
    b.append(&entry);
    b.append(&scroller);
    let dialog = adw::Dialog::builder().title(tr("Go To All")).child(&b).content_width(620).build();
    b.set_margin_top(14);
    b.set_margin_bottom(14);
    b.set_margin_start(14);
    b.set_margin_end(14);
    let shown: Rc<std::cell::RefCell<Vec<usize>>> = Rc::default();
    let fill = {
        let list = list.clone();
        let targets = targets.clone();
        let shown = shown.clone();
        move |q: &str| {
            while let Some(c) = list.first_child() {
                list.remove(&c);
            }
            // Fuzzy as in Feature Search: "dlpl" finds DoublyLinkedPersonList.
            let cands: Vec<anvil_core::search::Candidate> = targets.iter().map(|t| anvil_core::search::Candidate { text: &t.label, alt: None, key: "", weight: 0 }).collect();
            let ranked = anvil_core::search::rank(&cands, q, &anvil_core::search::Mru::default(), 80);
            let idx: Vec<usize> = ranked.iter().map(|r| r.index).collect();
            for r in &ranked {
                let t = &targets[r.index];
                let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
                row.append(&gtk::Label::builder().label(crate::ui::palette_search::highlight(&t.label, &r.text_positions)).use_markup(true).xalign(0.0).css_classes(["strong"]).build());
                row.append(&gtk::Label::builder().label(&t.detail).xalign(0.0).hexpand(true).ellipsize(gtk::pango::EllipsizeMode::End).css_classes(["dim"]).build());
                list.append(&row);
            }
            if let Some(r) = list.row_at_index(0) {
                list.select_row(Some(&r));
            }
            *shown.borrow_mut() = idx;
        }
    };
    fill("");
    let f = fill.clone();
    entry.connect_search_changed(move |e| f(&e.text()));
    let go = {
        let targets = targets.clone();
        let shown = shown.clone();
        let dialog = dialog.clone();
        move |row: i32| {
            let picked = shown.borrow().get(row as usize).copied();
            if let Some(i) = picked {
                let t = &targets[i];
                dialog.close();
                crate::ui::window::app().tabs.goto(&t.path, t.line, 1);
            }
        }
    };
    let g = go.clone();
    list.connect_row_activated(move |_, r| g(r.index()));
    let l = list.clone();
    entry.connect_activate(move |_| {
        if let Some(r) = l.selected_row() {
            go(r.index());
        }
    });
    crate::ui::palette_search::arrow_keys(&entry, &list, &scroller);
    let d = dialog.clone();
    entry.connect_stop_search(move |_| {
        d.close();
    });
    dialog.present(Some(&app.window));
    entry.grab_focus();
}

// ---- Snippets -------------------------------------------------------------

/// Ctrl+K, Ctrl+X (Insert Snippet) and Ctrl+K, Ctrl+S (Surround With).
pub fn insert_snippet(surround: bool) {
    let app = crate::ui::window::app();
    let Some(doc) = app.tabs.current_code() else { return };
    let defs: Vec<&'static anvil_core::snippets::SnippetDef> = anvil_core::snippets::SNIPPETS.iter().filter(|s| !surround || s.surrounds).collect();
    let titles: Vec<String> = defs.iter().map(|d| format!("{}  —  {}", d.shortcut, tr(d.title))).collect();
    let d2 = doc.clone();
    crate::ui::menus::choose_at_caret(&doc, &titles, move |i| expand_snippet(&d2, defs[i]));
}

pub fn expand_snippet(doc: &crate::ui::editor::CodeDoc, def: &anvil_core::snippets::SnippetDef) {
    let buffer = &doc.buffer;
    let selected = doc.selection_text();
    let (mut s, mut e) = buffer.selection_bounds().unwrap_or_else(|| {
        let it = buffer.iter_at_mark(&buffer.get_insert());
        (it, it)
    });
    let mut line_start = s;
    line_start.set_line_offset(0);
    let indent: String = buffer.text(&line_start, &s, false).chars().take_while(|c| c.is_whitespace()).collect();
    let class = {
        let text = doc.text();
        let byte = doc.cursor_byte();
        anvil_core::csharp::parse(&text).all_types().into_iter().filter(|(_, t)| t.range.contains(&byte)).max_by_key(|(_, t)| t.range.start).map(|(_, t)| t.name.clone()).unwrap_or_else(|| "MyClass".into())
    };
    let x = anvil_core::snippets::expand(def, &indent, &class, &selected, "\n");
    buffer.begin_user_action();
    buffer.delete(&mut s, &mut e);
    let start = s.offset();
    buffer.insert(&mut s, &x.text);
    buffer.end_user_action();
    let char_at = |byte: usize| start + x.text[..byte].chars().count() as i32;
    doc.finish_snippet();
    // Every field after the first waits for Tab; marks keep their place
    // while the user types into the fields before them.
    let mark = |byte: usize, left: bool| buffer.create_mark(None, &buffer.iter_at_offset(char_at(byte)), left);
    let rest: Vec<(gtk::TextMark, gtk::TextMark)> = x.stops.iter().skip(1).map(|st| (mark(st.ranges[0].start, true), mark(st.ranges[0].end, false))).collect();
    let end = mark(x.end, false);
    match x.stops.first() {
        Some(stop) => {
            let r = &stop.ranges[0];
            buffer.select_range(&buffer.iter_at_offset(char_at(r.start)), &buffer.iter_at_offset(char_at(r.end)));
            *doc.snippet.borrow_mut() = Some((rest, end));
        }
        None => {
            buffer.place_cursor(&buffer.iter_at_offset(char_at(x.end)));
            buffer.delete_mark(&end);
        }
    }
}

// ---- Add Existing Item, Add Project Reference -------------------------------

pub fn add_existing_item() {
    let Some((_, dir)) = crate::ui::explorer::target_folder() else { return };
    let fd = gtk::FileDialog::builder().title(tr("Add Existing Item")).build();
    fd.open_multiple(Some(&crate::ui::window::app().window), gio::Cancellable::NONE, move |r| {
        let Ok(files) = r else { return };
        for i in 0..files.n_items() {
            if let Some(f) = files.item(i).and_downcast::<gio::File>()
                && let Some(p) = f.path()
                && let Some(name) = p.file_name()
            {
                let target = dir.join(name);
                if target != p
                    && let Err(e) = std::fs::copy(&p, &target)
                {
                    window::show_error(&tr("Add Existing Item"), &e.to_string());
                }
            }
        }
        crate::ui::window::app().explorer.reload();
    });
}

pub fn add_project_reference() {
    let app = crate::ui::window::app();
    let Some(project) = crate::ui::explorer::selected_project().or_else(crate::ui::build::current_project) else { return };
    let Some(sol) = app.solution.borrow().clone() else { return };
    let Some(me) = sol.project_named(&project).cloned() else { return };
    let existing: Vec<String> = me.csproj.as_ref().map(|c| c.project_refs()).unwrap_or_default().iter().map(|r| anvil_core::slnx::stem(r)).collect();
    let b = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let mut checks = Vec::new();
    for p in sol.projects.iter().filter(|p| p.name != project) {
        let c = gtk::CheckButton::with_label(&p.name);
        c.set_active(existing.contains(&p.name));
        b.append(&c);
        checks.push((c, p.path.clone(), existing.contains(&p.name)));
    }
    if checks.is_empty() {
        b.append(&gtk::Label::new(Some(&tr("The solution has no other project."))));
    }
    let d = adw::AlertDialog::new(Some(&trf("Reference Manager - {}", &[&project])), None);
    d.set_extra_child(Some(&b));
    d.add_responses(&[("cancel", &tr("Cancel")), ("ok", &tr("OK"))]);
    d.choose(Some(&app.window), gio::Cancellable::NONE, move |r| {
        if r != "ok" {
            return;
        }
        let csproj = me.path.clone();
        let changes: Vec<(bool, PathBuf)> = checks.iter().filter(|(c, _, was)| c.is_active() != *was).map(|(c, p, _)| (c.is_active(), p.clone())).collect();
        glib::spawn_future_local(async move {
            for (add, other) in changes {
                let verb = if add { "add" } else { "remove" };
                if let Err(e) = dotnet_on_file(&csproj, vec![verb.into(), csproj.to_string_lossy().into_owned(), "reference".into(), other.to_string_lossy().into_owned()]).await {
                    window::show_error(&tr("Reference Manager"), &e);
                }
            }
            crate::ui::explorer::reload_solution_model();
        });
    });
}
