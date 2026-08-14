use std::path::Path;

use domain::PixelSize;
use fast_image_resize::images::{Image, ImageRef};
use fast_image_resize::{PixelType, ResizeAlg, ResizeOptions, Resizer};
use tracing::debug;

use crate::error::RenderError;

/// Lanczos3 is the sharpest of the practical filters, and downscaling a wallpaper
/// happens once per image rather than once per frame.
const FILTER: ResizeAlg = ResizeAlg::Convolution(fast_image_resize::FilterType::Lanczos3);

pub struct Decoded {
    pub size: PixelSize,
    pub rgba: Vec<u8>,
}

/// Reads an image and shrinks it to `target` if it is larger.
///
/// Peak memory is the decoded source plus the result. Both steps run on the caller's
/// thread, which is the loader's, never the event loop's. The source half is bounded by
/// `image`'s own default limit of 512 MB per decode, which is best-effort but is the
/// bound that exists; the result half is bounded by `needed_size`.
pub fn load(path: &Path, target: PixelSize) -> Result<Decoded, RenderError> {
    let reader = image::ImageReader::open(path)
        .map_err(|source| RenderError::ImageRead { path: path.to_owned(), source })?
        .with_guessed_format()
        .map_err(|source| RenderError::ImageRead { path: path.to_owned(), source })?;

    let decoded = reader
        .decode()
        .map_err(|source| RenderError::ImageDecode { path: path.to_owned(), source })?
        .into_rgba8();

    let source = PixelSize::new(decoded.width(), decoded.height());
    if source.is_empty() {
        return Err(RenderError::ImageResize {
            path: path.to_owned(),
            message: "the image has no pixels".to_owned(),
        });
    }

    let wanted = fit_within(source, target);
    if wanted == source {
        return Ok(Decoded { size: source, rgba: decoded.into_raw() });
    }

    debug!(
        from = format!("{}x{}", source.w, source.h),
        to = format!("{}x{}", wanted.w, wanted.h),
        "downscaling wallpaper"
    );
    let resize_error =
        |message: String| RenderError::ImageResize { path: path.to_owned(), message };

    let raw = decoded.into_raw();
    let src = ImageRef::new(source.w, source.h, &raw, PixelType::U8x4)
        .map_err(|error| resize_error(error.to_string()))?;
    let mut dst = Image::new(wanted.w, wanted.h, PixelType::U8x4);
    Resizer::new()
        .resize(&src, &mut dst, &ResizeOptions::new().resize_alg(FILTER))
        .map_err(|error| resize_error(error.to_string()))?;

    Ok(Decoded { size: wanted, rgba: dst.into_vec() })
}

/// Largest size an image will ever be sampled at: a cover fit of the buffer, enlarged by
/// the deepest zoom the parallax reaches, and never past what the driver can hold.
///
/// Anything beyond the zoom is texture memory that can never appear on screen; anything
/// beyond `ceiling` is a texture the driver would refuse outright. Clamping per axis
/// rather than by aspect is safe because this is only ever a request: `fit_within` still
/// preserves the source's own ratio.
///
/// The whole size policy is this one function, so a scale change, a rotation, a
/// `crop-ratio` edit and a second monitor all arrive at the same comparison.
pub fn needed_size(buffer: PixelSize, zoom: f32, ceiling: u32) -> PixelSize {
    let zoom = if zoom.is_finite() && zoom > 1.0 { zoom } else { 1.0 };
    let ceiling = ceiling.max(1);
    let scale = |value: u32| ((value as f32 * zoom).ceil() as u32).clamp(1, ceiling);
    PixelSize::new(scale(buffer.w), scale(buffer.h))
}

/// Shrinks to cover `target` while keeping the aspect ratio, and never enlarges: an
/// upscale would cost memory and bandwidth without adding detail.
fn fit_within(source: PixelSize, target: PixelSize) -> PixelSize {
    if source.is_empty() || target.is_empty() {
        return source;
    }
    let by_width = target.w as f64 / source.w as f64;
    let by_height = target.h as f64 / source.h as f64;
    let scale = by_width.max(by_height);
    if scale >= 1.0 {
        return source;
    }
    PixelSize::new(
        ((source.w as f64 * scale).ceil() as u32).max(1),
        ((source.h as f64 * scale).ceil() as u32).max(1),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_image_smaller_than_the_screen_is_left_alone() {
        let source = PixelSize::new(1280, 720);
        assert_eq!(fit_within(source, PixelSize::new(2560, 1440)), source);
    }

    #[test]
    fn an_oversized_image_shrinks_to_cover_the_target() {
        let fitted = fit_within(PixelSize::new(6000, 4000), PixelSize::new(2560, 1440));
        assert!(fitted.w >= 2560 && fitted.h >= 1440, "{fitted:?} must still cover");
        assert!(fitted.w < 6000, "{fitted:?} should have shrunk");
    }

    #[test]
    fn shrinking_keeps_the_aspect_ratio() {
        let source = PixelSize::new(6000, 4000);
        let fitted = fit_within(source, PixelSize::new(2560, 1440));
        let before = source.w as f64 / source.h as f64;
        let after = fitted.w as f64 / fitted.h as f64;
        assert!((before - after).abs() < 0.01, "{before} vs {after}");
    }

    #[test]
    fn an_image_too_short_to_cover_is_still_not_enlarged() {
        // Nothing here can letterbox: the cover fit happens when the rect is sampled, so
        // upscaling at decode time would only spend memory to invent detail.
        let source = PixelSize::new(10000, 1200);
        assert_eq!(fit_within(source, PixelSize::new(2560, 1440)), source);
    }

    /// Well above anything these tests ask for, so only the case testing it sees it.
    const CEILING: u32 = 16384;

    #[test]
    fn zoom_raises_the_resolution_the_parallax_needs() {
        let buffer = PixelSize::new(2560, 1440);
        assert_eq!(needed_size(buffer, 1.0, CEILING), buffer);

        let zoomed = needed_size(buffer, 1.0 / 0.9, CEILING);
        assert!(zoomed.w > buffer.w && zoomed.h > buffer.h, "{zoomed:?}");
    }

    #[test]
    fn a_nonsensical_zoom_does_not_inflate_the_texture() {
        let buffer = PixelSize::new(2560, 1440);
        for zoom in [0.0, -1.0, f32::NAN, 0.5] {
            assert_eq!(needed_size(buffer, zoom, CEILING), buffer, "zoom {zoom}");
        }
    }

    #[test]
    fn nothing_is_ever_asked_for_beyond_what_the_driver_can_hold() {
        let buffer = PixelSize::new(3200, 1800);
        let asked = needed_size(buffer, 100.0, CEILING);
        assert_eq!(asked, PixelSize::new(CEILING, CEILING));

        // A driver reporting nonsense must still leave a texture that can be created.
        assert_eq!(needed_size(buffer, 4.0, 0), PixelSize::new(1, 1));
    }
}
