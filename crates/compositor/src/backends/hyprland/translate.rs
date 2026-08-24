use std::collections::{BTreeMap, BTreeSet, HashMap};

use domain::OutputId;

use super::wire::{self, Event};
use super::{Params, Scope, When};
use crate::backends::Scoped;
use crate::event::Drive;

/// Everything the backend must remember to turn Hyprland's report into channel positions.
///
/// The compositor reports a workspace change without saying where, and reports the focused
/// monitor with a workspace id but no name. Holding both halves so either event can
/// complete the other is this file's entire job; where a name then sits in the travel is
/// the span's to answer.
#[derive(Debug, Default)]
pub struct Tracker {
    /// Monitor to the id of the workspace it is showing.
    monitors: BTreeMap<OutputId, i64>,
    /// Monitor to the name of the workspace it was showing before this one, which tells
    /// two stops equally far from an unlisted workspace apart. By name rather than by id:
    /// the workspace being left is usually destroyed on the way out, taking its id's name.
    previous: HashMap<OutputId, String>,
    /// Workspace id to name. Kept because a workspace is placed in the travel by name,
    /// while half the events identify one only by id.
    names: HashMap<i64, String>,
    focused: Option<OutputId>,
    /// Whether any window holds the focus, which is not the same as a monitor holding it.
    focused_window: bool,
    /// Window address to the workspace it is on, which is what says whether a workspace is
    /// empty. `None` where nothing asks; see [`Tracker::new`].
    windows: Option<HashMap<String, i64>>,
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
        self.names = workspaces.into_iter().map(|w| (w.id, w.name)).collect();
        self.focused_window = window.address.is_some();
        if let Some(windows) = &mut self.windows {
            *windows = clients
                .into_iter()
                .map(|client| (wire::address(&client.address), client.workspace.id))
                .collect();
        }
        self.resync(monitors);
    }

    /// Replaces what each monitor is showing, leaving the focused window alone.
    ///
    /// Narrower than a full seed because it answers one question, and the answer carries
    /// the names of the active workspaces with it, so nothing else has to be asked for.
    pub fn resync(&mut self, monitors: Vec<wire::Monitor>) {
        self.focused = None;
        let mut listed = BTreeSet::new();

        for monitor in monitors {
            let id = OutputId::new(monitor.name);
            if monitor.focused {
                self.focused = Some(id.clone());
            }
            self.names.insert(monitor.active_workspace.id, monitor.active_workspace.name);
            listed.insert(id.clone());
            self.show(id, monitor.active_workspace.id);
        }

        // Dropping what the answer left out rather than starting from nothing, so a monitor
        // it does still list keeps the workspace it was showing before this.
        self.monitors.retain(|id, _| listed.contains(id));
        self.previous.retain(|id, _| listed.contains(id));
    }

    /// Records what a monitor is showing, keeping what it was showing until then.
    fn show(&mut self, output: OutputId, workspace: i64) {
        let showing = self.monitors.get(&output).copied();
        if showing == Some(workspace) {
            return;
        }
        if let Some(name) = showing.and_then(|id| self.names.get(&id)).cloned() {
            self.previous.insert(output.clone(), name);
        }
        self.monitors.insert(output, workspace);
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
                self.previous.remove(&id);
                if self.focused.as_ref() == Some(&id) {
                    self.focused = None;
                }
            }
            Event::FocusedMonitor { monitor, workspace } => {
                let id = OutputId::new(monitor);
                self.show(id.clone(), workspace);
                self.focused = Some(id);
            }
            // Said of the focused monitor, which is the only one a workspace change can be
            // about: switching on an unfocused monitor focuses it first.
            Event::WorkspaceActive { id, name } => {
                self.names.insert(id, name);
                if let Some(focused) = self.focused.clone() {
                    self.show(focused, id);
                }
            }
            Event::WorkspaceCreated { id, name } | Event::WorkspaceRenamed { id, name } => {
                self.names.insert(id, name);
            }
            Event::ActiveWindow { focused } => self.focused_window = focused,
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
            // Deliberately nothing. Whether the workspace became the active one on the
            // monitor it landed on depends on where the focus was at the time, so the
            // backend asks what both monitors ended up showing instead of guessing.
            Event::WorkspaceMoved { .. } => {}
            // The name follows the new id unless the workspace had been renamed by hand,
            // and the event says which neither way. The backend asks alongside this too,
            // so all that is left here is dropping what the old id answered to.
            Event::WorkspaceIdChanged { from, to } => {
                self.names.remove(&from);
                if let Some(windows) = &mut self.windows {
                    for workspace in windows.values_mut() {
                        if *workspace == from {
                            *workspace = to;
                        }
                    }
                }
            }
            Event::WorkspaceDestroyed { id } => {
                // The replacement is activated before the one being left is destroyed, so
                // a monitor still pointing here means the two crossed. Keeping the name
                // until nothing wants it avoids centring the wallpaper on a workspace that
                // is in fact still up.
                if !self.monitors.values().any(|active| *active == id) {
                    self.names.remove(&id);
                    // Ids are reused, so what was on this one has to go with it or the
                    // next workspace to carry the number inherits an occupied look.
                    if let Some(windows) = &mut self.windows {
                        windows.retain(|_, workspace| *workspace != id);
                    }
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

        // Every monitor's own answer is taken before any of them is told, because
        // `scope = "global"` reads the whole set rather than the output it is driving.
        let reached: BTreeMap<&OutputId, bool> = self
            .monitors
            .iter()
            .map(|(output, workspace)| {
                let on = match settings.for_output(output).blur.when {
                    When::Focus => focused.as_ref() == Some(output),
                    When::NonEmpty => self.occupied(*workspace),
                };
                (output, on)
            })
            .collect();
        let anywhere = reached.values().any(|on| *on);

        for (output, workspace) in &self.monitors {
            let name = self.names.get(workspace).map(String::as_str);
            let from = self.previous.get(output).map(String::as_str);
            let params = settings.for_output(output);
            drives.push(Drive::Scrolled {
                output: output.clone(),
                x: params.axis(params.horizontal, name, from),
                y: params.axis(params.vertical, name, from),
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

    /// The monitor holding the focused window, which is not the monitor holding focus.
    ///
    /// A monitor showing an empty workspace is still the focused one, but nothing on it is
    /// focused, and `blur.when = "focus"` follows the window rather than the monitor. The
    /// compositor says so by reporting an active window with no address at all.
    fn focused_output(&self) -> Option<OutputId> {
        self.focused.clone().filter(|_| self.focused_window)
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
        self.names.iter().find(|(_, listed)| *listed == name).map(|(id, _)| *id)
    }
}

/// A monitor whose workspace has not been reported. No real workspace has this id: the
/// compositor numbers ordinary ones from 1 and named ones downward from -1.
const UNSET: i64 = 0;

#[cfg(test)]
mod tests {
    use domain::Stop;

    use super::super::{Blur, Span};
    use super::*;

    const NON_EMPTY: Blur = Blur { when: When::NonEmpty, scope: Scope::Output };
    const EVERYWHERE: Blur = Blur { when: When::Focus, scope: Scope::Global };

    fn output(name: &str) -> OutputId {
        OutputId::new(name)
    }

    fn feed(tracker: &mut Tracker, line: &str) {
        let event = wire::parse(line).expect("should parse").expect("should be modelled");
        tracker.apply(event);
    }

    /// Five numbered workspaces, so the two the fixtures use sit at either end.
    fn settings() -> Scoped<Params> {
        Scoped::new(Params { span: Span::Count(5), ..Params::default() })
    }

    /// The same, blurring on the rule given rather than on the focus.
    fn blurring(blur: Blur) -> Scoped<Params> {
        Scoped::new(Params { span: Span::Count(5), blur, ..Params::default() })
    }

    fn named_settings(names: &[&str]) -> Scoped<Params> {
        let span = Span::Names(names.iter().map(|name| (*name).to_owned()).collect());
        Scoped::new(Params { span, ..Params::default() })
    }

    /// The two monitors this was developed against, as `j/monitors` reports them, with a
    /// window holding the focus.
    fn seeded() -> Tracker {
        seeded_with(r#"{"address":"0x55c3da6fa460"}"#)
    }

    fn seeded_with(window: &str) -> Tracker {
        let mut tracker = Tracker::default();
        let workspaces: Vec<wire::Workspace> =
            wire::decode(r#"[{"id":1,"name":"1"},{"id":14,"name":"5"}]"#).unwrap();
        tracker.seed(monitors(true), workspaces, wire::decode(window).unwrap(), Vec::new());
        tracker
    }

    /// The same two monitors, following the open windows `clients` names.
    ///
    /// DP-1 shows workspace 1 and holds the focus; eDP-1 shows workspace 14.
    fn seeded_windows(clients: &str) -> Tracker {
        let mut tracker = Tracker::new(true);
        let workspaces: Vec<wire::Workspace> =
            wire::decode(r#"[{"id":1,"name":"1"},{"id":14,"name":"5"}]"#).unwrap();
        tracker.seed(
            monitors(true),
            workspaces,
            wire::decode(r#"{"address":"0x55c3da6fa460"}"#).unwrap(),
            wire::decode(clients).unwrap(),
        );
        tracker
    }

    /// `dp` picks which of the two the compositor says has the focus.
    fn monitors(dp: bool) -> Vec<wire::Monitor> {
        let (a, b) = (dp, !dp);
        wire::decode(&format!(
            r#"[{{"name":"DP-1","focused":{a},"activeWorkspace":{{"id":1,"name":"1"}}}},
                {{"name":"eDP-1","focused":{b},"activeWorkspace":{{"id":14,"name":"5"}}}}]"#
        ))
        .unwrap()
    }

    /// Where one output sits on the axis the defaults drive, which is the horizontal one.
    fn scrolled(drives: &[Drive], want: &str) -> Stop {
        drives
            .iter()
            .find_map(|drive| match drive {
                Drive::Scrolled { output, x, .. } if output.as_str() == want => Some(*x),
                _ => None,
            })
            .expect("the output should be reported")
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

    fn at(drives: &[Drive], want: &str) -> f32 {
        scrolled(drives, want).at
    }

    #[test]
    fn the_snapshot_answers_before_any_event_arrives() {
        let drives = seeded().drives(&settings());
        assert_eq!(at(&drives, "DP-1"), 0.0, "workspace 1 of 5");
        assert_eq!(at(&drives, "eDP-1"), 1.0, "workspace 5 of 5");
        assert_eq!(blurred(&drives), Some(output("DP-1")));
    }

    /// The axis the settings leave alone stays where an undriven output already sits.
    #[test]
    fn an_axis_pointed_at_nothing_stays_centred() {
        let drives = seeded().drives(&settings());
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
        let mut tracker = seeded();
        feed(&mut tracker, "createworkspacev2>>3,3");
        feed(&mut tracker, "workspacev2>>3,3");

        let drives = tracker.drives(&settings());
        assert!(!drives.iter().any(|drive| matches!(drive, Drive::ZoomedOut { .. })));
    }

    #[test]
    fn a_workspace_change_lands_on_the_focused_monitor() {
        let mut tracker = seeded();
        feed(&mut tracker, "createworkspacev2>>3,3");
        feed(&mut tracker, "workspacev2>>3,3");

        let drives = tracker.drives(&settings());
        assert_eq!(at(&drives, "DP-1"), 0.5, "workspace 3 of 5");
        assert_eq!(at(&drives, "eDP-1"), 1.0, "the other one is untouched");
    }

    #[test]
    fn focusing_a_monitor_reports_its_workspace_too() {
        let mut tracker = seeded();
        feed(&mut tracker, "focusedmonv2>>eDP-1,14");

        let drives = tracker.drives(&settings());
        assert_eq!(blurred(&drives), Some(output("eDP-1")));
        assert_eq!(at(&drives, "eDP-1"), 1.0);
    }

    #[test]
    fn a_workspace_change_after_focus_moves_follows_the_new_monitor() {
        let mut tracker = seeded();
        feed(&mut tracker, "focusedmonv2>>eDP-1,14");
        feed(&mut tracker, "createworkspacev2>>15,2");
        feed(&mut tracker, "workspacev2>>15,2");

        let drives = tracker.drives(&settings());
        assert_eq!(at(&drives, "eDP-1"), 0.25, "workspace 2 of 5");
        assert_eq!(at(&drives, "DP-1"), 0.0, "and not the one that lost focus");
    }

    /// The id is the compositor's bookkeeping and says nothing about where a workspace
    /// sits: named ones are numbered downward from -1.
    #[test]
    fn a_named_workspace_is_placed_by_its_name_not_its_id() {
        let mut tracker = seeded();
        feed(&mut tracker, "createworkspacev2>>-1337,browser");
        feed(&mut tracker, "workspacev2>>-1337,browser");

        let drives = tracker.drives(&named_settings(&["browser", "code", "mail"]));
        assert_eq!(at(&drives, "DP-1"), 0.0);
    }

    #[test]
    fn leaving_a_workspace_destroys_it_without_disturbing_the_new_one() {
        // The order the compositor really sends: activate the replacement, then destroy
        // the one just left.
        let mut tracker = seeded();
        feed(&mut tracker, "createworkspacev2>>3,3");
        feed(&mut tracker, "workspacev2>>3,3");
        feed(&mut tracker, "destroyworkspacev2>>1,1");

        assert_eq!(at(&tracker.drives(&settings()), "DP-1"), 0.5);
    }

    #[test]
    fn a_workspace_still_shown_keeps_its_name_through_a_destroy() {
        let mut tracker = seeded();
        feed(&mut tracker, "destroyworkspacev2>>1,1");
        assert_eq!(at(&tracker.drives(&settings()), "DP-1"), 0.0, "still shown, so still named");
    }

    #[test]
    fn a_renamed_workspace_is_placed_under_the_new_name() {
        let mut tracker = seeded();
        feed(&mut tracker, "renameworkspace>>1,4");
        assert_eq!(at(&tracker.drives(&settings()), "DP-1"), 0.75, "workspace 4 of 5");
    }

    /// A renumbered workspace takes a name with it that the event does not carry, so the
    /// stale one is dropped and the answer comes from asking again.
    #[test]
    fn a_renumbered_workspace_forgets_the_name_its_old_id_answered_to() {
        let mut tracker = seeded();
        feed(&mut tracker, "changeworkspaceid>>1,4");
        assert_eq!(
            scrolled(&tracker.drives(&settings()), "DP-1"),
            Stop::CENTRED,
            "nothing names what DP-1 shows until the backend asks"
        );
    }

    #[test]
    fn hotplug_adds_and_forgets_a_monitor() {
        let mut tracker = seeded();
        feed(&mut tracker, "monitoraddedv2>>2,HEADLESS-1,");
        let drives = tracker.drives(&settings());
        assert_eq!(
            scrolled(&drives, "HEADLESS-1"),
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
        let mut tracker = seeded();
        feed(&mut tracker, "monitorremovedv2>>0,DP-1,");
        assert_eq!(blurred(&tracker.drives(&settings())), None);
    }

    #[test]
    fn a_monitor_with_nothing_on_it_holds_no_focused_window() {
        let mut tracker = seeded();
        assert_eq!(blurred(&tracker.drives(&settings())), Some(output("DP-1")));

        // The order the compositor really sends: the focus leaves every window before the
        // monitor it is moving to is announced.
        feed(&mut tracker, "activewindowv2>>");
        feed(&mut tracker, "focusedmonv2>>eDP-1,14");

        let drives = tracker.drives(&settings());
        assert_eq!(blurred(&drives), None, "eDP-1 has the focus, nothing on it does");
        assert_eq!(at(&drives, "eDP-1"), 1.0, "which it still shows");
    }

    #[test]
    fn switching_to_an_empty_workspace_lets_the_focus_go_and_take_it_back() {
        let mut tracker = seeded();
        feed(&mut tracker, "activewindowv2>>");
        feed(&mut tracker, "workspacev2>>7,2");
        assert_eq!(blurred(&tracker.drives(&settings())), None);

        feed(&mut tracker, "activewindowv2>>55c3da6fa460");
        assert_eq!(
            blurred(&tracker.drives(&settings())),
            Some(output("DP-1")),
            "a window arriving"
        );
    }

    #[test]
    fn a_snapshot_with_nothing_focused_starts_that_way() {
        let drives = seeded_with("{}").drives(&settings());
        assert_eq!(blurred(&drives), None);
        assert_eq!(
            at(&drives, "DP-1"),
            0.0,
            "which says nothing about what the monitors are showing"
        );
    }

    #[test]
    fn moving_a_workspace_alone_does_not_move_the_monitor_it_landed_on() {
        // A workspace only becomes the active one on its new monitor if it was active on
        // the old one and that one had the focus. The event says neither, so it is not
        // read as an activation.
        let mut tracker = seeded();
        feed(&mut tracker, "moveworkspacev2>>9,9,eDP-1");

        assert_eq!(at(&tracker.drives(&settings()), "eDP-1"), 1.0);
    }

    #[test]
    fn asking_again_after_a_move_leaves_nothing_stale() {
        let mut tracker = seeded();
        feed(&mut tracker, "moveworkspacev2>>1,1,eDP-1");

        // What the backend asks for once the move is announced. DP-1 has been left with a
        // workspace nothing has named before, so the answer has to carry names too.
        let moved: Vec<wire::Monitor> = wire::decode(
            r#"[{"name":"DP-1","focused":false,"activeWorkspace":{"id":3,"name":"3"}},
                {"name":"eDP-1","focused":true,"activeWorkspace":{"id":1,"name":"1"}}]"#,
        )
        .unwrap();
        tracker.resync(moved);

        let drives = tracker.drives(&settings());
        assert_eq!(at(&drives, "eDP-1"), 0.0, "it landed here");
        assert_eq!(at(&drives, "DP-1"), 0.5, "and is no longer here");
        assert_eq!(blurred(&drives), Some(output("eDP-1")));
    }

    #[test]
    fn asking_again_does_not_disturb_the_focused_window() {
        let mut tracker = seeded_with("{}");
        tracker.resync(monitors(false));
        assert_eq!(
            blurred(&tracker.drives(&settings())),
            None,
            "the answer says which monitor has the focus, not whether a window does"
        );
    }

    /// A round trip through a gap between two equally far stops stays on one side of it.
    /// The workspace being left is destroyed on the way out, which is why what a monitor
    /// was showing is remembered by name.
    #[test]
    fn an_unlisted_workspace_holds_the_side_of_the_gap_it_came_from() {
        let sparse = named_settings(&["3", "5"]);
        let mut tracker = seeded();
        assert_eq!(at(&tracker.drives(&sparse), "DP-1"), 0.0, "1 is nearest 3");

        feed(&mut tracker, "createworkspacev2>>3,4");
        feed(&mut tracker, "workspacev2>>3,4");
        feed(&mut tracker, "destroyworkspacev2>>1,1");
        assert_eq!(at(&tracker.drives(&sparse), "DP-1"), 0.0, "reached from 1, so it stays at 3");

        feed(&mut tracker, "createworkspacev2>>8,8");
        feed(&mut tracker, "workspacev2>>8,8");
        feed(&mut tracker, "destroyworkspacev2>>3,4");
        assert_eq!(at(&tracker.drives(&sparse), "DP-1"), 1.0, "past the last, so clamped to 5");

        feed(&mut tracker, "createworkspacev2>>3,4");
        feed(&mut tracker, "workspacev2>>3,4");
        feed(&mut tracker, "destroyworkspacev2>>8,8");
        assert_eq!(at(&tracker.drives(&sparse), "DP-1"), 1.0, "reached from 8 this time, so 5");
    }

    #[test]
    fn a_workspace_with_a_window_on_it_blurs_without_holding_the_focus() {
        let tracker = seeded_windows(r#"[{"address":"0x1","workspace":{"id":14,"name":"5"}}]"#);
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
        feed(&mut tracker, "openwindow>>abc,1,kitty,zsh");
        assert_eq!(all_blurred(&tracker.drives(&blurring(NON_EMPTY))), vec!["DP-1"]);

        feed(&mut tracker, "closewindow>>abc");
        assert!(all_blurred(&tracker.drives(&blurring(NON_EMPTY))).is_empty());
    }

    #[test]
    fn a_window_handed_to_another_workspace_takes_the_blur_with_it() {
        let mut tracker =
            seeded_windows(r#"[{"address":"0xabc","workspace":{"id":1,"name":"1"}}]"#);
        assert_eq!(all_blurred(&tracker.drives(&blurring(NON_EMPTY))), vec!["DP-1"]);

        feed(&mut tracker, "movewindowv2>>abc,14,5");
        assert_eq!(all_blurred(&tracker.drives(&blurring(NON_EMPTY))), vec!["eDP-1"]);
    }

    /// A renumbered workspace is the same workspace, so what is on it is still on it.
    #[test]
    fn a_renumbered_workspace_keeps_its_windows() {
        let mut tracker =
            seeded_windows(r#"[{"address":"0xabc","workspace":{"id":1,"name":"1"}}]"#);
        feed(&mut tracker, "changeworkspaceid>>1,7");

        // What the backend asks for once the renumbering is announced.
        let renumbered: Vec<wire::Monitor> = wire::decode(
            r#"[{"name":"DP-1","focused":true,"activeWorkspace":{"id":7,"name":"1"}},
                {"name":"eDP-1","focused":false,"activeWorkspace":{"id":14,"name":"5"}}]"#,
        )
        .unwrap();
        tracker.resync(renumbered);

        assert_eq!(all_blurred(&tracker.drives(&blurring(NON_EMPTY))), vec!["DP-1"]);
    }

    /// Hyprland reuses the numbers it hands out, so a workspace going away has to take
    /// what was on it or the next one to carry its id looks occupied.
    #[test]
    fn a_destroyed_workspace_takes_its_windows_with_it() {
        let mut tracker =
            seeded_windows(r#"[{"address":"0xabc","workspace":{"id":3,"name":"3"}}]"#);
        feed(&mut tracker, "closewindow>>abc");
        feed(&mut tracker, "destroyworkspacev2>>3,3");

        feed(&mut tracker, "createworkspacev2>>3,3");
        feed(&mut tracker, "workspacev2>>3,3");
        assert!(all_blurred(&tracker.drives(&blurring(NON_EMPTY))).is_empty());
    }

    #[test]
    fn one_output_reaching_it_blurs_every_output() {
        let mut tracker = seeded();
        assert_eq!(
            all_blurred(&tracker.drives(&blurring(EVERYWHERE))),
            vec!["DP-1", "eDP-1"],
            "one window is focused, and it is on DP-1"
        );

        feed(&mut tracker, "activewindowv2>>");
        assert!(all_blurred(&tracker.drives(&blurring(EVERYWHERE))).is_empty());
    }

    #[test]
    fn one_output_may_blur_on_a_different_rule_from_the_rest() {
        let mut scoped = blurring(NON_EMPTY);
        scoped.set_output(output("DP-1"), Params { span: Span::Count(5), ..Params::default() });

        let mut tracker = seeded_windows(r#"[{"address":"0x1","workspace":{"id":14,"name":"5"}}]"#);
        feed(&mut tracker, "activewindowv2>>");
        assert_eq!(
            all_blurred(&tracker.drives(&scoped)),
            vec!["eDP-1"],
            "DP-1 follows the focus, which has left every window, while eDP-1 follows its own"
        );
    }

    /// `when` is read from the output being asked about and `scope` from the output being
    /// driven, so what `"global"` reads can hold two rules at once.
    #[test]
    fn an_output_deciding_alone_still_answers_for_one_deciding_together() {
        let alone = || Params { span: Span::Count(5), ..Params::default() };
        let mut together = blurring(Blur { scope: Scope::Global, ..NON_EMPTY });
        together.set_output(output("DP-1"), alone());

        let tracker = seeded_windows(r#"[{"address":"0x1","workspace":{"id":1,"name":"1"}}]"#);
        assert_eq!(
            all_blurred(&tracker.drives(&together)),
            vec!["DP-1", "eDP-1"],
            "eDP-1 shows an empty workspace, and blurs on DP-1 holding the focus"
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

    /// The span is per output like every other setting, and sharing one is what makes a
    /// monitor travel through only part of its wallpaper.
    #[test]
    fn an_output_is_placed_in_its_own_span() {
        let mut scoped = settings();
        scoped.set_output(output("eDP-1"), Params { span: Span::Count(14), ..Params::default() });

        let drives = seeded().drives(&scoped);
        assert_eq!(at(&drives, "DP-1"), 0.0, "workspace 1 of 5");
        assert_eq!(at(&drives, "eDP-1"), 4.0 / 13.0, "workspace 5 of 14");
    }
}
