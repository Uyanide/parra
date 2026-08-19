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
    pub zoom: ZoomSection,
    #[serde(default)]
    pub transition: TransitionSection,
    /// The running compositor's own settings, carried rather than read.
    ///
    /// Which keys belong here depends on which compositor the file is for, so the
    /// backend for that compositor is what gives them meaning.
    #[serde(default)]
    pub compositor: toml::Table,
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

/// One parallax axis. The two are configured apart because a compositor animates the two
/// movements apart, and one shared curve could only ever match one of them.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct AxisSection {
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
pub struct ZoomSection {
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
    /// Overrides for this monitor, over the file's own `[compositor]` section.
    #[serde(default)]
    pub compositor: toml::Table,
    #[serde(default)]
    pub scroll: ScrollSection,
    #[serde(default)]
    pub blur: BlurSection,
    #[serde(default)]
    pub zoom: ZoomSection,
    #[serde(default)]
    pub transition: TransitionSection,
}
