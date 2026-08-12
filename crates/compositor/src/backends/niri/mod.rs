mod socket;
mod translate;
mod wire;

use std::env;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use tracing::{info, warn};

use self::socket::Stream;
use self::translate::Tracker;
use crate::{BackendError, CompositorBackend, EventSink};

pub const NAME: &str = "niri";

const SOCKET_VARIABLE: &str = "NIRI_SOCKET";

const FIRST_RETRY: Duration = Duration::from_millis(250);
const LONGEST_RETRY: Duration = Duration::from_secs(10);

pub struct Backend {
    socket: PathBuf,
}

pub fn detect() -> Option<Backend> {
    let socket = env::var_os(SOCKET_VARIABLE).filter(|value| !value.is_empty())?;
    Some(Backend { socket: PathBuf::from(socket) })
}

impl CompositorBackend for Backend {
    fn name(&self) -> &'static str {
        NAME
    }

    /// Reconnects for as long as the daemon is running, so a compositor restart leaves
    /// the last known state on screen and starts updating it again by itself.
    fn run(&mut self, sink: &dyn EventSink) -> Result<(), BackendError> {
        let mut retry = FIRST_RETRY;
        while sink.is_open() {
            match self.session(sink) {
                Ok(()) => return Ok(()),
                Err(error) => warn!(backend = NAME, %error, "connection lost, will retry"),
            }
            if !sink.is_open() {
                break;
            }
            thread::sleep(retry);
            retry = (retry * 2).min(LONGEST_RETRY);
        }
        Ok(())
    }
}

impl Backend {
    /// One connection, from subscribing until it fails or the daemon stops.
    fn session(&self, sink: &dyn EventSink) -> Result<(), BackendError> {
        let mut stream = Stream::open(&self.socket, NAME)?;
        info!(backend = NAME, socket = %self.socket.display(), "watching the compositor");

        let mut tracker = Tracker::default();
        while sink.is_open() {
            let Some(line) = stream.next_line()? else { continue };
            match wire::parse(&line) {
                Ok(Some(event)) => {
                    tracker.apply(event);
                    for fact in tracker.facts() {
                        sink.emit(fact);
                    }
                }
                Ok(None) => {}
                // A modelled event that no longer parses means the format moved. Worth a
                // line explaining why the wallpaper stopped reacting, not a disconnect.
                Err(error) => {
                    warn!(backend = NAME, %error, "cannot read an event this daemon relies on");
                }
            }
        }
        Ok(())
    }
}
