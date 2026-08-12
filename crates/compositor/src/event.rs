use domain::{Facts, Index, OutputId};

/// Facts, normalized across compositors. What the wallpaper does about them is decided
/// in `domain::policy`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CompositorEvent {
    /// The full set of outputs the compositor knows about. Only the set is consumed:
    /// geometry comes from Wayland, so that it has exactly one source.
    OutputsChanged {
        outputs: Vec<OutputId>,
    },
    /// Which output holds the focused window, if any. Focus is global, so at most one.
    FocusMoved {
        output: Option<OutputId>,
    },
    WorkspaceActive {
        output: OutputId,
        idx: u32,
        count: u32,
    },
    /// Position in the scrolling layout, which drives the horizontal axis.
    ColumnActive {
        output: OutputId,
        idx: u32,
        count: u32,
    },
    OverviewToggled {
        active: bool,
    },
}

impl CompositorEvent {
    /// Folds the event into the accumulated view of the world. Returns whether anything
    /// actually changed, so a repeated event costs no re-resolve and no frame.
    pub fn apply_to(&self, facts: &mut Facts) -> bool {
        match self {
            CompositorEvent::OutputsChanged { outputs } => apply_outputs(facts, outputs),
            CompositorEvent::FocusMoved { output } => {
                replace(&mut facts.focused_output, output.clone())
            }
            CompositorEvent::WorkspaceActive { output, idx, count } => {
                let entry = facts.outputs.entry(output.clone()).or_default();
                replace(&mut entry.workspace, Index::new(*idx, *count))
            }
            CompositorEvent::ColumnActive { output, idx, count } => {
                let entry = facts.outputs.entry(output.clone()).or_default();
                replace(&mut entry.column, Index::new(*idx, *count))
            }
            CompositorEvent::OverviewToggled { active } => {
                replace(&mut facts.overview_active, *active)
            }
        }
    }
}

/// Adds what is new and forgets what is gone, leaving surviving outputs untouched so a
/// hotplug elsewhere does not reset their positions.
fn apply_outputs(facts: &mut Facts, outputs: &[OutputId]) -> bool {
    let before = facts.outputs.len();
    facts.outputs.retain(|id, _| outputs.contains(id));
    let mut changed = facts.outputs.len() != before;

    for id in outputs {
        if !facts.outputs.contains_key(id) {
            facts.outputs.insert(id.clone(), Default::default());
            changed = true;
        }
    }
    if facts.focused_output.as_ref().is_some_and(|id| !facts.outputs.contains_key(id)) {
        facts.focused_output = None;
        changed = true;
    }
    changed
}

fn replace<T: PartialEq>(slot: &mut T, value: T) -> bool {
    if *slot == value {
        return false;
    }
    *slot = value;
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output(name: &str) -> OutputId {
        OutputId::new(name)
    }

    #[test]
    fn a_repeated_event_reports_no_change() {
        let mut facts = Facts::default();
        let event = CompositorEvent::WorkspaceActive { output: output("DP-1"), idx: 2, count: 4 };
        assert!(event.apply_to(&mut facts));
        assert!(!event.apply_to(&mut facts));
    }

    #[test]
    fn workspaces_are_tracked_per_output() {
        let mut facts = Facts::default();
        CompositorEvent::WorkspaceActive { output: output("DP-1"), idx: 1, count: 3 }
            .apply_to(&mut facts);
        CompositorEvent::WorkspaceActive { output: output("eDP-1"), idx: 3, count: 3 }
            .apply_to(&mut facts);

        assert_eq!(facts.output(&output("DP-1")).workspace, Index::new(1, 3));
        assert_eq!(facts.output(&output("eDP-1")).workspace, Index::new(3, 3));
    }

    #[test]
    fn focus_is_exclusive() {
        let mut facts = Facts::default();
        CompositorEvent::FocusMoved { output: Some(output("DP-1")) }.apply_to(&mut facts);
        assert!(facts.is_focused(&output("DP-1")));

        CompositorEvent::FocusMoved { output: Some(output("eDP-1")) }.apply_to(&mut facts);
        assert!(!facts.is_focused(&output("DP-1")));
        assert!(facts.is_focused(&output("eDP-1")));

        CompositorEvent::FocusMoved { output: None }.apply_to(&mut facts);
        assert!(!facts.is_focused(&output("eDP-1")));
    }

    #[test]
    fn a_vanished_output_is_forgotten() {
        let mut facts = Facts::default();
        let both =
            CompositorEvent::OutputsChanged { outputs: vec![output("DP-1"), output("eDP-1")] };
        both.apply_to(&mut facts);
        CompositorEvent::WorkspaceActive { output: output("DP-1"), idx: 2, count: 4 }
            .apply_to(&mut facts);

        let one = CompositorEvent::OutputsChanged { outputs: vec![output("eDP-1")] };
        assert!(one.apply_to(&mut facts));
        assert!(!facts.outputs.contains_key(&output("DP-1")));
        assert!(facts.outputs.contains_key(&output("eDP-1")));
    }

    #[test]
    fn unplugging_the_focused_output_clears_focus() {
        let mut facts = Facts::default();
        CompositorEvent::OutputsChanged { outputs: vec![output("DP-1"), output("eDP-1")] }
            .apply_to(&mut facts);
        CompositorEvent::FocusMoved { output: Some(output("DP-1")) }.apply_to(&mut facts);

        CompositorEvent::OutputsChanged { outputs: vec![output("eDP-1")] }.apply_to(&mut facts);
        assert_eq!(facts.focused_output, None);
    }

    #[test]
    fn an_unchanged_output_set_reports_no_change() {
        let mut facts = Facts::default();
        let event = CompositorEvent::OutputsChanged { outputs: vec![output("DP-1")] };
        assert!(event.apply_to(&mut facts));
        assert!(!event.apply_to(&mut facts));
    }

    #[test]
    fn rediscovering_an_output_keeps_its_position() {
        let mut facts = Facts::default();
        let event = CompositorEvent::OutputsChanged { outputs: vec![output("DP-1")] };
        event.apply_to(&mut facts);
        CompositorEvent::WorkspaceActive { output: output("DP-1"), idx: 2, count: 4 }
            .apply_to(&mut facts);
        event.apply_to(&mut facts);

        assert_eq!(facts.output(&output("DP-1")).workspace, Index::new(2, 4));
    }

    #[test]
    fn the_overview_is_global() {
        let mut facts = Facts::default();
        assert!(CompositorEvent::OverviewToggled { active: true }.apply_to(&mut facts));
        assert!(facts.overview_active);
        assert!(!CompositorEvent::OverviewToggled { active: true }.apply_to(&mut facts));
    }
}
