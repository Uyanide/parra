use serde::{Deserialize, Serialize};

use crate::anim::{Easing, Tween};
use crate::color::Rgba;
use crate::wallpaper::WallpaperRef;

/// Smallest crop ratio the geometry stays well behaved at. Smaller ratio results in
/// larger zoom factors, larger size limits, and therefore possibly more resource
/// consumption.
pub const MIN_CROP_RATIO: f32 = 0.25;

/// One output's settings, fully resolved: nothing here is optional and nothing is still
/// inherited from elsewhere.
///
/// The semantic form rather than the text one:
///   - durations in seconds
///   - colours parsed
///   - paths expanded.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OutputParams {
    /// What to show when nothing more specific was asked for. Named for the config key
    /// it comes from, so the two cannot drift apart.
    pub fallback: Option<WallpaperRef>,
    pub scroll: ScrollParams,
    pub blur: BlurParams,
    pub zoom: ZoomParams,
    pub transition: TransitionParams,
}

/// The two parallax axes, each configured separately.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ScrollParams {
    pub vertical: AxisParams,
    pub horizontal: AxisParams,
}

/// How one axis answers the position its channel is driven to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AxisParams {
    /// Fraction of the available travel to use, about the centre. 0 pins it there.
    pub travel: f32,
    /// Whether the axis runs the other way, so the first stop sits where the last one
    /// otherwise would.
    ///
    /// Its own field rather than a sign on `travel`, so that an output overriding how far
    /// an axis moves does not have to restate which way it moves, and so that the shift
    /// cap goes on reading a positive stop length.
    pub invert: bool,
    /// Greatest distance the image may move between two adjacent stops, in screen extents
    /// of this axis. `None` lifts the cap.
    ///
    /// Resolved into a share of the travel in `policy::axis`, beside `travel`, because a
    /// share is what an animation can be run on: a distance would have to be measured
    /// against a headroom that the zoom is itself moving.
    pub max_shift: Option<f32>,
    pub tween: Tween,
}

/// Furthest a shift cap can usefully be asked to allow. Past the available travel a cap
/// does nothing, so the bound only exists to stop a number written in pixels reading as no
/// cap at all.
pub const MAX_SHIFT: f32 = 16.0;

impl Default for AxisParams {
    fn default() -> Self {
        Self {
            travel: 1.0,
            invert: false,
            max_shift: Some(0.3),
            tween: Tween::new(0.3, Easing::OutCubic),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlurParams {
    pub radius: u32,
    /// Linear downscale factor of the baked blur texture. Blur removes the high
    /// frequencies that downsampling would have cost anyway.
    pub downscale: u32,
    pub tint: Rgba,
    pub tint_opacity: f32,
    pub tween: Tween,
}

impl Default for BlurParams {
    fn default() -> Self {
        Self {
            radius: 32,
            downscale: 4,
            tint: Rgba::from_bytes([0x10, 0x10, 0x10, 0xff]),
            tint_opacity: 0.5,
            tween: Tween::new(0.3, Easing::InOutCubic),
        }
    }
}

impl BlurParams {
    /// Tint as the shader wants it, with the configured opacity already folded in.
    pub fn effective_tint(&self) -> Rgba {
        self.tint.with_alpha_scaled(self.tint_opacity)
    }

    pub fn is_enabled(&self) -> bool {
        self.radius > 0
    }
}

/// How far in the wallpaper sits while its output is not driven to zoom out.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ZoomParams {
    /// Fraction of the image visible when zoomed in. Below 1 it leaves headroom for the
    /// parallax to travel through.
    pub crop_ratio: f32,
    pub tween: Tween,
}

impl Default for ZoomParams {
    fn default() -> Self {
        Self { crop_ratio: 0.8, tween: Tween::new(0.3, Easing::OutCubic) }
    }
}

impl ZoomParams {
    /// The multiplier the renderer wants, which is what cropping to a ratio comes to.
    pub fn factor(&self) -> f32 {
        1.0 / self.crop_ratio.clamp(MIN_CROP_RATIO, 1.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TransitionParams {
    pub mode: TransitionMode,
    pub tween: Tween,
}

impl Default for TransitionParams {
    fn default() -> Self {
        Self { mode: TransitionMode::Fade, tween: Tween::new(0.8, Easing::InOutCubic) }
    }
}

impl TransitionParams {
    /// Swaps with no animation, whatever mode is configured.
    pub const INSTANT: TransitionParams =
        TransitionParams { mode: TransitionMode::None, tween: Tween::INSTANT };

    /// The tween actually used, which is instant for a mode that swaps outright.
    pub fn effective_tween(&self) -> Tween {
        match self.mode {
            TransitionMode::None => Tween::INSTANT,
            TransitionMode::Fade => self.tween,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TransitionMode {
    None,
    /// The outgoing wallpaper is kept and faded out.
    #[default]
    Fade,
}

/// Properties of the layer surface itself. Global rather than per output: a namespace
/// that differed between monitors would defeat the compositor rules that key on it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SurfaceParams {
    pub namespace: String,
    pub layer: Layer,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Layer {
    #[default]
    Background,
    Bottom,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_zoom_factor_is_the_reciprocal_of_the_crop_ratio() {
        let params = ZoomParams { crop_ratio: 0.5, ..ZoomParams::default() };
        assert_eq!(params.factor(), 2.0);
    }

    #[test]
    fn a_full_crop_ratio_means_no_zoom() {
        let params = ZoomParams { crop_ratio: 1.0, ..ZoomParams::default() };
        assert_eq!(params.factor(), 1.0);
    }

    #[test]
    fn the_zoom_factor_stays_finite_for_a_degenerate_crop_ratio() {
        let params = ZoomParams { crop_ratio: 0.0, ..ZoomParams::default() };
        assert!(params.factor().is_finite());
    }

    #[test]
    fn tint_opacity_scales_the_alpha() {
        let params = BlurParams { tint_opacity: 0.5, ..BlurParams::default() };
        assert_eq!(params.effective_tint().to_bytes()[3], 128);
    }
}
