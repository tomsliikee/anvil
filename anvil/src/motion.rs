//! The motion values of Anvil, in one place: two durations and one
//! ease-out curve for everything that moves in the shell (panel reveal,
//! popovers, tab switch, hover in the stylesheet). The springs are the
//! ones Islands uses, so Anvil, Atlas and Islands feel like one family.
//! Everything here honours GTK's `gtk-enable-animations`.

use adw::prelude::*;

/// Hover, popovers and the Ctrl+Tab switcher.
pub const FAST_MS: u32 = 150;
/// A state change: panel reveal, tab switch, start page to workspace.
pub const STATE_MS: u32 = 200;
/// The one curve (the stylesheet's `ease-out`).
pub const EASING: adw::Easing = adw::Easing::EaseOutCubic;

/// Damping ratio and stiffness, mass 1.
#[derive(Clone, Copy, Debug)]
pub struct Spring {
    pub damping: f64,
    pub stiffness: f64,
}

/// Camera moves (fly to a table, settle after a fling): no overshoot, a map
/// that wobbles past its target reads as a mistake.
pub const CAMERA: Spring = Spring { damping: 1.0, stiffness: 260.0 };

/// Whether the desktop wants animations (Settings → Accessibility →
/// Reduce Animation sets `gtk-enable-animations` to false).
pub fn enabled() -> bool {
    gtk::Settings::default().is_none_or(|s| s.is_gtk_enable_animations())
}

/// Fades `widget` in from `from` to full opacity. libadwaita skips the
/// animation itself when animations are off.
pub fn fade_in(widget: &impl IsA<gtk::Widget>, from: f64, ms: u32) {
    let w = widget.as_ref();
    if !enabled() {
        w.set_opacity(1.0);
        return;
    }
    let target = adw::PropertyAnimationTarget::new(w, "opacity");
    let anim = adw::TimedAnimation::builder().widget(w).value_from(from).value_to(1.0).duration(ms).easing(EASING).target(&target).build();
    anim.play();
}

#[cfg(test)]
mod tests {
    /// The stylesheet uses only the durations above, so hover and state
    /// changes feel the same everywhere.
    #[test]
    fn stylesheet_uses_only_motion_durations() {
        let css = include_str!("../../data/style.template.css");
        let allowed = [super::FAST_MS, super::STATE_MS];
        for (i, _) in css.match_indices("ms") {
            let digits: String = css[..i].chars().rev().take_while(|c| c.is_ascii_digit()).collect::<Vec<_>>().into_iter().rev().collect();
            if let Ok(ms) = digits.parse::<u32>() {
                assert!(allowed.contains(&ms), "{ms}ms in style.template.css is not a motion duration");
            }
        }
    }
}
