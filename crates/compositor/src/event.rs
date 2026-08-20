use domain::{Channels, Driven, OutputId, Stop};

/// One thing a compositor backend drives, already normalized.
#[derive(Clone, Debug, PartialEq)]
pub enum Drive {
    /// The full set of outputs the compositor knows about. Only the set is consumed:
    /// geometry comes from Wayland, so that it has exactly one source.
    OutputsChanged {
        outputs: Vec<OutputId>,
    },
    /// Where this output's wallpaper sits, each axis normalized to `0..=1`, with `0.5`
    /// for an axis the backend does not drive. Anything else is brought into range.
    ///
    /// Each axis carries the distance one of its stops covers as well as its position,
    /// because how far a single move goes is the backend's to know and nothing
    /// downstream can recover it from a position alone.
    Scrolled {
        output: OutputId,
        x: Stop,
        y: Stop,
    },
    Blurred {
        output: OutputId,
        on: bool,
    },
    ZoomedOut {
        output: OutputId,
        on: bool,
    },
}

impl Drive {
    /// Folds the event into the accumulated view of the world. Returns whether anything
    /// actually changed, so a repeated event costs no re-resolve and no frame.
    pub fn apply_to(&self, driven: &mut Driven) -> bool {
        match self {
            Drive::OutputsChanged { outputs } => apply_outputs(driven, outputs),
            Drive::Scrolled { output, x, y } => {
                let entry = driven.outputs.entry(output.clone()).or_default();
                let moved_x = replace(&mut entry.x, Stop::read(x.at, x.stride));
                let moved_y = replace(&mut entry.y, Stop::read(y.at, y.stride));
                moved_x || moved_y
            }
            Drive::Blurred { output, on } => {
                let entry = driven.outputs.entry(output.clone()).or_default();
                replace(&mut entry.blur, *on)
            }
            Drive::ZoomedOut { output, on } => {
                let entry = driven.outputs.entry(output.clone()).or_default();
                replace(&mut entry.zoom_out, *on)
            }
        }
    }
}

/// Adds what is new and forgets what is gone, leaving surviving outputs untouched so a
/// hotplug elsewhere does not reset their positions.
fn apply_outputs(driven: &mut Driven, outputs: &[OutputId]) -> bool {
    let before = driven.outputs.len();
    driven.outputs.retain(|id, _| outputs.contains(id));
    let mut changed = driven.outputs.len() != before;

    for id in outputs {
        if !driven.outputs.contains_key(id) {
            driven.outputs.insert(id.clone(), Channels::default());
            changed = true;
        }
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

    /// A position on each axis with a stride to match, which is what a compositor with
    /// three stops reports.
    fn scrolled(name: &str, x: f32, y: f32) -> Drive {
        Drive::Scrolled {
            output: output(name),
            x: Stop { at: x, stride: 0.5 },
            y: Stop { at: y, stride: 0.5 },
        }
    }

    #[test]
    fn a_repeated_event_reports_no_change() {
        let mut driven = Driven::default();
        let event = scrolled("DP-1", 0.25, 0.75);
        assert!(event.apply_to(&mut driven));
        assert!(!event.apply_to(&mut driven));
    }

    /// A workspace opening while the active one stays put moves no position but does
    /// change how far a stop is, and the cap downstream reads that. Reported as unchanged
    /// it would never be re-resolved.
    #[test]
    fn a_stride_changing_on_its_own_is_a_change() {
        let mut driven = Driven::default();
        scrolled("DP-1", 0.0, 0.0).apply_to(&mut driven);

        let widened = Drive::Scrolled {
            output: output("DP-1"),
            x: Stop { at: 0.0, stride: 0.25 },
            y: Stop { at: 0.0, stride: 0.25 },
        };
        assert!(widened.apply_to(&mut driven));
        assert_eq!(driven.output(&output("DP-1")).y.stride, 0.25);
    }

    #[test]
    fn a_stride_that_is_not_a_number_reads_as_no_stride() {
        let mut driven = Driven::default();
        let event = Drive::Scrolled {
            output: output("DP-1"),
            x: Stop { at: 0.25, stride: f32::NAN },
            y: Stop { at: 0.25, stride: 2.0 },
        };

        assert!(event.apply_to(&mut driven));
        assert_eq!(driven.output(&output("DP-1")).x.stride, 0.0);
        assert_eq!(driven.output(&output("DP-1")).y.stride, 1.0, "brought into range");
        assert!(!event.apply_to(&mut driven), "a value differing from itself is not movement");
    }

    #[test]
    fn one_axis_moving_is_still_a_change() {
        let mut driven = Driven::default();
        scrolled("DP-1", 0.25, 0.75).apply_to(&mut driven);
        assert!(scrolled("DP-1", 0.25, 0.5).apply_to(&mut driven));
    }

    #[test]
    fn a_position_outside_the_range_is_brought_into_it() {
        let mut driven = Driven::default();
        scrolled("DP-1", -3.0, 42.0).apply_to(&mut driven);

        assert_eq!(driven.output(&output("DP-1")).x.at, 0.0);
        assert_eq!(driven.output(&output("DP-1")).y.at, 1.0);
    }

    #[test]
    fn a_position_that_is_not_a_number_reads_as_centred() {
        let mut driven = Driven::default();
        let event = scrolled("DP-1", f32::NAN, 0.25);

        assert!(event.apply_to(&mut driven));
        assert_eq!(driven.output(&output("DP-1")).x.at, Channels::default().x.at);
        assert!(!event.apply_to(&mut driven), "a value differing from itself is not movement");
    }

    #[test]
    fn positions_are_tracked_per_output() {
        let mut driven = Driven::default();
        scrolled("DP-1", 0.0, 0.25).apply_to(&mut driven);
        scrolled("eDP-1", 1.0, 0.75).apply_to(&mut driven);

        assert_eq!(driven.output(&output("DP-1")).x.at, 0.0);
        assert_eq!(driven.output(&output("DP-1")).y.at, 0.25);
        assert_eq!(driven.output(&output("eDP-1")).x.at, 1.0);
        assert_eq!(driven.output(&output("eDP-1")).y.at, 0.75);
    }

    #[test]
    fn blur_is_tracked_per_output() {
        let mut driven = Driven::default();
        Drive::Blurred { output: output("DP-1"), on: true }.apply_to(&mut driven);
        Drive::Blurred { output: output("eDP-1"), on: true }.apply_to(&mut driven);

        assert!(driven.output(&output("DP-1")).blur, "two blurred outputs are representable");
        assert!(driven.output(&output("eDP-1")).blur);
    }

    #[test]
    fn zooming_out_is_tracked_per_output() {
        let mut driven = Driven::default();
        Drive::ZoomedOut { output: output("DP-1"), on: true }.apply_to(&mut driven);

        assert!(driven.output(&output("DP-1")).zoom_out);
        assert!(!driven.output(&output("eDP-1")).zoom_out, "one output zooming does not zoom both");
    }

    #[test]
    fn a_vanished_output_is_forgotten() {
        let mut driven = Driven::default();
        let both = Drive::OutputsChanged { outputs: vec![output("DP-1"), output("eDP-1")] };
        both.apply_to(&mut driven);
        scrolled("DP-1", 0.0, 0.25).apply_to(&mut driven);

        let one = Drive::OutputsChanged { outputs: vec![output("eDP-1")] };
        assert!(one.apply_to(&mut driven));
        assert!(!driven.outputs.contains_key(&output("DP-1")));
        assert!(driven.outputs.contains_key(&output("eDP-1")));
    }

    #[test]
    fn an_unchanged_output_set_reports_no_change() {
        let mut driven = Driven::default();
        let event = Drive::OutputsChanged { outputs: vec![output("DP-1")] };
        assert!(event.apply_to(&mut driven));
        assert!(!event.apply_to(&mut driven));
    }

    #[test]
    fn rediscovering_an_output_keeps_its_position() {
        let mut driven = Driven::default();
        let event = Drive::OutputsChanged { outputs: vec![output("DP-1")] };
        event.apply_to(&mut driven);
        scrolled("DP-1", 0.0, 0.25).apply_to(&mut driven);
        event.apply_to(&mut driven);

        assert_eq!(driven.output(&output("DP-1")).y.at, 0.25);
    }

    #[test]
    fn an_output_appearing_starts_from_the_neutral_position() {
        let mut driven = Driven::default();
        Drive::OutputsChanged { outputs: vec![output("DP-1")] }.apply_to(&mut driven);
        assert_eq!(driven.output(&output("DP-1")), Channels::default());
    }
}
