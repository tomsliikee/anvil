//! Long jobs, results and failures: the one way every part of Anvil says
//! that something runs, how it ended, and what went wrong.
//!
//! **Jobs.** [`start`] puts a job into the status line: a small spinner,
//! its text, the time it has run and, when [`Job::cancellable`] gave it a
//! way to stop, a cancel button. Jobs are keyed (`"build"`, `"run"`,
//! `"test"`, `"restore"`, `"lsp"`, `"debug"`, …): starting a key that runs
//! already replaces it, and [`finish`] ends a key without holding the
//! [`Job`]. A [`Job`] is a token, not a guard; dropping it changes
//! nothing, and [`Job::finish`] ends only that run of its key, so a late
//! callback of an old build never ends the next one.
//!
//! ```ignore
//! let job = jobs::start("test", &trf("Running tests of {}…", &[&p])).cancellable(move || stream.kill());
//! // …
//! job.finish();
//! jobs::result("test", &summary, jobs::Open::Pane("tests"));
//! ```
//!
//! **Results.** [`result`] shows a toast ("Build succeeded – 0 errors,
//! 4 warnings · 3.2 s") whose button opens where the details are; a newer
//! result of the same key replaces the older toast. Nothing opens by
//! itself.
//!
//! **Failures.** [`error`] never blocks with a dialog: the title goes into
//! a toast, the details into an Output pane, and the toast's Details
//! button shows them. No failure is silent.
//!
//! **Timing.** With `ANVIL_TIMING=1`, [`trace`] prints milestones since the
//! process started, and [`watch_main_thread`] reports every main-thread
//! stall over 16 ms, on stderr.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use gtk::glib;

use crate::i18n::tr;

type CancelFn = Rc<dyn Fn()>;

struct Entry {
    id: u64,
    key: String,
    text: String,
    started: Instant,
    cancel: Option<CancelFn>,
}

#[derive(Default)]
struct State {
    jobs: Vec<Entry>,
    next_id: u64,
    ticker: Option<glib::SourceId>,
    toasts: HashMap<String, adw::Toast>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::default();
}

/// One run of a keyed job.
#[derive(Clone, Debug)]
pub struct Job {
    id: u64,
    key: String,
}

/// Starts (or restarts) the job `key` with `text`, e.g. "Building Medium…".
pub fn start(key: &str, text: &str) -> Job {
    let id = STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.next_id += 1;
        let id = s.next_id;
        s.jobs.retain(|j| j.key != key);
        s.jobs.push(Entry { id, key: key.to_owned(), text: text.to_owned(), started: Instant::now(), cancel: None });
        id
    });
    trace(&format!("job {key} started"));
    ensure_ticker();
    render();
    Job { id, key: key.to_owned() }
}

/// Ends the job `key`, whichever run it is.
pub fn finish(key: &str) {
    let removed = STATE.with(|s| {
        let mut s = s.borrow_mut();
        let n = s.jobs.len();
        s.jobs.retain(|j| j.key != key);
        n != s.jobs.len()
    });
    if removed {
        trace(&format!("job {key} finished"));
        render();
    }
}

impl Job {
    /// Gives the job a cancel button that calls `f` (and then ends the job).
    pub fn cancellable(self, f: impl Fn() + 'static) -> Job {
        self.with_entry(|e| e.cancel = Some(Rc::new(f)));
        render();
        self
    }

    /// New text for a job that moves on ("Building · 3 of 8 projects").
    pub fn set_text(&self, text: &str) {
        if self.with_entry(|e| e.text = text.to_owned()) {
            render();
        }
    }

    /// How long this run has taken so far.
    pub fn elapsed(&self) -> Duration {
        STATE.with(|s| s.borrow().jobs.iter().find(|j| j.id == self.id).map(|j| j.started.elapsed())).unwrap_or_default()
    }

    /// Whether this run is still the current one of its key.
    pub fn is_current(&self) -> bool {
        STATE.with(|s| s.borrow().jobs.iter().any(|j| j.id == self.id))
    }

    /// Ends this run; a newer run of the same key keeps going.
    pub fn finish(&self) {
        let removed = STATE.with(|s| {
            let mut s = s.borrow_mut();
            let n = s.jobs.len();
            s.jobs.retain(|j| j.id != self.id);
            n != s.jobs.len()
        });
        if removed {
            trace(&format!("job {} finished", self.key));
            render();
        }
    }

    fn with_entry(&self, f: impl FnOnce(&mut Entry)) -> bool {
        STATE.with(|s| s.borrow_mut().jobs.iter_mut().find(|j| j.id == self.id).map(f).is_some())
    }
}

/// The status line's cancel button: stops the newest job.
pub fn cancel_current() {
    let cancel = STATE.with(|s| {
        let mut s = s.borrow_mut();
        let i = s.jobs.iter().rposition(|j| j.cancel.is_some())?;
        let e = s.jobs.remove(i);
        e.cancel
    });
    if let Some(f) = cancel {
        f();
    }
    render();
}

/// What the status line shows: the newest job, how many others run.
pub struct View {
    pub text: String,
    pub elapsed: String,
    pub cancellable: bool,
    pub others: Vec<String>,
}

fn current_view() -> Option<View> {
    STATE.with(|s| {
        let s = s.borrow();
        let newest = s.jobs.last()?;
        Some(View {
            text: newest.text.clone(),
            elapsed: anvil_core::build::format_running(newest.started.elapsed()),
            cancellable: newest.cancel.is_some(),
            others: s.jobs[..s.jobs.len() - 1].iter().map(|j| j.text.clone()).collect(),
        })
    })
}

fn render() {
    if let Some(app) = crate::ui::window::try_app() {
        app.status.show_job(current_view().as_ref());
    }
}

/// The running clocks tick once a second, and only while a job runs.
fn ensure_ticker() {
    STATE.with(|s| {
        if s.borrow().ticker.is_some() {
            return;
        }
        let id = glib::timeout_add_local(Duration::from_secs(1), || {
            let idle = STATE.with(|s| s.borrow().jobs.is_empty());
            if idle {
                STATE.with(|s| s.borrow_mut().ticker = None);
                return glib::ControlFlow::Break;
            }
            render();
            glib::ControlFlow::Continue
        });
        s.borrow_mut().ticker = Some(id);
    });
}

/// Where a result toast's button leads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Open {
    Nothing,
    ErrorList,
    /// An Output pane (`"Build"`, `"Run"`, `"Debug"`, …).
    Output(&'static str),
    /// A page of the bottom panel by name (`"terminal"`, `"tests"`).
    Pane(&'static str),
}

fn open(target: Open) {
    let Some(app) = crate::ui::window::try_app() else { return };
    match target {
        Open::Nothing => {}
        Open::ErrorList => crate::ui::window::show_bottom("errors"),
        Open::Output(pane) => {
            app.output.show(pane);
            crate::ui::window::show_bottom("output");
        }
        Open::Pane(name) => crate::ui::window::show_bottom(name),
    }
}

fn button_label(target: Open) -> Option<String> {
    match target {
        Open::Nothing => None,
        Open::ErrorList => Some(tr("Error List")),
        Open::Output(_) => Some(tr("Output")),
        Open::Pane("terminal") => Some(tr("Terminal")),
        Open::Pane("tests") => Some(tr("Test Explorer")),
        Open::Pane(_) => Some(tr("Show")),
    }
}

fn show_toast(key: &str, toast: adw::Toast) {
    let Some(app) = crate::ui::window::try_app() else { return };
    let old = STATE.with(|s| s.borrow_mut().toasts.insert(key.to_owned(), toast.clone()));
    if let Some(old) = old {
        old.dismiss();
    }
    app.toast.add_toast(toast);
}

/// A job's outcome as a toast with a button to the details.
pub fn result(key: &str, text: &str, target: Open) {
    let toast = adw::Toast::new(&glib::markup_escape_text(text));
    toast.set_timeout(6);
    // A result answers what the user just asked for; it does not wait
    // behind older toasts.
    toast.set_priority(adw::ToastPriority::High);
    if let Some(label) = button_label(target) {
        toast.set_button_label(Some(&label));
        toast.connect_button_clicked(move |_| open(target));
    }
    show_toast(key, toast);
}

/// A short note that needs no details ("Hot Reload is on").
pub fn info(text: &str) {
    if let Some(app) = crate::ui::window::try_app() {
        app.toast.add_toast(adw::Toast::new(&glib::markup_escape_text(text)));
    }
}

/// The longest detail that still reads as part of the toast's title.
const INLINE_DETAIL: usize = 90;

/// A failure: `title` in a toast, `detail` in the Output pane `pane`
/// (`"General"` when no other fits); the toast's Details button shows it.
pub fn error(pane: &'static str, title: &str, detail: &str) {
    let detail = detail.trim();
    eprintln!("anvil: {title}: {detail}");
    let Some(app) = crate::ui::window::try_app() else { return };
    let time = glib::DateTime::now_local().ok().and_then(|d| d.format("%H:%M:%S").ok()).map(|s| s.to_string()).unwrap_or_default();
    app.output.append(pane, &format!("[{time}] {title}\n{}{}", detail, if detail.is_empty() { "" } else { "\n" }));
    let short = !detail.is_empty() && !detail.contains('\n') && detail.chars().count() <= INLINE_DETAIL;
    let text = if short { format!("{title}: {detail}") } else { title.to_owned() };
    let toast = adw::Toast::new(&glib::markup_escape_text(&text));
    toast.set_use_markup(true);
    toast.set_timeout(10);
    toast.set_priority(adw::ToastPriority::High);
    if !detail.is_empty() && !short {
        toast.set_button_label(Some(&tr("Details")));
        toast.connect_button_clicked(move |_| open(Open::Output(pane)));
    }
    show_toast(&format!("error:{title}"), toast);
}

fn timing() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("ANVIL_TIMING").is_some_and(|v| v != "0"))
}

fn process_start() -> Instant {
    static START: OnceLock<Instant> = OnceLock::new();
    *START.get_or_init(Instant::now)
}

/// Remembers when the process started; call first thing in `main`.
pub fn mark_process_start() {
    process_start();
}

/// With `ANVIL_TIMING`, prints `what` with the time since the start.
pub fn trace(what: &str) {
    if timing() {
        eprintln!("[anvil timing] {:>8.1} ms  {what}", process_start().elapsed().as_secs_f64() * 1000.0);
    }
}

/// With `ANVIL_TIMING`, notes when `widget` has drawn its first frame.
pub fn trace_first_frame(widget: &impl gtk::prelude::IsA<gtk::Widget>) {
    use gtk::prelude::*;
    if !timing() {
        return;
    }
    widget.add_tick_callback(|_, _| {
        trace("first frame");
        glib::ControlFlow::Break
    });
}

/// With `ANVIL_TIMING`, reports main-thread stalls over 16 ms: a 4 ms
/// timer that fires late was blocked by whatever ran before it.
pub fn watch_main_thread() {
    if !timing() {
        return;
    }
    let last = Rc::new(Cell::new(Instant::now()));
    glib::timeout_add_local(Duration::from_millis(4), move || {
        let now = Instant::now();
        let gap = now.duration_since(last.get());
        last.set(now);
        if gap > Duration::from_millis(20) {
            trace(&format!("main thread blocked {} ms", gap.as_millis().saturating_sub(4)));
        }
        glib::ControlFlow::Continue
    });
}
