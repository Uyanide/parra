use domain::{LogicalSize, OutputId, Scale};
use wayland_client::QueueHandle;
use wayland_client::protocol::wl_compositor::WlCompositor;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_protocols::wp::fractional_scale::v1::client::wp_fractional_scale_v1::WpFractionalScaleV1;
use wayland_protocols::wp::viewporter::client::wp_viewport::WpViewport;
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::ZwlrLayerSurfaceV1;

use super::State;

/// One monitor's layer surface and the objects that describe its geometry. The native
/// EGL target lives in a parallel map in the renderer.
pub struct Surface {
    pub id: OutputId,
    pub wl_surface: WlSurface,
    pub layer: ZwlrLayerSurfaceV1,
    pub viewport: WpViewport,
    pub fractional: WpFractionalScaleV1,

    pub logical: LogicalSize,
    /// From `wp_fractional_scale_v1` once it arrives, from `wl_output` until then.
    pub scale: Scale,
    pub configured: bool,
    pub pacing: Pacing,
}

/// What an output should do with the current pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pass {
    /// Everything this output owes is already on screen.
    Skip,
    /// A frame is owed, but the compositor has not asked for one yet.
    Defer,
    Draw,
}

/// The flags that together decide when an output submits a frame.
///
/// One struct with one decision function, because the rule has to satisfy two things at
/// once: an idle daemon must submit nothing at all, and no animation may end on a value
/// that was never drawn. Both failures look identical from outside, a wallpaper stuck at
/// the wrong offset, so the rule is kept where it can be tested without a display.
#[derive(Clone, Copy, Debug, Default)]
pub struct Pacing {
    /// Something other than an animation needs a redraw: a configure, a scale change, a
    /// new wallpaper.
    pub dirty: bool,
    /// A frame has been committed and its callback has not come back yet. Drawing again
    /// before it does would queue work the compositor has not asked for.
    pub awaiting_frame: bool,
    /// The last frame presented was mid-animation, so the value it settles on still owes
    /// one. Without this an animation that finishes between two draws is never shown.
    pub was_running: bool,
}

impl Pacing {
    pub fn plan(self, drawable: bool, settled: bool) -> Pass {
        if !drawable || !(self.dirty || !settled || self.was_running) {
            return Pass::Skip;
        }
        if self.awaiting_frame {
            return Pass::Defer;
        }
        Pass::Draw
    }

    /// Records a frame that has just been handed to the compositor.
    ///
    /// Must be called before presenting rather than after: presenting reads the Wayland
    /// connection, so this frame's own callback can arrive while it is still running.
    pub fn submitted(&mut self, settled: bool) {
        self.dirty = false;
        self.was_running = !settled;
        self.awaiting_frame = true;
    }
}

impl Surface {
    pub fn is_drawable(&self) -> bool {
        self.configured && !self.logical.is_empty()
    }

    /// Records a new logical size, reporting whether it actually changed.
    pub fn set_logical(&mut self, logical: LogicalSize) -> bool {
        if self.logical == logical {
            return false;
        }
        self.logical = logical;
        self.pacing.dirty = true;
        true
    }

    pub fn set_scale(&mut self, scale: Scale) -> bool {
        if self.scale == scale {
            return false;
        }
        self.scale = scale;
        self.pacing.dirty = true;
        true
    }

    /// Tells the compositor how large the buffer should appear, and how much of it is
    /// opaque. Must be called before the commit that presenting performs.
    ///
    /// - The viewport maps a device-pixel buffer back to logical coordinates, which is
    ///   what stops a fractional scale from blurring the result.
    /// - Declaring the surface opaque lets the compositor skip blending and consider it
    ///   for direct scanout, so `opaque` is answered per frame in [`Renderer::draw`].
    /// - An empty region is published for a translucent frame. The region is
    ///   double-buffered state and would otherwise stand from the last commit.
    pub fn apply_geometry(&self, compositor: &WlCompositor, qh: &QueueHandle<State>, opaque: bool) {
        if self.logical.is_empty() {
            return;
        }
        let (width, height) = (self.logical.w as i32, self.logical.h as i32);
        self.viewport.set_destination(width, height);

        let region = compositor.create_region(qh, ());
        if opaque {
            region.add(0, 0, width, height);
        }
        self.wl_surface.set_opaque_region(Some(&region));
        region.destroy();
    }

    pub fn destroy(&self) {
        self.fractional.destroy();
        self.viewport.destroy();
        self.layer.destroy();
        self.wl_surface.destroy();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DRAWABLE: bool = true;
    const SETTLED: bool = true;
    const RUNNING: bool = false;

    /// One full pass: what is owed, then the callback that answers it.
    fn present(pacing: &mut Pacing, settled: bool) {
        pacing.submitted(settled);
        pacing.awaiting_frame = false;
    }

    #[test]
    fn an_idle_output_submits_nothing() {
        assert_eq!(Pacing::default().plan(DRAWABLE, SETTLED), Pass::Skip);
    }

    #[test]
    fn a_running_animation_draws_on_every_pass() {
        assert_eq!(Pacing::default().plan(DRAWABLE, RUNNING), Pass::Draw);
    }

    #[test]
    fn the_value_an_animation_settled_on_is_still_drawn() {
        // The frame on screen is mid-animation, and the target was reached between two
        // passes. Skipping here is what left the wallpaper one workspace behind.
        let mut pacing = Pacing::default();
        present(&mut pacing, RUNNING);
        assert_eq!(pacing.plan(DRAWABLE, SETTLED), Pass::Draw);
    }

    #[test]
    fn that_final_frame_is_drawn_once_and_then_the_output_goes_quiet() {
        let mut pacing = Pacing::default();
        present(&mut pacing, RUNNING);
        present(&mut pacing, SETTLED);
        assert_eq!(pacing.plan(DRAWABLE, SETTLED), Pass::Skip);
    }

    #[test]
    fn a_frame_arriving_during_its_own_present_is_not_overwritten() {
        // What `submitted` being called before the swap buys: the callback can land
        // inside it, and the wait it clears must stay cleared.
        let mut pacing = Pacing::default();
        pacing.submitted(RUNNING);
        pacing.awaiting_frame = false;
        assert_eq!(pacing.plan(DRAWABLE, RUNNING), Pass::Draw);
    }

    #[test]
    fn an_output_still_waiting_defers_instead_of_queueing_work() {
        let mut pacing = Pacing::default();
        pacing.submitted(RUNNING);
        assert_eq!(pacing.plan(DRAWABLE, RUNNING), Pass::Defer);
    }

    #[test]
    fn a_deferred_frame_is_not_forgotten_when_the_animation_settles() {
        let mut pacing = Pacing::default();
        pacing.submitted(RUNNING);
        assert_eq!(pacing.plan(DRAWABLE, SETTLED), Pass::Defer);

        pacing.dirty = true;
        pacing.awaiting_frame = false;
        assert_eq!(pacing.plan(DRAWABLE, SETTLED), Pass::Draw);
    }

    #[test]
    fn a_change_that_started_no_animation_is_still_drawn() {
        // A snap of no duration, or a reloaded parameter the rect is built from. Both
        // leave every value settled, so `dirty` is the whole of the evidence here.
        let mut pacing = Pacing::default();
        present(&mut pacing, SETTLED);
        assert_eq!(pacing.plan(DRAWABLE, SETTLED), Pass::Skip);

        pacing.dirty = true;
        assert_eq!(pacing.plan(DRAWABLE, SETTLED), Pass::Draw);
    }

    #[test]
    fn an_unconfigured_output_is_never_drawn() {
        let pacing = Pacing { dirty: true, ..Pacing::default() };
        assert_eq!(pacing.plan(false, RUNNING), Pass::Skip);
    }
}
