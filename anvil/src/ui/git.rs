//! Git Changes and Git Repository in reduced form, through the host's git
//! so credentials, config and hooks are the user's own.

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use adw::prelude::*;
use anvil_core::host::Command;
use anvil_git::{log, ops, status};
use gtk::{gio, glib};

use crate::host;
use crate::i18n::{tr, trf};
use crate::ui::window::{self};

pub struct GitPanel {
    pub widget: gtk::Box,
    branch: gtk::MenuButton,
    message: gtk::TextView,
    amend: gtk::CheckButton,
    staged: gtk::ListBox,
    changes: gtk::ListBox,
    empty: gtk::Label,
    status: RefCell<status::Status>,
    content: gtk::Box,
}

thread_local! {
    /// The repository's top folder (`git rev-parse --show-toplevel`): one
    /// repository per course often holds many solutions, and `git status`
    /// paths are relative to the top.
    static TOP: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

fn repo_dir() -> Option<PathBuf> {
    let sol = window::solution_dir()?;
    let top = TOP.with(|t| t.borrow().clone());
    Some(top.filter(|t| sol.starts_with(t)).unwrap_or(sol))
}

async fn find_top() {
    let Some(sol) = window::solution_dir() else { return };
    let top = host::run(Command::new("git").args(["rev-parse", "--show-toplevel"]).cwd(&sol), None).await.ok().filter(|o| o.ok()).map(|o| PathBuf::from(o.stdout_text().trim()));
    TOP.with(|t| *t.borrow_mut() = top);
}

async fn git(args: Vec<String>, stdin: Option<Vec<u8>>) -> Result<String, String> {
    let dir = repo_dir().ok_or_else(|| tr("Open a solution first."))?;
    let out = host::run(Command::new("git").args(args).cwd(&dir).env("GIT_TERMINAL_PROMPT", "0"), stdin).await.map_err(|e| e.to_string())?;
    let app = crate::ui::window::app();
    if !out.stderr.is_empty() {
        app.output.append("Git", &out.stderr_text());
    }
    if out.ok() { Ok(out.stdout_text()) } else { Err(format!("{}{}", out.stdout_text(), out.stderr_text()).trim().to_owned()) }
}

impl GitPanel {
    pub fn new() -> GitPanel {
        let branch = gtk::MenuButton::builder().label("main").css_classes(["flat"]).build();
        let fetch = gtk::Button::from_icon_name("view-refresh-symbolic");
        fetch.set_action_name(Some("win.git.fetch"));
        fetch.set_tooltip_text(Some(&tr("Fetch")));
        let pull = gtk::Button::from_icon_name("go-down-symbolic");
        pull.set_action_name(Some("win.git.pull"));
        pull.set_tooltip_text(Some(&tr("Pull")));
        let push = gtk::Button::from_icon_name("go-up-symbolic");
        push.set_action_name(Some("win.git.push"));
        push.set_tooltip_text(Some(&tr("Push")));
        let history = gtk::Button::from_icon_name("document-open-recent-symbolic");
        history.set_tooltip_text(Some(&tr("View History")));
        history.connect_clicked(|_| crate::ui::window::app().git.show_history());
        for b in [&fetch, &pull, &push, &history] {
            b.add_css_class("flat");
        }
        let top = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        top.append(&branch);
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        top.append(&spacer);
        top.append(&history);
        top.append(&fetch);
        top.append(&pull);
        top.append(&push);

        let message = gtk::TextView::builder().wrap_mode(gtk::WrapMode::WordChar).height_request(64).left_margin(8).top_margin(6).build();
        message.add_css_class("chat-input");
        let amend = gtk::CheckButton::with_label(&tr("Amend"));
        let commit = gtk::Button::with_label(&tr("Commit All"));
        commit.add_css_class("solid");
        commit.connect_clicked(|_| crate::ui::window::app().git.commit());
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        row.append(&commit);
        row.append(&amend);

        let staged = gtk::ListBox::new();
        staged.add_css_class("results");
        let changes = gtk::ListBox::new();
        changes.add_css_class("results");
        let lists = gtk::Box::new(gtk::Orientation::Vertical, 4);
        lists.append(&gtk::Label::builder().label(tr("Staged Changes")).xalign(0.0).css_classes(["group-label"]).build());
        lists.append(&staged);
        lists.append(&gtk::Label::builder().label(tr("Changes")).xalign(0.0).css_classes(["group-label"]).build());
        lists.append(&changes);

        let content = gtk::Box::new(gtk::Orientation::Vertical, 8);
        content.append(&top);
        content.append(&message);
        content.append(&row);
        content.append(&gtk::ScrolledWindow::builder().child(&lists).vexpand(true).build());
        let empty = gtk::Label::builder().label(tr("This solution is not in a Git repository.")).wrap(true).css_classes(["hint"]).build();
        let create = gtk::Button::with_label(&tr("Create Git Repository…"));
        create.add_css_class("quiet");
        create.set_action_name(Some("win.git.init"));
        let empty_box = gtk::Box::new(gtk::Orientation::Vertical, 12);
        empty_box.set_valign(gtk::Align::Center);
        empty_box.append(&empty);
        empty_box.append(&create);
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 0);
        widget.set_margin_start(10);
        widget.set_margin_end(10);
        widget.append(&content);
        widget.append(&empty_box);
        content.set_vexpand(true);
        empty_box.set_vexpand(true);
        GitPanel { widget, branch, message, amend, staged, changes, empty, status: RefCell::default(), content }
    }

    pub fn connect(&self) {
        let _ = &self.empty;
    }

    pub fn focus_message(&self) {
        self.message.grab_focus();
    }

    pub fn refresh(&self) {
        glib::spawn_future_local(async {
            find_top().await;
            let app = crate::ui::window::app();
            let ok = git(status::args(), None).await;
            let me = &app.git;
            let is_repo = ok.is_ok();
            me.content.set_visible(is_repo);
            if let Some(empty_box) = me.content.next_sibling() {
                empty_box.set_visible(!is_repo);
            }
            let Ok(out) = ok else { return };
            let s = status::parse(out.as_bytes());
            me.branch.set_label(&s.branch.clone().unwrap_or_else(|| tr("(detached)")));
            fill(&me.staged, s.staged().cloned().collect(), true);
            fill(&me.changes, s.unstaged().cloned().collect(), false);
            *me.status.borrow_mut() = s;
            me.refresh_branches().await;
        });
    }

    async fn refresh_branches(&self) {
        let Ok(out) = git(ops::branches(), None).await else { return };
        let (current, all) = ops::parse_branches(&out);
        let menu = gio::Menu::new();
        let section = gio::Menu::new();
        for b in all.iter().filter(|b| Some(*b) != current.as_ref()) {
            let item = gio::MenuItem::new(Some(b), None);
            item.set_action_and_target_value(Some("win.git-switch"), Some(&b.to_variant()));
            section.append_item(&item);
        }
        menu.append_section(Some(&tr("Switch to")), &section);
        let more = gio::Menu::new();
        more.append(Some(&tr("New Branch…")), Some("win.git-new-branch"));
        more.append(Some(&tr("Merge into Current Branch…")), Some("win.git-merge"));
        menu.append_section(None, &more);
        self.branch.set_menu_model(Some(&menu));
        install_branch_actions();
    }

    fn commit(&self) {
        let buf = self.message.buffer();
        let msg = buf.text(&buf.start_iter(), &buf.end_iter(), false).to_string();
        if msg.trim().is_empty() && !self.amend.is_active() {
            window::toast(&tr("Enter a commit message."));
            return;
        }
        let amend = self.amend.is_active();
        let all = self.status.borrow().staged().next().is_none();
        crate::ui::window::app().tabs.save_all();
        glib::spawn_future_local(async move {
            if all
                && let Err(e) = git(vec!["add".into(), "--all".into()], None).await
            {
                window::show_error(&tr("Commit"), &e);
                return;
            }
            match git(ops::commit(amend, false), Some(msg.into_bytes())).await {
                Ok(out) => {
                    let app = crate::ui::window::app();
                    app.git.message.buffer().set_text("");
                    app.git.amend.set_active(false);
                    app.output.append("Git", &out);
                    window::toast(&tr("Committed."));
                    app.git.refresh();
                }
                Err(e) => window::show_error(&tr("Commit"), &e),
            }
        });
    }

    pub fn remote(&self, what: &'static str) {
        crate::ui::window::app().status.set_message(&trf("git {}…", &[what]));
        glib::spawn_future_local(async move {
            let args = match what {
                "fetch" => ops::fetch(),
                "pull" => ops::pull(),
                _ => {
                    let s = crate::ui::window::app().git.status.borrow().clone();
                    if s.upstream.is_none() { ops::push(s.branch.as_deref()) } else { ops::push(None) }
                }
            };
            let r = git(args, None).await;
            let app = crate::ui::window::app();
            app.status.set_message("");
            match r {
                Ok(out) => {
                    app.output.append("Git", &out);
                    window::toast(&trf("git {} done.", &[what]));
                }
                Err(e) => window::show_error(&format!("git {what}"), &e),
            }
            app.git.refresh();
        });
    }

    /// Create Git Repository: `git init` and VS' `.gitignore`.
    pub fn init(&self) {
        let Some(dir) = repo_dir() else { return };
        glib::spawn_future_local(async move {
            if let Err(e) = git(ops::init(), None).await {
                window::show_error(&tr("Create Git Repository"), &e);
                return;
            }
            let ignore = dir.join(".gitignore");
            if !ignore.exists() {
                let _ = anvil_core::text::TextFile { encoding: anvil_core::text::Encoding::Utf8 { bom: false }, eol: anvil_core::text::Eol::Crlf, text: ops::VS_GITIGNORE.replace("\r\n", "\n") }.write(&ignore);
            }
            window::toast(&tr("Git repository created."));
            crate::ui::window::app().git.refresh();
        });
    }

    pub fn show_history(&self) {
        let file = crate::ui::window::app().tabs.current_path();
        glib::spawn_future_local(async move {
            let dir = repo_dir().unwrap_or_default();
            let rel = file.as_ref().and_then(|f| f.strip_prefix(&dir).ok()).map(|r| r.to_string_lossy().into_owned());
            let Ok(out) = git(log::args(None, 200), None).await else { return };
            let commits = log::parse(&out);
            let list = gtk::ListBox::new();
            list.add_css_class("results");
            for c in &commits {
                let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
                row.append(&gtk::Label::builder().label(&c.hash[..7.min(c.hash.len())]).css_classes(["mono", "dim"]).build());
                row.append(&gtk::Label::builder().label(&c.subject).xalign(0.0).hexpand(true).ellipsize(gtk::pango::EllipsizeMode::End).build());
                row.append(&gtk::Label::builder().label(format!("{} · {}", c.author, c.date)).css_classes(["count"]).build());
                list.append(&row);
            }
            let title = match &rel {
                Some(_) => tr("Git Repository"),
                None => tr("Git Repository"),
            };
            let dialog = adw::Dialog::builder().title(title).content_width(760).content_height(520).child(&gtk::ScrolledWindow::builder().child(&list).build()).build();
            dialog.present(Some(&crate::ui::window::app().window));
        });
    }
}

fn install_branch_actions() {
    let app = crate::ui::window::app();
    if app.window.lookup_action("git-switch").is_some() {
        return;
    }
    let switch = gio::SimpleAction::new("git-switch", Some(glib::VariantTy::STRING));
    switch.connect_activate(|_, v| {
        let Some(b) = v.and_then(|v| v.get::<String>()) else { return };
        let b = b.trim_start_matches("origin/").to_owned();
        glib::spawn_future_local(async move {
            if let Err(e) = git(ops::switch(&b), None).await {
                window::show_error(&tr("Switch Branch"), &e);
            }
            crate::ui::window::app().git.refresh();
            crate::ui::explorer::reload_solution_model();
        });
    });
    let new_branch = gio::SimpleAction::new("git-new-branch", None);
    new_branch.connect_activate(|_, _| ask_branch(&tr("New Branch"), |name| ops::create_branch(&name)));
    let merge = gio::SimpleAction::new("git-merge", None);
    merge.connect_activate(|_, _| ask_branch(&tr("Merge into Current Branch"), |name| ops::merge(&name)));
    app.window.add_action(&switch);
    app.window.add_action(&new_branch);
    app.window.add_action(&merge);
}

fn ask_branch(title: &str, args: impl Fn(String) -> Vec<String> + 'static) {
    let app = crate::ui::window::app();
    let d = adw::AlertDialog::new(Some(title), None);
    let e = gtk::Entry::builder().activates_default(true).placeholder_text(tr("Branch name")).build();
    d.set_extra_child(Some(&e));
    d.add_responses(&[("cancel", &tr("Cancel")), ("ok", &tr("OK"))]);
    d.set_default_response(Some("ok"));
    let title = title.to_owned();
    d.choose(Some(&app.window), gio::Cancellable::NONE, move |r| {
        let name = e.text().trim().to_owned();
        if r != "ok" || name.is_empty() {
            return;
        }
        let a = args(name);
        let title = title.clone();
        glib::spawn_future_local(async move {
            if let Err(err) = git(a, None).await {
                window::show_error(&title, &err);
            }
            crate::ui::window::app().git.refresh();
        });
    });
}

fn fill(list: &gtk::ListBox, entries: Vec<status::Entry>, staged: bool) {
    while let Some(c) = list.first_child() {
        list.remove(&c);
    }
    for e in entries {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let ch = if staged { e.staged } else { e.unstaged };
        let mark = gtk::Label::builder().label(ch.letter()).width_chars(2).css_classes(["mono", if ch == status::Change::Deleted { "removed" } else { "dim" }]).build();
        let name = Path::new(&e.path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let label = gtk::Label::builder().label(&name).xalign(0.0).hexpand(true).tooltip_text(&e.path).build();
        let action = gtk::Button::from_icon_name(if staged { "list-remove-symbolic" } else { "list-add-symbolic" });
        action.add_css_class("flat");
        action.set_tooltip_text(Some(&if staged { tr("Unstage") } else { tr("Stage") }));
        let path = e.path.clone();
        action.connect_clicked(move |_| {
            let path = path.clone();
            glib::spawn_future_local(async move {
                let args = if staged { ops::unstage(&[&path]) } else { ops::stage(&[&path]) };
                if let Err(e) = git(args, None).await {
                    window::show_error(&tr("Git"), &e);
                }
                crate::ui::window::app().git.refresh();
            });
        });
        row.append(&mark);
        row.append(&label);
        // Undo Changes, as VS' Git Changes window offers it.
        if !staged && ch != status::Change::Untracked {
            let discard = gtk::Button::from_icon_name("edit-undo-symbolic");
            discard.add_css_class("flat");
            discard.set_tooltip_text(Some(&tr("Undo Changes")));
            let path = e.path.clone();
            discard.connect_clicked(move |_| ask_discard(path.clone()));
            row.append(&discard);
        }
        row.append(&action);
        let gesture = gtk::GestureClick::new();
        let path = e.path.clone();
        gesture.connect_pressed(move |_, n, _, _| {
            if n == 2 {
                show_diff(path.clone(), staged);
            }
        });
        row.add_controller(gesture);
        list.append(&row);
    }
}

fn ask_discard(path: String) {
    let app = crate::ui::window::app();
    let d = adw::AlertDialog::new(Some(&trf("Undo the changes to {}?", &[&path])), Some(&tr("The file goes back to its last committed state. This cannot be undone.")));
    d.add_responses(&[("cancel", &tr("Cancel")), ("discard", &tr("Undo Changes"))]);
    d.set_response_appearance("discard", adw::ResponseAppearance::Destructive);
    d.set_default_response(Some("cancel"));
    d.choose(Some(&app.window), gtk::gio::Cancellable::NONE, move |r| {
        if r != "discard" {
            return;
        }
        let path = path.clone();
        glib::spawn_future_local(async move {
            if let Err(e) = git(ops::discard(&[&path]), None).await {
                window::show_error(&tr("Git"), &e);
            }
            crate::ui::window::app().git.refresh();
        });
    });
}

/// Side by side: HEAD on the left, the working file (or the index) on the
/// right; removed lines in `down`, added ones in `up`.
fn show_diff(path: String, staged: bool) {
    glib::spawn_future_local(async move {
        let dir = repo_dir().unwrap_or_default();
        let old = git(ops::show_head(&path), None).await.unwrap_or_default();
        let new = if staged { git(vec!["show".into(), format!(":{path}")], None).await.unwrap_or_default() } else { anvil_core::text::TextFile::read(&dir.join(&path)).map(|f| f.text).unwrap_or_default() };
        let old = old.replace("\r\n", "\n");
        let a: Vec<&str> = old.lines().collect();
        let b: Vec<&str> = new.lines().collect();
        let diff = anvil_core::diff::lines(&a, &b);
        let (left, right) = (diff_view(), diff_view());
        let (lb, rb) = (left.buffer(), right.buffer());
        let p = crate::palette::current();
        let add = gtk::TextTag::builder().name("added").foreground_rgba(&p.up).build();
        let del = gtk::TextTag::builder().name("removed").foreground_rgba(&p.down).build();
        let pad = gtk::TextTag::builder().name("pad").foreground_rgba(&p.ink3).build();
        lb.tag_table().add(&del);
        lb.tag_table().add(&pad);
        rb.tag_table().add(&add);
        rb.tag_table().add(&gtk::TextTag::builder().name("pad").foreground_rgba(&p.ink3).build());
        for l in &diff {
            type Side<'a> = Option<(&'a str, &'a str)>;
            let (lt, rt): (Side, Side) = match l.op {
                anvil_core::diff::Op::Equal => (Some((&l.text, "")), Some((&l.text, ""))),
                anvil_core::diff::Op::Delete => (Some((&l.text, "removed")), Some(("", "pad"))),
                anvil_core::diff::Op::Insert => (Some(("", "pad")), Some((&l.text, "added"))),
            };
            for (buf, part) in [(&lb, lt), (&rb, rt)] {
                if let Some((text, tag)) = part {
                    let mut end = buf.end_iter();
                    if tag.is_empty() {
                        buf.insert(&mut end, &format!("{text}\n"));
                    } else {
                        buf.insert_with_tags_by_name(&mut end, &format!("{text}\n"), &[tag]);
                    }
                }
            }
        }
        let paned = gtk::Paned::new(gtk::Orientation::Horizontal);
        let ls = gtk::ScrolledWindow::builder().child(&left).hexpand(true).build();
        let rs = gtk::ScrolledWindow::builder().child(&right).hexpand(true).build();
        rs.set_vadjustment(Some(&ls.vadjustment()));
        paned.set_start_child(Some(&ls));
        paned.set_end_child(Some(&rs));
        let dialog = adw::Dialog::builder().title(trf("{} (HEAD) vs. {}", &[&path, &if staged { tr("staged") } else { tr("working tree") }])).content_width(1100).content_height(700).child(&paned).build();
        dialog.present(Some(&crate::ui::window::app().window));
    });
}

fn diff_view() -> gtk::TextView {
    let v = gtk::TextView::builder().editable(false).monospace(true).left_margin(10).build();
    v.add_css_class("diff");
    v
}
