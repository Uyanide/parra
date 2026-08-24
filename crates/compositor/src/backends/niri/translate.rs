use std::collections::{BTreeMap, BTreeSet, HashMap};

use domain::{OutputId, Stop};

use super::wire::{Event, Window, Workspace};
use super::{Axis, Params, Scope, When};
use crate::backends::Scoped;
use crate::event::Drive;

/// Everything the backend must remember to turn niri's report into channel positions.
///
/// The compositor reports focus as a window id and scroll position as a workspace index.
/// Joining those to an output, and normalizing them, is this file's entire job.
#[derive(Debug, Default)]
pub struct Tracker {
    workspaces: HashMap<u64, WorkspaceSlot>,
    /// Window id to the workspace it sits on.
    window_workspace: HashMap<u64, u64>,
    /// Window id to its one-based column in the scrolling layout.
    window_column: HashMap<u64, u32>,
    /// Where the focus last sat in each workspace's scrolling layout.
    ///
    /// Per workspace rather than read from the focus when reporting, because focus is
    /// global: an output that does not hold it has a scroll position all the same.
    workspace_column: HashMap<u64, u32>,
    focused_window: Option<u64>,
    overview: bool,
}

#[derive(Debug)]
struct WorkspaceSlot {
    output: Option<OutputId>,
    idx: u32,
    active: bool,
}

/// Maps a one-based position within `count` onto `0..=1`, beside the distance one place
/// in that sequence covers.
///
/// A lone or not-yet-known position sits centred, having nothing to travel between, and
/// reports no stride for the same reason.
fn progress(idx: u32, count: u32) -> Stop {
    if count <= 1 || idx == 0 {
        return Stop::CENTRED;
    }
    let span = (count - 1) as f32;
    Stop { at: (idx.min(count) - 1) as f32 / span, stride: 1.0 / span }
}

impl Tracker {
    /// Folds one compositor event into the tracked state.
    pub fn apply(&mut self, event: Event) {
        match event {
            Event::WorkspacesChanged { workspaces } => self.replace_workspaces(workspaces),
            Event::WorkspaceActivated { id } => self.activate(id),
            Event::WindowsChanged { windows } => self.replace_windows(windows),
            Event::WindowOpenedOrChanged { window } => {
                if window.is_focused {
                    self.focused_window = Some(window.id);
                }
                self.add_window(window);
            }
            Event::WindowClosed { id } => {
                self.window_workspace.remove(&id);
                self.window_column.remove(&id);
                if self.focused_window == Some(id) {
                    self.focused_window = None;
                }
            }
            Event::WindowFocusChanged { id } => self.focused_window = id,
            Event::WindowLayoutsChanged { changes } => {
                for (id, layout) in changes {
                    match layout.pos_in_scrolling_layout {
                        Some((column, _row)) => self.window_column.insert(id, column),
                        None => self.window_column.remove(&id),
                    };
                }
            }
            Event::OverviewOpenedOrClosed { is_open } => self.overview = is_open,
        }
        self.remember_focused_column();
    }

    /// Records where the focus sits in its own workspace's scrolling layout.
    ///
    /// A focused window with no column, such as a floating or fullscreen one, leaves the
    /// last value standing: the workspace has not scrolled just because something is
    /// drawn over it.
    fn remember_focused_column(&mut self) {
        let Some(window) = self.focused_window else { return };
        let (Some(&workspace), Some(&column)) =
            (self.window_workspace.get(&window), self.window_column.get(&window))
        else {
            return;
        };
        self.workspace_column.insert(workspace, column);
    }

    /// Restates the whole world as channel positions.
    ///
    /// Everything is emitted after every event rather than diffed here, because the
    /// receiving end already ignores values that did not change.
    pub fn drives(&self, settings: &Scoped<Params>) -> Vec<Drive> {
        let mut drives = vec![Drive::OutputsChanged { outputs: self.outputs() }];
        let focused = self.focused_output();
        let grouped = self.workspaces_by_output();

        // Every output's own answer is taken before any of them is told, because
        // `scope = "global"` reads the whole set rather than the output it is driving.
        let reached: BTreeMap<&OutputId, bool> = grouped
            .keys()
            .map(|output| {
                let on = match settings.for_output(output).blur.when {
                    When::Focus => focused.as_ref() == Some(output),
                    When::NonEmpty => self.occupied(output),
                };
                (output, on)
            })
            .collect();
        let anywhere = reached.values().any(|on| *on);

        for (output, workspaces) in &grouped {
            let count = workspaces.len() as u32;
            let idx = workspaces
                .iter()
                .find(|slot| slot.active)
                .map_or(0, |slot| slot.idx.min(count.max(1)));
            let workspace = (idx, count);

            let params = settings.for_output(output);
            let x = self.axis(output, params.horizontal, workspace);
            let y = self.axis(output, params.vertical, workspace);
            drives.push(Drive::Scrolled { output: output.clone(), x, y });

            // Every output is told either way, so one that no longer qualifies hears
            // about it.
            let blurred = match params.blur.scope {
                Scope::Global => anywhere,
                Scope::Output => reached[output],
            };
            drives.push(Drive::Blurred { output: output.clone(), on: blurred });

            // The overview covers every output at once.
            drives.push(Drive::ZoomedOut { output: output.clone(), on: self.overview });
        }
        drives
    }

    /// Whether this output's active workspace holds any window at all.
    ///
    /// Counts floating and fullscreen windows, which have a workspace but no place in the
    /// scroll: what is asked here is whether anything is there, not where it sits.
    fn occupied(&self, output: &OutputId) -> bool {
        let Some(workspace) = self.active_workspace(output) else { return false };
        self.window_workspace.values().any(|id| *id == workspace)
    }

    /// Where one axis sits, given what it was configured to follow.
    fn axis(&self, output: &OutputId, axis: Axis, workspace: (u32, u32)) -> Stop {
        match axis {
            Axis::Workspace => progress(workspace.0, workspace.1),
            Axis::Column => {
                let (idx, count) = self.columns_on(output);
                progress(idx, count)
            }
            Axis::None => Stop::CENTRED,
        }
    }

    /// The monitor holding the focused window. Focus is global, so at most one.
    fn focused_output(&self) -> Option<OutputId> {
        let window = self.focused_window?;
        let workspace = self.window_workspace.get(&window)?;
        self.workspaces.get(workspace)?.output.clone()
    }

    /// Where this output's active workspace sits in its own scrolling layout, and how far
    /// it can travel. `(0, _)` means nothing has been focused there yet.
    fn columns_on(&self, output: &OutputId) -> (u32, u32) {
        let Some(workspace) = self.active_workspace(output) else { return (0, 0) };
        let mut columns = BTreeSet::new();
        for (window, id) in &self.window_workspace {
            if *id == workspace
                && let Some(column) = self.window_column.get(window)
            {
                columns.insert(*column);
            }
        }
        if columns.is_empty() {
            return (0, 0);
        }
        let count = columns.len() as u32;
        let Some(&remembered) = self.workspace_column.get(&workspace) else { return (0, count) };

        // Rank rather than raw column index, so a layout with gaps still maps onto the
        // full travel. A remembered column that has closed takes the next one along.
        let rank = columns.iter().take_while(|&&column| column < remembered).count();
        (rank.min(columns.len() - 1) as u32 + 1, count)
    }

    fn active_workspace(&self, output: &OutputId) -> Option<u64> {
        self.workspaces
            .iter()
            .find(|(_, slot)| slot.active && slot.output.as_ref() == Some(output))
            .map(|(id, _)| *id)
    }

    fn outputs(&self) -> Vec<OutputId> {
        let mut outputs: Vec<OutputId> =
            self.workspaces.values().filter_map(|slot| slot.output.clone()).collect();
        outputs.sort();
        outputs.dedup();
        outputs
    }

    /// Grouped and ordered, so the reported index matches the order the compositor lays
    /// the workspaces out in.
    fn workspaces_by_output(&self) -> BTreeMap<OutputId, Vec<&WorkspaceSlot>> {
        let mut grouped: BTreeMap<OutputId, Vec<&WorkspaceSlot>> = BTreeMap::new();
        for slot in self.workspaces.values() {
            if let Some(output) = &slot.output {
                grouped.entry(output.clone()).or_default().push(slot);
            }
        }
        for slots in grouped.values_mut() {
            slots.sort_by_key(|slot| slot.idx);
        }
        grouped
    }

    fn replace_workspaces(&mut self, workspaces: Vec<Workspace>) {
        self.workspaces = workspaces
            .into_iter()
            .map(|workspace| {
                let slot = WorkspaceSlot {
                    output: workspace.output.map(OutputId::new),
                    idx: workspace.idx,
                    active: workspace.is_active,
                };
                (workspace.id, slot)
            })
            .collect();
        // So the remembered positions do not accumulate across a long session.
        let live = &self.workspaces;
        self.workspace_column.retain(|workspace, _| live.contains_key(workspace));
    }

    fn replace_windows(&mut self, windows: Vec<Window>) {
        self.window_workspace.clear();
        self.window_column.clear();
        for window in windows {
            if window.is_focused {
                self.focused_window = Some(window.id);
            }
            self.add_window(window);
        }
    }

    fn add_window(&mut self, window: Window) {
        match window.workspace_id {
            Some(workspace) => {
                self.window_workspace.insert(window.id, workspace);
            }
            None => {
                self.window_workspace.remove(&window.id);
            }
        }
        match window.layout.pos_in_scrolling_layout {
            Some((column, _row)) => self.window_column.insert(window.id, column),
            None => self.window_column.remove(&window.id),
        };
    }

    /// Activates one workspace and deactivates the others sharing its output.
    fn activate(&mut self, id: u64) {
        let Some(output) = self.workspaces.get(&id).and_then(|slot| slot.output.clone()) else {
            return;
        };
        for (workspace, slot) in &mut self.workspaces {
            if slot.output.as_ref() == Some(&output) {
                slot.active = *workspace == id;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::Blur;
    use super::*;
    use crate::backends::niri::wire;

    const FOCUS: Blur = Blur { when: When::Focus, scope: Scope::Output };
    const NON_EMPTY: Blur = Blur { when: When::NonEmpty, scope: Scope::Output };
    const EVERYWHERE: Blur = Blur { when: When::Focus, scope: Scope::Global };

    const DEFAULTS: Params =
        Params { vertical: Axis::Workspace, horizontal: Axis::None, blur: FOCUS };
    const BY_COLUMN: Params =
        Params { vertical: Axis::Workspace, horizontal: Axis::Column, blur: FOCUS };

    /// The same settings for every output, which is what a file with no per-output table
    /// resolves to.
    fn everywhere(params: Params) -> Scoped<Params> {
        Scoped::new(params)
    }

    fn feed(tracker: &mut Tracker, line: &str) {
        let event = wire::parse(line).expect("valid JSON").expect("a modelled event");
        tracker.apply(event);
    }

    /// The layout actually running on the machine this was written on.
    fn populated() -> Tracker {
        let mut tracker = Tracker::default();
        feed(
            &mut tracker,
            r#"{"WorkspacesChanged":{"workspaces":[
                {"id":1,"idx":1,"output":"DP-1","is_active":true},
                {"id":3,"idx":2,"output":"DP-1","is_active":false},
                {"id":5,"idx":3,"output":"DP-1","is_active":false},
                {"id":2,"idx":1,"output":"eDP-1","is_active":true},
                {"id":6,"idx":2,"output":"eDP-1","is_active":false}]}}"#,
        );
        feed(
            &mut tracker,
            r#"{"WindowsChanged":{"windows":[
                {"id":4,"workspace_id":1,"is_focused":true,
                 "layout":{"pos_in_scrolling_layout":[1,1]}},
                {"id":3,"workspace_id":2,"is_focused":false,
                 "layout":{"pos_in_scrolling_layout":[1,1]}}]}}"#,
        );
        tracker
    }

    fn scroll_of(drives: &[Drive], want: &str) -> (Stop, Stop) {
        drives
            .iter()
            .find_map(|drive| match drive {
                Drive::Scrolled { output, x, y } if output.as_str() == want => Some((*x, *y)),
                _ => None,
            })
            .expect("the output should be reported")
    }

    fn vertical_of(drives: &[Drive], want: &str) -> f32 {
        scroll_of(drives, want).1.at
    }

    fn horizontal_of(drives: &[Drive], want: &str) -> f32 {
        scroll_of(drives, want).0.at
    }

    fn vertical_stride_of(drives: &[Drive], want: &str) -> f32 {
        scroll_of(drives, want).1.stride
    }

    fn blurred(drives: &[Drive]) -> Vec<&str> {
        drives
            .iter()
            .filter_map(|drive| match drive {
                Drive::Blurred { output, on: true } => Some(output.as_str()),
                _ => None,
            })
            .collect()
    }

    fn zoomed_out(drives: &[Drive]) -> Vec<(&str, bool)> {
        drives
            .iter()
            .filter_map(|drive| match drive {
                Drive::ZoomedOut { output, on } => Some((output.as_str(), *on)),
                _ => None,
            })
            .collect()
    }

    /// The names printed by [`Axis`] are the ones a file is written with, so `--check`
    /// reports settings in the spelling they were typed in.
    #[test]
    fn every_axis_prints_the_name_serde_reads() {
        for axis in [Axis::Workspace, Axis::Column, Axis::None] {
            let json = format!(r#"{{"vertical":"{axis}"}}"#);
            let params: Params = serde_json::from_str(&json).expect("its own name should parse");
            assert_eq!(params.vertical, axis);
        }
    }

    #[test]
    fn the_constants_here_are_the_real_defaults() {
        assert_eq!(DEFAULTS, Params::default());
    }

    #[test]
    fn a_position_spans_the_sequence() {
        assert_eq!(progress(1, 4).at, 0.0);
        assert!((progress(2, 4).at - 1.0 / 3.0).abs() < 1e-6);
        assert_eq!(progress(4, 4).at, 1.0);
    }

    #[test]
    fn a_lone_or_unknown_position_is_centred() {
        assert_eq!(progress(1, 1), Stop::CENTRED);
        assert_eq!(progress(0, 0), Stop::CENTRED);
        assert_eq!(progress(0, 5), Stop::CENTRED);
    }

    #[test]
    fn a_stride_is_the_distance_one_place_in_the_sequence_covers() {
        assert_eq!(progress(1, 2).stride, 1.0, "two stops, so one covers everything");
        assert_eq!(progress(1, 3).stride, 0.5);
        assert!((progress(3, 5).stride - 0.25).abs() < 1e-6);
    }

    /// What the shift cap reads, so a workspace opening has to loosen it.
    #[test]
    fn opening_a_stop_shortens_the_stride() {
        assert!(progress(1, 6).stride < progress(1, 3).stride);
    }

    #[test]
    fn an_axis_with_nothing_to_travel_between_reports_no_stride() {
        assert_eq!(progress(1, 1).stride, 0.0);
        assert_eq!(progress(0, 5).stride, 0.0, "a position nothing has reported yet");
    }

    #[test]
    fn an_axis_the_backend_does_not_drive_reports_no_stride() {
        let drives = populated().drives(&everywhere(DEFAULTS));
        assert_eq!(scroll_of(&drives, "DP-1").0, Stop::CENTRED, "horizontal follows nothing");
    }

    #[test]
    fn each_output_reports_the_stride_of_its_own_workspaces() {
        let drives = populated().drives(&everywhere(DEFAULTS));
        assert_eq!(vertical_stride_of(&drives, "DP-1"), 0.5, "three workspaces");
        assert_eq!(vertical_stride_of(&drives, "eDP-1"), 1.0, "two workspaces");
    }

    #[test]
    fn an_out_of_range_position_stays_in_bounds() {
        assert_eq!(progress(9, 4).at, 1.0);
    }

    #[test]
    fn each_output_reports_its_own_workspace_position() {
        let drives = populated().drives(&everywhere(DEFAULTS));
        assert_eq!(vertical_of(&drives, "DP-1"), 0.0, "workspace 1 of 3");
        assert_eq!(vertical_of(&drives, "eDP-1"), 0.0, "workspace 1 of 2");
    }

    #[test]
    fn the_horizontal_axis_is_centred_until_it_is_asked_for() {
        let mut tracker = populated();
        columns_with_gaps(&mut tracker);
        feed(&mut tracker, r#"{"WindowFocusChanged":{"id":9}}"#);

        assert_eq!(horizontal_of(&tracker.drives(&everywhere(DEFAULTS)), "DP-1"), 0.5);
        assert_eq!(horizontal_of(&tracker.drives(&everywhere(BY_COLUMN)), "DP-1"), 1.0);
    }

    #[test]
    fn one_output_may_follow_a_different_position_from_the_rest() {
        let mut tracker = populated();
        columns_with_gaps(&mut tracker);
        feed(&mut tracker, r#"{"WindowFocusChanged":{"id":9}}"#);

        let mut settings = everywhere(DEFAULTS);
        settings.set_output(OutputId::new("DP-1"), BY_COLUMN);

        let drives = tracker.drives(&settings);
        assert_eq!(horizontal_of(&drives, "DP-1"), 1.0, "the last of three columns");
        assert_eq!(horizontal_of(&drives, "eDP-1"), 0.5, "the others keep the global setting");
    }

    #[test]
    fn an_axis_may_follow_either_position() {
        let mut tracker = populated();
        columns_with_gaps(&mut tracker);
        feed(&mut tracker, r#"{"WindowFocusChanged":{"id":9}}"#);
        let swapped = Params { vertical: Axis::Column, horizontal: Axis::Workspace, blur: FOCUS };

        let drives = tracker.drives(&everywhere(swapped));
        assert_eq!(vertical_of(&drives, "DP-1"), 1.0, "the last of three columns");
        assert_eq!(horizontal_of(&drives, "DP-1"), 0.0, "the first of three workspaces");
    }

    #[test]
    fn activating_a_workspace_moves_only_its_own_output() {
        let mut tracker = populated();
        feed(&mut tracker, r#"{"WorkspaceActivated":{"id":5,"focused":true}}"#);

        let drives = tracker.drives(&everywhere(DEFAULTS));
        assert_eq!(vertical_of(&drives, "DP-1"), 1.0, "DP-1 should have moved");
        assert_eq!(vertical_of(&drives, "eDP-1"), 0.0, "eDP-1 should be untouched");
    }

    #[test]
    fn activating_deactivates_the_previous_workspace_on_that_output() {
        let mut tracker = populated();
        feed(&mut tracker, r#"{"WorkspaceActivated":{"id":3,"focused":true}}"#);
        feed(&mut tracker, r#"{"WorkspaceActivated":{"id":5,"focused":true}}"#);
        assert_eq!(vertical_of(&tracker.drives(&everywhere(DEFAULTS)), "DP-1"), 1.0);
    }

    #[test]
    fn every_output_is_told_whether_to_blur_and_only_one_is() {
        let drives = populated().drives(&everywhere(DEFAULTS));
        let told: Vec<&str> = drives
            .iter()
            .filter_map(|drive| match drive {
                Drive::Blurred { output, .. } => Some(output.as_str()),
                _ => None,
            })
            .collect();

        assert_eq!(told, vec!["DP-1", "eDP-1"], "every output is told, not only the blurred one");
        assert_eq!(blurred(&drives), vec!["DP-1"], "niri focuses one window, so one output");
    }

    #[test]
    fn focusing_a_window_on_the_other_monitor_moves_the_blur() {
        let mut tracker = populated();
        feed(&mut tracker, r#"{"WindowFocusChanged":{"id":3}}"#);
        assert_eq!(blurred(&tracker.drives(&everywhere(DEFAULTS))), vec!["eDP-1"]);
    }

    #[test]
    fn losing_focus_leaves_every_output_sharp() {
        let mut tracker = populated();
        feed(&mut tracker, r#"{"WindowFocusChanged":{"id":null}}"#);
        assert!(blurred(&tracker.drives(&everywhere(DEFAULTS))).is_empty());
    }

    #[test]
    fn closing_the_focused_window_leaves_every_output_sharp() {
        let mut tracker = populated();
        feed(&mut tracker, r#"{"WindowClosed":{"id":4}}"#);
        assert!(blurred(&tracker.drives(&everywhere(DEFAULTS))).is_empty());
    }

    #[test]
    fn a_focused_window_on_no_workspace_blurs_no_output() {
        let mut tracker = populated();
        feed(
            &mut tracker,
            r#"{"WindowOpenedOrChanged":{"window":
                {"id":99,"workspace_id":null,"is_focused":true,"layout":{}}}}"#,
        );
        assert!(blurred(&tracker.drives(&everywhere(DEFAULTS))).is_empty());
    }

    /// The names printed by [`When`] and [`Scope`] are the ones a file is written with, for
    /// the same reason [`Axis`]'s are.
    #[test]
    fn every_blur_setting_prints_the_name_serde_reads() {
        for when in [When::Focus, When::NonEmpty] {
            let json = format!(r#"{{"blur":{{"when":"{when}"}}}}"#);
            let params: Params = serde_json::from_str(&json).expect("its own name should parse");
            assert_eq!(params.blur.when, when);
        }
        for scope in [Scope::Global, Scope::Output] {
            let json = format!(r#"{{"blur":{{"scope":"{scope}"}}}}"#);
            let params: Params = serde_json::from_str(&json).expect("its own name should parse");
            assert_eq!(params.blur.scope, scope);
        }
    }

    #[test]
    fn a_workspace_with_a_window_on_it_blurs_without_holding_the_focus() {
        let params = Params { blur: NON_EMPTY, ..DEFAULTS };
        assert_eq!(
            blurred(&populated().drives(&everywhere(params))),
            vec!["DP-1", "eDP-1"],
            "both active workspaces hold a window, and only one of them the focus"
        );
    }

    #[test]
    fn an_empty_active_workspace_stays_sharp() {
        let mut tracker = populated();
        feed(&mut tracker, r#"{"WindowsChanged":{"windows":[]}}"#);
        let params = Params { blur: NON_EMPTY, ..DEFAULTS };
        assert!(blurred(&tracker.drives(&everywhere(params))).is_empty());
    }

    #[test]
    fn a_window_with_no_column_still_fills_a_workspace() {
        let mut tracker = populated();
        feed(&mut tracker, r#"{"WorkspaceActivated":{"id":5,"focused":true}}"#);
        let params = Params { blur: NON_EMPTY, ..DEFAULTS };
        assert_eq!(
            blurred(&tracker.drives(&everywhere(params))),
            vec!["eDP-1"],
            "DP-1 moved to a workspace nothing is on"
        );

        feed(
            &mut tracker,
            r#"{"WindowOpenedOrChanged":{"window":
                {"id":77,"workspace_id":5,"is_focused":false,"layout":{}}}}"#,
        );
        assert_eq!(blurred(&tracker.drives(&everywhere(params))), vec!["DP-1", "eDP-1"]);
    }

    #[test]
    fn one_output_reaching_it_blurs_every_output() {
        let params = Params { blur: EVERYWHERE, ..DEFAULTS };
        let mut tracker = populated();
        assert_eq!(
            blurred(&tracker.drives(&everywhere(params))),
            vec!["DP-1", "eDP-1"],
            "one window is focused, and it is on DP-1"
        );

        feed(&mut tracker, r#"{"WindowFocusChanged":{"id":null}}"#);
        assert!(blurred(&tracker.drives(&everywhere(params))).is_empty());
    }

    #[test]
    fn one_output_may_blur_on_a_different_rule_from_the_rest() {
        let mut settings = everywhere(Params { blur: NON_EMPTY, ..DEFAULTS });
        settings.set_output(OutputId::new("DP-1"), DEFAULTS);

        let mut tracker = populated();
        feed(&mut tracker, r#"{"WindowFocusChanged":{"id":3}}"#);
        assert_eq!(
            blurred(&tracker.drives(&settings)),
            vec!["eDP-1"],
            "DP-1 follows the focus, which has left it, while eDP-1 follows its windows"
        );
    }

    /// `when` is read from the output being asked about and `scope` from the output being
    /// driven, so what `"global"` reads can hold two rules at once.
    #[test]
    fn an_output_deciding_alone_still_answers_for_one_deciding_together() {
        let together = Blur { scope: Scope::Global, ..NON_EMPTY };
        let mut settings = everywhere(Params { blur: together, ..DEFAULTS });
        settings.set_output(OutputId::new("DP-1"), DEFAULTS);

        let mut tracker = populated();
        feed(
            &mut tracker,
            r#"{"WindowsChanged":{"windows":[
                {"id":4,"workspace_id":1,"is_focused":true,
                 "layout":{"pos_in_scrolling_layout":[1,1]}}]}}"#,
        );
        assert_eq!(
            blurred(&tracker.drives(&settings)),
            vec!["DP-1", "eDP-1"],
            "eDP-1 shows an empty workspace, and blurs on DP-1 holding the focus"
        );

        let mut alone = everywhere(Params { blur: NON_EMPTY, ..DEFAULTS });
        alone.set_output(OutputId::new("DP-1"), DEFAULTS);
        assert_eq!(
            blurred(&tracker.drives(&alone)),
            vec!["DP-1"],
            "reading only its own answer, eDP-1 hears nothing of DP-1's"
        );
    }

    #[test]
    fn the_overview_reaches_every_output_with_the_same_value() {
        let mut tracker = populated();
        assert_eq!(
            zoomed_out(&tracker.drives(&everywhere(DEFAULTS))),
            vec![("DP-1", false), ("eDP-1", false)]
        );

        feed(&mut tracker, r#"{"OverviewOpenedOrClosed":{"is_open":true}}"#);
        assert_eq!(
            zoomed_out(&tracker.drives(&everywhere(DEFAULTS))),
            vec![("DP-1", true), ("eDP-1", true)]
        );
    }

    #[test]
    fn outputs_are_taken_from_the_workspace_list() {
        let drives = populated().drives(&everywhere(DEFAULTS));
        let outputs = drives
            .iter()
            .find_map(|drive| match drive {
                Drive::OutputsChanged { outputs } => Some(outputs.clone()),
                _ => None,
            })
            .expect("outputs should be reported");
        assert_eq!(outputs, vec![OutputId::new("DP-1"), OutputId::new("eDP-1")]);
    }

    /// Three columns on DP-1's active workspace, at 1, 5 and 9, with the first focused.
    fn columns_with_gaps(tracker: &mut Tracker) {
        feed(
            tracker,
            r#"{"WindowsChanged":{"windows":[
                {"id":4,"workspace_id":1,"is_focused":true,
                 "layout":{"pos_in_scrolling_layout":[1,1]}},
                {"id":8,"workspace_id":1,"is_focused":false,
                 "layout":{"pos_in_scrolling_layout":[5,1]}},
                {"id":9,"workspace_id":1,"is_focused":false,
                 "layout":{"pos_in_scrolling_layout":[9,1]}}]}}"#,
        );
    }

    #[test]
    fn columns_are_ranked_within_the_active_workspace() {
        let mut tracker = populated();
        columns_with_gaps(&mut tracker);
        assert_eq!(
            horizontal_of(&tracker.drives(&everywhere(BY_COLUMN)), "DP-1"),
            0.0,
            "gaps must not inflate the travel"
        );

        feed(&mut tracker, r#"{"WindowFocusChanged":{"id":9}}"#);
        assert_eq!(horizontal_of(&tracker.drives(&everywhere(BY_COLUMN)), "DP-1"), 1.0);
    }

    #[test]
    fn an_output_without_the_focus_still_reports_its_own_column() {
        let mut tracker = populated();
        columns_with_gaps(&mut tracker);
        feed(&mut tracker, r#"{"WindowFocusChanged":{"id":9}}"#);
        assert_eq!(horizontal_of(&tracker.drives(&everywhere(BY_COLUMN)), "DP-1"), 1.0);

        // Focus leaves for the other monitor. DP-1 has not scrolled, so it must not move.
        feed(
            &mut tracker,
            r#"{"WindowOpenedOrChanged":{"window":
                {"id":20,"workspace_id":2,"is_focused":true,
                 "layout":{"pos_in_scrolling_layout":[1,1]}}}}"#,
        );
        let drives = tracker.drives(&everywhere(BY_COLUMN));
        assert_eq!(blurred(&drives), vec!["eDP-1"]);
        assert_eq!(
            horizontal_of(&drives, "DP-1"),
            1.0,
            "an unblurred output must keep its position rather than centring"
        );
        assert_eq!(horizontal_of(&drives, "eDP-1"), 0.5, "a lone column has nowhere to travel");
    }

    #[test]
    fn focusing_a_window_with_no_column_holds_the_position() {
        let mut tracker = populated();
        columns_with_gaps(&mut tracker);
        feed(&mut tracker, r#"{"WindowFocusChanged":{"id":8}}"#);
        assert_eq!(horizontal_of(&tracker.drives(&everywhere(BY_COLUMN)), "DP-1"), 0.5);

        // A floating window has no place in the scroll, so nothing has scrolled.
        feed(
            &mut tracker,
            r#"{"WindowOpenedOrChanged":{"window":
                {"id":50,"workspace_id":1,"is_focused":true,"layout":{}}}}"#,
        );
        assert_eq!(
            horizontal_of(&tracker.drives(&everywhere(BY_COLUMN)), "DP-1"),
            0.5,
            "a float must not recentre what has not moved"
        );
    }

    #[test]
    fn a_remembered_column_that_closed_falls_back_to_a_neighbour() {
        let mut tracker = populated();
        columns_with_gaps(&mut tracker);
        feed(&mut tracker, r#"{"WindowFocusChanged":{"id":9}}"#);
        assert_eq!(horizontal_of(&tracker.drives(&everywhere(BY_COLUMN)), "DP-1"), 1.0);

        // The last column closes while the focus is elsewhere.
        feed(
            &mut tracker,
            r#"{"WindowsChanged":{"windows":[
                {"id":4,"workspace_id":1,"is_focused":false,
                 "layout":{"pos_in_scrolling_layout":[1,1]}},
                {"id":8,"workspace_id":1,"is_focused":false,
                 "layout":{"pos_in_scrolling_layout":[5,1]}},
                {"id":20,"workspace_id":2,"is_focused":true,
                 "layout":{"pos_in_scrolling_layout":[1,1]}}]}}"#,
        );
        assert_eq!(
            horizontal_of(&tracker.drives(&everywhere(BY_COLUMN)), "DP-1"),
            1.0,
            "the nearest surviving column, which is now the last of two"
        );
    }

    #[test]
    fn a_workspace_nothing_has_been_focused_on_reports_the_centre() {
        let mut tracker = Tracker::default();
        feed(
            &mut tracker,
            r#"{"WorkspacesChanged":{"workspaces":[
                {"id":1,"idx":1,"output":"DP-1","is_active":true}]}}"#,
        );
        feed(
            &mut tracker,
            r#"{"WindowsChanged":{"windows":[
                {"id":8,"workspace_id":1,"is_focused":false,
                 "layout":{"pos_in_scrolling_layout":[1,1]}},
                {"id":9,"workspace_id":1,"is_focused":false,
                 "layout":{"pos_in_scrolling_layout":[2,1]}}]}}"#,
        );
        assert_eq!(horizontal_of(&tracker.drives(&everywhere(BY_COLUMN)), "DP-1"), 0.5);
    }

    #[test]
    fn an_empty_workspace_reports_the_centre() {
        let mut tracker = populated();
        feed(&mut tracker, r#"{"WindowsChanged":{"windows":[]}}"#);
        assert_eq!(horizontal_of(&tracker.drives(&everywhere(BY_COLUMN)), "DP-1"), 0.5);
    }
}
