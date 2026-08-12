use std::collections::{HashMap, HashSet};

use domain::{BlurParams, PixelSize, WallpaperRef};
use tracing::debug;

use crate::error::RenderError;
use crate::gl::{Kawase, Texture, plan};
use crate::loader::{Loaded, Loader};

/// Decoded wallpapers, keyed by path.
///
/// Two monitors showing the same image share one texture, sized for whichever of them
/// needs it larger. That is the difference between one 19 MB texture and two.
#[derive(Default)]
pub struct TextureCache {
    entries: HashMap<WallpaperRef, Resident>,
}

struct Resident {
    texture: Texture,
    /// The size the decode was asked for, which an image smaller than the screen comes
    /// back short of. Comparing against what came back would ask again on every pass.
    asked: PixelSize,
}

impl TextureCache {
    /// Asks for the wallpaper if it is missing, or if some output now needs it larger
    /// than it was decoded for.
    ///
    /// The work goes to the loader while the outputs keep drawing what they already
    /// have, which is why changing wallpaper costs no stall.
    pub fn ensure(&mut self, loader: &mut Loader, wallpaper: &WallpaperRef, needed: PixelSize) {
        if let Some(existing) = self.entries.get(wallpaper) {
            // Shrinking never re-asks: a monitor that got smaller will get larger again,
            // and the texture it already has stays correct meanwhile.
            if existing.asked.covers(needed) {
                return;
            }
            debug!(path = %wallpaper.path().display(), "reloading at a larger size");
        }
        loader.request(wallpaper, needed);
    }

    /// Takes a finished decode onto the GPU, replacing whatever it supersedes.
    pub fn accept(&mut self, gl: &glow::Context, loaded: Loaded) -> Result<(), RenderError> {
        let texture = Texture::upload(gl, loaded.decoded.size, &loaded.decoded.rgba)?;
        let resident = Resident { texture, asked: loaded.asked };
        if let Some(replaced) = self.entries.insert(loaded.wallpaper, resident) {
            replaced.texture.destroy(gl);
        }
        Ok(())
    }

    pub fn get(&self, wallpaper: &WallpaperRef) -> Option<&Texture> {
        self.entries.get(wallpaper).map(|resident| &resident.texture)
    }

    /// Frees everything no output is showing any more.
    pub fn retain(&mut self, gl: &glow::Context, in_use: &HashSet<WallpaperRef>) {
        let dropped: Vec<WallpaperRef> =
            self.entries.keys().filter(|key| !in_use.contains(*key)).cloned().collect();
        for key in dropped {
            if let Some(resident) = self.entries.remove(&key) {
                debug!(path = %key.path().display(), "releasing wallpaper texture");
                resident.texture.destroy(gl);
            }
        }
    }

    pub fn footprint(&self) -> u64 {
        self.entries.values().map(|resident| resident.texture.footprint()).sum()
    }
}

/// Identifies one baked blur.
///
/// Keyed on the settings as well as the wallpaper, so two monitors showing the same
/// image share the bake when their blur settings agree. How blurred an output currently
/// looks is a shader factor and stays out of the key.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct BlurKey {
    pub wallpaper: WallpaperRef,
    radius: u32,
    downscale: u32,
}

impl BlurKey {
    pub fn new(wallpaper: &WallpaperRef, params: &BlurParams) -> Self {
        Self { wallpaper: wallpaper.clone(), radius: params.radius, downscale: params.downscale }
    }
}

/// Blurred copies of wallpapers, baked once and then only sampled.
#[derive(Default)]
pub struct BlurCache {
    entries: HashMap<BlurKey, Baked>,
}

struct Baked {
    texture: Texture,
    /// Size of the sharp texture this came from. A wallpaper re-decoded larger has to be
    /// re-baked, or the blur would stay at the resolution it happened to start at.
    source: PixelSize,
}

impl BlurCache {
    /// Bakes the blur for `key` if it is missing or was baked from a smaller original.
    pub fn ensure(
        &mut self,
        gl: &glow::Context,
        kawase: &Kawase,
        key: &BlurKey,
        sharp: &Texture,
    ) -> Result<(), RenderError> {
        if self.entries.get(key).is_some_and(|baked| baked.source == sharp.size()) {
            return Ok(());
        }

        debug!(
            path = %key.wallpaper.path().display(),
            radius = key.radius,
            downscale = key.downscale,
            "baking blur"
        );
        let texture = kawase.bake(gl, sharp, plan(key.radius, key.downscale))?;
        let baked = Baked { texture, source: sharp.size() };
        if let Some(replaced) = self.entries.insert(key.clone(), baked) {
            replaced.texture.destroy(gl);
        }
        Ok(())
    }

    pub fn get(&self, key: &BlurKey) -> Option<&Texture> {
        self.entries.get(key).map(|baked| &baked.texture)
    }

    /// Frees bakes for wallpapers and settings no output is configured for any more.
    ///
    /// Configured for, not currently showing: dropping a bake when an output loses focus
    /// would re-bake on every focus change.
    pub fn retain(&mut self, gl: &glow::Context, in_use: &HashSet<BlurKey>) {
        let dropped: Vec<BlurKey> =
            self.entries.keys().filter(|key| !in_use.contains(*key)).cloned().collect();
        for key in dropped {
            if let Some(baked) = self.entries.remove(&key) {
                debug!(path = %key.wallpaper.path().display(), "releasing baked blur");
                baked.texture.destroy(gl);
            }
        }
    }

    pub fn footprint(&self) -> u64 {
        self.entries.values().map(|baked| baked.texture.footprint()).sum()
    }
}
