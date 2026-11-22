//! The motion values of Anvil, in one place. Durations follow the shared
//! guideline (120 ms hover, 200 ms state, 320 ms layout); the springs are the
//! ones Islands uses, so Anvil, Atlas and Islands feel like one family.

pub const STATE_MS: u32 = 200;

/// Damping ratio and stiffness, mass 1.
#[derive(Clone, Copy, Debug)]
pub struct Spring {
    pub damping: f64,
    pub stiffness: f64,
}

/// Camera moves (fly to a table, settle after a fling): no overshoot, a map
/// that wobbles past its target reads as a mistake.
pub const CAMERA: Spring = Spring { damping: 1.0, stiffness: 260.0 };



/// Wheel and Page Up/Down scrolling in the editor: short enough that a
/// fast wheel never trails behind, long enough to follow the text.
pub const SCROLL_MS: f64 = 150.0;

/// The caret sliding to its new place (VS Code's smooth caret is as quick).
pub const CARET_MOVE_MS: f64 = 80.0;

/// The caret's fade in and out when it blinks.
pub const CARET_FADE_MS: f64 = 150.0;

/// The one ease-out curve (cubic), for `t` from 0 to 1.
pub fn ease_out(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

/// How the caret blinks: GTK's cycle (`gtk-cursor-blink-time`, visible for
/// two thirds of it) and its timeout, after which it stays visible.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Blink {
    pub on: bool,
    pub cycle_ms: f64,
    pub timeout_ms: f64,
}

impl Blink {
    fn fade(&self) -> f64 {
        CARET_FADE_MS.min(self.cycle_ms / 6.0)
    }

    fn visible_ms(&self) -> f64 {
        self.cycle_ms * 2.0 / 3.0
    }

    /// The caret's opacity `elapsed` ms after it last moved: fully visible,
    /// fading out, hidden, fading in.
    pub fn alpha(&self, elapsed: f64) -> f64 {
        if !self.on || elapsed >= self.timeout_ms || self.cycle_ms <= 0.0 {
            return 1.0;
        }
        let (p, f, v, c) = (elapsed.max(0.0) % self.cycle_ms, self.fade(), self.visible_ms(), self.cycle_ms);
        if p < v - f {
            1.0
        } else if p < v {
            1.0 - (p - (v - f)) / f
        } else if p < c - f {
            0.0
        } else {
            (p - (c - f)) / f
        }
    }

    /// When the opacity next starts to change (ms since the last move);
    /// `elapsed` itself while it is changing, `None` once it holds for good.
    pub fn next_change(&self, elapsed: f64) -> Option<f64> {
        if !self.on || elapsed >= self.timeout_ms || self.cycle_ms <= 0.0 {
            return None;
        }
        let (p, f, v, c) = (elapsed.max(0.0) % self.cycle_ms, self.fade(), self.visible_ms(), self.cycle_ms);
        let at = if p < v - f {
            elapsed + (v - f - p)
        } else if p >= v && p < c - f {
            elapsed + (c - f - p)
        } else {
            elapsed
        };
        Some(at.min(self.timeout_ms))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const B: Blink = Blink { on: true, cycle_ms: 1200.0, timeout_ms: 10_000.0 };

    #[test]
    fn caret_is_solid_then_fades_out_hides_and_fades_in() {
        assert_eq!(B.alpha(0.0), 1.0);
        assert_eq!(B.alpha(600.0), 1.0);
        assert!((B.alpha(725.0) - 0.5).abs() < 1e-9);
        assert_eq!(B.alpha(900.0), 0.0);
        assert!((B.alpha(1125.0) - 0.5).abs() < 1e-9);
        assert_eq!(B.alpha(1200.0), 1.0);
    }

    #[test]
    fn caret_stops_blinking_visible_after_the_timeout_or_when_off() {
        assert_eq!(B.alpha(10_900.0), 1.0);
        assert_eq!(B.next_change(10_000.0), None);
        let off = Blink { on: false, ..B };
        assert_eq!(off.alpha(900.0), 1.0);
        assert_eq!(off.next_change(0.0), None);
    }

    #[test]
    fn next_change_skips_the_still_parts() {
        assert_eq!(B.next_change(100.0), Some(650.0));
        assert_eq!(B.next_change(700.0), Some(700.0));
        assert_eq!(B.next_change(850.0), Some(1050.0));
    }

    #[test]
    fn ease_out_starts_fast_and_ends_at_one() {
        assert_eq!(ease_out(0.0), 0.0);
        assert_eq!(ease_out(1.0), 1.0);
        assert_eq!(ease_out(2.0), 1.0);
        assert!(ease_out(0.5) > 0.5);
    }
}
