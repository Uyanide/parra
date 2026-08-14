mod dispatch;
mod globals;
mod surface;

use std::collections::HashMap;
use std::os::fd::{AsFd, BorrowedFd};

use domain::{OutputId, SurfaceParams};
use wayland_client::backend::WaylandError;
use wayland_client::globals::registry_queue_init;
use wayland_client::protocol::wl_output::WlOutput;
use wayland_client::{Connection, EventQueue, QueueHandle};
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::Layer;

use crate::error::RenderError;
use crate::event::RenderEvent;
use globals::Globals;
pub use surface::{Pass, Surface};

pub struct Wayland {
    connection: Connection,
    queue: EventQueue<State>,
    qh: QueueHandle<State>,
    state: State,
}

/// Dispatch target for every Wayland object.
pub(crate) struct State {
    globals: Globals,
    namespace: String,
    layer: Layer,
    /// Keyed by registry name, which is what `global_remove` reports.
    outputs: HashMap<u32, Output>,
    events: Vec<RenderEvent>,
}

struct Output {
    wl_output: WlOutput,
    /// Connector name from `wl_output` version 4. Until it arrives there is no way to
    /// tell which monitor this is, so no surface is created.
    connector: Option<String>,
    /// `wl_output.scale`, used until the fractional scale arrives.
    integer_scale: u32,
    surface: Option<Surface>,
}

impl Wayland {
    pub fn connect(params: &SurfaceParams) -> Result<Self, RenderError> {
        let connection =
            Connection::connect_to_env().map_err(|source| RenderError::Connect { source })?;
        let (globals, queue) = registry_queue_init::<State>(&connection)
            .map_err(|source| RenderError::Registry { source })?;
        let qh = queue.handle();

        let mut state = State {
            globals: Globals::bind(&globals, &qh)?,
            namespace: params.namespace.clone(),
            layer: match params.layer {
                domain::Layer::Background => Layer::Background,
                domain::Layer::Bottom => Layer::Bottom,
            },
            outputs: HashMap::new(),
            events: Vec::new(),
        };

        globals.contents().with_list(|list| {
            for global in list {
                if global.interface == "wl_output" {
                    state.add_output(globals.registry(), global.name, global.version, &qh);
                }
            }
        });

        let mut wayland = Self { connection, queue, qh, state };
        wayland.roundtrip()?;
        Ok(wayland)
    }

    pub fn fd(&self) -> BorrowedFd<'_> {
        self.connection.as_fd()
    }

    /// Reads whatever the compositor has sent and returns what the daemon must react to.
    pub fn dispatch(&mut self) -> Result<Vec<RenderEvent>, RenderError> {
        self.dispatch_pending()?;
        if let Some(guard) = self.queue.prepare_read() {
            match guard.read() {
                Ok(_) => {}
                Err(WaylandError::Io(error)) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(source) => return Err(RenderError::Disconnected { source }),
            }
        }
        self.dispatch_pending()?;
        Ok(std::mem::take(&mut self.state.events))
    }

    /// Delivers events already parsed into this queue, without touching the socket.
    ///
    /// EGL reads the same connection while presenting, so a frame callback can end up
    /// queued here with nothing left on the descriptor to wake a poll. Draining before
    /// sleeping is what stops that callback being lost and stranding an animation.
    pub fn dispatch_queued(&mut self) -> Result<Vec<RenderEvent>, RenderError> {
        self.dispatch_pending()?;
        Ok(std::mem::take(&mut self.state.events))
    }

    pub fn flush(&mut self) -> Result<(), RenderError> {
        match self.connection.flush() {
            Ok(()) => Ok(()),
            Err(WaylandError::Io(error)) if error.kind() == std::io::ErrorKind::WouldBlock => {
                Ok(())
            }
            Err(source) => Err(RenderError::Disconnected { source }),
        }
    }

    pub fn connection(&self) -> &Connection {
        &self.connection
    }

    pub fn surfaces(&self) -> impl Iterator<Item = &Surface> {
        self.state.outputs.values().filter_map(|output| output.surface.as_ref())
    }

    pub fn surfaces_mut(&mut self) -> impl Iterator<Item = &mut Surface> {
        self.state.outputs.values_mut().filter_map(|output| output.surface.as_mut())
    }

    pub fn surface_mut(&mut self, id: &OutputId) -> Option<&mut Surface> {
        self.surfaces_mut().find(|surface| &surface.id == id)
    }

    /// Republishes the surface geometry. Must happen before the commit that presenting
    /// performs, so the buffer and the size it claims to be arrive together.
    pub fn apply_geometry(&mut self, id: &OutputId) {
        let qh = self.qh.clone();
        let compositor = self.state.globals.compositor.clone();
        if let Some(surface) = self.surface_mut(id) {
            surface.apply_geometry(&compositor, &qh);
        }
    }

    /// Asks for one frame callback on this output. Called immediately before committing
    /// a frame, and only then, which is what keeps an idle output silent.
    pub fn request_frame(&mut self, id: &OutputId) {
        let qh = self.qh.clone();
        if let Some(surface) = self.surface_mut(id) {
            surface.wl_surface.frame(&qh, surface.id.clone());
        }
    }

    fn roundtrip(&mut self) -> Result<(), RenderError> {
        self.queue
            .roundtrip(&mut self.state)
            .map(|_| ())
            .map_err(|source| RenderError::Protocol { source })
    }

    fn dispatch_pending(&mut self) -> Result<(), RenderError> {
        self.queue
            .dispatch_pending(&mut self.state)
            .map(|_| ())
            .map_err(|source| RenderError::Protocol { source })
    }
}

impl Drop for Wayland {
    fn drop(&mut self) {
        for output in self.state.outputs.values() {
            if let Some(surface) = &output.surface {
                surface.destroy();
            }
        }
        let _ = self.connection.flush();
    }
}
