//! Opt-in main-thread stall log. With `ANVIL_FRAME_LOG=1` every turn of
//! the GTK main loop that keeps the thread busy for longer than one frame
//! (16 ms) is written to stderr with the named work that ran in it, and a
//! summary follows when Anvil quits. Without the variable nothing here
//! costs more than one flag check.
//!
//! A turn is measured from the return of `poll()` to its next call, so it
//! covers every dispatched source: input, timers, idles, futures and the
//! frame clock. The frame clock's phases (update, layout, paint) and the
//! editor's own passes are named with [`span`]; time no span claims is
//! GTK's own work or code without a span yet.

use std::cell::RefCell;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use gtk::glib;
use gtk::prelude::*;

const STALL: Duration = Duration::from_millis(16);

pub fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("ANVIL_FRAME_LOG").is_ok_and(|v| !v.is_empty() && v != "0"))
}

#[derive(Default)]
struct State {
    /// When the current turn started (the last `poll()` returned).
    turn: Option<Instant>,
    /// Named work in the current turn, in the order it finished.
    spans: Vec<(&'static str, Duration)>,
    turns: u64,
    stalls: Vec<Duration>,
    /// Per name: total time inside stalls and how often it was the
    /// largest part of one.
    blame: Vec<(&'static str, Duration, u32)>,
    worst: Duration,
    /// A simulated keystroke waiting for its frame, and the measured
    /// keystroke-to-painted-frame latencies.
    key: Option<Instant>,
    latencies: Vec<Duration>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::default();
    /// Frame clock phase start, for the phase spans.
    static PHASE: RefCell<Option<Instant>> = const { RefCell::new(None) };
}

/// Ends the current turn: logs it when it was a stall.
fn end_turn(now: Instant) {
    STATE.with(|s| {
        let Ok(mut s) = s.try_borrow_mut() else { return };
        let spans = std::mem::take(&mut s.spans);
        let Some(start) = s.turn.take() else { return };
        s.turns += 1;
        let total = now - start;
        if total < STALL {
            return;
        }
        s.stalls.push(total);
        s.worst = s.worst.max(total);
        let mut merged: Vec<(&'static str, Duration)> = Vec::new();
        for (name, d) in spans {
            match merged.iter_mut().find(|(n, _)| *n == name) {
                Some(m) => m.1 += d,
                None => merged.push((name, d)),
            }
        }
        merged.sort_by_key(|m| std::cmp::Reverse(m.1));
        let named: Duration = merged.iter().map(|(_, d)| *d).sum();
        let rest = total.saturating_sub(named);
        let top = merged.first().filter(|(_, d)| *d >= rest).map_or("(unnamed)", |(n, _)| *n);
        for (name, d) in merged.iter().copied().chain(std::iter::once(("(unnamed)", rest))) {
            match s.blame.iter_mut().find(|(n, _, _)| *n == name) {
                Some(b) => {
                    b.1 += d;
                    b.2 += u32::from(name == top);
                }
                None => s.blame.push((name, d, u32::from(name == top))),
            }
        }
        let parts: Vec<String> = merged.iter().map(|(n, d)| format!("{n} {:.1}", ms(*d))).chain(std::iter::once(format!("(unnamed) {:.1}", ms(rest)))).collect();
        eprintln!("anvil-frame: {:6.1} ms  [{}]", ms(total), parts.join(", "));
    });
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

unsafe extern "C" fn timed_poll(fds: *mut glib::ffi::GPollFD, n: std::ffi::c_uint, timeout: std::ffi::c_int) -> std::ffi::c_int {
    end_turn(Instant::now());
    // SAFETY: the arguments are the ones GLib passed to its poll function;
    // `g_poll` is GLib's default poll function.
    let r = unsafe { glib::ffi::g_poll(fds, n, timeout) };
    STATE.with(|s| {
        if let Ok(mut s) = s.try_borrow_mut() {
            s.turn = Some(Instant::now());
        }
    });
    r
}

/// Starts measuring the default main context (call once, on the main
/// thread, before the main loop runs).
pub fn install() {
    if !enabled() {
        return;
    }
    // SAFETY: the default context is owned by GLib and outlives the app;
    // `timed_poll` has GLib's poll signature and calls the default poll.
    unsafe { glib::ffi::g_main_context_set_poll_func(glib::ffi::g_main_context_default(), Some(timed_poll)) };
    eprintln!("anvil-frame: logging main-loop turns over {} ms", STALL.as_millis());
}

/// Names the frame clock's phases of `window` once it is realized.
pub fn watch_window(window: &impl IsA<gtk::Widget>) {
    if !enabled() {
        return;
    }
    let attach = |w: &gtk::Widget| {
        let Some(clock) = w.frame_clock() else { return };
        // GTK's own handlers run first (they were connected at realize),
        // so each timestamp marks the end of a phase.
        clock.connect_before_paint(|_| PHASE.with(|p| *p.borrow_mut() = Some(Instant::now())));
        let phase = |name: &'static str| {
            move |_: &gtk::gdk::FrameClock| {
                let now = Instant::now();
                if let Some(start) = PHASE.with(|p| p.borrow_mut().replace(now)) {
                    record(name, now - start);
                }
            }
        };
        clock.connect_update(phase("frame update"));
        clock.connect_layout(phase("frame layout"));
        clock.connect_paint(phase("frame paint"));
        clock.connect_after_paint(|_| {
            STATE.with(|s| {
                if let Ok(mut s) = s.try_borrow_mut()
                    && let Some(k) = s.key.take()
                {
                    s.latencies.push(k.elapsed());
                }
            })
        });
    };
    let w = window.as_ref().clone();
    if w.is_realized() {
        attach(&w);
    } else {
        w.connect_realize(move |w| attach(w));
    }
}

fn record(name: &'static str, d: Duration) {
    STATE.with(|s| {
        if let Ok(mut s) = s.try_borrow_mut() {
            s.spans.push((name, d));
        }
    });
}

/// Times a named piece of main-thread work until the guard drops.
pub struct Span(Option<(&'static str, Instant)>);

impl Drop for Span {
    fn drop(&mut self) {
        if let Some((name, start)) = self.0.take() {
            record(name, start.elapsed());
        }
    }
}

pub fn span(name: &'static str) -> Span {
    Span(enabled().then(|| (name, Instant::now())))
}

/// A keystroke happened now; the next painted frame ends its latency.
pub fn keystroke() {
    STATE.with(|s| {
        if let Ok(mut s) = s.try_borrow_mut()
            && s.key.is_none()
        {
            s.key = Some(Instant::now());
        }
    });
}

/// Forgets everything measured so far (a benchmark's warm-up).
pub fn reset() {
    STATE.with(|s| {
        if let Ok(mut s) = s.try_borrow_mut() {
            let turn = s.turn;
            *s = State { turn, ..State::default() };
        }
    });
}

/// The summary: turns, stalls, percentiles and who caused them.
pub fn report() {
    if !enabled() {
        return;
    }
    STATE.with(|s| {
        let Ok(s) = s.try_borrow() else { return };
        let mut st = s.stalls.clone();
        st.sort();
        let pct = |p: f64| st.get(((st.len() as f64 * p) as usize).min(st.len().saturating_sub(1))).map_or(0.0, |d| ms(*d));
        let over = |limit: u64| st.iter().filter(|d| d.as_millis() as u64 > limit).count();
        eprintln!(
            "anvil-frame: summary: {} turns, {} over 16 ms, {} over 50 ms, {} over 100 ms; stalls p50 {:.1} ms, p95 {:.1} ms, worst {:.1} ms",
            s.turns,
            st.len(),
            over(50),
            over(100),
            pct(0.5),
            pct(0.95),
            ms(s.worst)
        );
        if !s.latencies.is_empty() {
            let mut l = s.latencies.clone();
            l.sort();
            let at = |p: f64| ms(l[((l.len() as f64 * p) as usize).min(l.len() - 1)]);
            eprintln!("anvil-frame: keystroke to frame: {} keys, p50 {:.1} ms, p95 {:.1} ms, worst {:.1} ms", l.len(), at(0.5), at(0.95), ms(l[l.len() - 1]));
        }
        let mut blame = s.blame.clone();
        blame.sort_by_key(|b| std::cmp::Reverse(b.1));
        for (name, d, top) in blame.iter().take(12) {
            eprintln!("anvil-frame:   {name:<28} {:8.1} ms in stalls, largest part of {top}", ms(*d));
        }
    });
}
