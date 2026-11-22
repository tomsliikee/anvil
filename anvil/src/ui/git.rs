//! Git Changes and Git Repository in reduced form, through the host's git
//! so credentials, config and hooks are the user's own.
//!
//! Every git call runs on the I/O runtime; the main thread only fills the
//! lists. The panel refreshes after Anvil's own git commands, when files
//! change, when `.git` changes (a commit or checkout in a terminal) and
//! when the window gets the focus back; refreshes coalesce, so a burst of
//! events costs one `git status`.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use anvil_core::host::Command;
use anvil_git::{diff, log, ops, status};
use gtk::{gio, glib};

use crate::host;
use crate::i18n::{tr, trf};
use crate::ui::window::{self};

pub struct GitPanel {
    pub widget: gtk::Box,
    branch: gtk::MenuButton,
    message: gtk::TextView,
    amend: gtk::CheckButton,
    commit: gtk::Button,
    staged: gtk::ListBox,
    staged_label: gtk::Label,
    changes: gtk::ListBox,
    changes_label: gtk::Label,
    conflicts: gtk::ListBox,
    conflicts_box: gtk::Box,
    empty: gtk::Label,
    sync: gtk::Label,
    status: RefCell<status::Status>,
    content: gtk::Box,
    /// A refresh runs; `again` asks for one more after it.
    refreshing: Cell<bool>,
    again: Cell<bool>,
    /// The `.git` folder watched and its debounce timer.
    git_monitor: RefCell<Option<(PathBuf, gio::FileMonitor)>>,
    git_timer: RefCell<Option<glib::SourceId>>,
}

thread_local! {
    /// The repository's top folder (`git rev-parse --show-toplevel`) for the
    /// solution folder it was asked for: one repository per course often
    /// holds many solutions, and `git status` paths are relative to the top.
    static TOP: RefCell<Option<(PathBuf, Option<PathBuf>)>> = const { RefCell::new(None) };
}

fn repo_dir() -> Option<PathBuf> {
    let sol = window::solution_dir()?;
    let top = TOP.with(|t| t.borrow().as_ref().filter(|(s, _)| *s == sol).and_then(|(_, top)| top.clone()));
    Some(top.filter(|t| sol.starts_with(t)).unwrap_or(sol))
}

/// Asks git once per solution where the repository's top is.
async fn find_top() -> Option<PathBuf> {
    let sol = window::solution_dir()?;
    if let Some(known) = TOP.with(|t| t.borrow().as_ref().filter(|(s, _)| *s == sol).map(|(_, top)| top.clone())) {
        return known;
    }
    let top = host::run(Command::new("git").args(["rev-parse", "--show-toplevel"]).cwd(&sol), None).await.ok().filter(|o| o.ok()).map(|o| PathBuf::from(o.stdout_text().trim()));
    TOP.with(|t| *t.borrow_mut() = Some((sol, top.clone())));
    top
}

/// Fails a git action without a modal dialog: the title in a toast, the
/// details in the Output window's `pane`, shown by the toast's button.
pub(crate) fn report_error(pane: &'static str, title: &str, detail: &str) {
    let app = crate::ui::window::app();
    app.output.append(pane, &format!("{title}\n{}\n", detail.trim_end()));
    let first = detail.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    let text = if first.is_empty() { title.to_owned() } else { format!("{title}: {first}") };
    let t = adw::Toast::new(&glib::markup_escape_text(&text));
    t.set_button_label(Some(&tr("Details")));
    t.set_timeout(8);
    t.connect_button_clicked(move |_| {
        let app = crate::ui::window::app();
        window::show_bottom("output");
        app.output.show(pane);
    });
    app.toast.add_toast(t);
}

/// Where the askpass program and a typed password live for one command:
/// a folder of the user's runtime directory that the host sees under the
/// same path (in the Flatpak the app's own `/run/user/<id>/app/<id>`).
fn askpass_dir() -> PathBuf {
    let rt = glib::user_runtime_dir();
    match std::env::var("FLATPAK_ID") {
        Ok(id) if host::in_flatpak() => rt.join("app").join(id),
        _ => rt.join("anvil"),
    }
}

fn askpass_script() -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let dir = askpass_dir();
    std::fs::create_dir_all(&dir).ok()?;
    let p = dir.join("anvil-askpass.sh");
    if std::fs::read_to_string(&p).ok().as_deref() != Some(ops::ASKPASS_SCRIPT) {
        std::fs::write(&p, ops::ASKPASS_SCRIPT).ok()?;
    }
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o700)).ok()?;
    Some(p)
}

async fn git(args: Vec<String>, stdin: Option<Vec<u8>>) -> Result<String, String> {
    git_with(args, stdin, None).await
}

/// Runs git in the repository. Without a terminal git cannot prompt, so
/// `GIT_ASKPASS` answers from `credentials` (a 0600 file) when given and
/// fails otherwise, which [`ops::needs_credentials`] recognises.
async fn git_with(args: Vec<String>, stdin: Option<Vec<u8>>, credentials: Option<&Path>) -> Result<String, String> {
    let dir = repo_dir().ok_or_else(|| tr("Open a solution first."))?;
    let mut cmd = Command::new("git").args(args).cwd(&dir).env("GIT_TERMINAL_PROMPT", "0");
    if let Some(script) = askpass_script() {
        cmd = cmd.env("GIT_ASKPASS", script.to_string_lossy());
    }
    if let Some(c) = credentials {
        cmd = cmd.env("ANVIL_ASKPASS_FILE", c.to_string_lossy());
    }
    let out = host::run(cmd, stdin).await.map_err(|e| e.to_string())?;
    let app = crate::ui::window::app();
    if !out.stderr.is_empty() {
        app.output.append("Git", &out.stderr_text());
    }
    if out.ok() { Ok(out.stdout_text()) } else { Err(format!("{}{}", out.stdout_text(), out.stderr_text()).trim().to_owned()) }
}

/// Runs a command and refreshes the panel; a failure becomes a toast.
fn run_then_refresh(title: String, cmds: Vec<Vec<String>>) {
    glib::spawn_future_local(async move {
        for a in cmds {
            if let Err(e) = git(a, None).await {
                report_error("Git", &title, &e);
                break;
            }
        }
        crate::ui::window::app().git.refresh();
    });
}

fn section_label(text: &str) -> gtk::Label {
    gtk::Label::builder().label(text).xalign(0.0).hexpand(true).css_classes(["group-label"]).build()
}

fn small_button(icon: &str, tooltip: &str) -> gtk::Button {
    let b = gtk::Button::from_icon_name(icon);
    b.add_css_class("flat");
    b.set_tooltip_text(Some(tooltip));
    b.set_valign(gtk::Align::Center);
    b
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
        // Ahead/behind the upstream, as VS shows it next to the branch.
        let sync = gtk::Label::builder().css_classes(["count"]).build();
        let top = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        top.append(&branch);
        top.append(&sync);
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        top.append(&spacer);
        top.append(&history);
        top.append(&fetch);
        top.append(&pull);
        top.append(&push);

        let message = gtk::TextView::builder().wrap_mode(gtk::WrapMode::WordChar).height_request(64).left_margin(8).top_margin(6).build();
        message.add_css_class("chat-input");
        message.update_property(&[gtk::accessible::Property::Label(&tr("Commit message"))]);
        let amend = gtk::CheckButton::with_label(&tr("Amend"));
        let commit = gtk::Button::with_label(&tr("Commit All"));
        commit.add_css_class("solid");
        commit.set_tooltip_text(Some(&tr("Commit (Ctrl+Enter)")));
        commit.connect_clicked(|_| crate::ui::window::app().git.commit());
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        row.append(&commit);
        row.append(&amend);

        let conflicts = gtk::ListBox::new();
        conflicts.add_css_class("results");
        conflicts.set_selection_mode(gtk::SelectionMode::None);
        let abort = gtk::Button::with_label(&tr("Abort Merge"));
        abort.add_css_class("flat");
        abort.connect_clicked(|_| run_then_refresh(tr("Abort Merge"), vec![ops::merge_abort()]));
        let conflicts_head = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        conflicts_head.append(&section_label(&tr("Merge Conflicts")));
        conflicts_head.append(&abort);
        let conflicts_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
        conflicts_box.append(&conflicts_head);
        conflicts_box.append(&conflicts);
        conflicts_box.set_visible(false);

        let staged = gtk::ListBox::new();
        staged.add_css_class("results");
        staged.set_selection_mode(gtk::SelectionMode::None);
        let changes = gtk::ListBox::new();
        changes.add_css_class("results");
        changes.set_selection_mode(gtk::SelectionMode::None);
        let staged_label = section_label(&tr("Staged Changes"));
        let unstage_all = small_button("list-remove-all-symbolic", &tr("Unstage All"));
        unstage_all.connect_clicked(|_| run_then_refresh(tr("Unstage All"), vec![vec!["reset".into(), "--quiet".into()]]));
        let staged_head = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        staged_head.append(&staged_label);
        staged_head.append(&unstage_all);
        let changes_label = section_label(&tr("Changes"));
        let stage_all = small_button("list-add-symbolic", &tr("Stage All"));
        stage_all.connect_clicked(|_| run_then_refresh(tr("Stage All"), vec![vec!["add".into(), "--all".into()]]));
        let changes_head = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        changes_head.append(&changes_label);
        changes_head.append(&stage_all);
        let lists = gtk::Box::new(gtk::Orientation::Vertical, 4);
        lists.append(&conflicts_box);
        lists.append(&staged_head);
        lists.append(&staged);
        lists.append(&changes_head);
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
        GitPanel {
            widget,
            branch,
            message,
            amend,
            commit,
            staged,
            staged_label,
            changes,
            changes_label,
            conflicts,
            conflicts_box,
            empty,
            sync,
            status: RefCell::default(),
            content,
            refreshing: Cell::new(false),
            again: Cell::new(false),
            git_monitor: RefCell::default(),
            git_timer: RefCell::default(),
        }
    }

    pub fn connect(&self) {
        let _ = &self.empty;
        // Ctrl+Enter commits, as in VS' Git Changes.
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        keys.connect_key_pressed(|_, key, _, state| {
            if state.contains(gtk::gdk::ModifierType::CONTROL_MASK) && matches!(key, gtk::gdk::Key::Return | gtk::gdk::Key::KP_Enter) {
                crate::ui::window::app().git.commit();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        self.message.add_controller(keys);
        // Amend shows the last message to edit.
        self.amend.connect_toggled(|b| {
            if !b.is_active() {
                return;
            }
            glib::spawn_future_local(async {
                let Ok(msg) = git(ops::last_message(), None).await else { return };
                let buf = crate::ui::window::app().git.message.buffer();
                if buf.char_count() == 0 {
                    buf.set_text(msg.trim_end());
                }
            });
        });
        // Back from a terminal or another tool: whatever git did there.
        crate::ui::window::app().window.connect_is_active_notify(|w| {
            if w.is_active() && window::solution_dir().is_some() {
                crate::ui::window::app().git.refresh();
            }
        });
    }

    pub fn focus_message(&self) {
        self.message.grab_focus();
    }

    /// Reads the status again; calls during a refresh fold into one more.
    pub fn refresh(&self) {
        if self.refreshing.get() {
            self.again.set(true);
            return;
        }
        self.refreshing.set(true);
        glib::spawn_future_local(async {
            let top = find_top().await;
            let app = crate::ui::window::app();
            let ok = if top.is_some() { git(status::args(), None).await } else { Err(String::new()) };
            let me = &app.git;
            let is_repo = ok.is_ok();
            me.content.set_visible(is_repo);
            if let Some(empty_box) = me.content.next_sibling() {
                empty_box.set_visible(!is_repo);
            }
            if let Ok(out) = ok {
                let s = status::parse(out.as_bytes());
                me.branch.set_label(&s.branch.clone().unwrap_or_else(|| tr("(detached)")));
                me.sync.set_label(&if s.upstream.is_some() && (s.ahead > 0 || s.behind > 0) { format!("↑{} ↓{}", s.ahead, s.behind) } else { String::new() });
                let staged: Vec<status::Entry> = s.staged().cloned().collect();
                let unstaged: Vec<status::Entry> = s.unstaged().cloned().collect();
                let conflicts: Vec<status::Entry> = s.conflicts().cloned().collect();
                me.staged_label.set_label(&trf("Staged Changes ({})", &[&staged.len().to_string()]));
                me.changes_label.set_label(&trf("Changes ({})", &[&unstaged.len().to_string()]));
                me.commit.set_label(&if staged.is_empty() { tr("Commit All") } else { tr("Commit Staged") });
                me.conflicts_box.set_visible(!conflicts.is_empty());
                fill_conflicts(&me.conflicts, conflicts);
                fill(&me.staged, staged, true);
                fill(&me.changes, unstaged, false);
                *me.status.borrow_mut() = s;
                if let Some(top) = &top {
                    me.watch_git_dir(top);
                    me.prefill_merge_message(top);
                }
                me.refresh_branches().await;
            }
            me.refreshing.set(false);
            if me.again.replace(false) {
                me.refresh();
            }
        });
    }

    /// Watches `.git` itself (HEAD, index, MERGE_HEAD): the solution
    /// watcher skips it, and a commit in a terminal changes nothing else.
    fn watch_git_dir(&self, top: &Path) {
        let dir = top.join(".git");
        if !dir.is_dir() || self.git_monitor.borrow().as_ref().is_some_and(|(d, _)| *d == dir) {
            return;
        }
        let Ok(m) = gio::File::for_path(&dir).monitor_directory(gio::FileMonitorFlags::NONE, gio::Cancellable::NONE) else { return };
        m.connect_changed(|_, file, _, _| {
            let name = file.basename().map(|b| b.to_string_lossy().into_owned()).unwrap_or_default();
            // Lock files come and go during every git command.
            if name.ends_with(".lock") {
                return;
            }
            let app = crate::ui::window::app();
            if let Some(id) = app.git.git_timer.borrow_mut().take() {
                id.remove();
            }
            let id = glib::timeout_add_local_once(Duration::from_millis(300), || {
                let app = crate::ui::window::app();
                app.git.git_timer.borrow_mut().take();
                app.git.refresh();
            });
            *app.git.git_timer.borrow_mut() = Some(id);
        });
        *self.git_monitor.borrow_mut() = Some((dir, m));
    }

    /// During a merge the message box starts with git's merge message.
    fn prefill_merge_message(&self, top: &Path) {
        let buf = self.message.buffer();
        if buf.char_count() > 0 || !top.join(".git/MERGE_HEAD").exists() {
            return;
        }
        if let Ok(m) = std::fs::read_to_string(top.join(".git/MERGE_MSG")) {
            let text: Vec<&str> = m.lines().filter(|l| !l.starts_with('#')).collect();
            buf.set_text(text.join("\n").trim_end());
        }
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
        let amend = self.amend.is_active();
        if msg.trim().is_empty() && !amend {
            window::toast(&tr("Enter a commit message."));
            self.message.grab_focus();
            return;
        }
        let (all, conflicts) = {
            let s = self.status.borrow();
            (s.staged().next().is_none(), s.conflicts().count())
        };
        if conflicts > 0 {
            window::toast(&tr("Resolve the merge conflicts first."));
            return;
        }
        crate::ui::window::app().tabs.save_all();
        glib::spawn_future_local(async move {
            if all
                && let Err(e) = git(vec!["add".into(), "--all".into()], None).await
            {
                report_error("Git", &tr("Commit"), &e);
                return;
            }
            // Amend with an empty box keeps the last message.
            let r = if amend && msg.trim().is_empty() { git(vec!["commit".into(), "--amend".into(), "--no-edit".into()], None).await } else { git(ops::commit(amend, false), Some(msg.into_bytes())).await };
            let app = crate::ui::window::app();
            match r {
                Ok(out) => {
                    app.git.message.buffer().set_text("");
                    app.git.amend.set_active(false);
                    app.output.append("Git", &out);
                    window::toast(&out.lines().next().map(|l| trf("Committed: {}", &[l.trim()])).unwrap_or_else(|| tr("Committed.")));
                }
                Err(e) => report_error("Git", &tr("Commit"), &e),
            }
            app.git.refresh();
        });
    }

    pub fn remote(&self, what: &'static str) {
        let app = crate::ui::window::app();
        app.status.set_message(&trf("git {}…", &[what]));
        glib::spawn_future_local(async move {
            let args = match what {
                "fetch" => ops::fetch(),
                "pull" => ops::pull(),
                _ => {
                    let s = crate::ui::window::app().git.status.borrow().clone();
                    if s.upstream.is_none() { ops::push(s.branch.as_deref()) } else { ops::push(None) }
                }
            };
            let mut r = git(args.clone(), None).await;
            // No helper had a password: ask once, then try again with it.
            if let Err(e) = &r
                && ops::needs_credentials(e)
            {
                r = match ask_credentials(e).await {
                    Some(file) => {
                        let again = git_with(args, None, Some(&file)).await;
                        let _ = std::fs::remove_file(&file);
                        again
                    }
                    None => Err(tr("Cancelled: no credentials given.")),
                };
            }
            let app = crate::ui::window::app();
            app.status.set_message("");
            match r {
                Ok(out) => {
                    app.output.append("Git", &out);
                    window::toast(&trf("git {} done.", &[what]));
                }
                Err(e) if ops::needs_credentials(&e) => report_error("Git", &trf("git {}: authentication failed", &[what]), &e),
                Err(e) => report_error("Git", &format!("git {what}"), &e),
            }
            app.git.refresh();
        });
    }

    /// Create Git Repository: `git init` and VS' `.gitignore`.
    pub fn init(&self) {
        let Some(dir) = repo_dir() else { return };
        glib::spawn_future_local(async move {
            if let Err(e) = git(ops::init(), None).await {
                report_error("Git", &tr("Create Git Repository"), &e);
                return;
            }
            TOP.with(|t| t.borrow_mut().take());
            let ignore = dir.join(".gitignore");
            if !ignore.exists() {
                let _ = anvil_core::text::TextFile { encoding: anvil_core::text::Encoding::Utf8 { bom: false }, eol: anvil_core::text::Eol::Crlf, text: ops::VS_GITIGNORE.replace("\r\n", "\n") }.write(&ignore);
            }
            window::toast(&tr("Git repository created."));
            crate::ui::window::app().git.refresh();
        });
    }

    pub fn show_history(&self) {
        glib::spawn_future_local(async move {
            let out = match git(log::args(None, 200), None).await {
                Ok(o) => o,
                Err(e) => {
                    report_error("Git", &tr("View History"), &e);
                    return;
                }
            };
            let commits = log::parse(&out);
            let list = gtk::ListBox::new();
            list.add_css_class("results");
            for c in &commits {
                let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
                row.append(&gtk::Label::builder().label(&c.hash[..7.min(c.hash.len())]).css_classes(["mono", "dim"]).build());
                let subject = gtk::Box::new(gtk::Orientation::Horizontal, 6);
                subject.set_hexpand(true);
                if !c.refs.is_empty() {
                    subject.append(&gtk::Label::builder().label(&c.refs).css_classes(["count"]).build());
                }
                subject.append(&gtk::Label::builder().label(&c.subject).xalign(0.0).hexpand(true).ellipsize(gtk::pango::EllipsizeMode::End).build());
                row.append(&subject);
                row.append(&gtk::Label::builder().label(format!("{} · {}", c.author, c.date)).css_classes(["count"]).build());
                list.append(&row);
            }
            let dialog = adw::Dialog::builder().title(tr("Git Repository")).content_width(760).content_height(520).child(&gtk::ScrolledWindow::builder().child(&list).build()).build();
            dialog.present(Some(&crate::ui::window::app().window));
        });
    }
}

/// Asks for user name and password; writes them to a 0600 file only the
/// askpass program reads, and returns its path.
async fn ask_credentials(error: &str) -> Option<PathBuf> {
    use std::os::unix::fs::OpenOptionsExt;
    let app = crate::ui::window::app();
    // "…for 'https://host/…'": name the remote the prompt is for.
    let url = error.split('\'').nth(1).filter(|u| u.contains("://")).unwrap_or("").to_owned();
    let d = adw::AlertDialog::new(Some(&tr("Git Credentials")), Some(&if url.is_empty() { tr("The remote asks for a user name and a password or token.") } else { trf("{} asks for a user name and a password or token.", &[&url]) }));
    let user = gtk::Entry::builder().placeholder_text(tr("User name")).activates_default(true).build();
    let pass = gtk::PasswordEntry::builder().placeholder_text(tr("Password or token")).activates_default(true).show_peek_icon(true).build();
    let fields = gtk::Box::new(gtk::Orientation::Vertical, 8);
    fields.append(&user);
    fields.append(&pass);
    d.set_extra_child(Some(&fields));
    d.add_responses(&[("cancel", &tr("Cancel")), ("ok", &tr("Sign In"))]);
    d.set_response_appearance("ok", adw::ResponseAppearance::Suggested);
    d.set_default_response(Some("ok"));
    d.set_close_response("cancel");
    user.grab_focus();
    let r = d.choose_future(Some(&app.window)).await;
    if r != "ok" || user.text().is_empty() {
        return None;
    }
    let path = askpass_dir().join(format!("anvil-cred-{}", std::process::id()));
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&path).ok()?;
    use std::io::Write;
    writeln!(f, "{}\n{}", user.text(), pass.text()).ok()?;
    Some(path)
}

fn install_branch_actions() {
    let app = crate::ui::window::app();
    if app.window.lookup_action("git-switch").is_some() {
        return;
    }
    let switch = gio::SimpleAction::new("git-switch", Some(glib::VariantTy::STRING));
    switch.connect_activate(|_, v| {
        let Some(b) = v.and_then(|v| v.get::<String>()) else { return };
        // origin/feature: `git switch feature` creates the tracking branch.
        let b = b.split_once('/').filter(|(remote, _)| *remote == "origin").map(|(_, n)| n.to_owned()).unwrap_or(b);
        glib::spawn_future_local(async move {
            match git(ops::switch(&b), None).await {
                Ok(_) => window::toast(&trf("Switched to {}.", &[&b])),
                Err(e) => report_error("Git", &tr("Switch Branch"), &e),
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
                // A merge that stopped on conflicts fails too; the panel's
                // Merge Conflicts list then says what to do.
                report_error("Git", &title, &err);
            }
            crate::ui::window::app().git.refresh();
            crate::ui::explorer::reload_solution_model();
        });
    });
}

fn clear(list: &gtk::ListBox) {
    while let Some(c) = list.first_child() {
        list.remove(&c);
    }
}

fn name_of(path: &str) -> String {
    Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

fn fill_conflicts(list: &gtk::ListBox, entries: Vec<status::Entry>) {
    clear(list);
    for e in entries {
        let code = e.conflict.clone().unwrap_or_default();
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        row.append(&gtk::Label::builder().label(&code).width_chars(3).css_classes(["mono", "removed"]).build());
        row.append(&gtk::Label::builder().label(name_of(&e.path)).xalign(0.0).hexpand(true).ellipsize(gtk::pango::EllipsizeMode::Middle).tooltip_text(&e.path).build());
        let open = gtk::Button::with_label(&tr("Open"));
        let mine = gtk::Button::with_label(&tr("Take Current"));
        mine.set_tooltip_text(Some(&tr("Keep the version of the current branch")));
        let theirs = gtk::Button::with_label(&tr("Take Incoming"));
        theirs.set_tooltip_text(Some(&tr("Take the version of the branch being merged")));
        for b in [&open, &mine, &theirs] {
            b.add_css_class("flat");
            row.append(b);
        }
        let path = e.path.clone();
        open.connect_clicked(move |_| {
            if let Some(dir) = repo_dir() {
                crate::ui::window::app().tabs.open(&dir.join(&path), false);
            }
        });
        for (b, ours) in [(&mine, true), (&theirs, false)] {
            let (path, code) = (e.path.clone(), code.clone());
            b.connect_clicked(move |_| run_then_refresh(tr("Resolve Conflict"), ops::take_side(&path, &code, ours)));
        }
        list.append(&row);
    }
}

fn fill(list: &gtk::ListBox, entries: Vec<status::Entry>, staged: bool) {
    clear(list);
    for e in entries {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let ch = if staged { e.staged } else { e.unstaged };
        let mark = gtk::Label::builder().label(ch.letter()).width_chars(2).css_classes(["mono", if ch == status::Change::Deleted { "removed" } else { "dim" }]).build();
        let label = gtk::Label::builder().label(name_of(&e.path)).xalign(0.0).hexpand(true).ellipsize(gtk::pango::EllipsizeMode::Middle).tooltip_text(&e.path).build();
        let action = small_button(if staged { "list-remove-symbolic" } else { "list-add-symbolic" }, &if staged { tr("Unstage") } else { tr("Stage") });
        let path = e.path.clone();
        action.connect_clicked(move |_| {
            let args = if staged { ops::unstage(&[&path]) } else { ops::stage(&[&path]) };
            run_then_refresh(tr("Git"), vec![args]);
        });
        let open = small_button("view-dual-symbolic", &tr("Compare with Unmodified"));
        let (path, untracked) = (e.path.clone(), ch == status::Change::Untracked);
        open.connect_clicked(move |_| show_diff(path.clone(), staged, untracked));
        row.append(&mark);
        row.append(&label);
        row.append(&open);
        // Undo Changes, as VS' Git Changes window offers it.
        if !staged && ch != status::Change::Untracked {
            let discard = small_button("edit-undo-symbolic", &tr("Undo Changes"));
            let path = e.path.clone();
            discard.connect_clicked(move |_| ask_discard(path.clone()));
            row.append(&discard);
        }
        row.append(&action);
        let gesture = gtk::GestureClick::new();
        let (path, untracked) = (e.path.clone(), ch == status::Change::Untracked);
        gesture.connect_pressed(move |_, n, _, _| {
            if n == 2 {
                show_diff(path.clone(), staged, untracked);
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
        if r == "discard" {
            run_then_refresh(tr("Undo Changes"), vec![ops::discard(&[&path])]);
        }
    });
}

/// The diff of one file, hunk by hunk, each with Stage/Unstage (and Undo
/// for unstaged ones); the view reloads after every hunk action.
fn show_diff(path: String, staged: bool, untracked: bool) {
    let body = gtk::Box::new(gtk::Orientation::Vertical, 10);
    body.set_margin_start(12);
    body.set_margin_end(12);
    body.set_margin_top(8);
    body.set_margin_bottom(12);
    let title = trf("{} (HEAD) vs. {}", &[&path, &if staged { tr("staged") } else { tr("working tree") }]);
    let dialog = adw::Dialog::builder().title(title).content_width(1000).content_height(700).child(&gtk::ScrolledWindow::builder().child(&body).build()).build();
    dialog.present(Some(&crate::ui::window::app().window));
    load_diff(Rc::new(body), path, staged, untracked);
}

fn load_diff(body: Rc<gtk::Box>, path: String, staged: bool, untracked: bool) {
    glib::spawn_future_local(async move {
        let text = if untracked {
            // Untracked: everything is new; git diff has nothing to say.
            let full = repo_dir().map(|d| d.join(&path)).and_then(|p| anvil_core::text::TextFile::read(&p).ok()).map(|f| f.text).unwrap_or_default();
            let n = full.lines().count();
            let mut s = format!("@@ -0,0 +1,{n} @@\n");
            for l in full.lines() {
                s.push('+');
                s.push_str(l);
                s.push('\n');
            }
            Ok(s)
        } else {
            git(ops::diff(&path, staged), None).await
        };
        while let Some(c) = body.first_child() {
            body.remove(&c);
        }
        let d = match text {
            Ok(t) => diff::parse(&t),
            Err(e) => {
                body.append(&gtk::Label::builder().label(e).wrap(true).xalign(0.0).css_classes(["hint"]).build());
                return;
            }
        };
        if d.binary || d.hunks.is_empty() {
            body.append(&gtk::Label::builder().label(if d.binary { tr("Binary file.") } else { tr("No changes.") }).xalign(0.0).css_classes(["hint"]).build());
            return;
        }
        let p = crate::palette::current();
        for (i, h) in d.hunks.iter().enumerate() {
            let head = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            head.append(&gtk::Label::builder().label(&h.header).xalign(0.0).hexpand(true).ellipsize(gtk::pango::EllipsizeMode::End).css_classes(["mono", "dim"]).build());
            if !untracked {
                let patch = diff::hunk_patch(&d, i).unwrap_or_default();
                let mut actions: Vec<(String, bool, bool)> = Vec::new();
                if staged {
                    actions.push((tr("Unstage Hunk"), true, true));
                } else {
                    actions.push((tr("Undo Hunk"), false, true));
                    actions.push((tr("Stage Hunk"), true, false));
                }
                for (label, cached, reverse) in actions {
                    let b = gtk::Button::with_label(&label);
                    b.add_css_class("flat");
                    let (patch, body, path, label2) = (patch.clone(), body.clone(), path.clone(), label.clone());
                    b.connect_clicked(move |_| {
                        let (patch, body, path, label) = (patch.clone(), body.clone(), path.clone(), label2.clone());
                        glib::spawn_future_local(async move {
                            if let Err(e) = git(ops::apply_patch(cached, reverse), Some(patch.into_bytes())).await {
                                report_error("Git", &label, &e);
                            }
                            crate::ui::window::app().git.refresh();
                            load_diff(body, path, staged, false);
                        });
                    });
                    head.append(&b);
                }
            }
            body.append(&head);
            let view = gtk::TextView::builder().editable(false).monospace(true).left_margin(10).cursor_visible(false).build();
            view.add_css_class("diff");
            let buf = view.buffer();
            buf.tag_table().add(&gtk::TextTag::builder().name("added").foreground_rgba(&p.up).build());
            buf.tag_table().add(&gtk::TextTag::builder().name("removed").foreground_rgba(&p.down).build());
            buf.tag_table().add(&gtk::TextTag::builder().name("dim").foreground_rgba(&p.ink3).build());
            let mut new_line = h.new_start;
            for (k, t) in &h.lines {
                let (prefix, tag) = match k {
                    diff::LineKind::Added => ("+", "added"),
                    diff::LineKind::Removed => ("-", "removed"),
                    diff::LineKind::Context => (" ", ""),
                    diff::LineKind::NoNewline => ("\\", "dim"),
                };
                let num = if matches!(k, diff::LineKind::Removed | diff::LineKind::NoNewline) { "     ".to_owned() } else { format!("{new_line:>5}") };
                if matches!(k, diff::LineKind::Added | diff::LineKind::Context) {
                    new_line += 1;
                }
                let mut end = buf.end_iter();
                buf.insert_with_tags_by_name(&mut end, &format!("{num} "), &["dim"]);
                let line = format!("{prefix}{t}\n");
                let mut end = buf.end_iter();
                if tag.is_empty() {
                    buf.insert(&mut end, &line);
                } else {
                    buf.insert_with_tags_by_name(&mut end, &line, &[tag]);
                }
            }
            body.append(&view);
        }
    });
}
