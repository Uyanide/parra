//! Compositor adaptation: the one place that knows how any particular compositor talks.

// TODO: The current design is exclusively niri-shaped and NOT actually adaptable to
//       other compositors.

pub mod backends;
pub mod event;

use std::io;

use thiserror::Error;

pub use event::CompositorEvent;

#[derive(Debug, Error)]
pub enum BackendError {
    #[error("{backend} is not running here")]
    Unavailable { backend: &'static str },
    #[error("{backend} connection failed")]
    Io {
        backend: &'static str,
        #[source]
        source: io::Error,
    },
    #[error("{backend} sent something unexpected: {message}")]
    Protocol { backend: &'static str, message: String },
}

/// A source of normalized compositor facts.
///
/// [`CompositorBackend::run`] blocks, so the daemon gives each backend a thread and
/// receives events through the sink. Reconnection therefore stays with the protocol
/// knowledge rather than in the event loop.
pub trait CompositorBackend: Send {
    fn name(&self) -> &'static str;

    /// Runs until the sink closes or the connection fails unrecoverably.
    fn run(&mut self, sink: &dyn EventSink) -> Result<(), BackendError>;
}

/// Where a backend delivers what it observed, implemented by the daemon over its event
/// loop.
///
/// Only ever used from the backend's own thread, so it carries no thread-safety bound.
pub trait EventSink {
    fn emit(&self, event: CompositorEvent);

    /// False once the daemon is shutting down; a backend should return from `run`.
    fn is_open(&self) -> bool;
}
