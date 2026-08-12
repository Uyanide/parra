use crate::anim::{Animated, Motion};

/// How blurred an output currently is.
///
/// Only the mix factor is state. Radius, tint and downscale describe how the blur was
/// baked, so they stay in the resolved parameters.
#[derive(Clone, Copy, Debug, Default)]
pub struct BlurState {
    /// 0 samples the sharp texture, 1 the baked blurred one. Also scales the tint.
    pub amount: Animated,
}

impl BlurState {
    pub fn new() -> Self {
        Self { amount: Animated::new(0.0) }
    }

    pub fn tick(&mut self, dt: f32) -> Motion {
        self.amount.tick(dt)
    }
}
