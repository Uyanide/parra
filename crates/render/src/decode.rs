use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;

use domain::PixelSize;
use fast_image_resize::images::{Image, ImageRef};
use fast_image_resize::{MulDiv, PixelType, ResizeAlg, ResizeOptions, Resizer};
use tracing::debug;

use crate::error::RenderError;

/// Lanczos3 is the sharpest of the practical filters, and downscaling a wallpaper
/// happens once per image rather than once per frame.
const FILTER: ResizeAlg = ResizeAlg::Convolution(fast_image_resize::FilterType::Lanczos3);

pub struct Decoded {
    pub size: PixelSize,
    /// Straight alpha, as `image` and `fast_image_resize` both work in. [`premultiply`]
    /// is what turns it into what the GPU and the compositor want.
    pub rgba: Vec<u8>,
    /// Whether every pixel is fully opaque, which is what lets an output go on declaring
    /// its surface opaque.
    pub opaque: bool,
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
        .map_err(|source| RenderError::ImageDecode { path: path.to_owned(), source })?;

    // Asked before the conversion, which gives every image four channels whether or not
    // the file had them, and so is the last moment the format's own answer is available.
    let channel = decoded.color().has_alpha();
    let decoded = decoded.into_rgba8();
    // Read from the source: a convolution can land a constant alpha a step short of full,
    // and a copy is what the resize below produces.
    let opaque = !channel || is_opaque(&decoded);

    let source = PixelSize::new(decoded.width(), decoded.height());
    if source.is_empty() {
        return Err(RenderError::ImageResize {
            path: path.to_owned(),
            message: "the image has no pixels".to_owned(),
        });
    }

    let wanted = fit_within(source, target);
    if wanted == source {
        return Ok(Decoded { size: source, rgba: decoded.into_raw(), opaque });
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

    Ok(Decoded { size: wanted, rgba: dst.into_vec(), opaque })
}

/// Whether every pixel is fully opaque.
///
/// Stops at the first translucent pixel, so the walk runs in full only for an alpha
/// channel that says nothing. Whether it is worth starting is the caller's to judge.
pub fn is_opaque(rgba: &[u8]) -> bool {
    rgba.as_chunks::<4>().0.iter().all(|pixel| pixel[3] == u8::MAX)
}

/// Multiplies the colour channels by alpha, in place, which is the form GL and Wayland
/// both read a buffer as.
///
/// Must precede the upload: `LINEAR` filtering interpolates texels before any shader sees
/// them, and straight alpha fringes at every edge once it does. An opaque image needs
/// none of it, so the caller skips it there.
pub fn premultiply(decoded: &mut Decoded) -> Result<(), RenderError> {
    let (w, h) = (decoded.size.w, decoded.size.h);
    let mut image = Image::from_slice_u8(w, h, &mut decoded.rgba, PixelType::U8x4)
        .map_err(|error| RenderError::Premultiply(error.to_string()))?;
    MulDiv::default()
        .multiply_alpha_inplace(&mut image)
        .map_err(|error| RenderError::Premultiply(error.to_string()))
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
    use std::sync::atomic::{AtomicU32, Ordering};

    use image::{ExtendedColorType, ImageEncoder, codecs::png::PngEncoder};

    use super::*;

    /// Writes one PNG and answers where it landed. Named per test so two running at once
    /// cannot read each other's file.
    fn png(pixels: &[u8], colour: ExtendedColorType) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let file = std::env::temp_dir().join(format!("decode-{}-{unique}.png", std::process::id()));
        let mut bytes = Vec::new();
        PngEncoder::new(&mut bytes).write_image(pixels, 2, 2, colour).unwrap();
        std::fs::write(&file, &bytes).unwrap();
        file
    }

    /// Four pixels, every alpha at full.
    fn opaque_rgba() -> Vec<u8> {
        vec![10, 20, 30, 255, 40, 50, 60, 255, 70, 80, 90, 255, 100, 110, 120, 255]
    }

    /// Well above anything a test asks to shrink to.
    const UNLIMITED: PixelSize = PixelSize { w: 4096, h: 4096 };

    #[test]
    fn a_format_with_no_alpha_channel_is_opaque() {
        let file =
            png(&[10, 20, 30, 40, 50, 60, 70, 80, 90, 100, 110, 120], ExtendedColorType::Rgb8);
        assert!(load(&file, UNLIMITED).unwrap().opaque);
        std::fs::remove_file(&file).unwrap();
    }

    /// The common case the walk exists for: a channel is present and says nothing, and
    /// giving up the opaque region over it would cost a blend for no visible difference.
    #[test]
    fn an_alpha_channel_that_says_nothing_is_still_opaque() {
        let file = png(&opaque_rgba(), ExtendedColorType::Rgba8);
        assert!(load(&file, UNLIMITED).unwrap().opaque);
        std::fs::remove_file(&file).unwrap();
    }

    #[test]
    fn one_pixel_short_of_full_is_enough_to_lose_the_opaque_region() {
        let mut pixels = opaque_rgba();
        pixels[7] = 254;
        let file = png(&pixels, ExtendedColorType::Rgba8);
        assert!(!load(&file, UNLIMITED).unwrap().opaque);
        std::fs::remove_file(&file).unwrap();
    }

    #[test]
    fn opacity_survives_the_resize_that_follows_it() {
        let mut pixels = opaque_rgba();
        pixels[7] = 0;
        let file = png(&pixels, ExtendedColorType::Rgba8);
        let decoded = load(&file, PixelSize::new(1, 1)).unwrap();
        assert_eq!(decoded.size, PixelSize::new(1, 1), "the test needs the resize to happen");
        assert!(!decoded.opaque, "the answer is the source's, not the copy's");
        std::fs::remove_file(&file).unwrap();
    }

    #[test]
    fn premultiplication_brings_every_channel_within_its_alpha() {
        let mut decoded = Decoded {
            size: PixelSize::new(2, 1),
            rgba: vec![255, 255, 255, 0, 200, 100, 50, 128],
            opaque: false,
        };
        premultiply(&mut decoded).unwrap();

        for pixel in decoded.rgba.as_chunks::<4>().0 {
            let alpha = pixel[3];
            assert!(pixel[..3].iter().all(|&channel| channel <= alpha), "{pixel:?}");
        }
    }

    /// Why the caller skips it for an opaque image: there is nothing there to multiply.
    #[test]
    fn premultiplying_an_opaque_image_changes_nothing() {
        let original = opaque_rgba();
        let mut decoded =
            Decoded { size: PixelSize::new(2, 2), rgba: original.clone(), opaque: true };
        premultiply(&mut decoded).unwrap();
        assert_eq!(decoded.rgba, original);
    }

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
