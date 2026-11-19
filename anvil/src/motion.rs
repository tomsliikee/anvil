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


