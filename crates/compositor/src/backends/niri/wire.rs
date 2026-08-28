use std::collections::BTreeMap;

use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

/// The events this daemon acts on, and only the fields it reads.
///
/// Modelled here rather than pulled from the compositor's own crate, which is versioned
/// in lockstep with it and rejects any event variant it does not know. An unrecognized
/// event costs nothing here, so the two upgrade independently.
#[derive(Debug)]
pub enum Event {
    /// Replaces the entire workspace configuration. Anything absent was deleted.
    WorkspacesChanged {
        workspaces: Vec<Workspace>,
    },
    /// A workspace became the visible one on its output. Every other workspace on that
    /// same output is now inactive. Whether it also took focus is ignored here, because
    /// focus is tracked through the window that holds it.
    WorkspaceActivated {
        id: u64,
    },
    /// A workspace's own active window changed. Sent for any workspace, not only the
    /// focused one, so it is the way a background workspace's scroll position updates.
    WorkspaceActiveWindowChanged {
        workspace_id: u64,
        active_window_id: Option<u64>,
    },
    WindowsChanged {
        windows: Vec<Window>,
    },
    WindowOpenedOrChanged {
        window: Window,
    },
    WindowClosed {
        id: u64,
    },
    WindowFocusChanged {
        id: Option<u64>,
    },
    WindowLayoutsChanged {
        changes: Vec<(u64, WindowLayout)>,
    },
    OverviewOpenedOrClosed {
        is_open: bool,
    },
}

#[derive(Debug, Deserialize)]
pub struct Workspace {
    pub id: u64,
    /// One-based position on its own monitor. Changes as workspaces are reordered.
    pub idx: u32,
    /// Absent when no output is connected.
    pub output: Option<String>,
    pub is_active: bool,
    /// Id of the window this workspace currently reports as active, independent of
    /// global keyboard focus.
    #[serde(deserialize_with = "present_but_nullable")]
    pub active_window_id: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct Window {
    pub id: u64,
    pub workspace_id: Option<u64>,
    #[serde(default)]
    pub is_focused: bool,
    #[serde(default)]
    pub layout: WindowLayout,
}

#[derive(Debug, Default, Deserialize)]
pub struct WindowLayout {
    /// Column and row in the scrolling layout, both one-based. Absent for floating and
    /// fullscreen windows, which have no place in the scroll.
    #[serde(default)]
    pub pos_in_scrolling_layout: Option<(u32, u32)>,
}

/// Payloads, one per event that carries something worth reading.
mod payload {
    use super::{Deserialize, Window, WindowLayout, Workspace};

    #[derive(Deserialize)]
    pub struct Workspaces {
        pub workspaces: Vec<Workspace>,
    }

    #[derive(Deserialize)]
    pub struct Windows {
        pub windows: Vec<Window>,
    }

    #[derive(Deserialize)]
    pub struct OneWindow {
        pub window: Window,
    }

    #[derive(Deserialize)]
    pub struct Id {
        pub id: u64,
    }

    #[derive(Deserialize)]
    pub struct WorkspaceActiveWindow {
        pub workspace_id: u64,
        pub active_window_id: Option<u64>,
    }

    #[derive(Deserialize)]
    pub struct MaybeId {
        pub id: Option<u64>,
    }

    #[derive(Deserialize)]
    pub struct Layouts {
        pub changes: Vec<(u64, WindowLayout)>,
    }

    #[derive(Deserialize)]
    pub struct Overview {
        pub is_open: bool,
    }
}

/// One line of the event stream, or `None` if it names an event this daemon ignores.
///
/// The variant name is matched before the payload is read, which separates the two
/// failures that would otherwise look identical: an event added by a newer compositor is
/// uninteresting, while an event we do model failing to parse means the format moved
/// under us and scrolling is about to stop working. Only the second is an error.
///
/// The match arms are the only list of what is modelled, so nothing can drift out of
/// step with them.
pub fn parse(line: &str) -> Result<Option<Event>, serde_json::Error> {
    let envelope: BTreeMap<String, Value> = serde_json::from_str(line)?;
    let Some((name, body)) = envelope.into_iter().next() else { return Ok(None) };

    let event = match name.as_str() {
        "WorkspacesChanged" => {
            Event::WorkspacesChanged { workspaces: read::<payload::Workspaces>(body)?.workspaces }
        }
        "WorkspaceActivated" => Event::WorkspaceActivated { id: read::<payload::Id>(body)?.id },
        "WorkspaceActiveWindowChanged" => {
            let payload::WorkspaceActiveWindow { workspace_id, active_window_id } = read(body)?;
            Event::WorkspaceActiveWindowChanged { workspace_id, active_window_id }
        }
        "WindowsChanged" => {
            Event::WindowsChanged { windows: read::<payload::Windows>(body)?.windows }
        }
        "WindowOpenedOrChanged" => {
            Event::WindowOpenedOrChanged { window: read::<payload::OneWindow>(body)?.window }
        }
        "WindowClosed" => Event::WindowClosed { id: read::<payload::Id>(body)?.id },
        "WindowFocusChanged" => {
            Event::WindowFocusChanged { id: read::<payload::MaybeId>(body)?.id }
        }
        "WindowLayoutsChanged" => {
            Event::WindowLayoutsChanged { changes: read::<payload::Layouts>(body)?.changes }
        }
        "OverviewOpenedOrClosed" => {
            Event::OverviewOpenedOrClosed { is_open: read::<payload::Overview>(body)?.is_open }
        }
        _ => return Ok(None),
    };
    Ok(Some(event))
}

fn read<T: DeserializeOwned>(body: Value) -> Result<T, serde_json::Error> {
    serde_json::from_value(body)
}

/// Reads a field niri always writes, whose value may still be null.
///
/// serde reads a *missing* `Option` field as `None`, which for `active_window_id` would
/// make the field disappearing upstream look exactly like a workspace with nothing active:
/// the column axis would quietly freeze where it stood instead of saying the format moved.
/// Distinguishing the two is the whole reason [`parse`] treats a modelled event that no
/// longer reads as an error.
fn present_but_nullable<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::deserialize(deserializer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_workspace_list() {
        let line = r#"{"WorkspacesChanged":{"workspaces":[
            {"id":1,"idx":1,"name":null,"output":"DP-1","is_urgent":false,
             "is_active":true,"is_focused":true,"active_window_id":4}]}}"#;
        let Some(Event::WorkspacesChanged { workspaces }) = parse(line).unwrap() else {
            panic!("wrong event")
        };
        assert_eq!(workspaces.len(), 1);
        assert_eq!(workspaces[0].id, 1);
        assert_eq!(workspaces[0].idx, 1);
        assert_eq!(workspaces[0].output.as_deref(), Some("DP-1"));
        assert!(workspaces[0].is_active);
        assert_eq!(workspaces[0].active_window_id, Some(4));
    }

    #[test]
    fn a_workspace_without_an_output_still_parses() {
        let line = r#"{"WorkspacesChanged":{"workspaces":[
            {"id":9,"idx":1,"output":null,"is_active":false,"active_window_id":null}]}}"#;
        let Some(Event::WorkspacesChanged { workspaces }) = parse(line).unwrap() else {
            panic!("wrong event")
        };
        assert_eq!(workspaces[0].output, None);
        assert_eq!(workspaces[0].active_window_id, None, "nothing is active there");
    }

    #[test]
    fn reads_focus_changes_including_losing_focus() {
        let focused = r#"{"WindowFocusChanged":{"id":7}}"#;
        assert!(matches!(parse(focused).unwrap(), Some(Event::WindowFocusChanged { id: Some(7) })));

        let none = r#"{"WindowFocusChanged":{"id":null}}"#;
        assert!(matches!(parse(none).unwrap(), Some(Event::WindowFocusChanged { id: None })));
    }

    #[test]
    fn reads_a_workspace_active_window_change() {
        let some = r#"{"WorkspaceActiveWindowChanged":{"workspace_id":1,"active_window_id":4}}"#;
        assert!(matches!(
            parse(some).unwrap(),
            Some(Event::WorkspaceActiveWindowChanged {
                workspace_id: 1,
                active_window_id: Some(4)
            })
        ));

        let none = r#"{"WorkspaceActiveWindowChanged":{"workspace_id":1,"active_window_id":null}}"#;
        assert!(matches!(
            parse(none).unwrap(),
            Some(Event::WorkspaceActiveWindowChanged { workspace_id: 1, active_window_id: None })
        ));
    }

    #[test]
    fn reads_the_scrolling_position() {
        let line = r#"{"WindowLayoutsChanged":{"changes":[
            [7,{"pos_in_scrolling_layout":[2,1],"tile_size":[100.0,100.0]}]]}}"#;
        let Some(Event::WindowLayoutsChanged { changes }) = parse(line).unwrap() else {
            panic!("wrong event")
        };
        assert_eq!(changes[0].0, 7);
        assert_eq!(changes[0].1.pos_in_scrolling_layout, Some((2, 1)));
    }

    #[test]
    fn a_floating_window_has_no_scrolling_position() {
        let line = r#"{"WindowLayoutsChanged":{"changes":[[7,{"tile_size":[10.0,10.0]}]]}}"#;
        let Some(Event::WindowLayoutsChanged { changes }) = parse(line).unwrap() else {
            panic!("wrong event")
        };
        assert_eq!(changes[0].1.pos_in_scrolling_layout, None);
    }

    #[test]
    fn events_we_do_not_model_are_ignored_rather_than_fatal() {
        for line in [
            r#"{"KeyboardLayoutsChanged":{"keyboard_layouts":{"names":["German"],"current_idx":0}}}"#,
            r#"{"ConfigLoaded":{"failed":false}}"#,
            r#"{"CastsChanged":{"casts":[]}}"#,
            r#"{"SomethingInventedLater":{"whatever":[1,2,3]}}"#,
        ] {
            assert!(parse(line).unwrap().is_none(), "{line} should be ignored");
        }
    }

    #[test]
    fn unknown_fields_on_known_events_are_ignored() {
        let line = r#"{"OverviewOpenedOrClosed":{"is_open":true,"invented_later":42}}"#;
        assert!(matches!(
            parse(line).unwrap(),
            Some(Event::OverviewOpenedOrClosed { is_open: true })
        ));
    }

    #[test]
    fn a_line_that_is_not_json_is_an_error() {
        assert!(parse("not json at all").is_err());
    }

    #[test]
    fn an_event_we_model_that_no_longer_parses_is_an_error_not_a_shrug() {
        // What a rename or a type change upstream would look like. Silently ignoring
        // these is how scrolling would stop working with nothing in the log to explain it.
        let renamed = r#"{"WorkspacesChanged":{"workspace_list":[]}}"#;
        assert!(parse(renamed).is_err(), "a missing field must not be ignored");

        let retyped = r#"{"OverviewOpenedOrClosed":{"is_open":"yes"}}"#;
        assert!(parse(retyped).is_err(), "a changed type must not be ignored");

        let reshaped = r#"{"WorkspacesChanged":{"workspaces":[{"id":1}]}}"#;
        assert!(parse(reshaped).is_err(), "a workspace missing its index must not be ignored");

        let dropped = r#"{"WorkspacesChanged":{"workspaces":[
            {"id":1,"idx":1,"output":"DP-1","is_active":true}]}}"#;
        assert!(
            parse(dropped).is_err(),
            "the field the column axis is seeded from must not go missing quietly"
        );
    }

    #[test]
    fn an_empty_object_is_not_an_event() {
        assert!(parse("{}").unwrap().is_none());
    }
}
