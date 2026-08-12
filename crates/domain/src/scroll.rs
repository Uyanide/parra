use crate::anim::{Animated, Motion};

/// Parallax position on both axes, normalized to `0..=1` of the available travel.
///
/// The vertical axis follows the workspace, the horizontal one the column.
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
