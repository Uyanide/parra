use std::collections::BTreeMap;

use domain::{Easing, Layer, Rgba, TransitionMode};
use serde::Deserialize;

/// The TOML surface, one type per table.
///
/// Every leaf is optional so that merging is uniform: an absent key means "inherit".
/// The defaults themselves live in `domain`, so the file format and the meaning of a
/// value cannot drift apart.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct ConfigFile {
    #[serde(default)]
    pub general: GeneralSection,
    #[serde(default)]
    pub wallpaper: WallpaperSection,
    #[serde(default)]
    pub scroll: ScrollSection,
    #[serde(default)]
    pub blur: BlurSection,
    #[serde(default)]
    pub overview: OverviewSection,
    #[serde(default)]
    pub transition: TransitionSection,
    #[serde(default)]
    pub output: BTreeMap<String, OutputSection>,
}

/// Properties of the layer surface. Global: a namespace that varied between monitors
/// would break the compositor rules that match on it.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct GeneralSection {
    pub namespace: Option<String>,
    pub layer: Option<Layer>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct WallpaperSection {
    /// What to show when the control socket has not asked for anything.
    pub fallback: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct ScrollSection {
    #[serde(default)]
    pub vertical: AxisSection,
    #[serde(default)]
    pub horizontal: AxisSection,
}

/// One parallax axis. The two are configured apart because the compositor animates the
/// workspace switch and the column move apart.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct AxisSection {
    pub enabled: Option<bool>,
    pub travel: Option<f32>,
    pub duration_ms: Option<u32>,
    pub easing: Option<Easing>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct BlurSection {
    pub radius: Option<u32>,
    pub downscale: Option<u32>,
    pub tint: Option<Rgba>,
    pub tint_opacity: Option<f32>,
    pub duration_ms: Option<u32>,
    pub easing: Option<Easing>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct OverviewSection {
    pub crop_ratio: Option<f32>,
    pub duration_ms: Option<u32>,
    pub easing: Option<Easing>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct TransitionSection {
    pub mode: Option<TransitionMode>,
    pub duration_ms: Option<u32>,
    pub easing: Option<Easing>,
}

/// A `[output."DP-1"]` table: the global sections that can vary per monitor.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct OutputSection {
    #[serde(default)]
    pub wallpaper: WallpaperSection,
    #[serde(default)]
    pub scroll: ScrollSection,
    #[serde(default)]
    pub blur: BlurSection,
    #[serde(default)]
    pub overview: OverviewSection,
    #[serde(default)]
    pub transition: TransitionSection,
}
