use std::fmt;

use serde::Deserialize;

/// The events this daemon acts on, and only the fields it reads.
///
/// Modelled here rather than taken from a wrapper crate, for the same reasons the niri
/// events are: an unrecognized event costs nothing here, so the compositor and this daemon
/// upgrade independently.
///
/// A workspace is named by its address everywhere: the number of an ordinary one, the name
/// of one opened by name, and `special:` and a name for a special one.
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
        workspace: String,
    },
    /// A workspace became the active one. Carries no monitor, because it is always the
    /// focused one.
    WorkspaceActive {
        workspace: String,
    },
    WorkspaceCreated {
        workspace: String,
    },
    WorkspaceDestroyed {
        workspace: String,
    },
    /// A workspace was handed to another monitor, which nothing else reports. Moving one
    /// is the only way a monitor stops showing a workspace without activating another.
    WorkspaceMoved {
        workspace: String,
        monitor: String,
    },
    /// An ordinary workspace was renumbered, which changes its address. The event says
    /// nothing of which monitor holds it.
    WorkspaceIdChanged {
        from: u32,
        to: u32,
    },
    /// Whether any window holds the focus at all.
    ///
    /// Carries no monitor. Where that window is has to be read from what the focused monitor
    /// shows, which holds because this fires only when a window really takes or loses it.
    ActiveWindow {
        focused: bool,
    },
    /// A window appeared, on the workspace it names. The address is the only handle the
    /// stream gives one, and is what the two events below are joined on.
    WindowOpened {
        address: String,
        workspace: String,
    },
    WindowClosed {
        address: String,
    },
    WindowMoved {
        address: String,
        workspace: String,
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
/// Fields are comma separated, and a workspace address or a monitor description may hold
/// commas of its own. So every payload is cut from whichever end pins the fixed fields down.
///
/// The workspace events are read in their original spelling. Each carries the address
/// alone, where the `v2` spelling follows it with a display name that is free text too.
pub fn parse(line: &str) -> Result<Option<Event>, Malformed> {
    let Some((name, data)) = line.trim_end().split_once(">>") else { return Ok(None) };
    let bad = |reason| Malformed { event: name.to_owned(), reason };
    let whole = |data: &str| match data {
        "" => Err(bad(NAME)),
        workspace => Ok(workspace.to_owned()),
    };

    let event = match name {
        // Trailing description, which is free text.
        "monitoraddedv2" => Event::MonitorAdded { name: field(data, 1, 3).ok_or(bad(NAME))? },
        "monitorremovedv2" => Event::MonitorRemoved { name: field(data, 1, 3).ok_or(bad(NAME))? },
        // A monitor name holds no comma, so the workspace is whatever follows it.
        "focusedmonv2" => {
            let (monitor, workspace) = data.split_once(',').ok_or(bad(FIELDS))?;
            Event::FocusedMonitor { monitor: monitor.to_owned(), workspace: workspace.to_owned() }
        }
        "workspace" => Event::WorkspaceActive { workspace: whole(data)? },
        "createworkspace" => Event::WorkspaceCreated { workspace: whole(data)? },
        "destroyworkspace" => Event::WorkspaceDestroyed { workspace: whole(data)? },
        // The monitor trails, so the workspace is whatever precedes it.
        "moveworkspace" => {
            let (workspace, monitor) = data.rsplit_once(',').ok_or(bad(FIELDS))?;
            Event::WorkspaceMoved { workspace: whole(workspace)?, monitor: monitor.to_owned() }
        }
        // Two numbers and no free text at all, so this one really is a plain split.
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
        // Four fields, of which the title trails and is free text. Only the leading ones
        // can be pinned, so a workspace address carrying a comma reads short.
        "openwindow" => Event::WindowOpened {
            address: address(&field(data, 0, 4).ok_or(bad(FIELDS))?),
            workspace: field(data, 1, 4).ok_or(bad(FIELDS))?,
        },
        // One address and nothing else, so an empty payload is the format having moved
        // rather than a window without one.
        "closewindow" if data.is_empty() => return Err(bad(NAME)),
        "closewindow" => Event::WindowClosed { address: address(data) },
        // The window address leads and holds no comma, so the workspace is what follows.
        "movewindow" => {
            let (window, workspace) = data.split_once(',').ok_or(bad(FIELDS))?;
            Event::WindowMoved { address: address(window), workspace: whole(workspace)? }
        }
        _ => return Ok(None),
    };
    Ok(Some(event))
}

const NAME: &str = "a name is missing";
const FIELDS: &str = "too few fields";
const ID: &str = "a workspace number is not a number";

/// Field `at` of exactly `count`, counted from the left, with the last one allowed to
/// contain the separator itself.
fn field(data: &str, at: usize, count: usize) -> Option<String> {
    data.splitn(count, ',').nth(at).map(str::to_owned)
}

/// One window's address, without the prefix `j/clients` writes and the event stream does
/// not, so the two spellings of one handle compare equal.
pub fn address(raw: &str) -> String {
    raw.strip_prefix("0x").unwrap_or(raw).to_owned()
}

/// What `j/monitors` says, and only the fields this daemon reads.
#[derive(Debug, Deserialize)]
pub struct Monitor {
    pub name: String,
    pub focused: bool,
    #[serde(rename = "activeWorkspace")]
    pub active_workspace: WorkspaceRef,
}

/// One workspace as another answer names it, by the address the event stream uses too.
#[derive(Debug, Deserialize)]
pub struct WorkspaceRef {
    pub address: String,
}

/// What `j/workspaces` says. The monitor owns the workspace topology; active workspace
/// records from `j/monitors` repair it when this field is absent.
#[derive(Debug, Deserialize)]
pub struct Workspace {
    pub address: String,
    #[serde(default)]
    pub monitor: Option<String>,
}

/// What `j/clients` says, and only the fields this daemon reads.
///
/// Asked for only where some output blurs on whether a workspace is empty: the event stream
/// reports a window opening and closing but never how many one workspace holds.
#[derive(Debug, Deserialize)]
pub struct Client {
    pub address: String,
    pub workspace: WorkspaceRef,
}

/// What `j/activewindow` says, which is `{}` when the focus is on no window at all.
///
/// The address is read for its presence alone, which tells a focused window from none and
/// is the same thing `activewindowv2` reports once the stream is running.
///
/// The workspace says where that window is, which no event ever does. Read only at cold
/// start, where there is no earlier event to have said it.
#[derive(Debug, Default, Deserialize)]
pub struct ActiveWindow {
    pub address: Option<String>,
    pub workspace: Option<WorkspaceRef>,
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

    fn active(workspace: &str) -> Option<Event> {
        Some(Event::WorkspaceActive { workspace: workspace.to_owned() })
    }

    #[test]
    fn reads_a_workspace_becoming_active() {
        assert_eq!(parse("workspace>>3").unwrap(), active("3"));
    }

    #[test]
    fn a_workspace_is_named_by_its_address_whatever_its_kind() {
        assert_eq!(parse("workspace>>browser").unwrap(), active("browser"));
        assert_eq!(
            parse("createworkspace>>special:special").unwrap(),
            Some(Event::WorkspaceCreated { workspace: "special:special".to_owned() })
        );
        assert_eq!(
            parse("destroyworkspace>>browser").unwrap(),
            Some(Event::WorkspaceDestroyed { workspace: "browser".to_owned() })
        );
    }

    #[test]
    fn a_workspace_address_may_contain_a_comma() {
        assert_eq!(parse("workspace>>one, two").unwrap(), active("one, two"));
    }

    /// The display name these follow the address with is the one field nothing reads.
    #[test]
    fn the_v2_workspace_events_are_ignored() {
        for line in [
            "workspacev2>>3,3",
            "createworkspacev2>>browser,browser",
            "destroyworkspacev2>>3,3",
            "moveworkspacev2>>3,3,DP-1",
            "movewindowv2>>55c3da272150,3,3",
            "renameworkspace>>3,code",
        ] {
            assert!(parse(line).unwrap().is_none(), "{line} should be ignored");
        }
    }

    #[test]
    fn reads_the_focused_monitor_and_its_workspace() {
        assert_eq!(
            parse("focusedmonv2>>eDP-1,14").unwrap(),
            Some(Event::FocusedMonitor { monitor: "eDP-1".to_owned(), workspace: "14".to_owned() })
        );
        assert_eq!(
            parse("focusedmonv2>>DP-1,one, two").unwrap(),
            Some(Event::FocusedMonitor {
                monitor: "DP-1".to_owned(),
                workspace: "one, two".to_owned()
            })
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
        // active, so the wallpaper holds the position of the workspace it covers.
        for line in [
            "activespecial>>,DP-1",
            "activespecialv2>>,,DP-1",
            "activespecial>>special:special,DP-1",
            "activespecialv2>>special:special,special:special,DP-1",
        ] {
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
            parse("moveworkspace>>3,eDP-1").unwrap(),
            Some(Event::WorkspaceMoved { workspace: "3".to_owned(), monitor: "eDP-1".to_owned() })
        );
        assert_eq!(
            parse("moveworkspace>>one, two,DP-1").unwrap(),
            Some(Event::WorkspaceMoved {
                workspace: "one, two".to_owned(),
                monitor: "DP-1".to_owned(),
            })
        );

        assert!(parse("moveworkspace>>3").is_err(), "a missing monitor must not be ignored");
    }

    #[test]
    fn reads_the_focused_window_snapshot() {
        let none: ActiveWindow = decode("{}").unwrap();
        assert_eq!(none.address, None, "an empty object is how a focused nothing is reported");

        let some: ActiveWindow = decode(r#"{"address":"0x55c3da6fa460","class":"kitty"}"#).unwrap();
        assert!(some.address.is_some());

        let placed: ActiveWindow = decode(
            r#"{"address":"0x1","workspace":{"address":"5","id":5,"type":"normal","name":"5"}}"#,
        )
        .unwrap();
        assert_eq!(placed.workspace.map(|workspace| workspace.address), Some("5".to_owned()));
    }

    #[test]
    fn reads_a_window_opening_on_a_workspace() {
        assert_eq!(
            parse("openwindow>>55c3da272150,3,kitty,zsh").unwrap(),
            Some(Event::WindowOpened {
                address: "55c3da272150".to_owned(),
                workspace: "3".to_owned(),
            })
        );

        // The title trails and is free text, so a comma in one must not shift the
        // workspace out from under it.
        assert_eq!(
            parse("openwindow>>55c3da272150,browser,firefox,one, two").unwrap(),
            Some(Event::WindowOpened {
                address: "55c3da272150".to_owned(),
                workspace: "browser".to_owned(),
            })
        );
    }

    #[test]
    fn reads_a_window_closing_and_being_handed_on() {
        assert_eq!(
            parse("closewindow>>55c3da272150").unwrap(),
            Some(Event::WindowClosed { address: "55c3da272150".to_owned() })
        );
        assert_eq!(
            parse("movewindow>>55c3da272150,3").unwrap(),
            Some(Event::WindowMoved {
                address: "55c3da272150".to_owned(),
                workspace: "3".to_owned()
            })
        );
        assert_eq!(
            parse("movewindow>>55c3da272150,special:one, two").unwrap(),
            Some(Event::WindowMoved {
                address: "55c3da272150".to_owned(),
                workspace: "special:one, two".to_owned()
            })
        );
    }

    #[test]
    fn a_window_address_reads_the_same_from_either_source() {
        assert_eq!(address("0x55c3da6fa460"), "55c3da6fa460", "as `j/clients` writes it");
        assert_eq!(address("55c3da6fa460"), "55c3da6fa460", "as the event stream writes it");
    }

    /// Only an ordinary workspace carries an `id`, which is why none is read.
    #[test]
    fn reads_the_open_windows_on_every_kind_of_workspace() {
        let answer = r#"[
            {"address":"0x55c3da6fa460","class":"kitty",
             "workspace":{"address":"1","id":1,"type":"normal","name":"1"}},
            {"address":"0x55c3da6fa470","class":"kitty",
             "workspace":{"address":"special:special","type":"special","name":"special:special"}},
            {"address":"0x55c3da6fa480","class":"kitty",
             "workspace":{"address":"browser","type":"normal","name":"web"}}]"#;
        let clients: Vec<Client> = decode(answer).unwrap();
        assert_eq!(address(&clients[0].address), "55c3da6fa460");
        let on: Vec<&str> =
            clients.iter().map(|client| client.workspace.address.as_str()).collect();
        assert_eq!(on, ["1", "special:special", "browser"]);
    }

    #[test]
    fn reads_the_workspaces_of_every_kind() {
        let answer = r#"[
            {"address":"3","id":3,"type":"normal","name":"3","monitor":"DP-1","windows":3},
            {"address":"browser","type":"normal","name":"browser","monitor":"DP-1","windows":1},
            {"address":"special:special","type":"special","name":"special:special",
             "monitor":"DP-1","windows":1}]"#;
        let workspaces: Vec<Workspace> = decode(answer).unwrap();
        let addresses: Vec<&str> = workspaces.iter().map(|ws| ws.address.as_str()).collect();
        assert_eq!(addresses, ["3", "browser", "special:special"]);
    }

    #[test]
    fn a_line_with_no_separator_is_ignored() {
        assert!(parse("not an event at all").unwrap().is_none());
    }

    #[test]
    fn an_event_we_model_that_no_longer_parses_is_an_error_not_a_shrug() {
        // What a field being dropped or retyped upstream would look like. Ignoring these
        // is how the wallpaper would stop reacting with nothing in the log to explain it.
        assert!(parse("workspace>>").is_err(), "a missing field must not be ignored");
        assert!(parse("focusedmonv2>>eDP-1").is_err());
        assert!(parse("monitoraddedv2>>2").is_err());
        assert!(parse("changeworkspaceid>>3").is_err());
        assert!(parse("changeworkspaceid>>three,3").is_err(), "a changed type must not be ignored");
        assert!(parse("openwindow>>55c3da272150").is_err());
        assert!(parse("closewindow>>").is_err());
        assert!(parse("movewindow>>55c3da272150").is_err());
        assert!(parse("movewindow>>55c3da272150,").is_err());
    }

    #[test]
    fn a_refusal_is_told_apart_from_a_format_change() {
        let refused = decode::<Vec<Monitor>>("unknown request").unwrap_err();
        assert!(refused.contains("refused it"), "{refused}");

        let moved = decode::<Vec<Monitor>>(r#"[{"name":"DP-1"}]"#).unwrap_err();
        assert!(!moved.contains("refused it"), "{moved}");
    }

    /// As 0.57 writes it, with a named workspace showing and no special one open.
    #[test]
    fn reads_the_monitor_snapshot() {
        let answer = r#"[{"id":0,"name":"DP-1","focused":true,
            "activeWorkspace":{"address":"browser","type":"normal","name":"browser"},
            "specialWorkspace":{"address":"","type":"","name":""},"scale":1.0}]"#;
        let monitors: Vec<Monitor> = decode(answer).unwrap();
        assert_eq!(monitors[0].name, "DP-1");
        assert!(monitors[0].focused);
        assert_eq!(monitors[0].active_workspace.address, "browser");
    }
}
