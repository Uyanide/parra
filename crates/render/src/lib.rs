//! Pixels: Wayland surfaces, EGL/GLES contexts, image decoding and compositing.
//!
//! Which GPU this runs on is never asked: device selection belongs to the EGL
//! implementation and the compositor's dmabuf feedback. See `docs/environment.md`.

mod decode;
mod error;
mod gl;
mod loader;
mod textures;
mod wayland;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::os::fd::BorrowedFd;

use domain::{MonitorState, OutputId, PixelSize, SurfaceParams, WallpaperRef};
use glow::HasContext;
use tracing::debug;

pub use error::RenderError;
pub use gl::FrameCost;
pub use wayland::RenderEvent;

use gl::{Composite, Frame, Gl, Kawase, Target};
use loader::Loader;
use textures::{BlurCache, BlurKey, TextureCache};
use wayland::{Pass, Wayland};

/// The display side of the daemon: the Wayland connection, one shared GL context, and
/// the wallpaper textures.
pub struct Renderer {
    /// Declared before `wayland`, and only for that reason: fields drop in declaration
    /// order, and `eglTerminate` sends Wayland requests of its own, so the connection has
    /// to outlive the EGL display.
    gl: Gl,
    wayland: Wayland,
    composite: Composite,
    kawase: Kawase,
    targets: BTreeMap<OutputId, Target>,
    textures: TextureCache,
    blurs: BlurCache,
    loader: Loader,
    /// What each output actually has on screen. An arriving wallpaper starts no
    /// animation, so this is what tells an idle output that it owes a frame.
    presented: BTreeMap<OutputId, WallpaperRef>,
    /// Total frames presented, which the idle budget is checked against.
    frames: u64,
}

impl Renderer {
    pub fn new(params: &SurfaceParams) -> Result<Self, RenderError> {
        let wayland = Wayland::connect(params)?;
        let gl = Gl::new(wayland.connection())?;
        let composite = Composite::new(&gl.api)?;
        let kawase = Kawase::new(&gl.api)?;
        Ok(Self {
            wayland,
            gl,
            composite,
            kawase,
            targets: BTreeMap::new(),
            textures: TextureCache::default(),
            blurs: BlurCache::default(),
            loader: Loader::new()?,
            presented: BTreeMap::new(),
            frames: 0,
        })
    }

    /// The descriptor the daemon's event loop watches.
    pub fn fd(&self) -> BorrowedFd<'_> {
        self.wayland.fd()
    }

    /// A second descriptor to watch: readable when a wallpaper has finished decoding.
    ///
    /// Decoding happens on its own thread, so it can finish long after the last frame was
    /// drawn, when nothing else would ever wake the loop.
    pub fn decode_fd(&self) -> BorrowedFd<'_> {
        self.loader.fd()
    }

    /// Answers the readiness of [`Renderer::decode_fd`], which keeps a level-triggered
    /// descriptor from spinning. The image itself is taken by the next draw.
    pub fn noticed_decode(&self) {
        self.loader.clear_signal();
    }

    /// Reads the connection and returns what the daemon must react to.
    pub fn dispatch(&mut self) -> Result<Vec<RenderEvent>, RenderError> {
        let events = self.wayland.dispatch()?;
        Ok(self.absorb(events))
    }

    /// Events already queued but not yet delivered, without touching the socket.
    ///
    /// Presenting reads the same connection, so a frame callback can arrive with nothing
    /// left on the descriptor for a poll to notice. Call this before sleeping.
    pub fn dispatch_queued(&mut self) -> Result<Vec<RenderEvent>, RenderError> {
        let events = self.wayland.dispatch_queued()?;
        Ok(self.absorb(events))
    }

    fn absorb(&mut self, events: Vec<RenderEvent>) -> Vec<RenderEvent> {
        for event in &events {
            if let RenderEvent::OutputGone { id } = event {
                self.presented.remove(id);
                if let Some(target) = self.targets.remove(id) {
                    self.gl.destroy_target(target);
                }
            }
        }
        events
    }

    pub fn flush(&mut self) -> Result<(), RenderError> {
        self.wayland.flush()
    }

    /// Video memory currently held by wallpaper textures, sharp and baked alike.
    pub fn texture_bytes(&self) -> u64 {
        self.textures.footprint() + self.blurs.footprint()
    }

    /// Frames presented since startup, across every output.
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// What the GPU spent on this output's frames. Empty for an output that has never
    /// been drawn, and for a driver with no timer at all.
    pub fn frame_cost(&self, id: &OutputId) -> FrameCost {
        self.targets.get(id).map_or_else(FrameCost::default, Target::cost)
    }

    /// Outputs the compositor has configured and that are ready to be drawn.
    pub fn outputs(&self) -> impl Iterator<Item = &OutputId> {
        self.wayland.surfaces().filter(|surface| surface.is_drawable()).map(|surface| &surface.id)
    }

    /// Draws every output that needs it.
    ///
    /// An output is drawn when something changed outside the animation, or while its
    /// animation is still running. With everything settled this submits nothing at all,
    /// which is what makes an idle daemon free.
    pub fn draw(&mut self, states: &BTreeMap<OutputId, MonitorState>) -> Result<(), RenderError> {
        self.sync_textures(states)?;

        // After `sync_textures`, which is what leaves a context current. The query
        // objects belong to the context rather than to a surface, so any binding will do.
        for target in self.targets.values_mut() {
            target.collect(&self.gl.api);
        }

        let mut pending = Vec::new();
        for surface in self.wayland.surfaces_mut() {
            let Some(state) = states.get(&surface.id) else { continue };
            if self.presented.get(&surface.id) != state.wallpaper.current() {
                surface.pacing.dirty = true;
            }
            match surface.pacing.plan(surface.is_drawable(), state.is_settled()) {
                Pass::Skip => {}
                // Owed a frame the compositor has not asked for yet. Recording the debt
                // is what stops it being forgotten once the animation settles.
                Pass::Defer => surface.pacing.dirty = true,
                Pass::Draw => pending.push(surface.id.clone()),
            }
        }

        for id in pending {
            let Some(state) = states.get(&id) else { continue };
            self.draw_output(&id, state)?;
        }
        self.flush()
    }

    /// Takes in what the outputs are about to need and releases what they no longer show.
    ///
    /// Binds the offscreen surface first, because uploading and baking belong to no
    /// output and must not depend on whichever one happened to be drawn last.
    fn sync_textures(
        &mut self,
        states: &BTreeMap<OutputId, MonitorState>,
    ) -> Result<(), RenderError> {
        self.gl.make_current_offscreen()?;
        for loaded in self.loader.collect() {
            self.textures.accept(&self.gl.api, loaded)?;
        }

        let mut wanted: HashMap<&WallpaperRef, PixelSize> = HashMap::new();
        for state in states.values() {
            let Some(wallpaper) = state.wallpaper.current() else { continue };
            let needed = decode::needed_size(state.buffer_size(), state.params.overview.zoom());
            wanted.entry(wallpaper).and_modify(|size| *size = size.union(needed)).or_insert(needed);
        }

        for (wallpaper, needed) in &wanted {
            self.textures.ensure(&mut self.loader, wallpaper, *needed);
        }

        let in_use: HashSet<WallpaperRef> = wanted.keys().map(|w| (*w).clone()).collect();
        self.textures.retain(&self.gl.api, &in_use);
        self.loader.retain(&in_use);
        self.sync_blurs(states)
    }

    /// Bakes the blurs that are about to be sampled, and keeps them afterwards.
    ///
    /// An output that has never blurred pays nothing; one that has stops paying only when
    /// it changes wallpaper or blur settings. Dropping a bake on every focus change would
    /// put its cost back into the interaction it was moved out of.
    fn sync_blurs(&mut self, states: &BTreeMap<OutputId, MonitorState>) -> Result<(), RenderError> {
        let mut resident: HashSet<BlurKey> = HashSet::new();
        let mut needed: Vec<BlurKey> = Vec::new();
        for state in states.values() {
            let Some(key) = blur_key(state) else { continue };
            if wants_blur(state) && !needed.contains(&key) {
                needed.push(key.clone());
            }
            resident.insert(key);
        }

        for key in &needed {
            let Some(sharp) = self.textures.get(&key.wallpaper) else { continue };
            self.blurs.ensure(&self.gl.api, &self.kawase, key, sharp)?;
        }
        self.blurs.retain(&self.gl.api, &resident);
        Ok(())
    }

    fn draw_output(&mut self, id: &OutputId, state: &MonitorState) -> Result<(), RenderError> {
        let Some(wallpaper) = state.wallpaper.current() else { return Ok(()) };
        let buffer = state.buffer_size().max_one();
        self.ensure_target(id, buffer)?;

        let Some(texture) = self.textures.get(wallpaper) else { return Ok(()) };
        let Some(target) = self.targets.get_mut(id) else { return Ok(()) };

        // With no bake to sample, a zero factor makes the shader skip its second fetch,
        // so the sharp texture standing in for it is never read.
        let (blurred, blur) = match blur_key(state).and_then(|key| self.blurs.get(&key)) {
            Some(baked) => (baked, state.blur.amount.value()),
            None => (texture, 0.0),
        };

        self.gl.make_current(target)?;
        let uv = state.sample_rect(texture.size());
        tracing::trace!(
            output = %id,
            scroll = state.scroll.v.value(),
            blur,
            settled = state.is_settled(),
            "presenting"
        );
        let frame = Frame {
            sharp: texture,
            blurred,
            previous: None,
            uv,
            uv_previous: uv,
            blur,
            mix: 1.0,
            tint: state.params.blur.effective_tint(),
        };
        unsafe {
            self.gl.api.viewport(0, 0, buffer.w as i32, buffer.h as i32);
        }
        // The timed region is this pass and nothing else. Presenting is the compositor's
        // work, not ours, and it reads the Wayland connection besides.
        target.measure(&self.gl.api, || self.composite.draw(&self.gl.api, &frame));

        // Both must precede the commit that swapping performs.
        self.wayland.apply_geometry(id);
        self.wayland.request_frame(id);

        let settled = state.is_settled();
        if let Some(surface) = self.wayland.surface_mut(id) {
            surface.pacing.submitted(settled);
        }

        self.gl.swap(target)?;
        self.frames += 1;
        self.presented.insert(id.clone(), wallpaper.clone());
        Ok(())
    }

    fn ensure_target(&mut self, id: &OutputId, buffer: PixelSize) -> Result<(), RenderError> {
        if let Some(target) = self.targets.get_mut(id) {
            target.resize(buffer);
            return Ok(());
        }
        let Some(surface) = self.wayland.surface_mut(id) else { return Ok(()) };
        debug!(output = %id, width = buffer.w, height = buffer.h, "creating native surface");
        let target = self.gl.target(&surface.wl_surface, buffer, id.as_str())?;
        self.targets.insert(id.clone(), target);
        Ok(())
    }
}

impl Drop for Renderer {
    /// Takes the native surfaces down before anything they were made from.
    ///
    /// Each holds a handle into the EGL display and a pointer into a Wayland surface, and
    /// destroying it needs both alive. With the field order above, the teardown is:
    /// surfaces, then the EGL display, then the connection. Any other order segfaults in
    /// the driver after the last log line.
    fn drop(&mut self) {
        for (_, target) in std::mem::take(&mut self.targets) {
            self.gl.destroy_target(target);
        }
    }
}

/// The bake this output is configured for, if it is configured for one at all.
fn blur_key(state: &MonitorState) -> Option<BlurKey> {
    let wallpaper = state.wallpaper.current()?;
    state.params.blur.is_enabled().then(|| BlurKey::new(wallpaper, &state.params.blur))
}

/// Whether this output is showing its blur or on the way to it. Baking is deferred until
/// then, so a monitor that never takes focus never pays for one.
fn wants_blur(state: &MonitorState) -> bool {
    state.blur.amount.value() > 0.0 || state.blur.amount.target() > 0.0
}
