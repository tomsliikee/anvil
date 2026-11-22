//! The AI chat (Ollama on localhost only). Answers stream in; proposed
//! changes come as patches the user reviews per file and per hunk before
//! anything is written. Nothing runs, nothing is written unseen.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use adw::prelude::*;
use anvil_ai::ollama::{ChatMessage, Ollama};
use anvil_core::diff::{FilePatch, Op};
use gtk::glib;

use crate::i18n::{tr, trf};
use crate::ui::window::{self};

pub struct Chat {
    pub widget: gtk::Box,
    messages: gtk::Box,
    input: gtk::TextView,
    model: gtk::DropDown,
    attach_file: gtk::CheckButton,
    attach_error: gtk::CheckButton,
    history: RefCell<Vec<ChatMessage>>,
    scroller: gtk::ScrolledWindow,
    state: gtk::Label,
}

/// Signatures of the solution's types for the model's context. The open
/// files' texts are taken here; reading and parsing the rest of the
/// solution runs on a worker thread, not in the frame.
pub async fn project_signatures(max_chars: usize) -> String {
    let app = crate::ui::window::app();
    let Some(sol) = app.solution.borrow().clone() else { return String::new() };
    let sources: Vec<(PathBuf, Option<String>)> = sol.projects.iter().flat_map(|p| p.source_files()).map(|f| (f.clone(), app.tabs.code_doc(&f).map(|d| d.text()))).collect();
    gtk::gio::spawn_blocking(move || {
        let files: Vec<anvil_core::csharp::FileStructure> = sources
            .into_iter()
            .map(|(f, text)| {
                let t = text.or_else(|| anvil_core::text::TextFile::read(&f).ok().map(|x| x.text)).unwrap_or_default();
                anvil_core::csharp::parse(&t)
            })
            .collect();
        let refs: Vec<&anvil_core::csharp::FileStructure> = files.iter().collect();
        anvil_ai::context::signatures(&refs, max_chars)
    })
    .await
    .unwrap_or_default()
}

impl Chat {
    pub fn new() -> Chat {
        let model = gtk::DropDown::from_strings(&[]);
        model.set_hexpand(true);
        let state = gtk::Label::builder().xalign(0.0).css_classes(["count"]).build();
        let messages = gtk::Box::new(gtk::Orientation::Vertical, 10);
        messages.set_margin_start(4);
        messages.set_margin_end(4);
        let scroller = gtk::ScrolledWindow::builder().child(&messages).vexpand(true).hscrollbar_policy(gtk::PolicyType::Never).build();
        let input = gtk::TextView::builder().wrap_mode(gtk::WrapMode::WordChar).height_request(72).left_margin(8).top_margin(6).build();
        input.add_css_class("chat-input");
        let attach_file = gtk::CheckButton::with_label(&tr("Current file"));
        attach_file.set_active(true);
        let attach_error = gtk::CheckButton::with_label(&tr("Selected error"));
        let send = gtk::Button::with_label(&tr("Ask"));
        send.add_css_class("solid");
        send.connect_clicked(|_| crate::ui::window::app().chat.send());
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        row.append(&attach_file);
        row.append(&attach_error);
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        row.append(&spacer);
        row.append(&send);
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 8);
        widget.set_margin_start(10);
        widget.set_margin_end(10);
        widget.set_margin_bottom(10);
        widget.append(&model);
        widget.append(&state);
        widget.append(&scroller);
        widget.append(&input);
        widget.append(&row);
        Chat { widget, messages, input, model, attach_file, attach_error, history: RefCell::default(), scroller, state }
    }

    pub fn connect(&self) {
        // Ctrl+Enter sends.
        let keys = gtk::EventControllerKey::new();
        keys.connect_key_pressed(|_, k, _, m| {
            if (k == gtk::gdk::Key::Return || k == gtk::gdk::Key::KP_Enter) && m.contains(gtk::gdk::ModifierType::CONTROL_MASK) {
                crate::ui::window::app().chat.send();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        self.input.add_controller(keys);
        self.model.connect_selected_notify(|dd| {
            if let Some(s) = dd.selected_item().and_downcast::<gtk::StringObject>() {
                let _ = crate::ui::window::app().settings.set_string("ai-chat-model", &s.string());
            }
        });
        let w = self.widget.clone();
        w.connect_map(|_| crate::ui::window::app().chat.load_models());
    }

    pub fn focus(&self) {
        self.input.grab_focus();
    }

    fn load_models(&self) {
        glib::spawn_future_local(async {
            let app = crate::ui::window::app();
            let url = app.settings.string("ollama-url").to_string();
            let Some(o) = Ollama::new(&url) else {
                app.chat.state.set_label(&tr("Only an Ollama on this machine (localhost) can be used."));
                return;
            };
            let models = crate::runtime::run(async move { o.models().await }).await;
            match models {
                Ok(m) => {
                    let names: Vec<String> = m.into_iter().map(|m| m.name).collect();
                    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
                    // Read the stored choice first: setting the model selects
                    // row 0, which the notify handler would store.
                    let want = app.settings.string("ai-chat-model").to_string();
                    app.chat.model.set_model(Some(&gtk::StringList::new(&refs)));
                    let pick = names.iter().position(|n| *n == want).or_else(|| names.iter().position(|n| n.contains("coder"))).unwrap_or(0);
                    app.chat.model.set_selected(pick as u32);
                    app.chat.state.set_label(&if names.is_empty() { tr("Ollama has no models. Pull one with `ollama pull qwen2.5-coder`.") } else { String::new() });
                }
                Err(e) => app.chat.state.set_label(&e.to_string()),
            }
        });
    }

    fn bubble(&self, text: &str, user: bool) -> gtk::Label {
        let l = gtk::Label::builder().label(text).xalign(0.0).wrap(true).selectable(true).css_classes([if user { "chat-user" } else { "chat-assistant" }]).build();
        self.messages.append(&l);
        l
    }

    /// The screenshot walk asks a question as if typed.
    pub fn ask_for_shots(&self, q: &str) {
        self.load_models();
        self.input.buffer().set_text(q);
        glib::timeout_add_local_once(std::time::Duration::from_millis(4000), || crate::ui::window::app().chat.send());
    }

    fn send(&self) {
        let app = crate::ui::window::app();
        if !(app.settings.boolean("ai-enabled") && app.settings.boolean("ai-chat")) {
            window::toast(&tr("The AI chat is switched off."));
            return;
        }
        let buf = self.input.buffer();
        let question = buf.text(&buf.start_iter(), &buf.end_iter(), false).to_string();
        if question.trim().is_empty() {
            return;
        }
        buf.set_text("");
        self.bubble(&question, true);
        let doc = app.tabs.current_code();
        let root = window::solution_dir().unwrap_or_default();
        let file = if self.attach_file.is_active() { doc.as_ref().map(|d| (d.path().strip_prefix(&root).unwrap_or(&d.path()).display().to_string(), d.text())) } else { None };
        let selection = doc.as_ref().map(|d| d.selection_text()).filter(|s| !s.is_empty());
        let error = if self.attach_error.is_active() { app.errors_selected_text() } else { None };
        let content = anvil_ai::context::chat_message(&question, file.as_ref().map(|(p, t)| (p.as_str(), t.as_str())), selection.as_deref(), error.as_deref());
        let earlier: Vec<ChatMessage> = self.history.borrow().clone();
        let signatures = project_signatures(3000);
        self.history.borrow_mut().push(ChatMessage::new("user", question));
        let model = self.model.selected_item().and_downcast::<gtk::StringObject>().map(|s| s.string().to_string()).unwrap_or_default();
        let url = app.settings.string("ollama-url").to_string();
        let answer = self.bubble("…", false);
        glib::spawn_future_local(async move {
            let mut msgs = vec![ChatMessage::new("system", format!("{}\n\nProject types:\n{}", anvil_ai::context::SYSTEM_PROMPT, signatures.await))];
            msgs.extend(earlier);
            msgs.push(ChatMessage::new("user", content));
            let Some(o) = Ollama::new(&url) else { return };
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            let handle = crate::runtime::handle().spawn(async move { o.chat(&model, &msgs, tx).await });
            let mut text = String::new();
            while let Some(piece) = rx.recv().await {
                text.push_str(&piece);
                answer.set_label(&anvil_ai::proposal::prose(&text));
                let adj = crate::ui::window::app().chat.scroller.vadjustment();
                adj.set_value(adj.upper());
            }
            let result = handle.await.unwrap_or_else(|e| Err(anvil_ai::ollama::Error::Server(e.to_string())));
            let app = crate::ui::window::app();
            match result {
                Ok(full) => {
                    answer.set_label(&anvil_ai::proposal::prose(&full));
                    app.chat.history.borrow_mut().push(ChatMessage::new("assistant", full.clone()));
                    let patches = anvil_ai::proposal::patches(&full);
                    if !patches.is_empty() {
                        app.chat.review(patches);
                    }
                }
                Err(e) => answer.set_label(&e.to_string()),
            }
        });
    }

    /// One card per file: each hunk with a checkbox, then Apply.
    fn review(&self, patches: Vec<FilePatch>) {
        let root = window::solution_dir().unwrap_or_default();
        for fp in patches {
            let Some(rel) = fp.path().map(str::to_owned) else { continue };
            let path: PathBuf = if std::path::Path::new(&rel).is_absolute() { rel.clone().into() } else { root.join(&rel) };
            let card = gtk::Box::new(gtk::Orientation::Vertical, 6);
            card.add_css_class("card-panel");
            card.set_margin_top(4);
            let head = gtk::Label::builder().label(&rel).xalign(0.0).css_classes(["strong"]).margin_start(10).margin_top(8).build();
            card.append(&head);
            let checks: Rc<RefCell<Vec<gtk::CheckButton>>> = Rc::default();
            for h in &fp.hunks {
                let check = gtk::CheckButton::with_label(&format!("@@ -{},{} +{},{} @@", h.old_start, h.old_len, h.new_start, h.new_len));
                check.set_active(true);
                check.set_margin_start(8);
                card.append(&check);
                let view = gtk::TextView::builder().editable(false).monospace(true).left_margin(10).build();
                view.add_css_class("diff");
                let b = view.buffer();
                let p = crate::palette::current();
                b.tag_table().add(&gtk::TextTag::builder().name("added").foreground_rgba(&p.up).build());
                b.tag_table().add(&gtk::TextTag::builder().name("removed").foreground_rgba(&p.down).build());
                for (op, t) in &h.lines {
                    let mut end = b.end_iter();
                    match op {
                        Op::Insert => b.insert_with_tags_by_name(&mut end, &format!("+{t}\n"), &["added"]),
                        Op::Delete => b.insert_with_tags_by_name(&mut end, &format!("-{t}\n"), &["removed"]),
                        Op::Equal => b.insert(&mut end, &format!(" {t}\n")),
                    }
                }
                card.append(&view);
                checks.borrow_mut().push(check);
            }
            let apply = gtk::Button::with_label(&tr("Apply"));
            apply.add_css_class("solid");
            let reject = gtk::Button::with_label(&tr("Reject"));
            reject.add_css_class("quiet");
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            row.set_margin_start(8);
            row.set_margin_bottom(8);
            row.append(&apply);
            row.append(&reject);
            card.append(&row);
            let c2 = card.clone();
            reject.connect_clicked(move |_| c2.set_sensitive(false));
            let c3 = card.clone();
            apply.connect_clicked(move |_| {
                let chosen: Vec<anvil_core::diff::Hunk> = fp.hunks.iter().zip(checks.borrow().iter()).filter(|(_, c)| c.is_active()).map(|(h, _)| h.clone()).collect();
                match apply_hunks(&path, &chosen) {
                    Ok(()) => c3.set_sensitive(false),
                    Err(e) => window::show_error(&tr("The change could not be applied"), &e),
                }
            });
            self.messages.append(&card);
        }
    }
}

/// Accepted hunks become one undo step in the file's buffer (opened if
/// needed); a new file is created as VS would (BOM, CRLF).
fn apply_hunks(path: &std::path::Path, hunks: &[anvil_core::diff::Hunk]) -> Result<(), String> {
    let app = crate::ui::window::app();
    if !path.exists() {
        let text = anvil_core::diff::apply_hunks("", hunks).map_err(|e| e.to_string())?;
        anvil_core::text::TextFile::vs_new(&text).write(path).map_err(|e| e.to_string())?;
        app.explorer.reload();
        app.tabs.open(path, false);
        return Ok(());
    }
    app.tabs.open(path, false);
    let doc = app.tabs.code_doc(path).ok_or_else(|| tr("The file is not a text file."))?;
    let old = doc.text();
    let new = anvil_core::diff::apply_hunks(&old, hunks).map_err(|e| trf("{} (the file changed since the answer)", &[&e.to_string()]))?;
    doc.apply_edits(&[anvil_core::edit::TextEdit::new(0, old.len(), new)]);
    Ok(())
}

impl crate::ui::window::App {
    /// The Error List's selected entry as text, for "ask about this error".
    pub fn errors_selected_text(&self) -> Option<String> {
        self.errors.selected_text()
    }
}
