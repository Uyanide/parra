use crate::anim::{Animated, Motion};

/// Zoom level of an output, normalized to `1` for no zoom.
#[derive(Clone, Copy, Debug, Default)]
pub struct ZoomState {
    pub factor: Animated,
}

impl ZoomState {
    pub fn new(zoom: f32) -> Self {
        Self { factor: Animated::new(zoom) }
    }

    pub fn tick(&mut self, dt: f32) -> Motion {
        self.factor.tick(dt)
    }
}
