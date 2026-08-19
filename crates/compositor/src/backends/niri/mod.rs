mod socket;
mod translate;
mod wire;

use std::env;
use std::fmt;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use serde::Deserialize;
use tracing::{info, warn};

use self::socket::Stream;
use self::translate::Tracker;
use crate::backends::Scoped;
use crate::{BackendError, CompositorBackend, EventSink};

pub const NAME: &str = "niri";

const SOCKET_VARIABLE: &str = "NIRI_SOCKET";

const FIRST_RETRY: Duration = Duration::from_millis(250);
const LONGEST_RETRY: Duration = Duration::from_secs(10);

/// Which of niri's positions each parallax axis follows.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
pub struct Params {
    pub vertical: Axis,
    pub horizontal: Axis,
}

impl Default for Params {
    /// The workspace is the movement niri always has, so the vertical axis follows it.
    ///
    /// The horizontal one stays off until it is asked for, since a scrolling layout with
    /// a single column has nowhere to travel.
    fn default() -> Self {
        Self { vertical: Axis::Workspace, horizontal: Axis::None }
    }
}

/// One position niri exposes that an axis can follow.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Axis {
    /// The active workspace among the ones on that output.
    Workspace,
    /// The focused column of that output's active workspace.
    Column,
    /// Nothing, which leaves the axis centred.
    #[default]
    None,
}

impl fmt::Display for Params {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "vertical={},horizontal={}", self.vertical, self.horizontal)
    }
}

impl Axis {
    /// The spelling a configuration file uses, which is what serde reads.
    const fn as_str(self) -> &'static str {
        match self {
            Axis::Workspace => "workspace",
            Axis::Column => "column",
            Axis::None => "none",
        }
    }
}

impl fmt::Display for Axis {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

pub struct Backend {
    socket: PathBuf,
    settings: Scoped<Params>,
}

/// Whether niri is the compositor running here.
pub fn is_running() -> bool {
    socket().is_some()
}

fn socket() -> Option<PathBuf> {
    env::var_os(SOCKET_VARIABLE).filter(|value| !value.is_empty()).map(PathBuf::from)
}

impl Backend {
    pub fn connect(settings: Scoped<Params>) -> Result<Self, BackendError> {
        let socket = socket().ok_or(BackendError::Unavailable { backend: NAME })?;
        Ok(Self { socket, settings })
    }
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
                    for drive in tracker.drives(&self.settings) {
                        sink.emit(drive);
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
