use serde::{Deserialize, Serialize};

use crate::anim::{Animated, Motion};

/// Where both axes sit, as the share of the headroom the live zoom leaves that the image
/// is offset from its centre by. `0` is centred and `-0.5` and `0.5` are the two edges.
#[derive(Clone, Copy, Debug)]
pub struct ScrollState {
    pub v: Animated,
    pub h: Animated,
}

impl ScrollState {
    pub fn new() -> Self {
        Self { v: Animated::new(0.0), h: Animated::new(0.0) }
    }

    pub fn tick(&mut self, dt: f32) -> Motion {
        self.v.tick(dt) | self.h.tick(dt)
    }
}

impl Default for ScrollState {
    fn default() -> Self {
        Self::new()
    }
}

/// One scroll axis as a backend reads it off its compositor.
///
/// `stride` is the largest single discontinuous move the axis makes, in the same units as
/// `at`: `1 / (count - 1)` where it moves in stops, and `0` where it pans continuously.
/// Policy divides `max-shift` by it and by the image's travel to get a share, once,
/// before anything retargets.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stop {
    pub at: f32,
    pub stride: f32,
}

impl Stop {
    /// Centred with nothing to travel between, which is where an axis sits until the
    /// compositor reports a position.
    pub const CENTRED: Stop = Stop { at: 0.5, stride: 0.0 };

    /// One reported reading, brought into range. NaN differs from itself, so left alone it
    /// would look like movement on every report.
    pub fn read(at: f32, stride: f32) -> Self {
        Self {
            at: if at.is_nan() { Stop::CENTRED.at } else { at.clamp(0.0, 1.0) },
            stride: if stride.is_nan() { 0.0 } else { stride.clamp(0.0, 1.0) },
        }
    }
}

impl Default for Stop {
    fn default() -> Self {
        Self::CENTRED
    }
}
