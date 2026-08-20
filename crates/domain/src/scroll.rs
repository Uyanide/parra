use serde::{Deserialize, Serialize};

use crate::anim::{Animated, Motion};

/// Parallax position on both axes, normalized to `0..=1` of the available travel.
#[derive(Clone, Copy, Debug)]
pub struct ScrollState {
    pub v: Animated,
    pub h: Animated,
}

impl ScrollState {
    /// Centred, which is where an output sits until the compositor reports a position.
    pub const CENTRE: f32 = 0.5;

    pub fn new() -> Self {
        Self { v: Animated::new(Self::CENTRE), h: Animated::new(Self::CENTRE) }
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
/// `at`: `1 / (count - 1)` where it moves in stops, and `0` where it pans continuously and
/// therefore never jumps. Nothing reads it but the shift cap, which a `0` lifts.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stop {
    pub at: f32,
    pub stride: f32,
}

impl Stop {
    /// Centred with nothing to travel between, which is where an axis sits until the
    /// compositor reports a position.
    pub const CENTRED: Stop = Stop { at: ScrollState::CENTRE, stride: 0.0 };

    /// One reported reading, brought into range. NaN differs from itself, so left alone it
    /// would look like movement on every report.
    pub fn read(at: f32, stride: f32) -> Self {
        Self {
            at: if at.is_nan() { ScrollState::CENTRE } else { at.clamp(0.0, 1.0) },
            stride: if stride.is_nan() { 0.0 } else { stride.clamp(0.0, 1.0) },
        }
    }
}

impl Default for Stop {
    fn default() -> Self {
        Self::CENTRED
    }
}

/// How far one stop of each axis is, kept beside the animations rather than among them.
///
/// Not animated and not a target: it describes what is driving the axis, not somewhere the
/// axis is heading.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Stride {
    pub v: f32,
    pub h: f32,
}
