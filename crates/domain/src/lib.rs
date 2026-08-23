//! The state model the rest of the daemon agrees on, and the policy that turns
//! driven channels into animation targets.

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
pub use geometry::{Limit, Limits, UvRect, Zoom, sample_rect};
pub use monitor::{MonitorState, Moves};
pub use output::{LogicalSize, OutputId, PixelSize, SCALE_DENOMINATOR, Scale};
pub use params::{
    AxisParams, BlurParams, Layer, MAX_SHIFT, OutputParams, ScrollParams, SurfaceParams,
    TransitionMode, TransitionParams, ZoomParams,
};
pub use policy::{Channels, Driven, Signals, Targets, wallpaper_for};
pub use scroll::{ScrollState, Stop, Stride};
pub use wallpaper::{Swap, WallpaperRef, WallpaperSlot};
pub use zoom::ZoomState;
