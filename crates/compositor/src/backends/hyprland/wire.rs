use std::fmt;

use serde::Deserialize;

/// The events this daemon acts on, and only the fields it reads.
///
/// Modelled here rather than taken from a wrapper crate, for the same reasons the niri
/// events are: an unrecognized event costs nothing here, so the compositor and this daemon
/// upgrade independently.
///
/// Every variant that has one is the `v2` spelling of its event. The originals identify
/// workspaces and monitors by name alone, which cannot survive a rename, and carry no id
/// to join on.
#[derive(Debug, PartialEq, Eq)]
pub enum Event {
    MonitorAdded {
        name: String,
    },
    MonitorRemoved {
        name: String,
    },
    /// The focused monitor and the workspace it is showing. The only event that reports
    /// the two together, so it is what the join is anchored on.
    FocusedMonitor {
        monitor: String,
        workspace: i64,
    },
    /// A workspace became the active one. Carries no monitor, because it is always the
    /// focused one.
    WorkspaceActive {
        id: i64,
        name: String,
    },
    WorkspaceCreated {
        id: i64,
        name: String,
    },
    WorkspaceDestroyed {
        id: i64,
    },
    WorkspaceRenamed {
        id: i64,
        name: String,
    },
    /// A workspace was handed to another monitor, which nothing else reports. Moving one
    /// is the only way a monitor stops showing a workspace without activating another.
    WorkspaceMoved {
        id: i64,
        name: String,
        monitor: String,
    },
    /// A workspace was renumbered, taking its name with it unless it had been renamed by
    /// hand. The event says neither what it is called now nor which monitor holds it.
    WorkspaceIdChanged {
        from: i64,
        to: i64,
    },
    /// Whether any window holds the focus at all.
    ///
    /// Carries no monitor because the focused window is always on the focused one. What
    /// matters here is only the difference between some window and none, since a monitor
    /// showing an empty workspace has the focus without anything on it being focused.
    ActiveWindow {
        focused: bool,
    },
}

/// An event this daemon does model, in a shape it no longer recognizes.
///
/// Worth telling apart from an event we simply ignore: this one means the format moved and
/// the wallpaper is about to stop reacting, which is otherwise silent.
#[derive(Debug, PartialEq, Eq)]
pub struct Malformed {
    pub event: String,
    pub reason: &'static str,
}

impl fmt::Display for Malformed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} does not parse: {}", self.event, self.reason)
    }
}

/// One line of the event stream, or `None` if it names an event this daemon ignores.
///
/// Fields are comma separated and one of them is free text: a monitor description or a
/// workspace name may contain commas of its own. So every payload is cut a fixed number of
/// times from whichever ends pin the fixed fields down, and never simply split on every
/// comma.
pub fn parse(line: &str) -> Result<Option<Event>, Malformed> {
    let Some((name, data)) = line.trim_end().split_once(">>") else { return Ok(None) };
    let bad = |reason| Malformed { event: name.to_owned(), reason };

    let event = match name {
        // Trailing description, which is free text.
        "monitoraddedv2" => Event::MonitorAdded { name: field(data, 1, 3).ok_or(bad(NAME))? },
        "monitorremovedv2" => Event::MonitorRemoved { name: field(data, 1, 3).ok_or(bad(NAME))? },
        // The workspace id is last, so the monitor name is whatever precedes it.
        "focusedmonv2" => {
            let (monitor, id) = data.rsplit_once(',').ok_or(bad(FIELDS))?;
            Event::FocusedMonitor {
                monitor: monitor.to_owned(),
                workspace: id.parse().map_err(|_| bad(ID))?,
            }
        }
        // Leading id, then a workspace name which is free text.
        "workspacev2" => {
            let (id, name) = split_id(data).ok_or(bad(FIELDS))?;
            Event::WorkspaceActive { id: id.ok_or(bad(ID))?, name }
        }
        "createworkspacev2" => {
            let (id, name) = split_id(data).ok_or(bad(FIELDS))?;
            Event::WorkspaceCreated { id: id.ok_or(bad(ID))?, name }
        }
        "destroyworkspacev2" => {
            let (id, _) = split_id(data).ok_or(bad(FIELDS))?;
            Event::WorkspaceDestroyed { id: id.ok_or(bad(ID))? }
        }
        "renameworkspace" => {
            let (id, name) = split_id(data).ok_or(bad(FIELDS))?;
            Event::WorkspaceRenamed { id: id.ok_or(bad(ID))?, name }
        }
        // Pinned from both ends: the id leads and the monitor trails, so the workspace
        // name is whatever is left between them and may hold commas of its own.
        "moveworkspacev2" => {
            let (id, rest) = data.split_once(',').ok_or(bad(FIELDS))?;
            let (name, monitor) = rest.rsplit_once(',').ok_or(bad(FIELDS))?;
            Event::WorkspaceMoved {
                id: id.parse().map_err(|_| bad(ID))?,
                name: name.to_owned(),
                monitor: monitor.to_owned(),
            }
        }
        // Two ids and no free text at all, so this one really is a plain split.
        "changeworkspaceid" => {
            let (from, to) = data.split_once(',').ok_or(bad(FIELDS))?;
            Event::WorkspaceIdChanged {
                from: from.parse().map_err(|_| bad(ID))?,
                to: to.parse().map_err(|_| bad(ID))?,
            }
        }
        // An address, or nothing at all when the focus left every window. The address
        // itself is never used, so only the difference between the two is read.
        "activewindowv2" => Event::ActiveWindow { focused: !data.is_empty() },
        _ => return Ok(None),
    };
    Ok(Some(event))
}

const NAME: &str = "a name is missing";
const FIELDS: &str = "too few fields";
const ID: &str = "an id is not a number";

/// Field `at` of exactly `count`, counted from the left, with the last one allowed to
/// contain the separator itself.
fn field(data: &str, at: usize, count: usize) -> Option<String> {
    data.splitn(count, ',').nth(at).map(str::to_owned)
}

/// A leading id and everything after it, which is one free-text field.
fn split_id(data: &str) -> Option<(Option<i64>, String)> {
    let (id, rest) = data.split_once(',')?;
    Some((id.parse().ok(), rest.to_owned()))
}

/// What `j/monitors` says, and only the fields this daemon reads.
#[derive(Debug, Deserialize)]
pub struct Monitor {
    pub name: String,
    pub focused: bool,
    #[serde(rename = "activeWorkspace")]
    pub active_workspace: WorkspaceRef,
}

/// The workspace a monitor is showing, named as well as identified, so asking which
/// monitor shows what also answers what those workspaces are called.
#[derive(Debug, Deserialize)]
pub struct WorkspaceRef {
    pub id: i64,
    pub name: String,
}

/// What `j/workspaces` says. Read once at cold start for the names, which the event stream
/// then keeps current by itself.
#[derive(Debug, Deserialize)]
pub struct Workspace {
    pub id: i64,
    pub name: String,
}

/// What `j/activewindow` says, which is `{}` when the focus is on no window at all.
///
/// The address is never used for anything; it is read only because its presence is what
/// tells a focused window apart from none, which is the same thing `activewindowv2`
/// reports once the stream is running.
#[derive(Debug, Default, Deserialize)]
pub struct ActiveWindow {
    pub address: Option<String>,
}

/// A refused request answers in plain words rather than JSON, so a parse failure is how one
/// is recognized and is worth reporting as itself.
pub fn decode<T: serde::de::DeserializeOwned>(answer: &str) -> Result<T, String> {
    serde_json::from_str(answer).map_err(|error| {
        let answer = answer.trim();
        if answer.starts_with('{') || answer.starts_with('[') {
            format!("{error}")
        } else {
            format!("refused it: {answer}")
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_workspace_becoming_active() {
        let line = "workspacev2>>3,3";
        assert_eq!(
            parse(line).unwrap(),
            Some(Event::WorkspaceActive { id: 3, name: "3".to_owned() })
        );
    }

    #[test]
    fn a_named_workspace_keeps_its_negative_id() {
        // Named workspaces are allocated descending negative ids, so nothing may assume a
        // workspace id is positive or has anything to do with its place.
        let line = "createworkspacev2>>-1337,browser";
        assert_eq!(
            parse(line).unwrap(),
            Some(Event::WorkspaceCreated { id: -1337, name: "browser".to_owned() })
        );
    }

    #[test]
    fn a_workspace_name_may_contain_a_comma() {
        let line = "workspacev2>>7,one, two";
        assert_eq!(
            parse(line).unwrap(),
            Some(Event::WorkspaceActive { id: 7, name: "one, two".to_owned() })
        );
    }

    #[test]
    fn reads_the_focused_monitor_and_its_workspace() {
        let line = "focusedmonv2>>eDP-1,14";
        assert_eq!(
            parse(line).unwrap(),
            Some(Event::FocusedMonitor { monitor: "eDP-1".to_owned(), workspace: 14 })
        );
    }

    #[test]
    fn reads_a_monitor_appearing_past_its_description() {
        let line = "monitoraddedv2>>2,HEADLESS-1,";
        assert_eq!(
            parse(line).unwrap(),
            Some(Event::MonitorAdded { name: "HEADLESS-1".to_owned() })
        );

        let described = "monitoraddedv2>>0,DP-1,Dell Inc. AW2725DM, rev 3";
        assert_eq!(
            parse(described).unwrap(),
            Some(Event::MonitorAdded { name: "DP-1".to_owned() }),
            "a description containing a comma must not shift the name"
        );
    }

    #[test]
    fn reads_a_monitor_going_away() {
        let line = "monitorremovedv2>>2,HEADLESS-1,";
        assert_eq!(
            parse(line).unwrap(),
            Some(Event::MonitorRemoved { name: "HEADLESS-1".to_owned() })
        );
    }

    #[test]
    fn reads_a_workspace_being_renumbered() {
        assert_eq!(
            parse("changeworkspaceid>>3,7").unwrap(),
            Some(Event::WorkspaceIdChanged { from: 3, to: 7 })
        );
    }

    #[test]
    fn events_we_do_not_model_are_ignored_rather_than_fatal() {
        for line in [
            "activelayout>>hl-virtual-keyboard,German",
            "openlayer>>waybar",
            "somethinginventedlater>>1,2,3",
            "configreloaded>>",
        ] {
            assert!(parse(line).unwrap().is_none(), "{line} should be ignored");
        }
    }

    #[test]
    fn a_special_workspace_does_not_disturb_the_one_underneath() {
        // A scratchpad is announced on its own events and never as a workspace becoming
        // active, so the wallpaper holds the position of the workspace it covers. Pinned
        // here because the difference is invisible until a scratchpad yanks it to centre.
        for line in ["activespecial>>,DP-1", "activespecialv2>>,,DP-1"] {
            assert!(parse(line).unwrap().is_none(), "{line} should be ignored");
        }
    }

    #[test]
    fn reads_the_focus_leaving_every_window() {
        assert_eq!(
            parse("activewindowv2>>").unwrap(),
            Some(Event::ActiveWindow { focused: false }),
            "an empty address is how the compositor says nothing is focused"
        );
        assert_eq!(
            parse("activewindowv2>>55c3da272150").unwrap(),
            Some(Event::ActiveWindow { focused: true })
        );
    }

    #[test]
    fn reads_a_workspace_handed_to_another_monitor() {
        assert_eq!(
            parse("moveworkspacev2>>3,3,eDP-1").unwrap(),
            Some(Event::WorkspaceMoved {
                id: 3,
                name: "3".to_owned(),
                monitor: "eDP-1".to_owned(),
            })
        );

        // The one payload whose free text is neither first nor last, so a comma in the
        // name must not be mistaken for the separator before the monitor.
        assert_eq!(
            parse("moveworkspacev2>>-1337,one, two,DP-1").unwrap(),
            Some(Event::WorkspaceMoved {
                id: -1337,
                name: "one, two".to_owned(),
                monitor: "DP-1".to_owned(),
            })
        );

        assert!(parse("moveworkspacev2>>3,3").is_err(), "a missing monitor must not be ignored");
    }

    #[test]
    fn reads_the_focused_window_snapshot() {
        let none: ActiveWindow = decode("{}").unwrap();
        assert_eq!(none.address, None, "an empty object is how a focused nothing is reported");

        let some: ActiveWindow = decode(r#"{"address":"0x55c3da6fa460","class":"kitty"}"#).unwrap();
        assert!(some.address.is_some());
    }

    #[test]
    fn a_line_with_no_separator_is_ignored() {
        assert!(parse("not an event at all").unwrap().is_none());
    }

    #[test]
    fn an_event_we_model_that_no_longer_parses_is_an_error_not_a_shrug() {
        // What a field being dropped or retyped upstream would look like. Ignoring these
        // is how the wallpaper would stop reacting with nothing in the log to explain it.
        assert!(parse("workspacev2>>3").is_err(), "a missing field must not be ignored");
        assert!(parse("workspacev2>>three,3").is_err(), "a changed type must not be ignored");
        assert!(parse("focusedmonv2>>eDP-1").is_err());
        assert!(parse("monitoraddedv2>>2").is_err());
        assert!(parse("changeworkspaceid>>3").is_err());
    }

    #[test]
    fn a_refusal_is_told_apart_from_a_format_change() {
        let refused = decode::<Vec<Monitor>>("unknown request").unwrap_err();
        assert!(refused.contains("refused it"), "{refused}");

        let moved = decode::<Vec<Monitor>>(r#"[{"name":"DP-1"}]"#).unwrap_err();
        assert!(!moved.contains("refused it"), "{moved}");
    }

    #[test]
    fn reads_the_monitor_snapshot() {
        let answer = r#"[{"id":0,"name":"DP-1","focused":true,
            "activeWorkspace":{"id":1,"name":"1"},"scale":1.0}]"#;
        let monitors: Vec<Monitor> = decode(answer).unwrap();
        assert_eq!(monitors[0].name, "DP-1");
        assert!(monitors[0].focused);
        assert_eq!(monitors[0].active_workspace.id, 1);
    }
}
