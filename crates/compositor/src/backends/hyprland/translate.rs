use std::collections::{BTreeMap, HashMap};

use domain::{OutputId, Stop};

use super::wire::{self, Event};
use super::{Params, Scope, When};
use crate::backends::Scoped;
use crate::event::Drive;

/// Everything the backend must remember to turn Hyprland's report into channel positions.
#[derive(Debug, Default)]
pub struct Tracker {
    /// Monitor to the id of the workspace it is showing.
    monitors: BTreeMap<OutputId, i64>,
    /// Workspace id to its current name and monitor ownership.
    workspaces: HashMap<i64, Workspace>,
    focused: Option<OutputId>,
    /// Workspace the focused window is on, which the monitor showing it is read from.
    focused_workspace: Option<i64>,
    /// Window address to the workspace it is on, which is what says whether a workspace is
    /// empty. `None` where nothing asks; see [`Tracker::new`].
    windows: Option<HashMap<String, i64>>,
}

#[derive(Debug)]
struct Workspace {
    name: String,
    monitor: Option<OutputId>,
}

impl Tracker {
    /// `windows` asks for the bookkeeping `blur.when = "non-empty"` needs: a
    /// window-to-workspace map, which the compositor reports nothing else to answer from.
    ///
    /// Left unasked for, none of it is kept and the snapshot behind it is never fetched.
    pub fn new(windows: bool) -> Self {
        Self { windows: windows.then(HashMap::new), ..Self::default() }
    }

    /// Whether this tracker answers `blur.when = "non-empty"`, which is what decides
    /// whether the snapshot of open windows is worth asking for.
    pub fn tracks_windows(&self) -> bool {
        self.windows.is_some()
    }

    /// Replaces everything with what the compositor says is true right now.
    ///
    /// Needed because the event stream carries changes only: it never restates the world,
    /// so there is nothing to start from until this has been asked.
    ///
    /// `clients` is empty where nothing asked for it, which is where nothing reads it.
    pub fn seed(
        &mut self,
        monitors: Vec<wire::Monitor>,
        workspaces: Vec<wire::Workspace>,
        window: wire::ActiveWindow,
        clients: Vec<wire::Client>,
    ) {
        self.replace_topology(monitors, workspaces);
        if let Some(windows) = &mut self.windows {
            *windows = clients
                .into_iter()
                .map(|client| (wire::address(&client.address), client.workspace.id))
                .collect();
        }
        self.focused_workspace = self.holding(&window);
    }

    /// Which workspace the focused window is on, from the answer that carries it.
    ///
    /// One no monitor is showing is a special workspace, drawn over whichever monitor has
    /// the focus, and falls back to what that monitor shows as every event path does.
    fn holding(&self, window: &wire::ActiveWindow) -> Option<i64> {
        window.address.as_ref()?;
        let shown = window
            .workspace
            .as_ref()
            .map(|workspace| workspace.id)
            .filter(|id| self.monitors.values().any(|shown| shown == id));
        shown.or_else(|| self.showing())
    }

    /// What the monitor holding the focus is showing, which is where a window taking it
    /// lands.
    ///
    /// [`UNSET`] answers nothing rather than a workspace: it stands for a monitor that has
    /// not said what it shows, and every monitor waiting on that answer carries it.
    fn showing(&self) -> Option<i64> {
        let focused = self.focused.as_ref()?;
        self.monitors.get(focused).copied().filter(|id| *id != UNSET)
    }

    /// Replaces compositor-owned monitor and workspace topology, leaving window state alone.
    pub fn resync(&mut self, monitors: Vec<wire::Monitor>, workspaces: Vec<wire::Workspace>) {
        self.replace_topology(monitors, workspaces);
    }

    fn replace_topology(&mut self, monitors: Vec<wire::Monitor>, workspaces: Vec<wire::Workspace>) {
        self.focused = None;
        self.monitors.clear();
        self.workspaces = workspaces
            .into_iter()
            .map(|workspace| {
                let monitor = workspace.monitor.map(OutputId::new);
                (workspace.id, Workspace { name: workspace.name, monitor })
            })
            .collect();

        for monitor in monitors {
            let output = OutputId::new(monitor.name);
            if monitor.focused {
                self.focused = Some(output.clone());
            }
            self.workspaces.insert(
                monitor.active_workspace.id,
                Workspace { name: monitor.active_workspace.name, monitor: Some(output.clone()) },
            );
            self.monitors.insert(output, monitor.active_workspace.id);
        }
    }

    /// Records what a workspace is called, keeping the monitor it is already known to be on.
    ///
    /// The events carrying a name carry no monitor. One nothing has placed yet falls back to
    /// the focused monitor, which is where a workspace becoming active is.
    ///
    /// Letting that fallback replace a known answer would leave two rows wrong at once, and
    /// nothing short of a snapshot would put them back.
    fn record(&mut self, id: i64, name: String) {
        match self.workspaces.get_mut(&id) {
            Some(workspace) => workspace.name = name,
            None => {
                self.workspaces.insert(id, Workspace { name, monitor: self.focused.clone() });
            }
        }
    }

    fn show(&mut self, output: OutputId, workspace: i64) {
        self.monitors.insert(output.clone(), workspace);
        if let Some(record) = self.workspaces.get_mut(&workspace) {
            record.monitor = Some(output);
        }
    }

    /// Folds one compositor event into the tracked state.
    pub fn apply(&mut self, event: Event) {
        match event {
            Event::MonitorAdded { name } => {
                // Nothing says what it is showing yet. The backend asks alongside this, so
                // the entry only stands in for a monitor the answer did not list.
                self.monitors.entry(OutputId::new(name)).or_insert(UNSET);
            }
            Event::MonitorRemoved { name } => {
                let id = OutputId::new(name);
                self.monitors.remove(&id);
                if self.focused.as_ref() == Some(&id) {
                    self.focused = None;
                }
            }
            Event::FocusedMonitor { monitor, workspace } => {
                let id = OutputId::new(monitor);
                self.show(id.clone(), workspace);
                self.focused = Some(id);
            }
            // Taken to be the focused monitor. Every path that activates a workspace on
            // another one moves it there too, and moving asks for a snapshot.
            Event::WorkspaceActive { id, name } => {
                self.record(id, name);
                if let Some(focused) = self.focused.clone() {
                    // A window on the arriving workspace takes the focus before this says
                    // where it landed, so a focus on the one being left moves with it.
                    let showing = self.monitors.get(&focused).copied();
                    if self.focused_workspace.is_some() && self.focused_workspace == showing {
                        self.focused_workspace = Some(id);
                    }
                    self.show(focused, id);
                }
            }
            Event::WorkspaceCreated { id, name } | Event::WorkspaceRenamed { id, name } => {
                self.record(id, name);
            }
            // Taken to be on what the monitor holding the focus is showing, corrected by a
            // workspace change arriving after. A monitor the cursor reaches empty reports none.
            Event::ActiveWindow { focused } => {
                self.focused_workspace = if focused { self.showing() } else { None };
            }
            Event::WindowOpened { address, workspace } => {
                // Every workspace that exists has been named: one that did not is created
                // before anything can open on it, and the rest were named by the snapshot.
                let id = self.named(&workspace);
                if let (Some(id), Some(windows)) = (id, &mut self.windows) {
                    windows.insert(address, id);
                }
            }
            Event::WindowClosed { address } => {
                if let Some(windows) = &mut self.windows {
                    windows.remove(&address);
                }
            }
            Event::WindowMoved { address, workspace } => {
                if let Some(windows) = &mut self.windows {
                    windows.insert(address, workspace);
                }
            }
            Event::WorkspaceMoved { id, name, monitor } => {
                self.workspaces
                    .insert(id, Workspace { name, monitor: Some(OutputId::new(monitor)) });
            }
            Event::WorkspaceIdChanged { from, to } => {
                self.workspaces.remove(&from);
                if let Some(windows) = &mut self.windows {
                    for workspace in windows.values_mut() {
                        if *workspace == from {
                            *workspace = to;
                        }
                    }
                }
            }
            Event::WorkspaceDestroyed { id } => {
                self.workspaces.remove(&id);
                // Ids are reused, so what was on this one has to go with it or the next
                // workspace to carry the number inherits an occupied look.
                if let Some(windows) = &mut self.windows {
                    windows.retain(|_, workspace| *workspace != id);
                }
            }
        }
    }

    /// Restates the whole world as channel positions.
    ///
    /// Everything is emitted after every event rather than diffed here, because the
    /// receiving end already ignores values that did not change.
    ///
    /// No `Drive::ZoomedOut`, ever. Hyprland reports no wider view of an output to zoom
    /// out to: what stands in for one is a plugin, whose dispatcher is invisible from
    /// outside. Saying so by never driving the channel leaves it where an undriven output
    /// already sits, which is the fixed crop `zoom.crop-ratio` asks for.
    pub fn drives(&self, settings: &Scoped<Params>) -> Vec<Drive> {
        let outputs: Vec<OutputId> = self.monitors.keys().cloned().collect();
        let mut drives = vec![Drive::OutputsChanged { outputs }];
        let focused = self.focused_output();
        let topology = self.topology();
        // Every monitor's own answer is taken before any of them is told, because
        // `scope = "global"` reads the whole set rather than the output it is driving.
        let reached: BTreeMap<&OutputId, bool> = self
            .monitors
            .iter()
            .map(|(output, workspace)| {
                let on = match settings.for_output(output).blur.when {
                    When::Focused => focused == Some(output),
                    When::NonEmpty => self.occupied(*workspace),
                };
                (output, on)
            })
            .collect();
        let anywhere = reached.values().any(|on| *on);

        for (output, workspace) in &self.monitors {
            let stop = Self::stop(topology.get(output).map(Vec::as_slice), *workspace);
            let params = settings.for_output(output);
            drives.push(Drive::Scrolled {
                output: output.clone(),
                x: params.axis(params.horizontal, stop),
                y: params.axis(params.vertical, stop),
            });

            // Every output is told either way, so one that no longer qualifies hears
            // about it.
            let blurred = match params.blur.scope {
                Scope::Global => anywhere,
                Scope::Output => reached[output],
            };
            drives.push(Drive::Blurred { output: output.clone(), on: blurred });
        }
        drives
    }

    /// Groups each monitor's live positive workspaces once for one drive calculation.
    fn topology(&self) -> BTreeMap<OutputId, Vec<i64>> {
        let mut topology: BTreeMap<OutputId, Vec<i64>> =
            self.monitors.keys().cloned().map(|output| (output, Vec::new())).collect();
        for (id, workspace) in &self.workspaces {
            if *id >= 1
                && let Some(ids) =
                    workspace.monitor.as_ref().and_then(|output| topology.get_mut(output))
            {
                ids.push(*id);
            }
        }
        for ids in topology.values_mut() {
            ids.sort_unstable();
        }
        topology
    }

    fn stop(topology: Option<&[i64]>, active: i64) -> Stop {
        let Some(ids) = topology.filter(|ids| ids.len() > 1 && active >= 1) else {
            return Stop::CENTRED;
        };
        let Ok(index) = ids.binary_search(&active) else { return Stop::CENTRED };
        let span = (ids.len() - 1) as f32;
        Stop { at: index as f32 / span, stride: 1.0 / span }
    }

    /// The monitor showing the workspace the focused window is on, which is not the monitor
    /// holding the focus: the cursor moves that one alone wherever it lands.
    fn focused_output(&self) -> Option<&OutputId> {
        let workspace = self.focused_workspace?;
        self.monitors.iter().find(|(_, id)| **id == workspace).map(|(output, _)| output)
    }

    /// Whether the workspace a monitor is showing holds any window at all.
    ///
    /// A special workspace drawn over it contributes none of its own, since what is read
    /// is the workspace the monitor is showing.
    fn occupied(&self, workspace: i64) -> bool {
        self.windows.as_ref().is_some_and(|windows| windows.values().any(|id| *id == workspace))
    }

    /// The id a workspace name answers to, for the events that carry only the name.
    fn named(&self, name: &str) -> Option<i64> {
        self.workspaces.iter().find(|(_, workspace)| workspace.name == name).map(|(id, _)| *id)
    }
}

/// A monitor whose workspace has not been reported.
///
/// No workspace carries this id: ordinary ones count up from 1, special ones from -99, and
/// named ones down from -1337. The compositor answers 0 for absence itself.
const UNSET: i64 = 0;

#[cfg(test)]
mod tests {
    use super::super::Blur;
    use super::*;
    use domain::Stop;

    const FOCUSED: Blur = Blur { when: When::Focused, scope: Scope::Output };
    const NON_EMPTY: Blur = Blur { when: When::NonEmpty, scope: Scope::Output };
    const EVERYWHERE: Blur = Blur { when: When::Focused, scope: Scope::Global };

    /// A window holding the focus, as `j/activewindow` reports one that names no workspace.
    const WINDOW: &str = r#"{"address":"0x55c3da6fa460"}"#;

    fn output(name: &str) -> OutputId {
        OutputId::new(name)
    }

    /// Blurs on the focus, which is what most of the fixtures below are about.
    fn settings() -> Scoped<Params> {
        blurring(FOCUSED)
    }

    fn blurring(blur: Blur) -> Scoped<Params> {
        Scoped::new(Params { blur, ..Params::default() })
    }

    fn feed(tracker: &mut Tracker, line: &str) {
        tracker.apply(wire::parse(line).unwrap().unwrap());
    }

    fn monitor(name: &str, active: i64, focused: bool) -> wire::Monitor {
        showing(name, active, &active.to_string(), focused)
    }

    /// A monitor showing a workspace whose name is not its number, which every named one is.
    fn showing(name: &str, active: i64, workspace: &str, focused: bool) -> wire::Monitor {
        wire::decode(&format!(
            r#"{{"name":"{name}","focused":{focused},"activeWorkspace":{{"id":{active},"name":"{workspace}"}}}}"#
        )).unwrap()
    }

    fn workspaces(entries: &[(i64, &str, Option<&str>)]) -> Vec<wire::Workspace> {
        entries
            .iter()
            .map(|(id, name, monitor)| wire::Workspace {
                id: *id,
                name: (*name).to_owned(),
                monitor: monitor.map(str::to_owned),
            })
            .collect()
    }

    /// Two monitors with sparse numeric workspaces, plus the two kinds the compositor
    /// numbers below zero: `NAMED` from -1337 down, and a special one from -99 up.
    const LISTED: [(i64, &str, Option<&str>); 7] = [
        (1, "1", Some("DP-1")),
        (3, "3", Some("DP-1")),
        (8, "8", Some("DP-1")),
        (20, "20", Some("eDP-1")),
        (40, "40", Some("eDP-1")),
        (NAMED, "parra", Some("DP-1")),
        (-99, "special:special", Some("DP-1")),
    ];

    /// The id the compositor hands the first workspace opened by name.
    const NAMED: i64 = -1337;

    /// The two monitors as `j/monitors` reports them, with DP-1 showing `active` under the
    /// name [`LISTED`] gives it, and `dp` saying which of the two holds the focus.
    fn monitors(active: i64, dp: bool) -> Vec<wire::Monitor> {
        let name = LISTED
            .iter()
            .find(|(id, ..)| *id == active)
            .map_or_else(|| active.to_string(), |(_, name, _)| (*name).to_owned());
        vec![showing("DP-1", active, &name, dp), monitor("eDP-1", 20, !dp)]
    }

    /// DP-1 shows `active` and holds the focus, with a window on it holding the focused one.
    fn seeded(active: i64) -> Tracker {
        seeded_with(active, true, WINDOW)
    }

    /// The same, with `window` as `j/activewindow` answers it.
    fn seeded_with(active: i64, dp: bool, window: &str) -> Tracker {
        let mut tracker = Tracker::default();
        tracker.seed(
            monitors(active, dp),
            workspaces(&LISTED),
            wire::decode(window).unwrap(),
            Vec::new(),
        );
        tracker
    }

    /// DP-1 shows workspace 3 and holds the focus, following the open windows `clients`
    /// names, which is the bookkeeping `blur.when = "non-empty"` reads.
    fn seeded_windows(clients: &str) -> Tracker {
        let mut tracker = Tracker::new(true);
        tracker.seed(
            monitors(3, true),
            workspaces(&LISTED),
            wire::decode(WINDOW).unwrap(),
            wire::decode(clients).unwrap(),
        );
        tracker
    }

    fn scrolled(drives: &[Drive], want: &str) -> Stop {
        drives
            .iter()
            .find_map(|drive| match drive {
                Drive::Scrolled { output, x, .. } if output.as_str() == want => Some(*x),
                _ => None,
            })
            .expect("the output should be reported")
    }

    /// Where one output sits on the axis the defaults drive, which is the horizontal one.
    fn at(drives: &[Drive], want: &str) -> f32 {
        scrolled(drives, want).at
    }

    /// The one output driven to blur, if any.
    fn blurred(drives: &[Drive]) -> Option<OutputId> {
        drives.iter().find_map(|drive| match drive {
            Drive::Blurred { output, on: true } => Some(output.clone()),
            _ => None,
        })
    }

    /// Every output driven to blur, in the order they were told.
    fn all_blurred(drives: &[Drive]) -> Vec<&str> {
        drives
            .iter()
            .filter_map(|drive| match drive {
                Drive::Blurred { output, on: true } => Some(output.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn sparse_ids_form_independent_monitor_topologies() {
        for (active, expected) in [(1, 0.0), (3, 0.5), (8, 1.0)] {
            assert_eq!(
                scrolled(&seeded(active).drives(&settings()), "DP-1"),
                Stop { at: expected, stride: 0.5 }
            );
        }
        assert_eq!(
            scrolled(&seeded(3).drives(&settings()), "eDP-1"),
            Stop { at: 0.0, stride: 1.0 }
        );
    }

    #[test]
    fn topology_creation_and_destruction_recalculate_immediately() {
        let mut tracker = seeded(3);
        assert_eq!(scrolled(&tracker.drives(&settings()), "DP-1").at, 0.5);
        feed(&mut tracker, "createworkspacev2>>2,2");
        assert_eq!(
            scrolled(&tracker.drives(&settings()), "DP-1"),
            Stop { at: 2.0 / 3.0, stride: 1.0 / 3.0 }
        );
        feed(&mut tracker, "destroyworkspacev2>>2,2");
        assert_eq!(scrolled(&tracker.drives(&settings()), "DP-1"), Stop { at: 0.5, stride: 0.5 });
    }

    #[test]
    fn unusable_active_workspaces_are_centred() {
        assert_eq!(scrolled(&seeded(NAMED).drives(&settings()), "DP-1"), Stop::CENTRED);
        let mut unknown = seeded(3);
        feed(&mut unknown, "focusedmonv2>>DP-1,7");
        assert_eq!(scrolled(&unknown.drives(&settings()), "DP-1"), Stop::CENTRED);
        let mut singleton = Tracker::default();
        singleton.seed(
            vec![monitor("DP-1", 1, true)],
            workspaces(&[(1, "1", Some("DP-1"))]),
            wire::decode("{}").unwrap(),
            Vec::new(),
        );
        assert_eq!(scrolled(&singleton.drives(&settings()), "DP-1"), Stop::CENTRED);
    }

    /// A snapshot can report no focused monitor at all, and the guess a name event makes
    /// has nothing to stand on there.
    #[test]
    fn a_workspace_already_placed_keeps_its_monitor_when_nothing_holds_the_focus() {
        let mut tracker = Tracker::default();
        tracker.seed(
            vec![monitor("DP-1", 3, false)],
            workspaces(&LISTED),
            wire::decode("{}").unwrap(),
            Vec::new(),
        );
        feed(&mut tracker, "workspacev2>>3,3");
        assert_eq!(scrolled(&tracker.drives(&settings()), "DP-1"), Stop { at: 0.5, stride: 0.5 });
    }

    #[test]
    fn positive_rename_preserves_numeric_position() {
        let mut tracker = seeded(3);
        feed(&mut tracker, "renameworkspace>>3,code");
        assert_eq!(scrolled(&tracker.drives(&settings()), "DP-1").at, 0.5);
    }

    #[test]
    fn move_then_resync_replaces_ownership_and_active_state() {
        let mut tracker = seeded(3);
        feed(&mut tracker, "moveworkspacev2>>3,three,eDP-1");
        tracker.resync(
            vec![monitor("DP-1", 1, false), monitor("eDP-1", 3, true)],
            workspaces(&[
                (1, "1", Some("DP-1")),
                (8, "8", Some("DP-1")),
                (3, "three", Some("eDP-1")),
                (20, "20", Some("eDP-1")),
                (40, "40", Some("eDP-1")),
            ]),
        );
        assert_eq!(scrolled(&tracker.drives(&settings()), "DP-1").at, 0.0);
        assert_eq!(scrolled(&tracker.drives(&settings()), "eDP-1").at, 0.0);
    }

    #[test]
    fn id_change_then_resync_replaces_the_topology() {
        let mut tracker = seeded(3);
        feed(&mut tracker, "changeworkspaceid>>3,4");
        tracker.resync(
            vec![monitor("DP-1", 4, true), monitor("eDP-1", 20, false)],
            workspaces(&[
                (1, "1", Some("DP-1")),
                (4, "3", Some("DP-1")),
                (8, "8", Some("DP-1")),
                (20, "20", Some("eDP-1")),
                (40, "40", Some("eDP-1")),
            ]),
        );
        assert_eq!(scrolled(&tracker.drives(&settings()), "DP-1").at, 0.5);
    }

    #[test]
    fn monitor_resync_replaces_added_and_removed_outputs() {
        let mut tracker = seeded(3);
        tracker.resync(
            vec![monitor("HEADLESS-1", 9, true)],
            workspaces(&[(9, "9", Some("HEADLESS-1"))]),
        );
        let Drive::OutputsChanged { outputs } = &tracker.drives(&settings())[0] else {
            panic!("outputs are reported first")
        };
        assert_eq!(outputs, &[output("HEADLESS-1")]);
    }

    #[test]
    fn named_workspace_identity_still_joins_window_events() {
        let mut tracker = Tracker::new(true);
        tracker.seed(
            vec![showing("DP-1", NAMED, "parra", true)],
            workspaces(&[(NAMED, "parra", Some("DP-1"))]),
            wire::decode("{}").unwrap(),
            Vec::new(),
        );
        feed(&mut tracker, "openwindow>>abc,parra,kitty,zsh");
        let settings = blurring(NON_EMPTY);
        assert_eq!(all_blurred(&tracker.drives(&settings)), vec!["DP-1"]);
        assert_eq!(scrolled(&tracker.drives(&settings), "DP-1"), Stop::CENTRED);
    }

    #[test]
    fn the_snapshot_answers_before_any_event_arrives() {
        let drives = seeded(1).drives(&settings());
        assert_eq!(at(&drives, "DP-1"), 0.0, "the first of the three it owns");
        assert_eq!(at(&drives, "eDP-1"), 0.0, "the first of the two it owns");
        assert_eq!(blurred(&drives), Some(output("DP-1")));
    }

    /// The axis the settings leave alone stays where an undriven output already sits.
    #[test]
    fn an_axis_pointed_at_nothing_stays_centred() {
        let drives = seeded(1).drives(&settings());
        let centred = drives.iter().any(|drive| {
            matches!(drive, Drive::Scrolled { output, y, .. }
                if output.as_str() == "DP-1" && *y == Stop::CENTRED)
        });
        assert!(centred, "the vertical axis is off by default");
    }

    /// Hyprland reports no wider view of an output, so the channel is never driven and the
    /// zoom holds at the crop the configuration asks for.
    #[test]
    fn nothing_ever_drives_the_zoom() {
        let mut tracker = seeded(1);
        feed(&mut tracker, "createworkspacev2>>2,2");
        feed(&mut tracker, "workspacev2>>2,2");
        let drives = tracker.drives(&settings());
        assert!(!drives.iter().any(|drive| matches!(drive, Drive::ZoomedOut { .. })));
    }

    #[test]
    fn a_workspace_change_lands_on_the_focused_monitor() {
        let mut tracker = seeded(1);
        feed(&mut tracker, "createworkspacev2>>2,2");
        feed(&mut tracker, "workspacev2>>2,2");

        let drives = tracker.drives(&settings());
        assert_eq!(at(&drives, "DP-1"), 1.0 / 3.0, "the second of 1, 2, 3, 8");
        assert_eq!(at(&drives, "eDP-1"), 0.0, "the other one is untouched");
    }

    #[test]
    fn focusing_a_monitor_reports_its_workspace_too() {
        let mut tracker = seeded(1);
        feed(&mut tracker, "focusedmonv2>>eDP-1,40");

        let drives = tracker.drives(&settings());
        assert_eq!(at(&drives, "eDP-1"), 1.0, "the last of the two it owns");
        assert_eq!(
            blurred(&drives),
            Some(output("DP-1")),
            "the focus reached eDP-1 without any window on it doing so"
        );
    }

    /// The cursor crossing to a monitor showing nothing moves the focus and leaves the
    /// focused window where it is, which the compositor says by not mentioning one.
    #[test]
    fn a_monitor_the_cursor_reaches_holds_no_window_and_blurs_for_none() {
        let mut tracker = seeded(1);
        assert_eq!(blurred(&tracker.drives(&settings())), Some(output("DP-1")));

        feed(&mut tracker, "focusedmonv2>>eDP-1,20");
        assert_eq!(
            blurred(&tracker.drives(&settings())),
            Some(output("DP-1")),
            "eDP-1 has nothing on it, so the window still holding the focus is on DP-1"
        );

        feed(&mut tracker, "activewindowv2>>55c3da272150");
        assert_eq!(blurred(&tracker.drives(&settings())), Some(output("eDP-1")));
    }

    #[test]
    fn a_workspace_change_after_focus_moves_follows_the_new_monitor() {
        let mut tracker = seeded(1);
        feed(&mut tracker, "focusedmonv2>>eDP-1,20");
        feed(&mut tracker, "createworkspacev2>>30,30");
        feed(&mut tracker, "workspacev2>>30,30");

        let drives = tracker.drives(&settings());
        assert_eq!(at(&drives, "eDP-1"), 0.5, "the middle of 20, 30, 40");
        assert_eq!(at(&drives, "DP-1"), 0.0, "and not the one that lost focus");
    }

    /// The order the compositor really sends: activate the replacement, then destroy the
    /// one just left, which leaves the row a workspace shorter under the new position.
    #[test]
    fn leaving_a_workspace_destroys_it_without_disturbing_the_new_one() {
        let mut tracker = seeded(1);
        feed(&mut tracker, "createworkspacev2>>2,2");
        feed(&mut tracker, "workspacev2>>2,2");
        feed(&mut tracker, "destroyworkspacev2>>1,1");

        assert_eq!(
            scrolled(&tracker.drives(&settings()), "DP-1"),
            Stop { at: 0.0, stride: 0.5 },
            "the first of 2, 3, 8"
        );
    }

    #[test]
    fn hotplug_adds_and_forgets_a_monitor() {
        let mut tracker = seeded(1);
        feed(&mut tracker, "monitoraddedv2>>2,HEADLESS-1,");
        assert_eq!(
            scrolled(&tracker.drives(&settings()), "HEADLESS-1"),
            Stop::CENTRED,
            "nothing has said what it shows yet"
        );

        feed(&mut tracker, "monitorremovedv2>>2,HEADLESS-1,");
        let Drive::OutputsChanged { outputs } = &tracker.drives(&settings())[0] else {
            panic!("the output set is reported first")
        };
        assert_eq!(outputs, &[output("DP-1"), output("eDP-1")]);
    }

    #[test]
    fn unplugging_the_focused_monitor_leaves_nothing_blurred() {
        let mut tracker = seeded(1);
        feed(&mut tracker, "monitorremovedv2>>0,DP-1,");
        assert_eq!(blurred(&tracker.drives(&settings())), None);
    }

    #[test]
    fn a_monitor_with_nothing_on_it_holds_no_focused_window() {
        let mut tracker = seeded(1);
        assert_eq!(blurred(&tracker.drives(&settings())), Some(output("DP-1")));

        // Reaching it by keyboard rather than by cursor, which is the one of the two that
        // takes the focus off the window it was on.
        feed(&mut tracker, "activewindowv2>>");
        feed(&mut tracker, "focusedmonv2>>eDP-1,40");

        let drives = tracker.drives(&settings());
        assert_eq!(blurred(&drives), None, "eDP-1 has the focus, nothing on it does");
        assert_eq!(at(&drives, "eDP-1"), 1.0, "which it still shows");
    }

    #[test]
    fn switching_to_an_empty_workspace_lets_the_focus_go_and_take_it_back() {
        let mut tracker = seeded(1);
        feed(&mut tracker, "activewindowv2>>");
        feed(&mut tracker, "createworkspacev2>>2,2");
        feed(&mut tracker, "workspacev2>>2,2");
        assert_eq!(blurred(&tracker.drives(&settings())), None);

        feed(&mut tracker, "activewindowv2>>55c3da6fa460");
        assert_eq!(
            blurred(&tracker.drives(&settings())),
            Some(output("DP-1")),
            "a window arriving"
        );
    }

    /// The compositor names the window arriving before it names the workspace it arrived
    /// on, leaving the focus recorded on the one being left.
    #[test]
    fn switching_workspace_on_one_monitor_keeps_the_blur_on_it() {
        let mut tracker = seeded(1);
        feed(&mut tracker, "activewindowv2>>556a4d2bd690");
        feed(&mut tracker, "workspacev2>>3,3");

        let drives = tracker.drives(&settings());
        assert_eq!(blurred(&drives), Some(output("DP-1")));
        assert_eq!(at(&drives, "DP-1"), 0.5, "the middle of 1, 3, 8");
    }

    #[test]
    fn a_switch_on_the_monitor_the_cursor_reached_leaves_the_focus_where_it_is() {
        let mut tracker = seeded(1);
        feed(&mut tracker, "focusedmonv2>>eDP-1,20");
        feed(&mut tracker, "createworkspacev2>>30,30");
        feed(&mut tracker, "workspacev2>>30,30");

        assert_eq!(
            blurred(&tracker.drives(&settings())),
            Some(output("DP-1")),
            "the window holding it never left DP-1"
        );
    }

    #[test]
    fn a_snapshot_with_nothing_focused_starts_that_way() {
        let drives = seeded_with(1, true, "{}").drives(&settings());
        assert_eq!(blurred(&drives), None);
        assert_eq!(at(&drives, "DP-1"), 0.0, "which says nothing about what the monitors show");
    }

    /// A workspace only becomes the active one on its new monitor if it was active on the
    /// old one and that one had the focus, and the event says neither.
    #[test]
    fn moving_a_workspace_rebuilds_both_rows_without_activating_it() {
        let mut tracker = seeded(1);
        feed(&mut tracker, "moveworkspacev2>>8,8,eDP-1");

        let drives = tracker.drives(&settings());
        assert_eq!(
            scrolled(&drives, "eDP-1"),
            Stop { at: 0.5, stride: 0.5 },
            "it still shows 20, now the middle of 8, 20, 40"
        );
        assert_eq!(
            scrolled(&drives, "DP-1"),
            Stop { at: 0.0, stride: 1.0 },
            "and 1 is the first of the two left here"
        );
    }

    /// Cold start has no event behind it to say where the focused window is, so the answer
    /// carries the workspace and the monitor showing it is looked up.
    #[test]
    fn a_cold_start_finds_the_focused_window_away_from_the_focus() {
        let window = r#"{"address":"0x55c3da6fa460","workspace":{"id":1,"name":"1"}}"#;
        let tracker = seeded_with(1, false, window);
        assert_eq!(
            blurred(&tracker.drives(&settings())),
            Some(output("DP-1")),
            "eDP-1 has the focus, and the window holding it is on DP-1"
        );
    }

    #[test]
    fn asking_again_does_not_disturb_the_focused_window() {
        let mut tracker = seeded_with(1, true, "{}");
        tracker.resync(monitors(1, false), workspaces(&LISTED));
        assert_eq!(
            blurred(&tracker.drives(&settings())),
            None,
            "the answer says which monitor has the focus, not whether a window does"
        );
    }

    #[test]
    fn a_workspace_with_a_window_on_it_blurs_without_holding_the_focus() {
        let tracker = seeded_windows(r#"[{"address":"0x1","workspace":{"id":20,"name":"20"}}]"#);
        assert_eq!(
            all_blurred(&tracker.drives(&blurring(NON_EMPTY))),
            vec!["eDP-1"],
            "DP-1 holds the focused window and shows a workspace nothing else is on"
        );
    }

    #[test]
    fn an_empty_workspace_stays_sharp() {
        let tracker = seeded_windows("[]");
        assert!(all_blurred(&tracker.drives(&blurring(NON_EMPTY))).is_empty());
    }

    #[test]
    fn a_window_opening_and_closing_moves_the_answer() {
        let mut tracker = seeded_windows("[]");
        feed(&mut tracker, "openwindow>>abc,3,kitty,zsh");
        assert_eq!(all_blurred(&tracker.drives(&blurring(NON_EMPTY))), vec!["DP-1"]);

        feed(&mut tracker, "closewindow>>abc");
        assert!(all_blurred(&tracker.drives(&blurring(NON_EMPTY))).is_empty());
    }

    #[test]
    fn a_window_handed_to_another_workspace_takes_the_blur_with_it() {
        let mut tracker =
            seeded_windows(r#"[{"address":"0xabc","workspace":{"id":3,"name":"3"}}]"#);
        assert_eq!(all_blurred(&tracker.drives(&blurring(NON_EMPTY))), vec!["DP-1"]);

        feed(&mut tracker, "movewindowv2>>abc,20,20");
        assert_eq!(all_blurred(&tracker.drives(&blurring(NON_EMPTY))), vec!["eDP-1"]);
    }

    /// A renumbered workspace is the same workspace, so what is on it is still on it.
    #[test]
    fn a_renumbered_workspace_keeps_its_windows() {
        let mut tracker =
            seeded_windows(r#"[{"address":"0xabc","workspace":{"id":3,"name":"3"}}]"#);
        feed(&mut tracker, "changeworkspaceid>>3,7");

        // What the backend asks for once the renumbering is announced.
        tracker.resync(
            vec![monitor("DP-1", 7, true), monitor("eDP-1", 20, false)],
            workspaces(&[
                (1, "1", Some("DP-1")),
                (7, "3", Some("DP-1")),
                (8, "8", Some("DP-1")),
                (20, "20", Some("eDP-1")),
                (40, "40", Some("eDP-1")),
            ]),
        );
        assert_eq!(all_blurred(&tracker.drives(&blurring(NON_EMPTY))), vec!["DP-1"]);
    }

    /// Hyprland reuses the numbers it hands out, so a workspace going away has to take
    /// what was on it or the next one to carry its id looks occupied.
    ///
    /// The window is left in the map on purpose. A `closewindow` that stopped parsing is
    /// logged and skipped, and this is what keeps one lost that way from staining an id.
    #[test]
    fn a_destroyed_workspace_takes_its_windows_with_it() {
        let mut tracker =
            seeded_windows(r#"[{"address":"0xabc","workspace":{"id":3,"name":"3"}}]"#);
        feed(&mut tracker, "workspacev2>>1,1");
        feed(&mut tracker, "destroyworkspacev2>>3,3");

        feed(&mut tracker, "createworkspacev2>>3,3");
        feed(&mut tracker, "workspacev2>>3,3");
        assert!(all_blurred(&tracker.drives(&blurring(NON_EMPTY))).is_empty());
    }

    #[test]
    fn one_output_reaching_it_blurs_every_output() {
        let mut tracker = seeded(1);
        assert_eq!(
            all_blurred(&tracker.drives(&blurring(EVERYWHERE))),
            vec!["DP-1", "eDP-1"],
            "one window is focused, and it is on DP-1"
        );

        feed(&mut tracker, "activewindowv2>>");
        assert!(all_blurred(&tracker.drives(&blurring(EVERYWHERE))).is_empty());
    }

    /// `when` is read from the output being asked about, so two outputs can answer
    /// different questions at once.
    #[test]
    fn one_output_may_blur_on_a_different_rule_from_the_rest() {
        let tracker = seeded_windows("[]");
        assert!(
            all_blurred(&tracker.drives(&blurring(NON_EMPTY))).is_empty(),
            "no workspace holds a window"
        );

        let mut mixed = blurring(NON_EMPTY);
        mixed.set_output(output("DP-1"), Params { blur: FOCUSED, ..Params::default() });
        assert_eq!(
            all_blurred(&tracker.drives(&mixed)),
            vec!["DP-1"],
            "DP-1 asks for the focus instead, and holds it"
        );
    }

    /// `scope` is read from the output being driven, so what `"global"` reads can hold two
    /// rules at once.
    #[test]
    fn an_output_deciding_alone_still_answers_for_one_deciding_together() {
        let alone = || Params { blur: NON_EMPTY, ..Params::default() };
        let tracker = seeded_windows(r#"[{"address":"0x1","workspace":{"id":3,"name":"3"}}]"#);

        let mut together = blurring(Blur { scope: Scope::Global, ..NON_EMPTY });
        together.set_output(output("DP-1"), alone());
        assert_eq!(
            all_blurred(&tracker.drives(&together)),
            vec!["DP-1", "eDP-1"],
            "eDP-1 shows an empty workspace, and blurs on DP-1's own answer being yes"
        );

        let mut apart = blurring(NON_EMPTY);
        apart.set_output(output("DP-1"), alone());
        assert_eq!(
            all_blurred(&tracker.drives(&apart)),
            vec!["DP-1"],
            "reading only its own answer, eDP-1 hears nothing of DP-1's"
        );
    }

    /// Nothing asking the question is what makes the snapshot behind it not worth
    /// fetching, so the two have to agree.
    #[test]
    fn a_tracker_that_follows_no_windows_says_so() {
        assert!(!Tracker::default().tracks_windows());
        assert!(Tracker::new(true).tracks_windows());
    }

    #[test]
    fn untracked_windows_remain_unallocated() {
        let mut tracker = seeded(3);
        feed(&mut tracker, "openwindow>>abc,3,kitty,zsh");
        assert!(tracker.windows.is_none());
    }
}
