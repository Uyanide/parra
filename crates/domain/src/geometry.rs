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

/// Available movement from one edge to the other, measured in viewport widths and heights.
///
/// Policy reads this at the configured deepest zoom, which is the unit `max-shift` is
/// written in, and divides the cap by it to get the fraction that reaches the animator.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Travel {
    pub h: f32,
    pub v: f32,
}

impl Travel {
    /// An image with nothing outside the viewport on either axis.
    pub const NONE: Travel = Travel { h: 0.0, v: 0.0 };
}

/// Geometry of one image on one output.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    cover_w: f32,
    cover_h: f32,
}

impl View {
    pub fn new(image: PixelSize, viewport: PixelSize) -> Option<Self> {
        if image.is_empty() || viewport.is_empty() {
            return None;
        }
        let image_aspect = image.w as f32 / image.h as f32;
        let viewport_aspect = viewport.w as f32 / viewport.h as f32;
        let (cover_w, cover_h) = if image_aspect > viewport_aspect {
            (viewport_aspect / image_aspect, 1.0)
        } else {
            (1.0, image_aspect / viewport_aspect)
        };
        Some(Self { cover_w, cover_h })
    }

    /// Travel at one zoom, in screen extents rather than image coordinates.
    pub fn travel(self, zoom: f32) -> Travel {
        let w = visible(self.cover_w, factor(zoom));
        let h = visible(self.cover_h, factor(zoom));
        Travel { h: (1.0 - w) / w, v: (1.0 - h) / h }
    }

    /// Samples this image at one zoom, offset from its centre by a share of the headroom
    /// that zoom leaves. `0` is centred; `-0.5` and `0.5` are the two edges.
    ///
    /// The offset scales with the headroom rather than being measured against it, so the
    /// rect is one mapping scaled by the zoom instead of two mappings with a bound
    /// between them. That is what keeps a zoom animation from turning around partway:
    /// nothing here can start or stop binding as the zoom moves.
    pub fn sample(self, zoom: f32, scroll_h: f32, scroll_v: f32) -> UvRect {
        let w = visible(self.cover_w, factor(zoom));
        let h = visible(self.cover_h, factor(zoom));
        // Bounded by its own definition rather than by the geometry, so it holds a
        // nonsensical caller inside the image without depending on the zoom.
        let u0 = (1.0 - w) * (0.5 + scroll_h.clamp(-0.5, 0.5));
        let v0 = (1.0 - h) * (0.5 + scroll_v.clamp(-0.5, 0.5));
        UvRect { u0, v0, u1: u0 + w, v1: v0 + h }
    }
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

/// Fits the image over the viewport, zooms in, then offsets the crop by a share of the
/// headroom left over. `0` is centred; positive values move toward the right or bottom.
pub fn sample_rect(
    image: PixelSize,
    viewport: PixelSize,
    zoom: f32,
    scroll_h: f32,
    scroll_v: f32,
) -> UvRect {
    View::new(image, viewport).map_or(UvRect::FULL, |view| view.sample(zoom, scroll_h, scroll_v))
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
        let square = PixelSize::new(1000, 1000);
        assert_eq!(sample_rect(square, square, 1.0, 0.0, 0.0), UvRect::FULL);
    }

    #[test]
    fn travel_is_measured_in_screen_extents() {
        let view = View::new(PixelSize::new(1000, 2000), PixelSize::new(1000, 1000)).unwrap();
        assert_close(view.travel(1.0).v, 1.0, "one screen of vertical travel");
        assert_close(view.travel(1.0).h, 0.0, "no horizontal travel");
    }

    #[test]
    fn displacement_runs_from_the_top_edge_to_the_bottom() {
        let image = PixelSize::new(1000, 2000);
        let viewport = PixelSize::new(1000, 1000);
        let top = sample_rect(image, viewport, 1.0, 0.0, -0.5);
        let bottom = sample_rect(image, viewport, 1.0, 0.0, 0.5);
        assert_close(top.v0, 0.0, "top");
        assert_close(bottom.v0, 0.5, "bottom");
    }

    #[test]
    fn displacement_is_clamped_to_each_image() {
        for image in [PixelSize::new(1000, 2000), PixelSize::new(4000, 1000)] {
            let rect = sample_rect(image, PixelSize::new(1000, 1000), 1.2, 99.0, -99.0);
            assert!(rect.u0 >= -EPS && rect.u1 <= 1.0 + EPS);
            assert!(rect.v0 >= -EPS && rect.v1 <= 1.0 + EPS);
        }
    }

    /// What a crossfade between two differently shaped images does: one share, each
    /// image projecting it onto the headroom it has, and neither running out of image.
    #[test]
    fn crossfade_layers_project_one_share_onto_their_own_headroom() {
        let viewport = PixelSize::new(1000, 1000);
        for share in [-0.5, -0.4, 0.0, 0.4, 0.5] {
            let tall = sample_rect(PixelSize::new(1000, 2000), viewport, 1.0, 0.0, share);
            let shallow = sample_rect(PixelSize::new(1000, 1200), viewport, 1.0, 0.0, share);
            assert_close(tall.v0, 0.5 * (0.5 + share), "tall");
            assert_close(shallow.v0, (1.0 / 6.0) * (0.5 + share), "shallow");
            assert!(tall.v0 >= -EPS && tall.v1 <= 1.0 + EPS, "tall stays in the image");
            assert!(shallow.v0 >= -EPS && shallow.v1 <= 1.0 + EPS, "shallow stays in it");
        }
    }

    /// A screen row a fixed feature of the image lands on. UV alone is the wrong space to
    /// look for movement in: the rect is stretched to the viewport, so a rect that shrinks
    /// while its corner holds still moves everything but that corner.
    fn feature_row(rect: UvRect, feature: f32) -> f32 {
        (feature - rect.v0) / rect.height()
    }

    /// The geometry the overview jump was reported on: a 3364x2564 image on a 2560x1600
    /// output with two workspaces, where the cap and the zoom range used to interact.
    const REPORTED: PixelSize = PixelSize { w: 3364, h: 2564 };
    const SQUARE: PixelSize = PixelSize { w: 2000, h: 2000 };
    const WIDE: PixelSize = PixelSize { w: 5120, h: 1440 };
    const TALL: PixelSize = PixelSize { w: 2937, h: 4796 };

    /// The whole point of resolving the cap once: a zoom animation may only ever scale
    /// the mapping, so nothing on screen sets off one way and arrives from the other.
    #[test]
    fn a_zoom_animation_stays_between_the_two_ends_it_moves_between() {
        let viewport = PixelSize::new(2560, 1600);
        for image in [REPORTED, SQUARE, WIDE, TALL] {
            for crop in [0.9, 0.7, 0.4] {
                let deepest = 1.0 / crop;
                for share in [-0.5, -0.42, -0.15, 0.0, 0.15, 0.42, 0.5] {
                    for step in 0..=20 {
                        let feature = step as f32 / 20.0;
                        let at = |zoom| {
                            feature_row(sample_rect(image, viewport, zoom, 0.0, share), feature)
                        };
                        let (out, r#in) = (at(1.0), at(deepest));
                        let (low, high) = (out.min(r#in), out.max(r#in));
                        for tick in 0..=32 {
                            let zoom = 1.0 + (deepest - 1.0) * tick as f32 / 32.0;
                            let seen = at(zoom);
                            assert!(
                                seen >= low - EPS && seen <= high + EPS,
                                "{image:?} crop {crop} share {share} zoom {zoom} left {low}..{high} for {seen}"
                            );
                        }
                    }
                }
            }
        }
    }

    /// A share is uniform in the stop, so equal steps of it are equal steps on screen at
    /// whatever zoom the output happens to be at.
    #[test]
    fn equal_steps_of_share_move_the_image_equally() {
        let viewport = PixelSize::new(2560, 1600);
        for image in [REPORTED, TALL] {
            for zoom in [1.0, 1.05, 1.0 / 0.9] {
                let row = |share| feature_row(sample_rect(image, viewport, zoom, 0.0, share), 0.5);
                let first = row(-0.4) - row(-0.5);
                for step in 0..9 {
                    let (a, b) = (-0.5 + 0.1 * step as f32, -0.4 + 0.1 * step as f32);
                    assert_close(row(b) - row(a), first, &format!("{image:?} at {zoom}"));
                }
            }
        }
    }

    #[test]
    fn a_taller_image_leaves_vertical_headroom() {
        let rect =
            sample_rect(PixelSize::new(1000, 2000), PixelSize::new(1000, 1000), 1.0, 0.0, 0.0);
        assert_close(rect.width(), 1.0, "width");
        assert_close(rect.height(), 0.5, "height");
        assert_close(rect.v0, 0.25, "centred vertically");
    }

    #[test]
    fn a_wider_image_leaves_horizontal_headroom() {
        let rect =
            sample_rect(PixelSize::new(2000, 1000), PixelSize::new(1000, 1000), 1.0, 0.0, 0.0);
        assert_close(rect.height(), 1.0, "height");
        assert_close(rect.width(), 0.5, "width");
        assert_close(rect.u0, 0.25, "centred horizontally");
    }

    #[test]
    fn zoom_creates_headroom_where_the_cover_left_none() {
        let square = PixelSize::new(1000, 1000);
        assert_eq!(sample_rect(square, square, 1.0, 0.0, 0.0), UvRect::FULL);

        let zoomed = sample_rect(square, square, 1.0 / 0.9, 0.0, -0.5);
        assert_close(zoomed.height(), 0.9, "height");
        assert_close(zoomed.v0, 0.0, "pinned to the top");
    }

    #[test]
    fn the_rect_never_leaves_the_image() {
        let viewport = PixelSize::new(2560, 1600);
        for image in [SQUARE, WIDE, TALL, REPORTED] {
            for step in 0..=16 {
                let zoom = 1.0 + 3.0 * step as f32 / 16.0;
                for share in [-0.5, -0.37, 0.0, 0.37, 0.5] {
                    let rect = sample_rect(image, viewport, zoom, share, share);
                    assert!(rect.u0 >= -EPS && rect.u1 <= 1.0 + EPS, "u at {zoom} {share}");
                    assert!(rect.v0 >= -EPS && rect.v1 <= 1.0 + EPS, "v at {zoom} {share}");
                }
            }
        }
    }

    #[test]
    fn a_zoom_below_one_is_floored_rather_than_zooming_out() {
        let image = PixelSize::new(3840, 2160);
        let viewport = PixelSize::new(2560, 1600);
        let unzoomed = sample_rect(image, viewport, 1.0, 0.03, 0.0);
        for zoom in [0.5, 0.9, f32::MIN_POSITIVE] {
            assert_eq!(sample_rect(image, viewport, zoom, 0.03, 0.0), unzoomed, "zoom {zoom}");
        }
    }

    #[test]
    fn degenerate_sizes_fall_back_to_the_full_image() {
        let ok = PixelSize::new(100, 100);
        assert_eq!(sample_rect(PixelSize::new(0, 100), ok, 1.0, 0.0, 0.0), UvRect::FULL);
        assert_eq!(sample_rect(ok, PixelSize::new(100, 0), 1.0, 0.0, 0.0), UvRect::FULL);
    }

    #[test]
    fn nonsensical_zoom_is_ignored() {
        let square = PixelSize::new(100, 100);
        for zoom in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert_eq!(sample_rect(square, square, zoom, 0.0, 0.0), UvRect::FULL);
        }
    }
}
