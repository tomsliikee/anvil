//! Roslyn in the editor: starting the server, keeping documents in sync,
//! diagnostics, IntelliSense, Quick Info, signature help, navigation,
//! rename, quick actions and formatting. Every request carries the
//! document version it was made for; an answer for an older version is
//! dropped, so typing never waits and never jumps.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::rc::Rc;

use adw::prelude::*;
use anvil_lsp::client::{Client, Incoming};
use anvil_lsp::position::{to_offset, to_position};
use anvil_lsp::roslyn;
use gtk::{gio, glib};
use lsp_types as lt;
use serde_json::{json, Value};
use sourceview5::prelude::*;
use sourceview5::subclass::prelude::*;

use crate::host;
use crate::i18n::{tr, trf};
use crate::ui::editor::CodeDoc;

struct Running {
    /// Which start this is: messages of an older server are dropped.
    id: u64,
    client: Client,
    kill: Option<tokio::sync::oneshot::Sender<()>>,
    ready: bool,
    /// Pull-diagnostic identifiers the server registered.
    identifiers: Vec<String>,
}

#[derive(Default)]
pub struct Lsp {
    running: RefCell<Option<Running>>,
    opened: RefCell<HashMap<PathBuf, i32>>,
    /// The text the server holds for each open file, to send ranged changes.
    sent: RefCell<HashMap<PathBuf, String>>,
    /// Diagnostics per file and source identifier.
    diags: RefCell<HashMap<PathBuf, BTreeMap<String, Vec<lt::Diagnostic>>>>,
    starting: Cell<bool>,
    next_id: Cell<u64>,
    /// Automatic restarts after the server ended by itself.
    restarts: Cell<u32>,
    /// Files changed since the last didChange: sent once per idle pass, so
    /// a burst of keystrokes sends the text once, and before any request.
    pending: RefCell<BTreeSet<PathBuf>>,
    flush_source: RefCell<Option<glib::SourceId>>,
}

fn uri_string(p: &Path) -> String {
    roslyn::file_uri(p)
}

impl Lsp {
    pub fn new() -> Lsp {
        Lsp::default()
    }

    /// The ready server, with every pending change sent first, so a
    /// request never sees older text than the buffer.
    pub fn client(&self) -> Option<Client> {
        self.flush_changes();
        self.raw_client()
    }

    fn raw_client(&self) -> Option<Client> {
        self.running.borrow().as_ref().filter(|r| r.ready).map(|r| r.client.clone())
    }

    fn flush_changes(&self) {
        if let Some(id) = self.flush_source.borrow_mut().take() {
            id.remove();
        }
        let paths = std::mem::take(&mut *self.pending.borrow_mut());
        if paths.is_empty() {
            return;
        }
        let Some(c) = self.raw_client() else { return };
        let app = crate::ui::window::app();
        for path in paths {
            let Some(doc) = app.tabs.code_doc(&path) else { continue };
            let text = doc.text();
            let Some(old) = self.sent.borrow_mut().insert(path.clone(), text.clone()) else { continue };
            if old == text {
                continue;
            }
            self.opened.borrow_mut().insert(path.clone(), doc.version.get());
            let change = anvil_lsp::position::change_event(&old, &text);
            c.notify_raw("textDocument/didChange", json!({"textDocument": {"uri": uri_string(&path), "version": doc.version.get()}, "contentChanges": [change]}));
            schedule_diagnostics(&path);
        }
    }

    /// Files changed on disk (`workspace/didChangeWatchedFiles`), so
    /// Roslyn sees new classes, packages and branch switches.
    pub fn files_changed(&self, changes: &[(PathBuf, crate::ui::watch::Change)]) {
        let Some(c) = self.raw_client() else { return };
        let list: Vec<Value> = changes.iter().map(|(p, kind)| json!({"uri": uri_string(p), "type": *kind as u8})).collect();
        if !list.is_empty() {
            c.notify_raw("workspace/didChangeWatchedFiles", json!({"changes": list}));
        }
    }

    /// A new solution gets its automatic restart again.
    pub fn reset_restarts(&self) {
        self.restarts.set(0);
    }

    /// Tools > Restart Language Server.
    pub fn restart(&self) {
        self.stop();
        self.restarts.set(0);
        glib::timeout_add_local_once(std::time::Duration::from_millis(1000), || crate::ui::window::app().lsp.start_for_solution());
    }

    pub fn stop(&self) {
        let running = self.running.borrow_mut().take();
        if let Some(mut r) = running {
            let c = r.client.clone();
            glib::spawn_future_local(async move {
                let _ = c.request_raw("shutdown", Value::Null).await;
                c.notify_raw("exit", Value::Null);
            });
            if let Some(k) = r.kill.take() {
                glib::timeout_add_local_once(std::time::Duration::from_millis(800), move || {
                    let _ = k.send(());
                });
            }
        }
        self.opened.borrow_mut().clear();
        self.sent.borrow_mut().clear();
        self.diags.borrow_mut().clear();
        crate::ui::window::app().status.set_server("");
    }

    /// Starts Roslyn for the open solution, downloading it first (with the
    /// user's consent) when it is missing.
    pub fn start_for_solution(&self) {
        if self.starting.get() || self.running.borrow().is_some() {
            return;
        }
        let app = crate::ui::window::app();
        if !roslyn::is_installed(&host::home(), roslyn::VERSION) {
            if app.settings.boolean("roslyn-consent") {
                download_roslyn();
            } else if crate::shots::requested().is_none() {
                ask_roslyn_consent();
            }
            return;
        }
        self.starting.set(true);
        app.status.set_server(&tr("Language server starting…"));
        glib::spawn_future_local(start());
    }

    pub fn did_open(&self, doc: &CodeDoc) {
        if !doc.is_csharp() {
            return;
        }
        let Some(c) = self.client() else { return };
        let path = doc.path();
        self.opened.borrow_mut().insert(path.clone(), doc.version.get());
        let text = doc.text();
        c.notify_raw("textDocument/didOpen", json!({"textDocument": {"uri": uri_string(&path), "languageId": "csharp", "version": doc.version.get(), "text": text}}));
        self.sent.borrow_mut().insert(path.clone(), text);
        schedule_diagnostics(&path);
    }

    pub fn did_change(&self, doc: &CodeDoc) {
        if !doc.is_csharp() {
            return;
        }
        if self.raw_client().is_none() {
            return;
        }
        let path = doc.path();
        if !self.opened.borrow().contains_key(&path) {
            self.did_open(doc);
            return;
        }
        self.pending.borrow_mut().insert(path);
        if self.flush_source.borrow().is_none() {
            let id = glib::idle_add_local_once(|| {
                let app = crate::ui::window::app();
                app.lsp.flush_source.borrow_mut().take();
                app.lsp.flush_changes();
            });
            *self.flush_source.borrow_mut() = Some(id);
        }
    }

    pub fn did_save(&self, doc: &CodeDoc) {
        if let Some(c) = self.client()
            && doc.is_csharp()
        {
            c.notify_raw("textDocument/didSave", json!({"textDocument": {"uri": uri_string(&doc.path())}}));
        }
    }

    pub fn did_close(&self, path: &Path) {
        self.sent.borrow_mut().remove(path);
        // Its IntelliSense entries leave the Error List with it.
        if self.diags.borrow_mut().remove(path).is_some() {
            crate::ui::window::app().errors.set_intellisense(self.all_diagnostics());
        }
        if self.opened.borrow_mut().remove(path).is_some()
            && let Some(c) = self.client()
        {
            c.notify_raw("textDocument/didClose", json!({"textDocument": {"uri": uri_string(path)}}));
        }
    }

    /// Every IntelliSense diagnostic, for the Error List.
    pub fn all_diagnostics(&self) -> Vec<(PathBuf, lt::Diagnostic)> {
        self.diags.borrow().iter().flat_map(|(p, m)| m.values().flatten().map(move |d| (p.clone(), d.clone()))).collect()
    }
}

fn ask_roslyn_consent() {
    let app = crate::ui::window::app();
    let d = adw::AlertDialog::new(
        Some(&tr("Download the C# language server?")),
        Some(&trf("IntelliSense, squiggles, rename and quick actions come from Microsoft's Roslyn language server ({}, MIT licence, about 60 MB). Anvil downloads it once from Microsoft's package feed into ~/.local/share/anvil/roslyn.", &[roslyn::VERSION])),
    );
    d.add_responses(&[("later", &tr("Not Now")), ("download", &tr("Download"))]);
    d.set_default_response(Some("download"));
    d.choose(Some(&app.window), gio::Cancellable::NONE, |r| {
        if r == "download" {
            let _ = crate::ui::window::app().settings.set_boolean("roslyn-consent", true);
            download_roslyn();
        }
    });
}

fn download_roslyn() {
    let app = crate::ui::window::app();
    app.status.set_server(&tr("Downloading the language server…"));
    let home = host::home();
    glib::spawn_future_local(async move {
        let result = crate::runtime::run(async move {
            let http = reqwest::Client::new();
            let index = http.get(roslyn::FEED_INDEX).send().await.map_err(|e| e.to_string())?.text().await.map_err(|e| e.to_string())?;
            let base = roslyn::base_address(&index).ok_or("the feed has no package address")?;
            let bytes = http.get(roslyn::package_url(&base, roslyn::VERSION)).send().await.map_err(|e| e.to_string())?.error_for_status().map_err(|e| e.to_string())?.bytes().await.map_err(|e| e.to_string())?;
            let dir = roslyn::install_dir(&home, roslyn::VERSION);
            let tmp = dir.with_extension("partial");
            let _ = std::fs::remove_dir_all(&tmp);
            anvil_core::handin::extract_prefix(&bytes, roslyn::CONTENT_DIR, &tmp).map_err(|e| e.to_string())?;
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::rename(&tmp, &dir).map_err(|e| e.to_string())?;
            Ok::<(), String>(())
        })
        .await;
        let app = crate::ui::window::app();
        match result {
            Ok(()) => app.lsp.start_for_solution(),
            Err(e) => {
                app.status.set_server(&tr("Language server not available"));
                app.output.append("Language Server", &format!("{}: {e}\n", tr("Download failed")));
            }
        }
    });
}

async fn start() {
    let app = crate::ui::window::app();
    let Some(sol) = app.solution.borrow().clone() else {
        app.lsp.starting.set(false);
        return;
    };
    let home = host::home();
    let dir = roslyn::install_dir(&home, roslyn::VERSION);
    let logs = crate::config_dir().join("logs");
    let _ = std::fs::create_dir_all(&logs);
    let cmd = anvil_core::host::Command::dotnet().args(roslyn::args(&dir, &logs)).cwd(&sol.dir);
    let (kill_tx, kill_rx) = tokio::sync::oneshot::channel::<()>();
    let started = crate::runtime::run(async move {
        let mut c = host::tokio_command(&cmd);
        c.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = c.spawn().map_err(|e| e.to_string())?;
        let stdin = child.stdin.take().ok_or("no stdin")?;
        let stdout = child.stdout.take().ok_or("no stdout")?;
        let stderr = child.stderr.take();
        let (client, rx) = Client::start(stdout, stdin);
        tokio::spawn(async move {
            tokio::select! {
                _ = child.wait() => {}
                _ = kill_rx => { let _ = child.kill().await; }
            }
        });
        Ok::<_, String>((client, rx, stderr))
    })
    .await;
    let (client, mut rx, stderr) = match started {
        Ok(x) => x,
        Err(e) => {
            app.lsp.starting.set(false);
            app.status.set_server(&tr("Language server not available"));
            app.output.append("Language Server", &format!("{e}\n"));
            return;
        }
    };
    if let Some(err) = stderr {
        let (tx, mut erx) = tokio::sync::mpsc::unbounded_channel::<String>();
        crate::runtime::handle().spawn(async move {
            use tokio::io::AsyncBufReadExt;
            let mut lines = tokio::io::BufReader::new(err).lines();
            while let Ok(Some(l)) = lines.next_line().await {
                let _ = tx.send(l);
            }
        });
        glib::spawn_future_local(async move {
            while let Some(l) = erx.recv().await {
                crate::ui::window::app().output.append("Language Server", &format!("{l}\n"));
            }
        });
    }
    let id = app.lsp.next_id.get() + 1;
    app.lsp.next_id.set(id);
    *app.lsp.running.borrow_mut() = Some(Running { id, client: client.clone(), kill: Some(kill_tx), ready: false, identifiers: Vec::new() });
    glib::spawn_future_local(async move {
        while let Some(msg) = rx.recv().await {
            let current = crate::ui::window::app().lsp.running.borrow().as_ref().is_some_and(|r| r.id == id);
            if !current {
                // A stopped server's last words (its exit after a solution
                // switch) must not touch the new one.
                continue;
            }
            handle_incoming(msg);
        }
    });
    let params = json!({
        "processId": Value::Null,
        "clientInfo": {"name": "Anvil", "version": env!("CARGO_PKG_VERSION")},
        "locale": "en",
        "rootUri": uri_string(&sol.dir),
        "workspaceFolders": [{"uri": uri_string(&sol.dir), "name": sol.name}],
        "capabilities": roslyn::client_capabilities(),
    });
    match client.request_raw("initialize", params).await {
        Ok(_) => {
            client.notify_raw("initialized", json!({}));
            let open = if sol.format == anvil_core::solution::Format::Slnx || sol.format == anvil_core::solution::Format::Sln {
                (roslyn::OPEN_SOLUTION, roslyn::open_solution_params(&sol.path))
            } else {
                (roslyn::OPEN_PROJECTS, roslyn::open_projects_params(&sol.projects.iter().map(|p| p.path.clone()).collect::<Vec<_>>()))
            };
            client.notify_raw(open.0, open.1);
            if let Some(r) = app.lsp.running.borrow_mut().as_mut() {
                r.ready = true;
            }
            app.status.set_server(&tr("Loading projects…"));
            for d in app.tabs.code_docs() {
                app.lsp.did_open(&d);
            }
        }
        Err(e) => {
            app.status.set_server(&tr("Language server not available"));
            app.output.append("Language Server", &format!("initialize: {e}\n"));
        }
    }
    app.lsp.starting.set(false);
}

fn handle_incoming(msg: Incoming) {
    let app = crate::ui::window::app();
    match msg {
        Incoming::Notification { method, params } => match method.as_str() {
            roslyn::PROJECTS_LOADED => {
                app.status.set_server(&tr("Ready"));
                for d in app.tabs.code_docs() {
                    schedule_diagnostics(&d.path());
                }
            }
            "textDocument/publishDiagnostics" => {
                let Ok(p) = serde_json::from_value::<lt::PublishDiagnosticsParams>(params) else { return };
                if let Some(path) = roslyn::path_from_uri(p.uri.as_str()) {
                    set_diagnostics(&path, "push", p.diagnostics);
                }
            }
            "window/logMessage" | "window/showMessage" => {
                if let Some(m) = params.get("message").and_then(Value::as_str) {
                    app.output.append("Language Server", &format!("{m}\n"));
                }
            }
            "client/registerCapability" => {
                if let Some(regs) = params.get("registrations").and_then(Value::as_array) {
                    for r in regs {
                        if r.get("method").and_then(Value::as_str) == Some("textDocument/diagnostic")
                            && let Some(id) = r.pointer("/registerOptions/identifier").and_then(Value::as_str)
                            && let Some(run) = app.lsp.running.borrow_mut().as_mut()
                            && !run.identifiers.iter().any(|x| x == id)
                        {
                            run.identifiers.push(id.to_owned());
                        }
                    }
                }
            }
            _ => {}
        },
        Incoming::Request { id, method, params } => {
            let Some(client) = app.lsp.running.borrow().as_ref().map(|r| r.client.clone()) else { return };
            match method.as_str() {
                "workspace/applyEdit" => {
                    let applied = params.get("edit").and_then(|e| serde_json::from_value::<lt::WorkspaceEdit>(e.clone()).ok()).map(|e| apply_workspace_edit(&e)).unwrap_or(false);
                    client.respond(id, json!({"applied": applied}));
                }
                "workspace/diagnostic/refresh" | "workspace/semanticTokens/refresh" | "workspace/codeLens/refresh" | "workspace/inlayHint/refresh" => {
                    client.respond(id, Value::Null);
                    for d in app.tabs.code_docs() {
                        schedule_diagnostics(&d.path());
                    }
                }
                // A project without obj/ (a freshly unzipped assignment):
                // restore it as VS does, then answer.
                "workspace/_roslyn_projectNeedsRestore" => {
                    let paths: Vec<String> = params.get("projectFilePaths").and_then(Value::as_array).map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect()).unwrap_or_default();
                    let client = client.clone();
                    glib::spawn_future_local(async move {
                        let app = crate::ui::window::app();
                        app.status.set_server(&tr("Restoring packages…"));
                        for p in paths {
                            let cmd = anvil_core::host::Command::dotnet().args(["restore", p.as_str()]);
                            match host::run(cmd, None).await {
                                Ok(o) => app.output.append("Language Server", &format!("dotnet restore {p}\n{}{}", o.stdout_text(), o.stderr_text())),
                                Err(e) => app.output.append("Language Server", &format!("dotnet restore {p}: {e}\n")),
                            }
                        }
                        app.status.set_server(&tr("Loading projects…"));
                        client.respond(id, Value::Null);
                    });
                }
                _ => client.respond_error(id, -32601, "not supported by Anvil"),
            }
        }
        Incoming::Exited => {
            *app.lsp.running.borrow_mut() = None;
            app.lsp.opened.borrow_mut().clear();
            app.lsp.sent.borrow_mut().clear();
            // It ended by itself (a crash): one automatic restart, then
            // Tools > Restart Language Server.
            if app.lsp.restarts.get() < 1 && app.solution.borrow().is_some() {
                app.lsp.restarts.set(app.lsp.restarts.get() + 1);
                app.status.set_server(&tr("Language server stopped; restarting…"));
                app.output.append("Language Server", "The language server stopped; restarting it.\n");
                app.lsp.start_for_solution();
            } else {
                app.status.set_server(&tr("Language server stopped"));
                crate::ui::window::toast(&tr("The language server stopped. Tools > Restart Language Server starts it again."));
            }
        }
    }
}

fn set_diagnostics(path: &Path, source: &str, list: Vec<lt::Diagnostic>) {
    let app = crate::ui::window::app();
    // The same list again (the usual pull after a keystroke) changes nothing.
    if app.lsp.diags.borrow().get(path).and_then(|m| m.get(source)) == Some(&list) {
        return;
    }
    app.lsp.diags.borrow_mut().entry(path.to_owned()).or_default().insert(source.to_owned(), list);
    if let Some(doc) = app.tabs.code_doc(path) {
        apply_squiggles(&doc);
    }
    app.errors.set_intellisense(app.lsp.all_diagnostics());
}

fn apply_squiggles(doc: &CodeDoc) {
    let app = crate::ui::window::app();
    let all: Vec<lt::Diagnostic> = app.lsp.diags.borrow().get(&doc.path()).map(|m| m.values().flatten().cloned().collect()).unwrap_or_default();
    let (s, e) = doc.buffer.bounds();
    for t in ["diag-error", "diag-warning", "diag-info"] {
        doc.buffer.remove_tag_by_name(t, &s, &e);
    }
    let text = doc.text();
    let at = doc.byte_iters(&text);
    for d in &all {
        let tag = match d.severity {
            Some(lt::DiagnosticSeverity::ERROR) => "diag-error",
            Some(lt::DiagnosticSeverity::WARNING) => "diag-warning",
            Some(lt::DiagnosticSeverity::HINT) => continue,
            _ => "diag-info",
        };
        let (a, mut b) = (to_offset(&text, d.range.start), to_offset(&text, d.range.end));
        if a == b {
            // Zero width: one whole character, never half an umlaut.
            b = anvil_core::text::next_char_end(&text, a);
        }
        doc.buffer.apply_tag_by_name(tag, &at(a), &at(b));
    }
    *doc.diagnostics.borrow_mut() = all;
}

thread_local! {
    static DIAG_TIMERS: RefCell<HashMap<PathBuf, glib::SourceId>> = RefCell::default();
}

/// Pull diagnostics 400 ms after the last change.
fn schedule_diagnostics(path: &Path) {
    let p = path.to_owned();
    DIAG_TIMERS.with(|t| {
        if let Some(id) = t.borrow_mut().remove(&p) {
            id.remove();
        }
        let key = p.clone();
        let id = glib::timeout_add_local_once(std::time::Duration::from_millis(400), move || {
            DIAG_TIMERS.with(|t| t.borrow_mut().remove(&key));
            glib::spawn_future_local(pull_diagnostics(key));
        });
        t.borrow_mut().insert(p, id);
    });
}

async fn pull_diagnostics(path: PathBuf) {
    let app = crate::ui::window::app();
    let Some(client) = app.lsp.client() else { return };
    let Some(doc) = app.tabs.code_doc(&path) else { return };
    let version = doc.version.get();
    let ids: Vec<Option<String>> = {
        let r = app.lsp.running.borrow();
        let ids = r.as_ref().map(|r| r.identifiers.clone()).unwrap_or_default();
        if ids.is_empty() { vec![None] } else { ids.into_iter().map(Some).collect() }
    };
    for id in ids {
        let mut params = json!({"textDocument": {"uri": uri_string(&path)}});
        if let Some(i) = &id {
            params["identifier"] = json!(i);
        }
        let Ok(reply) = client.request_raw("textDocument/diagnostic", params).await else { continue };
        if doc.version.get() != version {
            return;
        }
        if reply.get("kind").and_then(Value::as_str) == Some("unchanged") {
            continue;
        }
        let items: Vec<lt::Diagnostic> = reply.get("items").and_then(|i| serde_json::from_value(i.clone()).ok()).unwrap_or_default();
        set_diagnostics(&path, id.as_deref().unwrap_or("pull"), items);
    }
}

// ---- Edits ------------------------------------------------------------------

fn to_core_edits(text: &str, edits: &[lt::TextEdit]) -> Vec<anvil_core::edit::TextEdit> {
    edits.iter().map(|e| anvil_core::edit::TextEdit::new(to_offset(text, e.range.start), to_offset(text, e.range.end), e.new_text.replace("\r\n", "\n"))).collect()
}

/// Applies edits to a file: in its open buffer as one undo step, or on
/// disk keeping BOM and line endings.
pub fn apply_file_edits(path: &Path, edits: &[lt::TextEdit]) -> bool {
    let app = crate::ui::window::app();
    if let Some(doc) = app.tabs.code_doc(path) {
        let text = doc.text();
        doc.apply_edits(&to_core_edits(&text, edits));
        return true;
    }
    let Ok(mut file) = anvil_core::text::TextFile::read(path) else { return false };
    let core = to_core_edits(&file.text, edits);
    file.text = anvil_core::edit::apply(&file.text, &core);
    file.write(path).is_ok()
}

pub fn apply_workspace_edit(edit: &lt::WorkspaceEdit) -> bool {
    let mut ok = true;
    if let Some(changes) = &edit.changes {
        for (u, edits) in changes {
            if let Some(p) = roslyn::path_from_uri(u.as_str()) {
                ok &= apply_file_edits(&p, edits);
            }
        }
    }
    if let Some(dc) = &edit.document_changes {
        let ops: Vec<lt::DocumentChangeOperation> = match dc {
            lt::DocumentChanges::Edits(e) => e.iter().cloned().map(lt::DocumentChangeOperation::Edit).collect(),
            lt::DocumentChanges::Operations(o) => o.clone(),
        };
        for op in ops {
            match op {
                lt::DocumentChangeOperation::Edit(e) => {
                    if let Some(p) = roslyn::path_from_uri(e.text_document.uri.as_str()) {
                        let edits: Vec<lt::TextEdit> = e
                            .edits
                            .into_iter()
                            .map(|x| match x {
                                lt::OneOf::Left(t) => t,
                                lt::OneOf::Right(a) => a.text_edit,
                            })
                            .collect();
                        ok &= apply_file_edits(&p, &edits);
                    }
                }
                lt::DocumentChangeOperation::Op(lt::ResourceOp::Create(c)) => {
                    if let Some(p) = roslyn::path_from_uri(c.uri.as_str()) {
                        ok &= anvil_core::text::TextFile::vs_new("").write(&p).is_ok();
                    }
                }
                lt::DocumentChangeOperation::Op(lt::ResourceOp::Rename(r)) => {
                    if let (Some(a), Some(b)) = (roslyn::path_from_uri(r.old_uri.as_str()), roslyn::path_from_uri(r.new_uri.as_str())) {
                        ok &= std::fs::rename(&a, &b).is_ok();
                        if let Some((_, d)) = crate::ui::window::app().tabs.find(&a)
                            && let crate::ui::editor::Doc::Code(c) = &*d
                        {
                            *c.path.borrow_mut() = b.clone();
                            crate::ui::window::app().tabs.refresh_titles();
                        }
                    }
                }
                lt::DocumentChangeOperation::Op(lt::ResourceOp::Delete(d)) => {
                    if let Some(p) = roslyn::path_from_uri(d.uri.as_str()) {
                        ok &= gio::File::for_path(&p).trash(gio::Cancellable::NONE).is_ok();
                    }
                }
            }
        }
    }
    crate::ui::window::app().explorer.reload();
    ok
}

fn position_params(doc: &CodeDoc) -> Value {
    let text = doc.text();
    let pos = to_position(&text, doc.cursor_byte());
    json!({"textDocument": {"uri": uri_string(&doc.path())}, "position": pos})
}

fn need_server() -> Option<(Client, Rc<CodeDoc>)> {
    let app = crate::ui::window::app();
    let doc = app.tabs.current_code()?;
    match app.lsp.client() {
        Some(c) if doc.is_csharp() => Some((c, doc)),
        _ => {
            app.status.flash(&tr("The language server is not ready yet."));
            None
        }
    }
}

fn locations(v: Value) -> Vec<lt::Location> {
    if v.is_null() {
        return Vec::new();
    }
    if let Ok(one) = serde_json::from_value::<lt::Location>(v.clone()) {
        return vec![one];
    }
    if let Ok(many) = serde_json::from_value::<Vec<lt::Location>>(v.clone()) {
        return many;
    }
    serde_json::from_value::<Vec<lt::LocationLink>>(v)
        .map(|links| links.into_iter().map(|l| lt::Location { uri: l.target_uri, range: l.target_selection_range }).collect())
        .unwrap_or_default()
}

fn goto_location(l: &lt::Location) {
    if let Some(p) = roslyn::path_from_uri(l.uri.as_str()) {
        crate::ui::window::app().tabs.goto(&p, l.range.start.line + 1, l.range.start.character + 1);
    }
}

fn hits(locs: &[lt::Location]) -> Vec<crate::ui::find::Hit> {
    locs.iter()
        .filter_map(|l| {
            let p = roslyn::path_from_uri(l.uri.as_str())?;
            let text = crate::ui::window::app().tabs.code_doc(&p).map(|d| d.text()).or_else(|| anvil_core::text::TextFile::read(&p).ok().map(|f| f.text)).unwrap_or_default();
            let line = text.lines().nth(l.range.start.line as usize).unwrap_or("").to_owned();
            Some(crate::ui::find::Hit { path: p, line: l.range.start.line + 1, col: l.range.start.character + 1, text: line })
        })
        .collect()
}

/// F12, Ctrl+F12 (`textDocument/definition`, `…/implementation`).
pub fn goto(method: &'static str) {
    let Some((client, doc)) = need_server() else { return };
    let params = position_params(&doc);
    glib::spawn_future_local(async move {
        match client.request_raw(method, params).await.map(locations) {
            Ok(l) if l.len() == 1 => goto_location(&l[0]),
            Ok(l) if !l.is_empty() => crate::ui::window::app().find_results.show(&tr("Go To Implementation"), &hits(&l)),
            Ok(_) => crate::ui::window::app().status.flash(&tr("Cannot navigate to the symbol under the caret.")),
            Err(e) => crate::ui::window::app().status.flash(&e.to_string()),
        }
    });
}

/// Shift+F12.
pub fn find_references() {
    let Some((client, doc)) = need_server() else { return };
    let mut params = position_params(&doc);
    params["context"] = json!({"includeDeclaration": true});
    glib::spawn_future_local(async move {
        if let Ok(v) = client.request_raw("textDocument/references", params).await {
            let l = locations(v);
            crate::ui::window::app().find_results.show(&tr("Find All References"), &hits(&l));
        }
    });
}

/// Alt+F12: the definition in a read-only view under the caret.
pub fn peek_definition() {
    let Some((client, doc)) = need_server() else { return };
    let params = position_params(&doc);
    glib::spawn_future_local(async move {
        let Ok(v) = client.request_raw("textDocument/definition", params).await else { return };
        let Some(l) = locations(v).into_iter().next() else { return };
        let Some(path) = roslyn::path_from_uri(l.uri.as_str()) else { return };
        let text = crate::ui::window::app().tabs.code_doc(&path).map(|d| d.text()).or_else(|| anvil_core::text::TextFile::read(&path).ok().map(|f| f.text)).unwrap_or_default();
        let buffer = sourceview5::Buffer::new(None);
        buffer.set_language(sourceview5::LanguageManager::default().language("anvil-csharp").as_ref());
        buffer.set_style_scheme(sourceview5::StyleSchemeManager::default().scheme(if adw::StyleManager::default().is_dark() { "anvil-dark" } else { "anvil-light" }).as_ref());
        buffer.set_text(&text);
        let view = sourceview5::View::with_buffer(&buffer);
        view.set_editable(false);
        view.set_monospace(true);
        view.set_show_line_numbers(true);
        view.add_css_class("editor");
        let scroller = gtk::ScrolledWindow::builder().child(&view).min_content_height(260).min_content_width(640).build();
        let title = gtk::Label::builder().label(path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()).xalign(0.0).css_classes(["panel-title"]).build();
        let b = gtk::Box::new(gtk::Orientation::Vertical, 6);
        b.append(&title);
        b.append(&scroller);
        let pop = gtk::Popover::new();
        pop.set_child(Some(&b));
        pop.set_parent(&doc.view);
        let rect = doc.view.iter_location(&doc.buffer.iter_at_mark(&doc.buffer.get_insert()));
        let (x, y) = doc.view.buffer_to_window_coords(gtk::TextWindowType::Widget, rect.x(), rect.y() + rect.height());
        pop.set_pointing_to(Some(&gtk::gdk::Rectangle::new(x, y, 1, 1)));
        pop.set_position(gtk::PositionType::Bottom);
        pop.popup();
        if let Some(mut it) = buffer.iter_at_line(l.range.start.line as i32) {
            it.forward_chars(l.range.start.character as i32);
            buffer.place_cursor(&it);
            glib::idle_add_local_once(move || {
                view.scroll_to_mark(&buffer.get_insert(), 0.0, true, 0.0, 0.3);
            });
        }
        pop.connect_closed(|p| p.unparent());
    });
}

/// Ctrl+R, Ctrl+R / F2.
pub fn rename() {
    let Some((client, doc)) = need_server() else { return };
    let params = position_params(&doc);
    let app = crate::ui::window::app();
    let current = word_at_cursor(&doc);
    let dialog = adw::AlertDialog::new(Some(&tr("Rename")), Some(&trf("New name for {}:", &[&current])));
    let entry = gtk::Entry::builder().text(current.as_str()).activates_default(true).build();
    dialog.set_extra_child(Some(&entry));
    dialog.add_responses(&[("cancel", &tr("Cancel")), ("apply", &tr("Apply"))]);
    dialog.set_default_response(Some("apply"));
    dialog.choose(Some(&app.window), gio::Cancellable::NONE, move |r| {
        if r != "apply" {
            return;
        }
        let mut params = params.clone();
        params["newName"] = json!(entry.text().to_string());
        let client = client.clone();
        glib::spawn_future_local(async move {
            match client.request_raw("textDocument/rename", params).await {
                Ok(v) => {
                    if let Ok(edit) = serde_json::from_value::<lt::WorkspaceEdit>(v) {
                        apply_workspace_edit(&edit);
                    }
                }
                Err(e) => crate::ui::window::show_error(&tr("Rename"), &e.to_string()),
            }
        });
    });
}

fn word_at_cursor(doc: &CodeDoc) -> String {
    let text = doc.text();
    let at = doc.cursor_byte();
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let start = text[..at].rfind(|c: char| !is_word(c)).map_or(0, |i| i + 1);
    let end = text[at..].find(|c: char| !is_word(c)).map_or(text.len(), |i| at + i);
    text[start..end].to_owned()
}

/// Ctrl+. : code actions at the caret (with the diagnostics there), as a
/// list under the caret; the chosen one is resolved and applied.
pub fn quick_actions() {
    let Some((client, doc)) = need_server() else { return };
    let text = doc.text();
    let at = doc.cursor_byte();
    let (start, end) = doc.buffer.selection_bounds().map(|(a, b)| (text[..].char_indices().nth(a.offset() as usize).map_or(text.len(), |(i, _)| i), text.char_indices().nth(b.offset() as usize).map_or(text.len(), |(i, _)| i))).unwrap_or((at, at));
    let range = lt::Range { start: to_position(&text, start), end: to_position(&text, end) };
    let line = range.start.line;
    let diags: Vec<lt::Diagnostic> = doc.diagnostics.borrow().iter().filter(|d| d.range.start.line <= line && d.range.end.line >= line).cloned().collect();
    let params = json!({"textDocument": {"uri": uri_string(&doc.path())}, "range": range, "context": {"diagnostics": diags}});
    glib::spawn_future_local(async move {
        let Ok(v) = client.request_raw("textDocument/codeAction", params).await else { return };
        let actions: Vec<Value> = v.as_array().cloned().unwrap_or_default();
        if actions.is_empty() {
            crate::ui::window::app().status.flash(&tr("No quick actions here."));
            return;
        }
        let items: Vec<(String, Value)> = actions.into_iter().map(|a| (a.get("title").and_then(Value::as_str).unwrap_or("?").to_owned(), a)).collect();
        let doc2 = doc.clone();
        crate::ui::menus::choose_at_caret(&doc2, &items.iter().map(|(t, _)| t.clone()).collect::<Vec<_>>(), move |i| {
            let (_, action) = items[i].clone();
            let client = client.clone();
            glib::spawn_future_local(async move {
                let action = if action.get("edit").is_none() && action.get("data").is_some() { client.request_raw("codeAction/resolve", action.clone()).await.unwrap_or(action) } else { action };
                if let Some(edit) = action.get("edit").and_then(|e| serde_json::from_value::<lt::WorkspaceEdit>(e.clone()).ok()) {
                    apply_workspace_edit(&edit);
                }
                if let Some(cmd) = action.get("command") {
                    let _ = client.request_raw("workspace/executeCommand", json!({"command": cmd.get("command"), "arguments": cmd.get("arguments")})).await;
                }
            });
        });
    });
}

/// Ctrl+K, Ctrl+D and Ctrl+K, Ctrl+F.
pub fn format(selection_only: bool) {
    let Some((client, doc)) = need_server() else { return };
    let text = doc.text();
    let version = doc.version.get();
    let opts = json!({"tabSize": 4, "insertSpaces": doc.view.is_insert_spaces_instead_of_tabs()});
    let (method, params) = match doc.buffer.selection_bounds().filter(|_| selection_only) {
        Some((a, b)) => {
            let sa = text.char_indices().nth(a.offset() as usize).map_or(text.len(), |(i, _)| i);
            let sb = text.char_indices().nth(b.offset() as usize).map_or(text.len(), |(i, _)| i);
            ("textDocument/rangeFormatting", json!({"textDocument": {"uri": uri_string(&doc.path())}, "range": {"start": to_position(&text, sa), "end": to_position(&text, sb)}, "options": opts}))
        }
        None => ("textDocument/formatting", json!({"textDocument": {"uri": uri_string(&doc.path())}, "options": opts})),
    };
    glib::spawn_future_local(async move {
        let Ok(v) = client.request_raw(method, params).await else { return };
        if doc.version.get() != version {
            return;
        }
        let edits: Vec<lt::TextEdit> = serde_json::from_value(v).unwrap_or_default();
        doc.apply_edits(&to_core_edits(&text, &edits));
    });
}

/// Ctrl+K, Ctrl+I and the hover provider share this.
async fn hover_text(client: &Client, doc: &CodeDoc, byte: usize) -> Option<String> {
    let text = doc.text();
    let params = json!({"textDocument": {"uri": uri_string(&doc.path())}, "position": to_position(&text, byte)});
    let v = client.request_raw("textDocument/hover", params).await.ok()?;
    let h: lt::Hover = serde_json::from_value(v).ok()?;
    let s = match h.contents {
        lt::HoverContents::Markup(m) => m.value,
        lt::HoverContents::Scalar(lt::MarkedString::String(s)) => s,
        lt::HoverContents::Scalar(lt::MarkedString::LanguageString(l)) => l.value,
        lt::HoverContents::Array(a) => a
            .into_iter()
            .map(|m| match m {
                lt::MarkedString::String(s) => s,
                lt::MarkedString::LanguageString(l) => l.value,
            })
            .collect::<Vec<_>>()
            .join("\n"),
    };
    Some(markdown_to_plain(&s)).filter(|s| !s.trim().is_empty())
}

/// Roslyn's markdown has code fences and escapes; the popover shows text.
pub fn markdown_to_plain(md: &str) -> String {
    let mut out = String::new();
    for line in md.lines() {
        if line.trim_start().starts_with("```") {
            continue;
        }
        let l = line.replace("\\", "").replace("&nbsp;", " ").replace("**", "").replace('`', "");
        out.push_str(l.trim_end());
        out.push('\n');
    }
    out.trim().to_owned()
}

pub fn quick_info() {
    let Some((client, doc)) = need_server() else { return };
    let byte = doc.cursor_byte();
    glib::spawn_future_local(async move {
        if let Some(t) = hover_text(&client, &doc, byte).await {
            crate::ui::menus::tooltip_at_caret(&doc, &t);
        }
    });
}

/// Ctrl+Shift+Space and after `(` / `,`.
pub fn signature_help() {
    let Some((client, doc)) = need_server() else { return };
    let params = position_params(&doc);
    glib::spawn_future_local(async move {
        let Ok(v) = client.request_raw("textDocument/signatureHelp", params).await else { return };
        let Ok(h) = serde_json::from_value::<lt::SignatureHelp>(v) else { return };
        let active = h.active_signature.unwrap_or(0) as usize;
        let Some(sig) = h.signatures.get(active) else { return };
        let param = sig.active_parameter.or(h.active_parameter).unwrap_or(0) as usize;
        let mut markup = glib::markup_escape_text(&sig.label).to_string();
        if let Some(p) = sig.parameters.as_ref().and_then(|ps| ps.get(param)) {
            let label = match &p.label {
                lt::ParameterLabel::Simple(s) => s.clone(),
                lt::ParameterLabel::LabelOffsets([a, b]) => sig.label.chars().skip(*a as usize).take((*b - *a) as usize).collect(),
            };
            let esc = glib::markup_escape_text(&label).to_string();
            markup = markup.replacen(&esc, &format!("<b>{esc}</b>"), 1);
        }
        crate::ui::menus::markup_at_caret(&doc, &markup);
    });
}

/// Ctrl+Alt+T: the document's symbols, chosen from a list.
pub fn document_outline() {
    let Some((client, doc)) = need_server() else { return };
    let params = json!({"textDocument": {"uri": uri_string(&doc.path())}});
    glib::spawn_future_local(async move {
        let Ok(v) = client.request_raw("textDocument/documentSymbol", params).await else { return };
        let mut flat: Vec<(String, u32)> = Vec::new();
        fn walk(v: &[lt::DocumentSymbol], depth: usize, out: &mut Vec<(String, u32)>) {
            for s in v {
                out.push((format!("{}{}", "    ".repeat(depth), s.name), s.selection_range.start.line));
                if let Some(c) = &s.children {
                    walk(c, depth + 1, out);
                }
            }
        }
        if let Ok(tree) = serde_json::from_value::<Vec<lt::DocumentSymbol>>(v) {
            walk(&tree, 0, &mut flat);
        }
        let titles: Vec<String> = flat.iter().map(|(t, _)| t.clone()).collect();
        let d2 = doc.clone();
        crate::ui::menus::choose_at_caret(&doc, &titles, move |i| d2.goto_line_col(flat[i].1 as i32, 0));
    });
}

// ---- Completion ------------------------------------------------------------

mod proposal {
    use super::*;
    use glib::subclass::prelude::*;

    #[derive(Default)]
    pub struct Item {
        pub data: RefCell<Option<lt::CompletionItem>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Item {
        const NAME: &'static str = "AnvilCompletionItem";
        type Type = super::Proposal;
        type Interfaces = (sourceview5::CompletionProposal,);
    }

    impl ObjectImpl for Item {}
    impl sourceview5::subclass::prelude::CompletionProposalImpl for Item {}
}

glib::wrapper! {
    pub struct Proposal(ObjectSubclass<proposal::Item>) @implements sourceview5::CompletionProposal;
}

impl Proposal {
    fn new(item: lt::CompletionItem) -> Proposal {
        let p: Proposal = glib::Object::new();
        *p.imp().data.borrow_mut() = Some(item);
        p
    }

    fn item(&self) -> lt::CompletionItem {
        self.imp().data.borrow().clone().expect("set at creation")
    }
}

use glib::subclass::prelude::ObjectSubclassIsExt;

mod provider {
    use super::*;
    use glib::subclass::prelude::*;

    #[derive(Default)]
    pub struct Provider {
        pub doc: RefCell<std::rc::Weak<CodeDoc>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Provider {
        const NAME: &'static str = "AnvilCompletionProvider";
        type Type = super::CompletionProvider;
        type Interfaces = (sourceview5::CompletionProvider,);
    }

    impl ObjectImpl for Provider {}

    fn filter_text(i: &lt::CompletionItem) -> String {
        i.filter_text.clone().unwrap_or_else(|| i.label.clone()).to_lowercase()
    }

    impl CompletionProviderImpl for Provider {
        fn title(&self) -> Option<glib::GString> {
            Some("C#".into())
        }

        fn is_trigger(&self, _iter: &gtk::TextIter, c: char) -> bool {
            c == '.'
        }

        fn populate_future(&self, context: &sourceview5::CompletionContext) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<gio::ListModel, glib::Error>>>> {
            let doc = self.doc.borrow().upgrade();
            let word = context.word().to_lowercase();
            Box::pin(async move {
                let store = gio::ListStore::new::<Proposal>();
                let (Some(doc), Some(client)) = (doc, crate::ui::window::app().lsp.client()) else { return Ok(store.upcast()) };
                let version = doc.version.get();
                let params = position_params(&doc);
                let v = client.request_raw("textDocument/completion", params).await.unwrap_or(Value::Null);
                if doc.version.get() != version {
                    return Ok(store.upcast());
                }
                let items: Vec<lt::CompletionItem> = match serde_json::from_value::<lt::CompletionResponse>(v) {
                    Ok(lt::CompletionResponse::Array(a)) => a,
                    Ok(lt::CompletionResponse::List(l)) => l.items,
                    Err(_) => Vec::new(),
                };
                let mut items: Vec<lt::CompletionItem> = items.into_iter().filter(|i| word.is_empty() || filter_text(i).contains(&word)).collect();
                items.sort_by(|a, b| {
                    let pa = !filter_text(a).starts_with(&word);
                    let pb = !filter_text(b).starts_with(&word);
                    (pa, a.sort_text.clone().unwrap_or_else(|| a.label.clone())).cmp(&(pb, b.sort_text.clone().unwrap_or_else(|| b.label.clone())))
                });
                for i in items.into_iter().take(400) {
                    store.append(&Proposal::new(i));
                }
                Ok(store.upcast())
            })
        }

        fn refilter(&self, context: &sourceview5::CompletionContext, model: &gio::ListModel) {
            let word = context.word().to_lowercase();
            let Some(store) = model.downcast_ref::<gio::ListStore>() else { return };
            let mut keep = Vec::new();
            for i in 0..store.n_items() {
                if let Some(p) = store.item(i).and_downcast::<Proposal>()
                    && filter_text(&p.item()).contains(&word)
                {
                    keep.push(p);
                }
            }
            store.remove_all();
            for p in keep {
                store.append(&p);
            }
        }

        fn display(&self, _context: &sourceview5::CompletionContext, proposal: &sourceview5::CompletionProposal, cell: &sourceview5::CompletionCell) {
            let Some(p) = proposal.downcast_ref::<Proposal>() else { return };
            let item = p.item();
            match cell.column() {
                sourceview5::CompletionColumn::TypedText => cell.set_text(Some(&item.label)),
                sourceview5::CompletionColumn::Details => cell.set_text(item.detail.as_deref()),
                sourceview5::CompletionColumn::Icon => {
                    if let Some(i) = kind_icon(item.kind) {
                        cell.set_icon_name(i);
                    }
                }
                sourceview5::CompletionColumn::Comment => {
                    let doc = match &item.documentation {
                        Some(lt::Documentation::String(s)) => s.clone(),
                        Some(lt::Documentation::MarkupContent(m)) => markdown_to_plain(&m.value),
                        None => String::new(),
                    };
                    cell.set_text(Some(doc.lines().next().unwrap_or("")));
                }
                _ => {}
            }
        }

        fn activate(&self, context: &sourceview5::CompletionContext, proposal: &sourceview5::CompletionProposal) {
            let Some(p) = proposal.downcast_ref::<Proposal>() else { return };
            let Some(doc) = self.doc.borrow().upgrade() else { return };
            let item = p.item();
            let buffer = &doc.buffer;
            let Some((mut s, mut e)) = context.bounds() else { return };
            let raw = match &item.text_edit {
                Some(lt::CompletionTextEdit::Edit(t)) => t.new_text.clone(),
                Some(lt::CompletionTextEdit::InsertAndReplace(t)) => t.new_text.clone(),
                None => item.insert_text.clone().unwrap_or_else(|| item.label.clone()),
            };
            let (insert, caret) = strip_snippet(&raw, item.insert_text_format == Some(lt::InsertTextFormat::SNIPPET));
            buffer.begin_user_action();
            if let Some(lt::CompletionTextEdit::Edit(t)) = &item.text_edit {
                let text = doc.text();
                s = doc.iter_at_byte(to_offset(&text, t.range.start));
                e = doc.iter_at_byte(to_offset(&text, t.range.end));
                // The user may have typed past the server's range.
                let cursor = buffer.iter_at_mark(&buffer.get_insert());
                if cursor.offset() > e.offset() {
                    e = cursor;
                }
            }
            buffer.delete(&mut s, &mut e);
            let start_offset = s.offset();
            buffer.insert(&mut s, &insert);
            if let Some(c) = caret {
                let it = buffer.iter_at_offset(start_offset + c as i32);
                buffer.place_cursor(&it);
            }
            buffer.end_user_action();
            if let Some(extra) = item.additional_text_edits.filter(|v| !v.is_empty()) {
                let text = doc.text();
                doc.apply_edits(&to_core_edits(&text, &extra));
            }
        }
    }
}

glib::wrapper! {
    pub struct CompletionProvider(ObjectSubclass<provider::Provider>) @implements sourceview5::CompletionProvider;
}

/// Snippet syntax (`$0`, `${1:x}`) to plain text and the caret position
/// (in chars).
pub fn strip_snippet(s: &str, snippet: bool) -> (String, Option<usize>) {
    if !snippet {
        return (s.to_owned(), None);
    }
    let mut out = String::new();
    let mut caret = None;
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\\' && i + 1 < chars.len() {
            out.push(chars[i + 1]);
            i += 2;
        } else if chars[i] == '$' && i + 1 < chars.len() && chars[i + 1].is_ascii_digit() {
            if caret.is_none() || chars[i + 1] == '0' {
                caret = Some(out.chars().count());
            }
            i += 2;
            while i < chars.len() && chars[i].is_ascii_digit() {
                i += 1;
            }
        } else if chars[i] == '$' && i + 1 < chars.len() && chars[i + 1] == '{' {
            let close = chars[i..].iter().position(|c| *c == '}').map_or(chars.len(), |p| i + p);
            let inner: String = chars[i + 2..close].iter().collect();
            let default = inner.split_once(':').map_or("", |(_, d)| d);
            if caret.is_none() {
                caret = Some(out.chars().count());
            }
            out.push_str(default);
            i = close + 1;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    (out, caret)
}

fn kind_icon(kind: Option<lt::CompletionItemKind>) -> Option<&'static str> {
    Some(match kind? {
        lt::CompletionItemKind::METHOD | lt::CompletionItemKind::FUNCTION | lt::CompletionItemKind::CONSTRUCTOR => "system-run-symbolic",
        lt::CompletionItemKind::FIELD | lt::CompletionItemKind::VARIABLE | lt::CompletionItemKind::PROPERTY => "view-list-symbolic",
        lt::CompletionItemKind::CLASS | lt::CompletionItemKind::STRUCT | lt::CompletionItemKind::INTERFACE | lt::CompletionItemKind::ENUM => "application-x-addon-symbolic",
        lt::CompletionItemKind::KEYWORD => "format-text-bold-symbolic",
        lt::CompletionItemKind::SNIPPET => "edit-paste-symbolic",
        lt::CompletionItemKind::MODULE => "folder-symbolic",
        _ => return None,
    })
}

// ---- Hover --------------------------------------------------------------------

mod hover {
    use super::*;
    use glib::subclass::prelude::*;

    #[derive(Default)]
    pub struct Hover {
        pub doc: RefCell<std::rc::Weak<CodeDoc>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Hover {
        const NAME: &'static str = "AnvilHoverProvider";
        type Type = super::HoverProvider;
        type Interfaces = (sourceview5::HoverProvider,);
    }

    impl ObjectImpl for Hover {}

    impl HoverProviderImpl for Hover {
        fn populate_future(&self, context: &sourceview5::HoverContext, display: &sourceview5::HoverDisplay) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), glib::Error>> + 'static>> {
            let doc = self.doc.borrow().upgrade();
            let it = context.iter();
            let display = display.clone();
            Box::pin(async move {
                let fail = || glib::Error::new(gio::IOErrorEnum::NotFound, "no hover");
                let (Some(doc), Some(it)) = (doc, it) else { return Err(fail()) };
                let start = doc.buffer.start_iter();
                let byte = doc.buffer.text(&start, &it, true).len();
                // Debugger first: while stopped, a hover shows the value as
                // a DataTip, with or without Roslyn.
                if let Some(tip) = crate::ui::window::app().debugger.data_tip(&doc, byte).await {
                    display.append(&tip);
                    return Ok(());
                }
                let Some(client) = crate::ui::window::app().lsp.client() else { return Err(fail()) };
                // Diagnostics under the pointer, then Quick Info.
                let text = doc.text();
                let pos = to_position(&text, byte);
                let diags: Vec<String> = doc
                    .diagnostics
                    .borrow()
                    .iter()
                    .filter(|d| d.range.start <= pos && pos <= d.range.end)
                    .map(|d| format!("{}: {}", d.code.as_ref().map(|c| match c { lt::NumberOrString::String(s) => s.clone(), lt::NumberOrString::Number(n) => n.to_string() }).unwrap_or_default(), d.message))
                    .collect();
                let info = hover_text(&client, &doc, byte).await;
                if diags.is_empty() && info.is_none() {
                    return Err(fail());
                }
                for d in diags {
                    let l = gtk::Label::builder().label(d).xalign(0.0).wrap(true).max_width_chars(80).build();
                    display.append(&l);
                }
                if let Some(t) = info {
                    let l = gtk::Label::builder().label(t).xalign(0.0).wrap(true).max_width_chars(90).css_classes(["mono"]).selectable(true).build();
                    display.append(&l);
                }
                Ok(())
            })
        }
    }
}

glib::wrapper! {
    pub struct HoverProvider(ObjectSubclass<hover::Hover>) @implements sourceview5::HoverProvider;
}

thread_local! {
    static COMPLETION_OPEN: Cell<bool> = const { Cell::new(false) };
}

pub fn completion_visible(_doc: &CodeDoc) -> bool {
    COMPLETION_OPEN.with(Cell::get)
}

pub fn attach_providers(doc: &Rc<CodeDoc>) {
    if !doc.is_csharp() {
        return;
    }
    let completion = doc.view.completion();
    let provider: CompletionProvider = glib::Object::new();
    *provider.imp().doc.borrow_mut() = Rc::downgrade(doc);
    completion.add_provider(&provider);
    completion.set_select_on_show(true);
    completion.connect_show(|_| COMPLETION_OPEN.with(|c| c.set(true)));
    completion.connect_hide(|_| COMPLETION_OPEN.with(|c| c.set(false)));
    let hover = doc.view.hover();
    let hp: HoverProvider = glib::Object::new();
    *hp.imp().doc.borrow_mut() = Rc::downgrade(doc);
    hover.add_provider(&hp);
    hover.set_hover_delay(450);
    // Signature help after `(` and `,`, closed by `)`.
    let weak = Rc::downgrade(doc);
    doc.buffer.connect_insert_text(move |_, _, text| {
        if let Some(d) = weak.upgrade()
            && !d.quiet.get()
        {
            if text == "(" || text == "," {
                glib::idle_add_local_once(signature_help);
            } else if text == ")" {
                crate::ui::menus::close_caret_popup();
            }
        }
    });
}

/// VS' editor context menu.
pub fn attach_context_menu(doc: &Rc<CodeDoc>) {
    let click = gtk::GestureClick::new();
    click.set_button(gtk::gdk::BUTTON_SECONDARY);
    click.set_propagation_phase(gtk::PropagationPhase::Capture);
    let weak = Rc::downgrade(doc);
    click.connect_pressed(move |g, _, x, y| {
        let Some(doc) = weak.upgrade() else { return };
        g.set_state(gtk::EventSequenceState::Claimed);
        // Place the caret where the user clicked, unless inside a selection.
        let (bx, by) = doc.view.window_to_buffer_coords(gtk::TextWindowType::Widget, x as i32, y as i32);
        if let Some(it) = doc.view.iter_at_location(bx, by) {
            let inside = doc.buffer.selection_bounds().is_some_and(|(a, b)| it.offset() >= a.offset() && it.offset() <= b.offset());
            if !inside {
                doc.buffer.place_cursor(&it);
            }
        }
        let is_test = doc.is_csharp() && doc.text().contains("[Test") || doc.text().contains("[Fact");
        let mut items: Vec<crate::ui::menus::Item> = vec![
            crate::ui::menus::Item::Command("edit.quick-actions"),
            crate::ui::menus::Item::Command("edit.rename"),
            crate::ui::menus::Item::Separator,
            crate::ui::menus::Item::Command("edit.goto-definition"),
            crate::ui::menus::Item::Command("edit.peek-definition"),
            crate::ui::menus::Item::Command("edit.goto-implementation"),
            crate::ui::menus::Item::Command("edit.find-references"),
            crate::ui::menus::Item::Separator,
        ];
        if is_test {
            items.push(crate::ui::menus::Item::Command("test.run-context"));
            items.push(crate::ui::menus::Item::Command("test.debug-context"));
            items.push(crate::ui::menus::Item::Separator);
        }
        items.extend([
            crate::ui::menus::Item::Command("edit.insert-snippet"),
            crate::ui::menus::Item::Command("edit.surround-with"),
            crate::ui::menus::Item::Separator,
            crate::ui::menus::Item::Action(tr("Cut"), "clipboard.cut"),
            crate::ui::menus::Item::Action(tr("Copy"), "clipboard.copy"),
            crate::ui::menus::Item::Action(tr("Paste"), "clipboard.paste"),
        ]);
        crate::ui::menus::popup(doc.view.upcast_ref(), x, y, &items);
    });
    doc.view.add_controller(click);
}

// ---- AI ghost text ---------------------------------------------------------

thread_local! {
    static AI_TIMER: RefCell<Option<glib::SourceId>> = const { RefCell::new(None) };
}

/// Requests ghost text 250 ms after typing stops; any keystroke cancels it
/// (the serial changes and the answer is dropped).
pub fn schedule_ai(doc: &Rc<CodeDoc>) {
    doc.view.set_ghost(None);
    let app = crate::ui::window::app();
    if !(app.settings.boolean("ai-enabled") && app.settings.boolean("ai-inline")) || !doc.is_csharp() || doc.quiet.get() {
        return;
    }
    AI_TIMER.with(|t| {
        if let Some(id) = t.borrow_mut().take() {
            id.remove();
        }
    });
    let weak = Rc::downgrade(doc);
    let id = glib::timeout_add_local_once(std::time::Duration::from_millis(250), move || {
        AI_TIMER.with(|t| t.borrow_mut().take());
        if let Some(doc) = weak.upgrade() {
            glib::spawn_future_local(ai_complete(doc));
        }
    });
    AI_TIMER.with(|t| *t.borrow_mut() = Some(id));
}

async fn ai_complete(doc: Rc<CodeDoc>) {
    let app = crate::ui::window::app();
    let serial = doc.change_serial.get();
    let offset = doc.cursor_offset();
    let text = doc.text();
    let cursor = doc.cursor_byte();
    // Only at the end of a line, as VS suggests.
    if !text[cursor..].split('\n').next().unwrap_or("").trim().is_empty() {
        return;
    }
    let model = app.settings.string("ai-inline-model").to_string();
    let url = app.settings.string("ollama-url").to_string();
    let sigs = crate::ui::chat::project_signatures(2000);
    let (prefix, suffix) = anvil_ai::context::fim(&text, cursor, &sigs, 6000);
    let Some(ollama) = anvil_ai::ollama::Ollama::new(&url) else { return };
    let result = crate::runtime::run(async move {
        let model = if model.is_empty() { ollama.models().await.ok().and_then(|m| m.into_iter().find(|m| m.is_coder()).map(|m| m.name)).unwrap_or_default() } else { model };
        if model.is_empty() {
            return Err(anvil_ai::ollama::Error::Server("no model".into()));
        }
        ollama.complete(&model, &prefix, &suffix, 64).await.map(|s| anvil_ai::ollama::clean_completion(&s, &suffix))
    })
    .await;
    if doc.change_serial.get() != serial || doc.cursor_offset() != offset {
        return;
    }
    match result {
        Ok(s) if !s.trim().is_empty() => doc.view.set_ghost(Some((offset, s))),
        Ok(_) => {}
        Err(e) => app.status.flash(&e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snippets_become_text_and_caret() {
        assert_eq!(strip_snippet("Console.WriteLine($0);", true), ("Console.WriteLine();".to_owned(), Some(18)));
        assert_eq!(strip_snippet("for (${1:i})", true), ("for (i)".to_owned(), Some(5)));
        assert_eq!(strip_snippet("plain$0", false), ("plain$0".to_owned(), None));
    }
}
