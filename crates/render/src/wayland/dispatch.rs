use domain::{LogicalSize, OutputId, Scale};
use tracing::{debug, warn};
use wayland_client::globals::GlobalListContents;
use wayland_client::protocol::wl_callback::{self, WlCallback};
use wayland_client::protocol::wl_compositor::WlCompositor;
use wayland_client::protocol::wl_output::{self, WlOutput};
use wayland_client::protocol::wl_region::WlRegion;
use wayland_client::protocol::wl_registry::{self, WlRegistry};
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, delegate_noop};
use wayland_protocols::wp::fractional_scale::v1::client::wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1;
use wayland_protocols::wp::fractional_scale::v1::client::wp_fractional_scale_v1::{
    self, WpFractionalScaleV1,
};
use wayland_protocols::wp::viewporter::client::wp_viewport::WpViewport;
use wayland_protocols::wp::viewporter::client::wp_viewporter::WpViewporter;
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::ZwlrLayerShellV1;
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::{
    self, Anchor, KeyboardInteractivity, ZwlrLayerSurfaceV1,
};

use super::globals::OUTPUT_VERSION;
use super::surface::Pacing;
use super::{Output, State, Surface};
use crate::event::RenderEvent;

/// Cover the whole output and ignore every other exclusive zone, so panels do not carve
/// pieces out of the wallpaper.
const ANCHORS: Anchor = Anchor::Top.union(Anchor::Bottom).union(Anchor::Left).union(Anchor::Right);
const IGNORE_EXCLUSIVE_ZONES: i32 = -1;

delegate_noop!(State: ignore WlCompositor);
delegate_noop!(State: ignore WlSurface);
delegate_noop!(State: ignore WlRegion);
delegate_noop!(State: ignore ZwlrLayerShellV1);
delegate_noop!(State: ignore WpViewporter);
delegate_noop!(State: ignore WpViewport);
delegate_noop!(State: ignore WpFractionalScaleManagerV1);

impl State {
    pub(super) fn add_output(
        &mut self,
        registry: &WlRegistry,
        name: u32,
        version: u32,
        qh: &QueueHandle<Self>,
    ) {
        let version = version.min(*OUTPUT_VERSION.end());
        if version < *OUTPUT_VERSION.start() {
            warn!(version, "ignoring an output that cannot report its connector name");
            return;
        }
        let wl_output = registry.bind::<WlOutput, _, _>(name, version, qh, name);
        let output = Output { wl_output, connector: None, integer_scale: 1, surface: None };
        self.outputs.insert(name, output);
    }

    fn remove_output(&mut self, name: u32) {
        let Some(output) = self.outputs.remove(&name) else { return };
        if let Some(surface) = output.surface {
            debug!(output = %surface.id, "output went away");
            surface.destroy();
            self.events.push(RenderEvent::OutputGone { id: surface.id });
        }
        output.wl_output.release();
    }

    /// Creates the layer surface once the connector name is known. Before that there is
    /// no identity to key configuration or state on.
    fn ensure_surface(&mut self, name: u32, qh: &QueueHandle<Self>) {
        let (wl_output, id, integer_scale) = {
            let Some(output) = self.outputs.get(&name) else { return };
            if output.surface.is_some() {
                return;
            }
            let Some(connector) = output.connector.as_ref() else { return };
            (output.wl_output.clone(), OutputId::new(connector.clone()), output.integer_scale)
        };

        debug!(output = %id, namespace = %self.namespace, "creating layer surface");
        let surface = self.build_surface(&wl_output, id, integer_scale, qh);
        if let Some(output) = self.outputs.get_mut(&name) {
            output.surface = Some(surface);
        }
    }

    fn build_surface(
        &self,
        wl_output: &WlOutput,
        id: OutputId,
        integer_scale: u32,
        qh: &QueueHandle<Self>,
    ) -> Surface {
        let wl_surface = self.globals.compositor.create_surface(qh, ());
        let layer = self.globals.layer_shell.get_layer_surface(
            &wl_surface,
            Some(wl_output),
            self.layer,
            self.namespace.clone(),
            qh,
            id.clone(),
        );
        layer.set_anchor(ANCHORS);
        layer.set_exclusive_zone(IGNORE_EXCLUSIVE_ZONES);
        layer.set_size(0, 0);
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);

        let viewport = self.globals.viewporter.get_viewport(&wl_surface, qh, ());
        let fractional =
            self.globals.fractional_scale.get_fractional_scale(&wl_surface, qh, id.clone());

        // The compositor answers a bare commit with the configure that sets the size.
        wl_surface.commit();

        Surface {
            id,
            wl_surface,
            layer,
            viewport,
            fractional,
            logical: LogicalSize::default(),
            scale: Scale::from_integer(integer_scale),
            configured: false,
            pacing: Pacing { dirty: true, ..Pacing::default() },
        }
    }

    fn surface_by_id(&mut self, id: &OutputId) -> Option<&mut Surface> {
        self.outputs.values_mut().filter_map(|output| output.surface.as_mut()).find(|s| &s.id == id)
    }

    /// Reports geometry that the daemon may need to act on, but only once the surface is
    /// actually usable.
    fn announce(&mut self, id: &OutputId) {
        let Some(surface) = self.surface_by_id(id) else { return };
        if !surface.is_drawable() {
            return;
        }
        let event = RenderEvent::OutputReady {
            id: surface.id.clone(),
            logical: surface.logical,
            scale: surface.scale,
        };
        self.events.push(event);
    }
}

impl Dispatch<WlRegistry, GlobalListContents> for State {
    fn event(
        state: &mut Self,
        registry: &WlRegistry,
        event: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_registry::Event::Global { name, interface, version }
                if interface == WlOutput::interface().name =>
            {
                state.add_output(registry, name, version, qh);
            }
            wl_registry::Event::GlobalRemove { name } => state.remove_output(name),
            _ => {}
        }
    }
}

impl Dispatch<WlOutput, u32> for State {
    fn event(
        state: &mut Self,
        _: &WlOutput,
        event: wl_output::Event,
        &name: &u32,
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_output::Event::Name { name: connector } => {
                if let Some(output) = state.outputs.get_mut(&name) {
                    output.connector = Some(connector);
                }
            }
            wl_output::Event::Scale { factor } => {
                let Some(output) = state.outputs.get_mut(&name) else { return };
                output.integer_scale = factor.max(1) as u32;
            }
            wl_output::Event::Done => state.ensure_surface(name, qh),
            _ => {}
        }
    }
}

impl Dispatch<ZwlrLayerSurfaceV1, OutputId> for State {
    fn event(
        state: &mut Self,
        layer: &ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        id: &OutputId,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_layer_surface_v1::Event::Configure { serial, width, height } => {
                layer.ack_configure(serial);
                let Some(surface) = state.surface_by_id(id) else { return };
                surface.configured = true;
                let changed = surface.set_logical(LogicalSize::new(width, height));
                if changed || surface.pacing.dirty {
                    state.announce(id);
                }
            }
            zwlr_layer_surface_v1::Event::Closed => {
                debug!(output = %id, "the compositor closed our layer surface");
                state.events.push(RenderEvent::OutputGone { id: id.clone() });
                if let Some(output) = state
                    .outputs
                    .values_mut()
                    .find(|o| o.surface.as_ref().is_some_and(|s| &s.id == id))
                    && let Some(surface) = output.surface.take()
                {
                    surface.destroy();
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<WpFractionalScaleV1, OutputId> for State {
    fn event(
        state: &mut Self,
        _: &WpFractionalScaleV1,
        event: wp_fractional_scale_v1::Event,
        id: &OutputId,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let wp_fractional_scale_v1::Event::PreferredScale { scale } = event else { return };
        let Some(surface) = state.surface_by_id(id) else { return };
        if surface.set_scale(Scale::from_120ths(scale)) {
            debug!(output = %id, scale = %surface.scale, "fractional scale changed");
            state.announce(id);
        }
    }
}

impl Dispatch<WlCallback, OutputId> for State {
    fn event(
        state: &mut Self,
        _: &WlCallback,
        event: wl_callback::Event,
        id: &OutputId,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let wl_callback::Event::Done { .. } = event else { return };
        let Some(surface) = state.surface_by_id(id) else { return };
        surface.pacing.awaiting_frame = false;
        state.events.push(RenderEvent::FrameDue { id: id.clone() });
    }
}
