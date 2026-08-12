use crate::output::PixelSize;

/// Region of the source image that fills the viewport, in normalized image coordinates.
///
/// The renderer turns this straight into texture coordinates, so panning costs one
/// uniform: no pixels move and nothing is re-uploaded.
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

/// Fits the image over the viewport, zooms in by `zoom`, then pans within whatever
/// headroom that leaves.
///
/// The fit always covers: the image is scaled until it spans both axes, so no letterbox
/// can appear. Whatever the cover leaves outside the viewport, plus whatever `zoom`
/// adds, becomes the travel that `scroll_v` and `scroll_h` move through. Both are
/// fractions of that travel, `0` at the top or left edge and `1` at the bottom or right.
/// An axis with no headroom ignores its value.
pub fn sample_rect(
    image: PixelSize,
    viewport: PixelSize,
    zoom: f32,
    scroll_h: f32,
    scroll_v: f32,
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

    let zoom = if zoom.is_finite() && zoom > 0.0 { zoom } else { 1.0 };
    let w = (cover_w / zoom).clamp(f32::MIN_POSITIVE, 1.0);
    let h = (cover_h / zoom).clamp(f32::MIN_POSITIVE, 1.0);

    let u0 = (1.0 - w) * scroll_h.clamp(0.0, 1.0);
    let v0 = (1.0 - h) * scroll_v.clamp(0.0, 1.0);
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
        let rect =
            sample_rect(PixelSize::new(2560, 1440), PixelSize::new(2560, 1440), 1.0, 0.5, 0.5);
        assert_eq!(rect, UvRect::FULL);
    }

    #[test]
    fn a_taller_image_leaves_vertical_headroom() {
        let rect =
            sample_rect(PixelSize::new(1000, 2000), PixelSize::new(1000, 1000), 1.0, 0.5, 0.5);
        assert_close(rect.width(), 1.0, "width");
        assert_close(rect.height(), 0.5, "height");
        assert_close(rect.v0, 0.25, "centred vertically");
    }

    #[test]
    fn a_wider_image_leaves_horizontal_headroom() {
        let rect =
            sample_rect(PixelSize::new(2000, 1000), PixelSize::new(1000, 1000), 1.0, 0.5, 0.5);
        assert_close(rect.height(), 1.0, "height");
        assert_close(rect.width(), 0.5, "width");
        assert_close(rect.u0, 0.25, "centred horizontally");
    }

    #[test]
    fn zoom_creates_headroom_where_the_cover_left_none() {
        let square = PixelSize::new(1000, 1000);
        assert_eq!(sample_rect(square, square, 1.0, 0.5, 0.5), UvRect::FULL);

        let zoomed = sample_rect(square, square, 1.0 / 0.9, 0.5, 0.0);
        assert_close(zoomed.height(), 0.9, "height");
        assert_close(zoomed.v0, 0.0, "pinned to the top");
    }

    #[test]
    fn scroll_runs_from_the_top_edge_to_the_bottom() {
        let image = PixelSize::new(1000, 2000);
        let viewport = PixelSize::new(1000, 1000);

        let top = sample_rect(image, viewport, 1.0, 0.5, 0.0);
        assert_close(top.v0, 0.0, "top");
        assert_close(top.v1, 0.5, "top");

        let bottom = sample_rect(image, viewport, 1.0, 0.5, 1.0);
        assert_close(bottom.v0, 0.5, "bottom");
        assert_close(bottom.v1, 1.0, "bottom");
    }

    #[test]
    fn the_rect_never_leaves_the_image() {
        let image = PixelSize::new(3840, 2160);
        let viewport = PixelSize::new(2560, 1600);
        for zoom in [0.5, 1.0, 1.111, 4.0] {
            for scroll in [0.0, 0.37, 1.0] {
                let rect = sample_rect(image, viewport, zoom, scroll, scroll);
                assert!(rect.u0 >= -EPS && rect.u1 <= 1.0 + EPS, "u out of range at {zoom}");
                assert!(rect.v0 >= -EPS && rect.v1 <= 1.0 + EPS, "v out of range at {zoom}");
            }
        }
    }

    #[test]
    fn a_degenerate_size_falls_back_to_the_full_image() {
        let ok = PixelSize::new(100, 100);
        assert_eq!(sample_rect(PixelSize::new(0, 100), ok, 1.0, 0.5, 0.5), UvRect::FULL);
        assert_eq!(sample_rect(ok, PixelSize::new(100, 0), 1.0, 0.5, 0.5), UvRect::FULL);
    }

    #[test]
    fn a_nonsensical_zoom_is_ignored_rather_than_propagated() {
        let square = PixelSize::new(100, 100);
        for zoom in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            let rect = sample_rect(square, square, zoom, 0.5, 0.5);
            assert!(rect.width() > 0.0 && rect.height() > 0.0, "zoom {zoom} produced {rect:?}");
        }
    }
}
