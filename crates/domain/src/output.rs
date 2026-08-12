use std::fmt;

use serde::{Deserialize, Serialize};

/// Denominator of `wp_fractional_scale_v1` ratios.
pub const SCALE_DENOMINATOR: u32 = 120;

/// Stable identity of a monitor: the connector name the compositor reports, e.g. `DP-1`.
///
/// Comparison is case sensitive. Compositors report connector names verbatim and the
/// config file keys them the same way, so case folding here would only hide typos.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct OutputId(String);

impl OutputId {
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for OutputId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for OutputId {
    fn from(name: &str) -> Self {
        Self::new(name)
    }
}

impl From<String> for OutputId {
    fn from(name: String) -> Self {
        Self(name)
    }
}

/// Surface scale as a numerator over [`SCALE_DENOMINATOR`], which is exactly how the
/// fractional scale protocol expresses it. Kept integral so equality is exact.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Scale(u32);

impl Scale {
    pub const ONE: Scale = Scale(SCALE_DENOMINATOR);

    /// From `wp_fractional_scale_v1::preferred_scale`, which is already in 120ths.
    pub fn from_120ths(numerator: u32) -> Self {
        Self(numerator.max(1))
    }

    /// From `wl_output::scale`, the integer fallback when fractional scaling is absent.
    pub fn from_integer(factor: u32) -> Self {
        Self(factor.max(1) * SCALE_DENOMINATOR)
    }

    pub fn numerator(self) -> u32 {
        self.0
    }

    pub fn as_f64(self) -> f64 {
        f64::from(self.0) / f64::from(SCALE_DENOMINATOR)
    }
}

impl Default for Scale {
    fn default() -> Self {
        Self::ONE
    }
}

impl fmt::Display for Scale {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_f64())
    }
}

/// Size in the compositor's coordinate space, before scaling.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogicalSize {
    pub w: u32,
    pub h: u32,
}

/// Size of an actual buffer, in device pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PixelSize {
    pub w: u32,
    pub h: u32,
}

impl LogicalSize {
    pub fn new(w: u32, h: u32) -> Self {
        Self { w, h }
    }

    pub fn is_empty(self) -> bool {
        self.w == 0 || self.h == 0
    }

    /// Rounds up, so a fractional scale never leaves the surface a pixel short.
    pub fn to_pixels(self, scale: Scale) -> PixelSize {
        PixelSize { w: scale_up(self.w, scale), h: scale_up(self.h, scale) }
    }
}

impl PixelSize {
    pub fn new(w: u32, h: u32) -> Self {
        Self { w, h }
    }

    pub fn is_empty(self) -> bool {
        self.w == 0 || self.h == 0
    }

    pub fn area(self) -> u64 {
        u64::from(self.w) * u64::from(self.h)
    }

    /// At least one pixel on each axis. Native surfaces and textures reject zero, and a
    /// monitor can legitimately report an empty size while it is being configured.
    pub fn max_one(self) -> Self {
        Self { w: self.w.max(1), h: self.h.max(1) }
    }

    /// The smallest size covering both, used when two outputs share one wallpaper.
    pub fn union(self, other: Self) -> Self {
        Self { w: self.w.max(other.w), h: self.h.max(other.h) }
    }

    /// Whether something this large is enough for a request for `needed`.
    pub fn covers(self, needed: Self) -> bool {
        self.w >= needed.w && self.h >= needed.h
    }
}

fn scale_up(value: u32, scale: Scale) -> u32 {
    let numerator = u64::from(value) * u64::from(scale.numerator());
    let denominator = u64::from(SCALE_DENOMINATOR);
    numerator.div_ceil(denominator) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integer_scale_is_expressed_in_120ths() {
        assert_eq!(Scale::from_integer(2).numerator(), 240);
        assert_eq!(Scale::ONE.as_f64(), 1.0);
    }

    #[test]
    fn zero_scale_never_survives() {
        assert_eq!(Scale::from_120ths(0).numerator(), 1);
        assert_eq!(Scale::from_integer(0), Scale::ONE);
    }

    #[test]
    fn buffer_size_rounds_up_under_fractional_scale() {
        let logical = LogicalSize::new(2048, 1280);
        let scale = Scale::from_120ths(150);
        assert_eq!(logical.to_pixels(scale), PixelSize::new(2560, 1600));
    }

    #[test]
    fn covering_is_per_axis_and_not_by_area() {
        let have = PixelSize::new(2560, 1440);
        assert!(have.covers(PixelSize::new(2560, 1440)));
        assert!(have.covers(PixelSize::new(1000, 1000)));
        assert!(!have.covers(PixelSize::new(1000, 1441)), "a taller need is not covered");
    }

    #[test]
    fn buffer_size_never_truncates() {
        let logical = LogicalSize::new(1001, 1);
        let scale = Scale::from_120ths(150);
        assert_eq!(logical.to_pixels(scale), PixelSize::new(1252, 2));
    }
}
