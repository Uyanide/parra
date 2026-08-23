mod socket1;
mod translate;
mod wire;

use std::collections::HashSet;
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

/// The workspaces `1` through `10`, which is the block nearly every Hyprland
/// configuration binds to the number row.
const DEFAULT_SPAN: u32 = 10;

/// The longest span that still places anything: one stop of a thousand moves the wallpaper
/// by around a pixel, so a range this long is a typo rather than a configuration.
const LONGEST_SPAN: usize = 1000;

const EMPTY_SPAN: &str = "expected at least one workspace to travel through";

/// Which of Hyprland's positions each parallax axis follows, and what it travels through.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
pub struct Params {
    pub vertical: Axis,
    pub horizontal: Axis,
    pub span: Span,
}

impl Default for Params {
    /// Sideways, because that is the way Hyprland moves a workspace switch: its workspaces
    /// are one global row, and its own animation slides along it.
    ///
    /// The vertical axis stays off until it is asked for, since driving both from the one
    /// position Hyprland reports would only send the wallpaper diagonally.
    fn default() -> Self {
        Self { vertical: Axis::None, horizontal: Axis::Workspace, span: Span::Count(DEFAULT_SPAN) }
    }
}

/// One position Hyprland exposes that an axis can follow.
///
/// No `column`, unlike niri: Hyprland's layouts report no position within a workspace, so
/// there is nothing a second axis could follow that the first does not already.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Axis {
    /// The workspace the output is showing, placed by [`Span`].
    Workspace,
    /// Nothing, which leaves the axis centred.
    #[default]
    None,
}

/// The workspaces the travel spans, in the order they are travelled through.
///
/// Declared rather than counted, because Hyprland creates and destroys workspaces as they
/// are used: counting the live ones would change the length of the travel whenever a
/// workspace appeared or went away, moving the wallpaper with no user action behind it.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(try_from = "SpanFile")]
pub enum Span {
    /// How many there are, which names the workspaces `"1"` through `"n"`.
    Count(u32),
    /// What they are called, for workspaces a number does not describe.
    Names(Vec<String>),
}

/// What the file may say, before it is known to say anything usable.
#[derive(Deserialize)]
#[serde(untagged)]
enum SpanFile {
    Count(u32),
    Names(Vec<String>),
}

impl TryFrom<SpanFile> for Span {
    type Error = String;

    fn try_from(raw: SpanFile) -> Result<Self, Self::Error> {
        match raw {
            SpanFile::Count(0) => Err(EMPTY_SPAN.to_owned()),
            SpanFile::Count(count) if count as usize > LONGEST_SPAN => Err(too_long()),
            SpanFile::Count(count) => Ok(Span::Count(count)),
            SpanFile::Names(entries) => listed(entries),
        }
    }
}

/// The stops a written list names, with each range expanded where it stands.
///
/// A workspace listed twice is refused: two stops equally far from an unlisted workspace
/// are told apart by number, which needs every stop to carry a different one.
fn listed(entries: Vec<String>) -> Result<Span, String> {
    let mut names: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    for entry in entries {
        for name in expand(&entry) {
            if names.len() >= LONGEST_SPAN {
                return Err(too_long());
            }
            // Numbers are compared as numbers everywhere else, so `"1"` and `"01"` are the
            // one workspace written two ways rather than two stops.
            let key =
                name.parse::<i64>().map_or_else(|_| name.clone(), |number| number.to_string());
            if !seen.insert(key) {
                return Err(format!("expected the workspace `{name}` to appear once"));
            }
            names.push(name);
        }
    }

    if names.is_empty() { Err(EMPTY_SPAN.to_owned()) } else { Ok(Span::Names(names)) }
}

/// The workspaces one entry names: `"3-6"` is the range `"3"` to `"6"`, both ends included
/// and counting downward when it is written backwards.
///
/// Read as a range only between digits, so a workspace called `"my-project"`, or one named
/// for the negative id Hyprland gave it, stays a single name.
fn expand(entry: &str) -> Vec<String> {
    let single = || vec![entry.to_owned()];
    let Some((first, last)) = entry.split_once('-') else { return single() };
    let digits = |part: &str| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit());
    if !digits(first) || !digits(last) {
        return single();
    }
    let (Ok(first), Ok(last)) = (first.parse::<i64>(), last.parse::<i64>()) else {
        return single();
    };

    let step = if first <= last { 1 } else { -1 };
    // One stop past the longest span is all it takes to be refused, and building the rest
    // of a mistyped range is what the limit is there to avoid.
    let count = (first.abs_diff(last) + 1).min(LONGEST_SPAN as u64 + 1) as usize;
    (0..count).map(|n| (first + step * n as i64).to_string()).collect()
}

fn too_long() -> String {
    format!("expected at most {LONGEST_SPAN} workspaces to travel through")
}

impl Span {
    /// How many stops the travel has.
    fn len(&self) -> usize {
        match self {
            Span::Count(count) => *count as usize,
            Span::Names(names) => names.len(),
        }
    }

    /// Where one workspace sits in the travel, beside the distance one stop of it covers.
    ///
    /// `from` is the workspace this output was showing before, which only an unlisted
    /// workspace equally far from two stops has any use for.
    ///
    /// A lone workspace sits centred, having nothing to travel between, and reports no
    /// stride for the same reason.
    fn stop(&self, workspace: &str, from: Option<&str>) -> Stop {
        let count = self.len();
        if count <= 1 {
            return Stop::CENTRED;
        }
        let Some(at) = self.place(workspace, from) else { return Stop::CENTRED };
        let span = (count - 1) as f32;
        Stop { at: at as f32 / span, stride: 1.0 / span }
    }

    /// Which stop a workspace is at, or the nearest one when it is not a stop at all.
    ///
    /// A number lands on the nearest stop by number, which clamps anything past either end.
    /// Where the stops are names there is nothing to measure, and centred is the answer.
    ///
    /// Two stops equally far off are told apart by `from`, the nearer to it winning, so
    /// the wallpaper holds its side of the gap instead of crossing it and crossing back.
    ///
    /// With nothing behind it the lower-numbered stop wins, which with the ban on repeats
    /// leaves the answer independent of the order the span was written in.
    fn place(&self, workspace: &str, from: Option<&str>) -> Option<usize> {
        let wanted: Option<i64> = workspace.parse().ok();
        match self {
            Span::Count(count) => Some(wanted?.clamp(1, i64::from(*count)) as usize - 1),
            Span::Names(names) => {
                if let Some(at) = names.iter().position(|listed| listed == workspace) {
                    return Some(at);
                }
                let wanted = wanted?;
                let numbered: Vec<i64> =
                    names.iter().map(|name| name.parse().ok()).collect::<Option<_>>()?;
                let from: Option<i64> = from.and_then(|name| name.parse().ok());
                // Nearest, then the side `from` is on, then the lower number. Repeats are
                // refused, so the last key always settles it and the written order never
                // does.
                numbered
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, at)| {
                        (at.abs_diff(wanted), from.map_or(0, |from| at.abs_diff(from)), **at)
                    })
                    .map(|(at, _)| at)
            }
        }
    }
}

impl Params {
    /// Where one axis sits, given what it was configured to follow.
    ///
    /// A monitor nothing has named a workspace for sits centred, which is the neutral
    /// answer everywhere else too.
    fn axis(&self, axis: Axis, workspace: Option<&str>, from: Option<&str>) -> Stop {
        match (axis, workspace) {
            (Axis::Workspace, Some(name)) => self.span.stop(name, from),
            _ => Stop::CENTRED,
        }
    }
}

impl fmt::Display for Params {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "vertical={},horizontal={},span={}", self.vertical, self.horizontal, self.span)
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

impl fmt::Display for Span {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Span::Count(count) => write!(f, "{count}"),
            Span::Names(names) => write!(f, "[{}]", names.join(",")),
        }
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
        Ok(Self { dir, settings })
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

        let mut tracker = Tracker::default();
        self.seed(&mut tracker)?;
        for drive in tracker.drives(&self.settings) {
            sink.emit(drive);
        }

        while sink.is_open() {
            let Some(line) = stream.next_line()? else { continue };
            match wire::parse(&line) {
                Ok(Some(event)) => {
                    if let Some(reason) = unanswered(&event) {
                        debug!(backend = NAME, reason, "asking what each monitor is showing");
                        self.resync(&mut tracker)?;
                    }
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

    /// Asks what is true right now, which is what this socket is used for at cold start.
    fn seed(&self, tracker: &mut Tracker) -> Result<(), BackendError> {
        // Monitors carry which one is focused and what each is showing; workspaces carry
        // the names those ids stand for, which is what places them in the travel; the
        // active window says whether anything at all holds the focus.
        let monitors = self.ask("j/monitors", "the monitors")?;
        let workspaces = self.ask("j/workspaces", "the workspaces")?;
        let window = self.ask("j/activewindow", "the focused window")?;

        tracker.seed(monitors, workspaces, window);
        Ok(())
    }

    /// Re-reads what each monitor is showing, for the events that change it without saying
    /// what the monitors ended up with.
    fn resync(&self, tracker: &mut Tracker) -> Result<(), BackendError> {
        tracker.resync(self.ask("j/monitors", "the monitors")?);
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

/// Why one event leaves a monitor showing something nothing has stated, if it does.
///
/// A moved workspace says where it went but not what either monitor ended up with, a
/// renumbered one takes a name with it that the event does not carry, and a monitor that
/// has just appeared has not been told what it shows.
fn unanswered(event: &Event) -> Option<&'static str> {
    match event {
        Event::WorkspaceMoved { .. } => Some("a workspace moved"),
        Event::WorkspaceIdChanged { .. } => Some("a workspace was renumbered"),
        Event::MonitorAdded { .. } => Some("a monitor appeared"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(names: &[&str]) -> Span {
        Span::Names(names.iter().map(|name| (*name).to_owned()).collect())
    }

    /// A span as a file gives it, so the ranges and the refusals are exercised through the
    /// parsing that has to enforce them.
    fn written(span: &str) -> Result<Span, String> {
        serde_json::from_str(span).map_err(|error| error.to_string())
    }

    /// Where a workspace sits, to the nearest thousandth, which is enough to tell the
    /// stops of any usable span apart.
    fn at(span: &Span, workspace: &str) -> f32 {
        reached(span, workspace, None)
    }

    /// The same, for a workspace the output arrived at from `from`.
    fn reached(span: &Span, workspace: &str, from: Option<&str>) -> f32 {
        (span.stop(workspace, from).at * 1000.0).round() / 1000.0
    }

    #[test]
    fn a_count_spans_the_workspaces_numbered_one_upward() {
        let span = Span::Count(5);
        assert_eq!(at(&span, "1"), 0.0);
        assert_eq!(at(&span, "3"), 0.5);
        assert_eq!(at(&span, "5"), 1.0);
        assert_eq!(span.stop("2", None).stride, 0.25, "one stop of four");
    }

    #[test]
    fn a_named_span_travels_in_the_order_it_was_written() {
        let span = names(&["browser", "code", "mail"]);
        assert_eq!(at(&span, "browser"), 0.0);
        assert_eq!(at(&span, "code"), 0.5);
        assert_eq!(at(&span, "mail"), 1.0);
    }

    /// A number that is not a place: `["3", "1", "5"]` travels in that order, so `"1"` is
    /// the middle stop however it sorts.
    #[test]
    fn a_number_names_a_stop_rather_than_ordering_them() {
        assert_eq!(at(&names(&["3", "1", "5"]), "1"), 0.5);
    }

    #[test]
    fn a_workspace_past_either_end_clamps_to_it() {
        let span = Span::Count(5);
        assert_eq!(at(&span, "9"), 1.0, "past the last");
        assert_eq!(at(&span, "0"), 0.0, "before the first");
        assert_eq!(at(&span, "-1"), 0.0, "and a named workspace's negative id with it");

        let sparse = names(&["1", "3", "5"]);
        assert_eq!(at(&sparse, "7"), 1.0);
    }

    #[test]
    fn a_number_between_two_stops_takes_the_nearer_one() {
        let span = names(&["1", "3", "6"]);
        assert_eq!(at(&span, "2"), 0.0, "nearer 1 than 3");
        assert_eq!(at(&span, "5"), 1.0, "nearer 6 than 3");
    }

    /// Between two stops equally far off, the wallpaper stays on the side of the gap it
    /// came from rather than crossing it only to cross back on the way home.
    #[test]
    fn a_number_equally_far_from_two_stops_takes_the_one_it_came_from() {
        let span = names(&["3", "5"]);
        assert_eq!(reached(&span, "4", Some("1")), 0.0, "3 is nearer 1 than 5 is");
        assert_eq!(reached(&span, "4", Some("10")), 1.0, "and 5 is nearer 10");
        assert_eq!(reached(&span, "4", Some("3")), 0.0, "a stop counts as somewhere to be");
    }

    /// At startup, and for a workspace whose name is no number, there is no side to have
    /// come from, so the tie falls back to the lower-numbered stop.
    #[test]
    fn a_tie_with_nowhere_behind_it_takes_the_lower_number() {
        assert_eq!(at(&names(&["1", "3", "5"]), "4"), 0.5, "3 rather than 5");
        assert_eq!(at(&names(&["1", "2", "10"]), "6"), 0.5, "2 rather than 10");
        assert_eq!(reached(&names(&["3", "5"]), "4", Some("browser")), 0.0);
    }

    /// Where an entry was written decides where its stop sits in the travel, and nothing
    /// else: which stop an unlisted workspace lands on is settled by number alone.
    #[test]
    fn the_written_order_never_decides_which_stop_wins() {
        assert_eq!(reached(&names(&["3", "1", "5"]), "4", Some("1")), 0.0, "3, the first stop");
        assert_eq!(reached(&names(&["3", "1", "5"]), "4", Some("10")), 1.0, "5, the last stop");
        assert_eq!(at(&names(&["3", "1", "5"]), "4"), 0.0, "3 again, and first again");
        assert_eq!(at(&names(&["5", "1", "3"]), "4"), 1.0, "3 again, now the last stop");
    }

    /// The stops of a count are consecutive, so no workspace can sit between two of them
    /// and the question never arises.
    #[test]
    fn a_count_has_no_tie_to_break() {
        assert_eq!(reached(&Span::Count(5), "9", Some("1")), 1.0, "clamped either way");
    }

    #[test]
    fn a_range_expands_to_every_workspace_between_its_ends() {
        assert_eq!(written(r#"["3-6"]"#).unwrap(), names(&["3", "4", "5", "6"]));
        assert_eq!(written(r#"["6-3"]"#).unwrap(), names(&["6", "5", "4", "3"]), "backwards");
        assert_eq!(written(r#"["3-3"]"#).unwrap(), names(&["3"]), "one end to itself");
        assert_eq!(written(r#"["1-2","mail"]"#).unwrap(), names(&["1", "2", "mail"]), "in place");
    }

    /// A hyphen is a range only between digits, so it stays part of a workspace's name
    /// wherever it could not have been meant as one.
    #[test]
    fn a_hyphen_in_a_name_is_left_alone() {
        for entry in [r#"["my-project"]"#, r#"["-1"]"#, r#"["1-"]"#, r#"["1-2-3"]"#] {
            let raw = entry.trim_matches(|c| "[]\"".contains(c));
            assert_eq!(written(entry).unwrap(), names(&[raw]), "{entry}");
        }
    }

    /// Two stops carrying the same number cannot be told apart, so the span would place an
    /// unlisted workspace by where they were written rather than by what they are.
    #[test]
    fn a_workspace_listed_twice_is_refused() {
        assert!(written(r#"["1","2","1"]"#).is_err());
        assert!(written(r#"["1-3","2"]"#).is_err(), "a range and an entry inside it");
        assert!(written(r#"["1-3","3-5"]"#).is_err(), "two ranges that overlap");
        assert!(written(r#"["1","01"]"#).is_err(), "one number written two ways");
        assert!(written(r#"["mail","mail"]"#).is_err());
    }

    /// A mistyped range would otherwise be expanded in full before anything could refuse
    /// it, and a span this long has stopped placing anything either way.
    #[test]
    fn a_span_longer_than_anything_useful_is_refused() {
        assert!(written(r#"["1-1000"]"#).is_ok());
        assert!(written(r#"["1-1001"]"#).is_err());
        assert!(written(r#"["0-4000000000"]"#).is_err());
        assert!(written("100000").is_err(), "and a count that says the same thing");
    }

    /// Where the stops are names there is no distance to measure, so a stray workspace has
    /// no place in the travel at all.
    #[test]
    fn an_unlisted_workspace_sits_centred_when_the_span_is_named() {
        let span = names(&["browser", "code", "mail"]);
        assert_eq!(span.stop("scratch", None), Stop::CENTRED);
        assert_eq!(span.stop("2", None), Stop::CENTRED);
    }

    #[test]
    fn a_workspace_with_a_name_no_number_describes_sits_centred_in_a_numbered_span() {
        assert_eq!(Span::Count(5).stop("browser", None), Stop::CENTRED);
        assert_eq!(names(&["1", "3"]).stop("browser", None), Stop::CENTRED);
    }

    /// A lone stop is a real span, and centred is the only place it can be: there is no
    /// second stop for it to sit apart from.
    #[test]
    fn a_span_of_one_has_nowhere_to_travel() {
        assert_eq!(Span::Count(1).stop("1", None), Stop::CENTRED);
        assert_eq!(names(&["browser"]).stop("browser", None), Stop::CENTRED);
    }

    #[test]
    fn an_axis_follows_only_what_it_was_pointed_at() {
        let params = Params::default();
        assert_eq!(params.axis(Axis::None, Some("1"), None), Stop::CENTRED);
        assert_eq!(params.axis(Axis::Workspace, Some("1"), None).at, 0.0);
        assert_eq!(
            params.axis(Axis::Workspace, None, None),
            Stop::CENTRED,
            "a monitor nothing has named a workspace for has no place to report"
        );
    }

    /// So that `--check` and the per-output report say what the file would.
    #[test]
    fn the_settings_print_the_spellings_serde_reads() {
        assert_eq!(Params::default().to_string(), "vertical=none,horizontal=workspace,span=10");
        let named = Params { span: names(&["browser", "code"]), ..Params::default() };
        assert_eq!(named.to_string(), "vertical=none,horizontal=workspace,span=[browser,code]");
    }
}
