use std::ffi::c_void;

use domain::PixelSize;
use glow::HasContext;
use khronos_egl as egl;
use tracing::{debug, info};
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Proxy};
use wayland_egl::WlEglSurface;

use crate::error::RenderError;
use crate::gl::timer::{self, FrameCost, Timer};

/// `EGL_PLATFORM_WAYLAND_KHR`. The platform entry point is what lets the EGL
/// implementation pick the device the compositor asked for. See `docs/environment.md`.
const PLATFORM_WAYLAND: egl::Enum = 0x31D8;
const OPENGL_ES3_BIT: egl::Int = 0x0040;

/// Swapping must not block: frames are paced by `wl_surface.frame` callbacks, and a
/// blocking swap would stall the whole single-threaded event loop behind one monitor.
const SWAP_IMMEDIATE: egl::Int = 0;

/// Smallest `GL_MAX_TEXTURE_SIZE` GLES 3.0 permits, and therefore the floor for what a
/// driver's answer is believed to mean.
const MIN_MAX_TEXTURE: u32 = 2048;

/// One EGL display and one context, shared by every output, which is what lets two
/// monitors sample the same wallpaper texture instead of holding a copy each.
pub struct Gl {
    egl: egl::Instance<egl::Static>,
    display: egl::Display,
    config: egl::Config,
    context: egl::Context,
    /// Bound for work that belongs to no output: uploading textures and baking blurs.
    /// A 1x1 pbuffer rather than a surfaceless binding, because that is an extension and
    /// its absence would fail silently, leaving uploads to go nowhere.
    offscreen: egl::Surface,
    /// Whether this driver can time a frame. Decided once, because an extension does not
    /// appear halfway through a session.
    timing: bool,
    /// `GL_MAX_TEXTURE_SIZE`, the largest edge this driver will allocate. Asked once: it
    /// is a property of the implementation and does not change.
    max_texture: u32,
    pub api: glow::Context,
}

impl Gl {
    pub fn new(connection: &Connection) -> Result<Self, RenderError> {
        let egl = egl::Instance::new(egl::Static);
        let display_ptr = connection.backend().display_ptr().cast::<c_void>();

        let display =
            unsafe { egl.get_platform_display(PLATFORM_WAYLAND, display_ptr, &[egl::ATTRIB_NONE]) }
                .map_err(|source| RenderError::Egl {
                    operation: "eglGetPlatformDisplay",
                    source,
                })?;

        let (major, minor) = egl
            .initialize(display)
            .map_err(|source| RenderError::Egl { operation: "eglInitialize", source })?;
        debug!(major, minor, "EGL initialized");

        egl.bind_api(egl::OPENGL_ES_API)
            .map_err(|source| RenderError::Egl { operation: "eglBindAPI", source })?;

        let config = choose_config(&egl, display)?;
        let context = egl
            .create_context(
                display,
                config,
                None,
                &[egl::CONTEXT_MAJOR_VERSION, 3, egl::CONTEXT_MINOR_VERSION, 0, egl::NONE],
            )
            .map_err(|source| RenderError::Egl { operation: "eglCreateContext", source })?;

        let offscreen = egl
            .create_pbuffer_surface(display, config, &[egl::WIDTH, 1, egl::HEIGHT, 1, egl::NONE])
            .map_err(|source| RenderError::Egl { operation: "eglCreatePbufferSurface", source })?;
        bind(&egl, display, offscreen, context)?;

        let api = unsafe {
            glow::Context::from_loader_function(|name| {
                egl.get_proc_address(name).map_or(std::ptr::null(), |address| address as *const _)
            })
        };

        let timing = api.supported_extensions().contains(timer::EXTENSION);
        if !timing {
            info!(
                extension = timer::EXTENSION,
                "this driver cannot time a frame, so the GPU cost will not be reported"
            );
        }

        // GLES3 guarantees at least 2048, so a driver reporting less is reporting nonsense
        // and the guaranteed minimum is a better answer than a texture nothing can hold.
        let reported = unsafe { api.get_parameter_i32(glow::MAX_TEXTURE_SIZE) };
        let max_texture = u32::try_from(reported).unwrap_or(0).max(MIN_MAX_TEXTURE);
        debug!(max_texture, "largest texture this driver will allocate");

        Ok(Self { egl, display, config, context, offscreen, timing, max_texture, api })
    }

    /// Largest edge a texture may have here. The size policy clamps to it, so a deep zoom
    /// on a large monitor asks for something the driver can actually give.
    pub fn max_texture_size(&self) -> u32 {
        self.max_texture
    }

    /// Binds the context to work that belongs to no output: every texture upload and
    /// every blur bake goes through here.
    pub fn make_current_offscreen(&self) -> Result<(), RenderError> {
        bind(&self.egl, self.display, self.offscreen, self.context)
    }

    /// Creates the native surface for one output. Fails only if the compositor has
    /// already taken the underlying surface away.
    pub fn target(
        &self,
        surface: &WlSurface,
        size: PixelSize,
        output: &str,
    ) -> Result<Target, RenderError> {
        let size = size.max_one();
        let window = WlEglSurface::new(surface.id(), size.w as i32, size.h as i32)
            .map_err(|_| RenderError::NativeSurface { output: output.to_owned() })?;

        let egl_surface = unsafe {
            self.egl.create_window_surface(
                self.display,
                self.config,
                window.ptr().cast_mut().cast(),
                None,
            )
        }
        .map_err(|source| RenderError::Egl { operation: "eglCreateWindowSurface", source })?;

        let timer = self.timing.then(|| Timer::new(&self.api)).flatten();
        Ok(Target { window, surface: egl_surface, size, timer })
    }

    pub fn make_current(&self, target: &Target) -> Result<(), RenderError> {
        bind(&self.egl, self.display, target.surface, self.context)?;
        self.egl
            .swap_interval(self.display, SWAP_IMMEDIATE)
            .map_err(|source| RenderError::Egl { operation: "eglSwapInterval", source })
    }

    /// Presents the frame. This is also what commits the Wayland surface.
    pub fn swap(&self, target: &Target) -> Result<(), RenderError> {
        self.egl
            .swap_buffers(self.display, target.surface)
            .map_err(|source| RenderError::Egl { operation: "eglSwapBuffers", source })
    }

    pub fn destroy_target(&self, target: Target) {
        if let Some(timer) = target.timer {
            timer.destroy(&self.api);
        }
        let _ = self.egl.destroy_surface(self.display, target.surface);
    }
}

impl Drop for Gl {
    fn drop(&mut self) {
        let _ = self.egl.make_current(self.display, None, None, None);
        let _ = self.egl.destroy_surface(self.display, self.offscreen);
        let _ = self.egl.destroy_context(self.display, self.context);
        let _ = self.egl.terminate(self.display);
    }
}

fn bind(
    egl: &egl::Instance<egl::Static>,
    display: egl::Display,
    surface: egl::Surface,
    context: egl::Context,
) -> Result<(), RenderError> {
    egl.make_current(display, Some(surface), Some(surface), Some(context))
        .map_err(|source| RenderError::Egl { operation: "eglMakeCurrent", source })
}

/// One output's native surface, sized in device pixels.
pub struct Target {
    window: WlEglSurface,
    surface: egl::Surface,
    size: PixelSize,
    /// Times this output's frames. `None` when the driver has no timer, or refused the
    /// one it advertised. It lives here because it is born and dies with the surface.
    timer: Option<Timer>,
}

impl Target {
    /// Resizes in place. Recreating the EGL surface would drop the buffer chain and
    /// flash, which a scale change on a live monitor must not do.
    pub fn resize(&mut self, size: PixelSize) {
        let size = size.max_one();
        if size == self.size {
            return;
        }
        self.window.resize(size.w as i32, size.h as i32, 0, 0);
        self.size = size;
    }

    /// Draws this output's frame, timed where the driver has a timer.
    pub fn measure(&mut self, gl: &glow::Context, draw: impl FnOnce()) {
        let Some(timer) = self.timer.as_mut() else { return draw() };
        if !timer.measure(gl, draw)
            && let Some(refused) = self.timer.take()
        {
            refused.destroy(gl);
        }
    }

    /// Takes the last frame's measurement if the GPU has finished with it.
    pub fn collect(&mut self, gl: &glow::Context) {
        if let Some(timer) = self.timer.as_mut() {
            timer.collect(gl);
        }
    }

    pub fn cost(&self) -> FrameCost {
        self.timer.as_ref().map_or_else(FrameCost::default, Timer::cost)
    }
}

/// Asks for the plainest window configuration that can show an opaque photograph:
/// 8 bits per channel, no depth, no stencil, no multisampling.
///
/// Pbuffers are asked for alongside windows so one config serves both the outputs and
/// the offscreen surface, leaving no second choice to keep in agreement with this one.
fn choose_config(
    egl: &egl::Instance<egl::Static>,
    display: egl::Display,
) -> Result<egl::Config, RenderError> {
    let attributes = [
        egl::SURFACE_TYPE,
        egl::WINDOW_BIT | egl::PBUFFER_BIT,
        egl::RENDERABLE_TYPE,
        OPENGL_ES3_BIT,
        egl::RED_SIZE,
        8,
        egl::GREEN_SIZE,
        8,
        egl::BLUE_SIZE,
        8,
        egl::ALPHA_SIZE,
        8,
        egl::DEPTH_SIZE,
        0,
        egl::STENCIL_SIZE,
        0,
        egl::NONE,
    ];
    egl.choose_first_config(display, &attributes)
        .map_err(|source| RenderError::Egl { operation: "eglChooseConfig", source })?
        .ok_or(RenderError::NoEglConfig)
}
