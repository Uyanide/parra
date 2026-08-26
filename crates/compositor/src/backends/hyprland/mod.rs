mod socket1;
mod translate;
mod wire;

use std::env;
use std::fmt;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use domain::Stop;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use tracing::{debug, info, warn};

use self::translate::Tracker;
use self::wire::Event;
use crate::backends::Scoped;
use crate::backends::lines::Lines;
use crate::{BackendError, CompositorBackend, EventSink};

pub const NAME: &str = "hyprland";

/// Names the directory both sockets live in, one per running compositor.
const SIGNATURE_VARIABLE: &str = "HYPRLAND_INSTANCE_SIGNATURE";
const RUNTIME_VARIABLE: &str = "XDG_RUNTIME_DIR";

const EVENTS_SOCKET: &str = ".socket2.sock";
const REQUEST_SOCKET: &str = ".socket.sock";

const FIRST_RETRY: Duration = Duration::from_millis(250);
const LONGEST_RETRY: Duration = Duration::from_secs(10);

/// Which of Hyprland's positions each parallax axis follows, and when an output blurs.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
pub struct Params {
    pub vertical: Axis,
    pub horizontal: Axis,
    pub blur: Blur,
}

impl Default for Params {
    /// Sideways, because that is the way Hyprland moves a workspace switch: its workspaces
    /// are one global row, and its own animation slides along it.
    ///
    /// The vertical axis stays off until it is asked for, since driving both from the one
    /// position Hyprland reports would only send the wallpaper diagonally.
    fn default() -> Self {
        Self { vertical: Axis::None, horizontal: Axis::Workspace, blur: Blur::DEFAULT }
    }
}

/// When an output blurs, and whether the outputs decide it one by one or together.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
pub struct Blur {
    pub when: When,
    pub scope: Scope,
}

impl Blur {
    const DEFAULT: Self = Self { when: When::NonEmpty, scope: Scope::Output };
}

impl Default for Blur {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// What an output has to reach to be blurred.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum When {
    /// It holds the focused window.
    Focused,
    /// The workspace it is showing holds at least one window, whether or not one is
    /// focused.
    #[default]
    NonEmpty,
}

/// Whose answer to [`When`] an output reads.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Scope {
    /// Every output blurs as soon as one of them reaches it.
    Global,
    /// Each output answers for itself.
    #[default]
    Output,
}

/// One position Hyprland exposes that an axis can follow.
///
/// No `column`, unlike niri: Hyprland's layouts report no position within a workspace, so
/// there is nothing a second axis could follow that the first does not already.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Axis {
    /// The workspace the output is showing in its monitor's live numeric topology.
    Workspace,
    /// Nothing, which leaves the axis centred.
    #[default]
    None,
}

impl Params {
    /// Where one axis sits, given what it was configured to follow.
    fn axis(&self, axis: Axis, workspace: Stop) -> Stop {
        match axis {
            Axis::Workspace => workspace,
            Axis::None => Stop::CENTRED,
        }
    }
}

impl fmt::Display for Params {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "vertical={},horizontal={},blur.when={},blur.scope={}",
            self.vertical, self.horizontal, self.blur.when, self.blur.scope
        )
    }
}

impl Axis {
    /// The spelling a configuration file uses, which is what serde reads.
    const fn as_str(self) -> &'static str {
        match self {
            Axis::Workspace => "workspace",
            Axis::None => "none",
        }
    }
}

impl fmt::Display for Axis {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl When {
    /// The spelling a configuration file uses, which is what serde reads.
    const fn as_str(self) -> &'static str {
        match self {
            When::Focused => "focused",
            When::NonEmpty => "non-empty",
        }
    }
}

impl fmt::Display for When {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Scope {
    /// The spelling a configuration file uses, which is what serde reads.
    const fn as_str(self) -> &'static str {
        match self {
            Scope::Global => "global",
            Scope::Output => "output",
        }
    }
}

impl fmt::Display for Scope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

pub struct Backend {
    dir: PathBuf,
    settings: Scoped<Params>,
}

/// Whether Hyprland is the compositor running here.
pub fn is_running() -> bool {
    directory().is_some_and(|dir| dir.join(EVENTS_SOCKET).exists())
}

fn directory() -> Option<PathBuf> {
    let signature = env::var_os(SIGNATURE_VARIABLE).filter(|value| !value.is_empty())?;
    let runtime = env::var_os(RUNTIME_VARIABLE).filter(|value| !value.is_empty())?;
    Some(Path::new(&runtime).join("hypr").join(signature))
}

impl Backend {
    pub fn connect(settings: Scoped<Params>) -> Result<Self, BackendError> {
        let dir = directory().ok_or(BackendError::Unavailable { backend: NAME })?;
        // Said once rather than left to be guessed at, since an effect that will never
        // move otherwise looks like one that is stuck.
        info!(backend = NAME, "no wider view of an output is reported, so nothing drives the zoom");
        let backend = Self { dir, settings };
        if backend.tracks_windows() {
            debug!(
                backend = NAME,
                "an output's blur follows whether its workspace is empty, so windows are followed"
            );
        }
        Ok(backend)
    }

    /// Whether any output blurs on whether a workspace is empty, which is the one setting
    /// that costs a snapshot of the open windows and the bookkeeping to follow it.
    fn tracks_windows(&self) -> bool {
        self.settings.all().any(|params| params.blur.when == When::NonEmpty)
    }
}

impl CompositorBackend for Backend {
    fn name(&self) -> &'static str {
        NAME
    }

    /// Reconnects for as long as the daemon is running, so a compositor restart leaves the
    /// last known state on screen and starts updating it again by itself.
    ///
    /// Always to the same directory. The signature is fixed for the life of one compositor,
    /// and a restart takes this daemon's own Wayland connection with it, so hunting for a
    /// newer instance would only ever attach to a session that is not ours.
    fn run(&mut self, sink: &dyn EventSink) -> Result<(), BackendError> {
        let mut retry = FIRST_RETRY;
        while sink.is_open() {
            let started = Instant::now();
            match self.session(sink) {
                Ok(()) => return Ok(()),
                Err(error) => warn!(backend = NAME, %error, "connection lost, will retry"),
            }
            if !sink.is_open() {
                break;
            }
            // A session that held for longer than the longest wait was a working one, so
            // what ends it is a new outage and waits from the shortest again.
            if started.elapsed() >= LONGEST_RETRY {
                retry = FIRST_RETRY;
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
        // Listening before asking, and not the other way round. The event stream carries
        // only changes, so anything that happens while the snapshot is being fetched has to
        // already be queued here or it is lost outright.
        let mut stream = Lines::connect(&self.dir.join(EVENTS_SOCKET), NAME)?;
        info!(backend = NAME, dir = %self.dir.display(), "watching the compositor");

        let mut tracker = Tracker::new(self.tracks_windows());
        self.seed(&mut tracker)?;
        for drive in tracker.drives(&self.settings) {
            sink.emit(drive);
        }

        while sink.is_open() {
            let Some(line) = stream.next_line()? else { continue };
            match wire::parse(&line) {
                Ok(Some(event)) => {
                    let reason = unanswered(&event);
                    tracker.apply(event);
                    if let Some(reason) = reason {
                        debug!(backend = NAME, reason, "asking what each monitor is showing");
                        self.resync(&mut tracker)?;
                    }
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

    /// Asks what is true right now, which is what this socket is used for at cold start.
    fn seed(&self, tracker: &mut Tracker) -> Result<(), BackendError> {
        // Monitors carry which one is focused and what each is showing; workspaces carry
        // the names those ids stand for, which is what places them in the travel; the
        // active window says whether anything at all holds the focus.
        let monitors = self.ask("j/monitors", "the monitors")?;
        let workspaces = self.ask("j/workspaces", "the workspaces")?;
        let window = self.ask("j/activewindow", "the focused window")?;
        // Which workspace each open window is on, asked for only where an output blurs
        // on that.
        let clients = if tracker.tracks_windows() {
            self.ask("j/clients", "the open windows")?
        } else {
            Vec::new()
        };

        tracker.seed(monitors, workspaces, window, clients);
        Ok(())
    }

    /// Re-reads compositor-owned topology for events whose payload is incomplete.
    fn resync(&self, tracker: &mut Tracker) -> Result<(), BackendError> {
        let monitors = self.ask("j/monitors", "the monitors")?;
        let workspaces = self.ask("j/workspaces", "the workspaces")?;
        tracker.resync(monitors, workspaces);
        Ok(())
    }

    /// One request, with the connection opened and closed around it.
    fn ask<T: DeserializeOwned>(&self, request: &str, what: &str) -> Result<T, BackendError> {
        let answer = socket1::ask(&self.dir.join(REQUEST_SOCKET), request, NAME)?;
        wire::decode(&answer).map_err(|error| BackendError::Protocol {
            backend: NAME,
            message: format!("cannot read {what}: {error}"),
        })
    }
}

/// Why one event leaves monitor topology incomplete, if it does.
fn unanswered(event: &Event) -> Option<&'static str> {
    match event {
        // Says which workspace but not whose. Most land on the focused monitor, and the
        // few a rule binds elsewhere would otherwise sit in the wrong monitor's row.
        Event::WorkspaceCreated { .. } => Some("a workspace was created"),
        Event::WorkspaceMoved { .. } => Some("a workspace moved"),
        Event::WorkspaceIdChanged { .. } => Some("a workspace was renumbered"),
        Event::MonitorAdded { .. } => Some("a monitor appeared"),
        Event::MonitorRemoved { .. } => Some("a monitor disappeared"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_axis_follows_only_the_resolved_workspace_stop() {
        let params = Params::default();
        let stop = Stop { at: 0.25, stride: 0.25 };
        assert_eq!(params.axis(Axis::Workspace, stop), stop);
        assert_eq!(params.axis(Axis::None, stop), Stop::CENTRED);
    }

    #[test]
    fn settings_report_only_live_configuration_fields() {
        assert_eq!(
            Params::default().to_string(),
            "vertical=none,horizontal=workspace,blur.when=non-empty,blur.scope=output"
        );
        let params = Params {
            blur: Blur { when: When::NonEmpty, scope: Scope::Global },
            ..Params::default()
        };
        assert_eq!(
            params.to_string(),
            "vertical=none,horizontal=workspace,blur.when=non-empty,blur.scope=global"
        );
    }

    #[test]
    fn only_incomplete_topology_events_require_resync() {
        for event in [
            Event::WorkspaceCreated { id: 1, name: "1".into() },
            Event::WorkspaceMoved { id: 1, name: "1".into(), monitor: "DP-1".into() },
            Event::WorkspaceIdChanged { from: 1, to: 2 },
            Event::MonitorAdded { name: "DP-1".into() },
            Event::MonitorRemoved { name: "DP-1".into() },
        ] {
            assert!(unanswered(&event).is_some());
        }
        assert!(unanswered(&Event::WorkspaceActive { id: 1, name: "1".into() }).is_none());
    }
}
