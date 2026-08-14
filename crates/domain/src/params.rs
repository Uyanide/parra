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
    pub overview: OverviewParams,
    pub transition: TransitionParams,
}

/// The two parallax axes, each configured separately.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScrollParams {
    /// Follows the active workspace.
    pub vertical: AxisParams,
    /// Follows the column in the scrolling layout.
    pub horizontal: AxisParams,
}

impl Default for ScrollParams {
    fn default() -> Self {
        Self {
            vertical: AxisParams::default(),
            horizontal: AxisParams { enabled: false, ..AxisParams::default() },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AxisParams {
    /// When false the image is pinned to its centre on this axis.
    pub enabled: bool,
    /// Fraction of the available travel to use, about the centre. 0 pins to centre.
    pub travel: f32,
    pub tween: Tween,
}

impl Default for AxisParams {
    fn default() -> Self {
        Self { enabled: true, travel: 1.0, tween: Tween::new(0.4, Easing::OutCubic) }
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
            tint: Rgba::from_bytes([0x1e, 0x1e, 0x2e, 0xff]),
            tint_opacity: 0.5,
            tween: Tween::new(0.4, Easing::InOutCubic),
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

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OverviewParams {
    /// Fraction of the image visible when the overview is closed. Below 1 it leaves
    /// headroom for the parallax to travel through.
    pub crop_ratio: f32,
    pub tween: Tween,
}

impl Default for OverviewParams {
    fn default() -> Self {
        Self { crop_ratio: 0.9, tween: Tween::new(0.4, Easing::OutCubic) }
    }
}

impl OverviewParams {
    pub fn zoom(&self) -> f32 {
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
        Self { mode: TransitionMode::None, tween: Tween::new(0.4, Easing::InOutCubic) }
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
    #[default]
    None,
    /// The outgoing wallpaper is kept and faded out rather than dropped at once.
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
    fn zoom_is_the_reciprocal_of_the_crop_ratio() {
        let params = OverviewParams { crop_ratio: 0.5, ..OverviewParams::default() };
        assert_eq!(params.zoom(), 2.0);
    }

    #[test]
    fn a_full_crop_ratio_means_no_zoom() {
        let params = OverviewParams { crop_ratio: 1.0, ..OverviewParams::default() };
        assert_eq!(params.zoom(), 1.0);
    }

    #[test]
    fn zoom_stays_finite_for_a_degenerate_crop_ratio() {
        let params = OverviewParams { crop_ratio: 0.0, ..OverviewParams::default() };
        assert!(params.zoom().is_finite());
    }

    #[test]
    fn tint_opacity_scales_the_alpha() {
        let params = BlurParams { tint_opacity: 0.5, ..BlurParams::default() };
        assert_eq!(params.effective_tint().to_bytes()[3], 128);
    }
}
