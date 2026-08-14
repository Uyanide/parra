//! Pixels: Wayland surfaces, EGL/GLES contexts, image decoding and compositing.

mod cache;
mod decode;
mod error;
mod event;
mod gl;
mod loader;
mod textures;
mod wayland;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::os::fd::BorrowedFd;

use domain::{MonitorState, OutputId, PixelSize, SurfaceParams, WallpaperRef};
use glow::HasContext;
use tracing::debug;

pub use cache::Cache;
pub use error::RenderError;
pub use event::RenderEvent;
pub use gl::FrameCost;

use gl::{Composite, Frame, Gl, Kawase, Target, Texture};
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

    /// Says where this wallpaper's resized copy belongs, and how large the one already
    /// there was asked for. `None` means nothing is remembering it, so it decodes from
    /// its source and leaves nothing behind.
    pub fn set_cache(&mut self, wallpaper: &WallpaperRef, cache: Option<Cache>) {
        self.loader.set_cache(wallpaper, cache);
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

    /// Draws every output that needs it, and reports what the decode thread finished.
    ///
    /// An output is drawn when something changed outside the animation, or while its
    /// animation is still running. With everything settled this submits nothing at all,
    /// which is what makes an idle daemon free.
    ///
    /// The events come back rather than joining the queue `dispatch_queued` drains,
    /// because they are not queued on the Wayland connection and pretending otherwise
    /// would make that method's meaning depend on who called it.
    pub fn draw(
        &mut self,
        states: &BTreeMap<OutputId, MonitorState>,
    ) -> Result<Vec<RenderEvent>, RenderError> {
        let decoded = self.sync_textures(states)?;

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
        self.flush()?;
        Ok(decoded)
    }

    /// Takes in what the outputs are about to need and releases what they no longer show.
    ///
    /// Binds the offscreen surface first, because uploading and baking belong to no
    /// output and must not depend on whichever one happened to be drawn last.
    fn sync_textures(
        &mut self,
        states: &BTreeMap<OutputId, MonitorState>,
    ) -> Result<Vec<RenderEvent>, RenderError> {
        self.gl.make_current_offscreen()?;
        let (ready, lost) = self.loader.collect();
        let mut events: Vec<RenderEvent> = lost
            .into_iter()
            .map(|failed| RenderEvent::WallpaperFailed { wallpaper: failed.wallpaper })
            .collect();
        for loaded in ready {
            if loaded.stored {
                let wallpaper = loaded.wallpaper.clone();
                events.push(RenderEvent::WallpaperStored { wallpaper, asked: loaded.asked });
            }
            self.textures.accept(&self.gl.api, loaded)?;
        }

        // Only what an output is arriving at is worth decoding larger. What it is leaving
        // is still held, or its texture would go on the first frame of the crossfade.
        let mut wanted: HashMap<&WallpaperRef, PixelSize> = HashMap::new();
        let mut in_use: HashSet<WallpaperRef> = HashSet::new();
        for state in states.values() {
            let needed = decode::needed_size(
                state.buffer_size(),
                state.params.overview.zoom(),
                self.gl.max_texture_size(),
            );
            if let Some(wallpaper) = state.wallpaper.current() {
                wanted
                    .entry(wallpaper)
                    .and_modify(|size| *size = size.union(needed))
                    .or_insert(needed);
            }
            for wallpaper in
                [state.wallpaper.current(), state.wallpaper.outgoing()].into_iter().flatten()
            {
                in_use.insert(wallpaper.clone());
            }
        }

        for (wallpaper, needed) in &wanted {
            self.textures.ensure(&mut self.loader, wallpaper, *needed);
        }

        self.textures.retain(&self.gl.api, &in_use);
        self.loader.retain(&in_use);
        self.sync_blurs(states)?;
        Ok(events)
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
            for wallpaper in
                [state.wallpaper.current(), state.wallpaper.outgoing()].into_iter().flatten()
            {
                let Some(key) = blur_key(state, wallpaper) else { continue };
                if wants_blur(state) && !needed.contains(&key) {
                    needed.push(key.clone());
                }
                resident.insert(key);
            }
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

        let Some((sharp, baked)) = layer(&self.textures, &self.blurs, state, wallpaper) else {
            return Ok(());
        };

        // One blur factor covers the whole frame, so a layer that cannot be sampled at it
        // leaves rather than being drawn at another. That degrades to an instant swap.
        let outgoing = state
            .wallpaper
            .outgoing()
            .filter(|_| state.wallpaper.fade() < 1.0)
            .and_then(|previous| layer(&self.textures, &self.blurs, state, previous))
            .filter(|(_, previous_baked)| previous_baked.is_some() || baked.is_none());

        // With no bake to sample, a zero factor makes the shader skip its second fetch,
        // so the sharp texture standing in for it is never read.
        let blur = if baked.is_some() { state.blur.amount.value() } else { 0.0 };
        let uv = state.sample_rect(sharp.size());
        let (previous, uv_previous, mix) = match outgoing {
            Some((previous_sharp, previous_baked)) => (
                Some((previous_sharp, previous_baked.unwrap_or(previous_sharp))),
                state.sample_rect(previous_sharp.size()),
                state.wallpaper.fade(),
            ),
            None => (None, uv, 1.0),
        };

        let Some(target) = self.targets.get_mut(id) else { return Ok(()) };
        self.gl.make_current(target)?;
        tracing::trace!(
            output = %id,
            scroll = state.scroll.v.value(),
            blur,
            mix,
            settled = state.is_settled(),
            "presenting"
        );
        let frame = Frame {
            sharp,
            blurred: baked.unwrap_or(sharp),
            previous,
            uv,
            uv_previous,
            blur,
            mix,
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

/// The bake this wallpaper is given on this output, if it is configured for one at all.
fn blur_key(state: &MonitorState, wallpaper: &WallpaperRef) -> Option<BlurKey> {
    state.params.blur.is_enabled().then(|| BlurKey::new(wallpaper, &state.params.blur))
}

/// The textures one wallpaper is drawn from here: the sharp one, and its bake where this
/// output has one. `None` for a wallpaper that is not resident at all.
fn layer<'a>(
    textures: &'a TextureCache,
    blurs: &'a BlurCache,
    state: &MonitorState,
    wallpaper: &WallpaperRef,
) -> Option<(&'a Texture, Option<&'a Texture>)> {
    let sharp = textures.get(wallpaper)?;
    Some((sharp, blur_key(state, wallpaper).and_then(|key| blurs.get(&key))))
}

/// Whether this output is showing its blur or on the way to it. Baking is deferred until
/// then, so a monitor that never takes focus never pays for one.
fn wants_blur(state: &MonitorState) -> bool {
    state.blur.amount.value() > 0.0 || state.blur.amount.target() > 0.0
}
