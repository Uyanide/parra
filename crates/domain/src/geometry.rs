use crate::output::PixelSize;

/// Region of the source image that fills the viewport, in normalized image coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UvRect {
    pub u0: f32,
    pub v0: f32,
    pub u1: f32,
    pub v1: f32,
}

impl UvRect {
    pub const FULL: UvRect = UvRect { u0: 0.0, v0: 0.0, u1: 1.0, v1: 1.0 };

    pub fn width(self) -> f32 {
        self.u1 - self.u0
    }

    pub fn height(self) -> f32 {
        self.v1 - self.v0
    }
}

/// What bounds one axis beyond the headroom the cover fit and the zoom leave it.
///
/// No `Default`: uncapped already has a name in [`Limits::NONE`], and a second spelling of
/// it would only be a way to disagree with itself.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Limit {
    /// Distance between two adjacent stops, as a fraction of the range the axis is
    /// driven over. `0` is a channel that pans continuously and so never jumps.
    pub stride: f32,
    /// Greatest distance the image may move between two adjacent stops, in screen
    /// extents of this axis, measured at the deepest zoom the output reaches. `None`
    /// lifts the cap.
    pub max_shift: Option<f32>,
}

/// Both axes' limits.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Limits {
    pub h: Limit,
    pub v: Limit,
}

impl Limits {
    /// Neither axis capped, which is what the geometry did before there was a cap.
    pub const NONE: Limits = Limits {
        h: Limit { stride: 0.0, max_shift: None },
        v: Limit { stride: 0.0, max_shift: None },
    };
}

/// Where the zoom is, beside the deepest the same output reaches.
///
/// - `at` sizes the sampled rect.
/// - `deepest` is where the shift cap is measured, which holds the cap still while `at`
///   animates. See `docs/architecture.md`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Zoom {
    pub at: f32,
    pub deepest: f32,
}

impl Zoom {
    /// An output that is not animating, where the two are one number.
    pub const fn fixed(zoom: f32) -> Self {
        Self { at: zoom, deepest: zoom }
    }
}

/// Fraction of one axis's travel the cap leaves.
///
/// `span / visible` is that travel measured in screens, which is the unit `max_shift` is
/// written in, so one stop of it comes to `span / visible * stride`. Capping that distance
/// scales the whole mapping by the same factor, which is why the answer is a fraction of
/// the travel rather than a clamp on the position.
///
/// Three cases leave the whole of it: no cap asked for, an axis that never jumps, and one
/// with nothing on screen to measure a screen by.
fn allowed_fraction(span: f32, visible: f32, limit: Limit) -> f32 {
    let Some(max_shift) = limit.max_shift else { return 1.0 };
    if limit.stride <= 0.0 || visible <= 0.0 {
        return 1.0;
    }
    let shift = span / visible * limit.stride;
    if shift <= max_shift { 1.0 } else { max_shift / shift }
}

/// A zoom as the geometry reads it. The cover is already the largest rect that keeps the
/// viewport's aspect, so anything below `1` or non-finite counts as `1`.
fn factor(zoom: f32) -> f32 {
    if zoom.is_finite() { zoom.max(1.0) } else { 1.0 }
}

/// Extent of the sampled rect on one axis, as a fraction of the image.
fn visible(cover: f32, zoom: f32) -> f32 {
    (cover / zoom).clamp(f32::MIN_POSITIVE, 1.0)
}

/// Fits the image over the viewport, zooms in by `zoom.at`, then pans within whatever
/// headroom that leaves.
///
/// - The fit always covers: the image is scaled until it spans both axes, so no
///   letterbox can appear.
/// - The travel is what the cover leaves outside the viewport, plus what `zoom.at` adds,
///   shortened by the fraction [`Limits`] allows at `zoom.deepest`.
/// - `scroll_h` and `scroll_v` are fractions of that travel, `0` at the left or top edge
///   and `1` at the right or bottom.
/// - An axis with no headroom ignores its value.
pub fn sample_rect(
    image: PixelSize,
    viewport: PixelSize,
    zoom: Zoom,
    scroll_h: f32,
    scroll_v: f32,
    limits: Limits,
) -> UvRect {
    if image.is_empty() || viewport.is_empty() {
        return UvRect::FULL;
    }

    let image_aspect = image.w as f32 / image.h as f32;
    let viewport_aspect = viewport.w as f32 / viewport.h as f32;
    let (cover_w, cover_h) = if image_aspect > viewport_aspect {
        (viewport_aspect / image_aspect, 1.0)
    } else {
        (1.0, image_aspect / viewport_aspect)
    };

    let at = factor(zoom.at);
    let w = visible(cover_w, at);
    let h = visible(cover_h, at);

    // The deepest zoom is where a stop moves furthest, so a fraction taken there holds the
    // cap at every zoom, and a zoom animation only ever scales it.
    let deepest = factor(zoom.deepest).max(at);
    let w_deepest = visible(cover_w, deepest);
    let h_deepest = visible(cover_h, deepest);

    // Shortened about the centre rather than from one edge, so a cap moves the wallpaper
    // less without also sliding it toward the top or the left.
    let span_h = 1.0 - w;
    let span_v = 1.0 - h;
    let allowed_h = span_h * allowed_fraction(1.0 - w_deepest, w_deepest, limits.h);
    let allowed_v = span_v * allowed_fraction(1.0 - h_deepest, h_deepest, limits.v);

    let u0 = (span_h - allowed_h) * 0.5 + allowed_h * scroll_h.clamp(0.0, 1.0);
    let v0 = (span_v - allowed_v) * 0.5 + allowed_v * scroll_v.clamp(0.0, 1.0);
    UvRect { u0, v0, u1: u0 + w, v1: v0 + h }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPS: f32 = 1e-5;

    fn assert_close(actual: f32, expected: f32, what: &str) {
        assert!((actual - expected).abs() < EPS, "{what}: {actual} != {expected}");
    }

    #[test]
    fn a_matching_aspect_at_zoom_one_uses_the_whole_image() {
        let rect = sample_rect(
            PixelSize::new(2560, 1440),
            PixelSize::new(2560, 1440),
            Zoom::fixed(1.0),
            0.5,
            0.5,
            Limits::NONE,
        );
        assert_eq!(rect, UvRect::FULL);
    }

    #[test]
    fn a_taller_image_leaves_vertical_headroom() {
        let rect = sample_rect(
            PixelSize::new(1000, 2000),
            PixelSize::new(1000, 1000),
            Zoom::fixed(1.0),
            0.5,
            0.5,
            Limits::NONE,
        );
        assert_close(rect.width(), 1.0, "width");
        assert_close(rect.height(), 0.5, "height");
        assert_close(rect.v0, 0.25, "centred vertically");
    }

    #[test]
    fn a_wider_image_leaves_horizontal_headroom() {
        let rect = sample_rect(
            PixelSize::new(2000, 1000),
            PixelSize::new(1000, 1000),
            Zoom::fixed(1.0),
            0.5,
            0.5,
            Limits::NONE,
        );
        assert_close(rect.height(), 1.0, "height");
        assert_close(rect.width(), 0.5, "width");
        assert_close(rect.u0, 0.25, "centred horizontally");
    }

    #[test]
    fn zoom_creates_headroom_where_the_cover_left_none() {
        let square = PixelSize::new(1000, 1000);
        assert_eq!(
            sample_rect(square, square, Zoom::fixed(1.0), 0.5, 0.5, Limits::NONE),
            UvRect::FULL
        );

        let zoomed = sample_rect(square, square, Zoom::fixed(1.0 / 0.9), 0.5, 0.0, Limits::NONE);
        assert_close(zoomed.height(), 0.9, "height");
        assert_close(zoomed.v0, 0.0, "pinned to the top");
    }

    #[test]
    fn scroll_runs_from_the_top_edge_to_the_bottom() {
        let image = PixelSize::new(1000, 2000);
        let viewport = PixelSize::new(1000, 1000);

        let top = sample_rect(image, viewport, Zoom::fixed(1.0), 0.5, 0.0, Limits::NONE);
        assert_close(top.v0, 0.0, "top");
        assert_close(top.v1, 0.5, "top");

        let bottom = sample_rect(image, viewport, Zoom::fixed(1.0), 0.5, 1.0, Limits::NONE);
        assert_close(bottom.v0, 0.5, "bottom");
        assert_close(bottom.v1, 1.0, "bottom");
    }

    #[test]
    fn the_rect_never_leaves_the_image() {
        let image = PixelSize::new(3840, 2160);
        let viewport = PixelSize::new(2560, 1600);
        for zoom in [1.0, 1.111, 4.0] {
            for scroll in [0.0, 0.37, 1.0] {
                let rect =
                    sample_rect(image, viewport, Zoom::fixed(zoom), scroll, scroll, Limits::NONE);
                assert!(rect.u0 >= -EPS && rect.u1 <= 1.0 + EPS, "u out of range at {zoom}");
                assert!(rect.v0 >= -EPS && rect.v1 <= 1.0 + EPS, "v out of range at {zoom}");
            }
        }
    }

    /// The cap is stated in screens, so this image is the one the numbers are worked out
    /// against: 2.226 screen heights of travel at the default crop ratio.
    fn tall() -> (PixelSize, PixelSize) {
        (PixelSize::new(2937, 4796), PixelSize::new(2560, 1440))
    }

    fn capped(stride: f32, max_shift: f32) -> Limits {
        Limits { v: Limit { stride, max_shift: Some(max_shift) }, ..Limits::NONE }
    }

    /// How far the wallpaper moves between the two ends of the axis, in screen heights.
    fn excursion(limits: Limits) -> f32 {
        let (image, viewport) = tall();
        let top = sample_rect(image, viewport, Zoom::fixed(ZOOM), 0.5, 0.0, limits);
        let bottom = sample_rect(image, viewport, Zoom::fixed(ZOOM), 0.5, 1.0, limits);
        (bottom.v0 - top.v0) / top.height()
    }

    const ZOOM: f32 = 1.0 / 0.9;

    #[test]
    fn an_uncapped_axis_travels_everything_the_fit_and_the_zoom_leave() {
        assert_close(excursion(Limits::NONE), 2.2256, "uncapped");
    }

    #[test]
    fn a_cap_shortens_one_adjacent_stop_to_what_it_allows() {
        // One stop is the whole travel at two stops, half of it at three, and so on, so
        // the excursion the cap leaves is the allowance multiplied by the stop count.
        for (stride, stops) in [(1.0, 1.0), (0.5, 2.0), (0.25, 4.0)] {
            let moved = excursion(capped(stride, 0.5));
            assert_close(moved, 0.5 * stops, &format!("stride {stride}"));
            assert_close(moved * stride, 0.5, &format!("one stop at stride {stride}"));
        }
    }

    #[test]
    fn a_stop_already_shorter_than_the_cap_is_left_alone() {
        // Six workspaces put one stop at 0.445 screens, which is inside the allowance.
        assert_close(excursion(capped(0.2, 0.5)), 2.2256, "uncapped");
    }

    #[test]
    fn a_cap_shortens_about_the_centre_rather_than_from_an_edge() {
        let (image, viewport) = tall();
        let limits = capped(1.0, 0.5);
        let centre = sample_rect(image, viewport, Zoom::fixed(ZOOM), 0.5, 0.5, limits);
        assert_eq!(centre, sample_rect(image, viewport, Zoom::fixed(ZOOM), 0.5, 0.5, Limits::NONE));

        let top = sample_rect(image, viewport, Zoom::fixed(ZOOM), 0.5, 0.0, limits);
        let bottom = sample_rect(image, viewport, Zoom::fixed(ZOOM), 0.5, 1.0, limits);
        assert_close(centre.v0 - top.v0, bottom.v0 - centre.v0, "symmetric about the centre");
    }

    #[test]
    fn an_axis_that_never_jumps_is_never_capped() {
        // A continuous channel has no adjacent stop to measure, which a zero stride says.
        assert_close(excursion(capped(0.0, 0.001)), 2.2256, "stride 0");
    }

    #[test]
    fn a_cap_cannot_invent_travel_an_axis_does_not_have() {
        let square = PixelSize::new(1000, 1000);
        let limits = Limits {
            v: Limit { stride: 1.0, max_shift: Some(8.0) },
            h: Limit { stride: 1.0, max_shift: Some(8.0) },
        };
        assert_eq!(sample_rect(square, square, Zoom::fixed(1.0), 0.5, 0.5, limits), UvRect::FULL);
    }

    #[test]
    fn each_axis_is_capped_by_its_own_stride() {
        let image = PixelSize::new(4000, 4000);
        let viewport = PixelSize::new(1000, 1000);
        let limits = Limits {
            h: Limit { stride: 1.0, max_shift: Some(0.25) },
            v: Limit { stride: 0.0, max_shift: Some(0.25) },
        };
        let top_left = sample_rect(image, viewport, Zoom::fixed(2.0), 0.0, 0.0, limits);
        let bottom_right = sample_rect(image, viewport, Zoom::fixed(2.0), 1.0, 1.0, limits);
        let uncapped = sample_rect(image, viewport, Zoom::fixed(2.0), 1.0, 1.0, Limits::NONE);

        assert!(bottom_right.u0 - top_left.u0 < uncapped.u0, "the stepped axis is capped");
        assert_close(bottom_right.v0, uncapped.v0, "the continuous one is not");
    }

    /// The geometry this was reported on: a 3364x2564 image on a 2560x1600 output with two
    /// workspaces puts the zoom the cap starts binding at inside the range the overview
    /// animates over.
    fn straddling() -> (PixelSize, PixelSize) {
        (PixelSize::new(3364, 2564), PixelSize::new(2560, 1600))
    }

    #[test]
    fn a_zoom_animation_stays_between_the_two_ends_it_moves_between() {
        let (image, viewport) = straddling();
        let limits = capped(1.0, 0.3);
        let deepest = ZOOM;
        let centre = |at: f32, scroll: f32| {
            let rect = sample_rect(image, viewport, Zoom { at, deepest }, 0.5, scroll, limits);
            rect.v0 + rect.height() * 0.5
        };

        for scroll in [0.0, 1.0] {
            let (settled_out, settled_in) = (centre(1.0, scroll), centre(deepest, scroll));
            let (low, high) = (settled_out.min(settled_in), settled_out.max(settled_in));
            for step in 0..=64 {
                let at = 1.0 + (deepest - 1.0) * step as f32 / 64.0;
                let seen = centre(at, scroll);
                assert!(
                    seen >= low - EPS && seen <= high + EPS,
                    "zoom {at} at scroll {scroll} left {low}..{high} for {seen}"
                );
            }
        }
    }

    #[test]
    fn one_stop_stays_within_the_cap_at_every_zoom() {
        let (image, viewport) = straddling();
        let limits = capped(1.0, 0.3);
        let deepest = ZOOM;
        for step in 0..=16 {
            let at = 1.0 + (deepest - 1.0) * step as f32 / 16.0;
            let zoom = Zoom { at, deepest };
            let top = sample_rect(image, viewport, zoom, 0.5, 0.0, limits);
            let bottom = sample_rect(image, viewport, zoom, 0.5, 1.0, limits);
            let moved = (bottom.v0 - top.v0) / top.height();
            assert!(moved <= 0.3 + EPS, "{moved} screens at zoom {at}");
        }
    }

    #[test]
    fn the_deepest_zoom_is_where_a_stop_moves_furthest() {
        let (image, viewport) = straddling();
        let limits = capped(1.0, 0.3);
        let deepest = ZOOM;
        let moved = |at: f32| {
            let zoom = Zoom { at, deepest };
            let top = sample_rect(image, viewport, zoom, 0.5, 0.0, limits);
            let bottom = sample_rect(image, viewport, zoom, 0.5, 1.0, limits);
            (bottom.v0 - top.v0) / top.height()
        };
        assert_close(moved(deepest), 0.3, "the cap is met where it is measured");
        assert!(moved(1.0) < moved(deepest), "a shallower zoom moves less");
    }

    #[test]
    fn a_degenerate_size_falls_back_to_the_full_image() {
        let ok = PixelSize::new(100, 100);
        assert_eq!(
            sample_rect(PixelSize::new(0, 100), ok, Zoom::fixed(1.0), 0.5, 0.5, Limits::NONE),
            UvRect::FULL
        );
        assert_eq!(
            sample_rect(ok, PixelSize::new(100, 0), Zoom::fixed(1.0), 0.5, 0.5, Limits::NONE),
            UvRect::FULL
        );
    }

    #[test]
    fn a_zoom_below_one_is_floored_rather_than_zooming_out() {
        let image = PixelSize::new(3840, 2160);
        let viewport = PixelSize::new(2560, 1600);
        let unzoomed = sample_rect(image, viewport, Zoom::fixed(1.0), 0.37, 0.37, Limits::NONE);
        for zoom in [0.5, 0.9, f32::MIN_POSITIVE] {
            assert_eq!(
                sample_rect(image, viewport, Zoom::fixed(zoom), 0.37, 0.37, Limits::NONE),
                unzoomed,
                "zoom {zoom}"
            );
        }
    }

    #[test]
    fn a_nonsensical_zoom_is_ignored_rather_than_propagated() {
        let square = PixelSize::new(100, 100);
        for zoom in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert_eq!(
                sample_rect(square, square, Zoom::fixed(zoom), 0.5, 0.5, Limits::NONE),
                UvRect::FULL,
                "zoom {zoom}"
            );
        }
    }
}
