//! The state model the rest of the daemon agrees on, and the policy that turns
//! compositor facts into animation targets.

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
pub mod zoom;

pub use anim::{Animated, Easing, Motion, Move, Tween};
pub use blur::BlurState;
pub use color::Rgba;
pub use geometry::{UvRect, sample_rect};
pub use monitor::{MonitorState, Moves};
pub use output::{LogicalSize, OutputId, PixelSize, SCALE_DENOMINATOR, Scale};
pub use params::{
    AxisParams, BlurParams, Layer, OutputParams, OverviewParams, ScrollParams, SurfaceParams,
    TransitionMode, TransitionParams,
};
pub use policy::{Facts, Index, OutputFacts, Signals, Targets, wallpaper_for};
pub use scroll::ScrollState;
pub use wallpaper::{Swap, WallpaperRef, WallpaperSlot};
pub use zoom::ZoomState;
