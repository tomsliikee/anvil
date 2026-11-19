//! The terminal panel (VTE). Programs run here so `Console.ReadLine`
//! works under Ctrl+F5 and F5; the user's shell runs here otherwise. In
//! the Flatpak everything runs on the host through `flatpak-spawn --host`.

use std::cell::RefCell;
use std::path::Path;

use adw::prelude::*;
use anvil_core::host::{self, Command};
use gtk::glib;
use vte4::prelude::*;

use crate::i18n::tr;
use crate::palette;

type ExitFn = Box<dyn Fn(i32)>;

pub struct Terminal {
    pub widget: gtk::Box,
    term: vte4::Terminal,
    pid: RefCell<Option<glib::Pid>>,
    on_exit: RefCell<Option<ExitFn>>,
}

fn apply_colors(term: &vte4::Terminal) {
    let p = palette::current();
    term.set_colors(Some(&p.ink), Some(&p.card), &[]);
    term.set_color_cursor(Some(&p.ink));
}

impl Terminal {
    pub fn new() -> Terminal {
        let term = vte4::Terminal::new();
        term.set_vexpand(true);
        term.set_hexpand(true);
        term.set_font(Some(&gtk::pango::FontDescription::from_string("Cascadia Mono 10.5")));
        term.set_scrollback_lines(10_000);
        term.set_margin_start(10);
        term.set_margin_end(10);
        term.set_margin_bottom(6);
        apply_colors(&term);
        adw::StyleManager::default().connect_dark_notify(glib::clone!(
            #[weak]
            term,
            move |_| apply_colors(&term)
        ));
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 0);
        widget.append(&term);
        let t = Terminal { widget, term, pid: RefCell::new(None), on_exit: RefCell::new(None) };
        t.term.connect_child_exited(|_, status| {
            let app = crate::ui::window::app();
            app.terminal.pid.borrow_mut().take();
            let on_exit = app.terminal.on_exit.borrow_mut().take();
            if let Some(f) = on_exit {
                f(status);
            }
        });
        t
    }

    pub fn is_running(&self) -> bool {
        self.pid.borrow().is_some()
    }

    /// Runs a shell command line on the host in this terminal.
    /// `on_exit` gets the exit status.
    pub fn run(&self, cwd: &Path, line: &str, env: &[(String, String)], on_exit: Option<Box<dyn Fn(i32)>>) {
        self.kill();
        self.term.reset(true, true);
        let shell = Command::new("bash").args(["-lc", line]).cwd(cwd);
        let shell = env.iter().fold(shell, |c, (k, v)| c.env(k.clone(), v.clone()));
        let argv = host::wrap(&shell, crate::host::in_flatpak());
        let argv_refs: Vec<&str> = argv.iter().map(String::as_str).collect();
        let envv: Vec<String> = if crate::host::in_flatpak() { Vec::new() } else { env.iter().map(|(k, v)| format!("{k}={v}")).collect() };
        let env_refs: Vec<&str> = envv.iter().map(String::as_str).collect();
        *self.on_exit.borrow_mut() = on_exit;
        let cwd = cwd.to_string_lossy().into_owned();
        self.term.spawn_async(
            vte4::PtyFlags::DEFAULT,
            Some(&cwd),
            &argv_refs,
            &env_refs,
            glib::SpawnFlags::SEARCH_PATH,
            || {},
            -1,
            None::<&gtk::gio::Cancellable>,
            |res| {
                let app = crate::ui::window::app();
                match res {
                    Ok(pid) => *app.terminal.pid.borrow_mut() = Some(pid),
                    Err(e) => app.terminal.feed_line(&format!("{}: {e}", tr("Could not start"))),
                }
            },
        );
        self.term.grab_focus();
    }

    /// Runs a program the way VS' console host does: then waits for a key.
    pub fn run_program(&self, cwd: &Path, cmd: &Command, on_exit: Option<Box<dyn Fn(i32)>>) {
        self.run_program_noting_exit(cwd, cmd, None, on_exit);
    }

    /// As [`Terminal::run_program`]; when the program ends (before the
    /// key press) its exit code is written to `exit_file`.
    pub fn run_program_noting_exit(&self, cwd: &Path, cmd: &Command, exit_file: Option<&Path>, on_exit: Option<Box<dyn Fn(i32)>>) {
        let prog = host::sh_line(&Command { env: Vec::new(), ..cmd.clone() });
        let note = exit_file.map(|f| format!(" echo $code > {};", host::sh_quote(&f.to_string_lossy()))).unwrap_or_default();
        let line = format!(
            "{prog}; code=$?;{note} printf '\\n%s\\n' \"{}\"; printf '%s' \"{}\"; read -rsn1; exit $code",
            tr("The program exited with code $code."),
            tr("Press any key to close this window . . .")
        );
        self.run(cwd, &line, &cmd.env, on_exit);
    }

    /// Output that arrives from elsewhere (the debugger) as if the program
    /// had written it.
    pub fn feed_text(&self, text: &str) {
        self.term.feed(text.replace("\r\n", "\n").replace('\n', "\r\n").as_bytes());
    }

    /// Types into the running program (the screenshot run).
    pub fn send_input(&self, text: &str) {
        self.term.feed_child(text.as_bytes());
    }

    pub fn shell_in(&self, dir: &Path) {
        self.run(dir, "exec \"${SHELL:-bash}\" -l", &[], None);
    }

    pub fn ensure_shell(&self) {
        if !self.is_running() {
            let dir = crate::ui::window::solution_dir().unwrap_or_else(crate::host::home);
            self.shell_in(&dir);
        }
    }

    pub fn kill(&self) {
        if let Some(pid) = self.pid.borrow_mut().take() {
            // SIGHUP closes the shell and what runs in it, as closing a
            // terminal window does. flatpak-spawn forwards it to the host.
            let _ = std::process::Command::new("kill").args(["-HUP", &pid.0.to_string()]).status();
        }
        self.on_exit.borrow_mut().take();
    }

    pub fn feed_line(&self, text: &str) {
        self.term.feed(format!("{text}\r\n").as_bytes());
    }
}
