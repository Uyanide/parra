use std::fs;
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use domain::PixelSize;
use image::codecs::qoi::{QoiDecoder, QoiEncoder};
use image::{ColorType, ExtendedColorType, ImageDecoder, ImageEncoder};
use tracing::debug;

use crate::decode::Decoded;
use crate::error::RenderError;

/// Where a wallpaper's resized copy is kept, and how large it was asked for when written.
///
/// A wallpaper with no `Cache` at all is one nothing is remembering: it decodes from its
/// source every time, which is what `--no-save` and the configured fallback both do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cache {
    pub file: PathBuf,
    /// `None` until a copy exists, which is also the state between a `set` and the decode
    /// it started.
    pub asked: Option<PixelSize>,
}

impl Cache {
    /// Whether the copy on disk was decoded for a request at least this large, and is
    /// therefore usable without touching the source.
    ///
    /// Compared against what was asked for rather than what came back: an image smaller
    /// than the screen is never enlarged, so its own size would never satisfy the request
    /// that produced it and the source would be re-decoded on every pass.
    pub fn serves(&self, needed: PixelSize) -> bool {
        self.asked.is_some_and(|asked| asked.covers(needed))
    }
}

/// Reads a resized copy.
///
/// QOI because the renderer already links against it, so this costs no dependency, and
/// because decoding it is a memcpy-shaped loop rather than the entropy decode plus
/// Lanczos3 resample that reading the original costs.
pub fn read(file: &Path) -> Result<Decoded, RenderError> {
    let bytes = fs::read(file)
        .map_err(|source| RenderError::ImageRead { path: file.to_owned(), source })?;
    let decoder = QoiDecoder::new(bytes.as_slice())
        .map_err(|source| RenderError::ImageDecode { path: file.to_owned(), source })?;

    let (w, h) = decoder.dimensions();
    // Only ever written from RGBA8 here. Anything else is a file that wandered in.
    if decoder.color_type() != ColorType::Rgba8 {
        return Err(RenderError::Cache {
            path: file.to_owned(),
            message: format!("expected RGBA8, found {:?}", decoder.color_type()),
        });
    }

    let mut rgba = vec![0u8; decoder.total_bytes() as usize];
    decoder
        .read_image(&mut rgba)
        .map_err(|source| RenderError::ImageDecode { path: file.to_owned(), source })?;
    // Always walked: the copy records nothing about the format it was made from. It is
    // screen-sized, so this is a fraction of the read that just produced it.
    let opaque = crate::decode::is_opaque(&rgba);
    Ok(Decoded { size: PixelSize::new(w, h), rgba, opaque })
}

/// Writes a resized copy, atomically, so a copy that exists is always a whole one.
///
/// Written before the buffer is premultiplied, so the file holds straight alpha and a copy
/// from any earlier version still reads.
pub fn write(file: &Path, decoded: &Decoded) -> Result<(), RenderError> {
    let cache_error = |message: String| RenderError::Cache { path: file.to_owned(), message };

    let mut bytes = Vec::new();
    QoiEncoder::new(BufWriter::new(&mut bytes))
        .write_image(&decoded.rgba, decoded.size.w, decoded.size.h, ExtendedColorType::Rgba8)
        .map_err(|error| cache_error(error.to_string()))?;

    debug!(
        path = %file.display(),
        width = decoded.size.w,
        height = decoded.size.h,
        bytes = bytes.len(),
        "caching a resized wallpaper"
    );
    store::atomic::write(file, &bytes).map_err(|error| cache_error(error.to_string()))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;

    fn directory() -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("cache-{}-{unique}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        path
    }

    /// Something with structure, so a decoder that returned the wrong stride would show.
    /// The 251 stride never lands on 255, so every alpha here is short of opaque.
    fn sample() -> Decoded {
        let size = PixelSize::new(7, 5);
        let rgba: Vec<u8> = (0..size.area() * 4).map(|i| (i % 251) as u8).collect();
        Decoded { size, rgba, opaque: false }
    }

    /// The same shape with every alpha at full.
    fn opaque_sample() -> Decoded {
        let mut decoded = sample();
        for pixel in decoded.rgba.chunks_exact_mut(4) {
            pixel[3] = u8::MAX;
        }
        decoded.opaque = true;
        decoded
    }

    #[test]
    fn a_copy_survives_the_round_trip_intact() {
        let dir = directory();
        let file = dir.join("global-1.qoi");
        let original = sample();
        write(&file, &original).unwrap();

        let read_back = read(&file).unwrap();
        assert_eq!(read_back.size, original.size);
        assert_eq!(read_back.rgba, original.rgba, "alpha included");
        fs::remove_dir_all(&dir).unwrap();
    }

    /// The copy carries no flag of its own, so reading one has to arrive at the same
    /// answer the decode did or an output would declare the wrong opaque region.
    #[test]
    fn opacity_is_re_derived_from_the_copy() {
        let dir = directory();
        for original in [sample(), opaque_sample()] {
            let file = dir.join(format!("global-{}.qoi", u32::from(original.opaque)));
            write(&file, &original).unwrap();
            assert_eq!(read(&file).unwrap().opaque, original.opaque);
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_damaged_copy_is_refused_rather_than_trusted() {
        let dir = directory();
        let file = dir.join("global-1.qoi");
        write(&file, &sample()).unwrap();

        let whole = fs::read(&file).unwrap();
        for damaged in [&whole[..whole.len() / 2], b"".as_slice(), b"not an image".as_slice()] {
            fs::write(&file, damaged).unwrap();
            assert!(read(&file).is_err(), "{} bytes should not decode", damaged.len());
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_missing_copy_is_an_error_and_not_a_panic() {
        let dir = directory();
        assert!(read(&dir.join("absent.qoi")).is_err());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_copy_serves_only_requests_it_was_large_enough_for() {
        let cache = Cache {
            file: PathBuf::from("/tmp/global-1.qoi"),
            asked: Some(PixelSize::new(2560, 1440)),
        };
        assert!(cache.serves(PixelSize::new(2560, 1440)));
        assert!(cache.serves(PixelSize::new(1920, 1080)));
        assert!(!cache.serves(PixelSize::new(1440, 2560)), "a rotated output needs a new copy");

        let pending = Cache { file: cache.file, asked: None };
        assert!(!pending.serves(PixelSize::new(1, 1)), "nothing has been written yet");
    }
}
