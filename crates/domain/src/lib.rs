//! Shared state model: identities, geometry, animation, resolved parameters and the
//! policy that turns compositor facts into animation targets.
//!
//! Depends only on `serde`, and holds no clock: elapsed time is fed to `tick` from
//! outside, which is what makes animation and policy testable without a display.

pub mod anim;
pub mod blur;
pub mod color;
pub mod geometry;
pub mod monitor;
pub mod output;
pub mod params;
pub mod policy;
pub mod scroll;
pub mod wallpaper;

pub use anim::{Animated, Easing, Motion, Tween};
pub use blur::BlurState;
pub use color::Rgba;
pub use geometry::{UvRect, sample_rect};
pub use monitor::MonitorState;
pub use output::{LogicalSize, OutputId, PixelSize, SCALE_DENOMINATOR, Scale};
pub use params::{
    AxisParams, BlurParams, Layer, OutputParams, OverviewParams, ScrollParams, SurfaceParams,
    TransitionMode, TransitionParams,
};
pub use policy::{Facts, Index, OutputFacts, Signals, Targets};
pub use scroll::ScrollState;
pub use wallpaper::{WallpaperRef, WallpaperSlot};
