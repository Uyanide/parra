use std::path::PathBuf;

use domain::{Facts, LogicalSize, MonitorState, OutputId, Rgba};
use serde::{Deserialize, Serialize};

/// Bumped whenever the wire format changes, including when it only gains a field.
/// `Ping` reports it, so a client can tell a stale daemon from an unreachable one, and
/// can tell whether a field it wants exists at all.
pub const PROTOCOL_VERSION: u32 = 4;

/// Every duration on the wire is in microseconds, so nothing has to be read twice to
/// find out which unit it is in.
pub type Micros = u64;

/// One request per line of JSON. Variants are kebab-case so they read as commands;
/// fields stay snake_case so `jq` paths need no quoting.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum Request {
    GetState,
    GetOutput {
        output: OutputId,
    },
    /// `output: null` addresses every output.
    SetWallpaper {
        output: Option<OutputId>,
        /// `null` empties the addressed slot instead of filling it, so an output that was
        /// given its own image goes back to the broadcast one, or to the configured
        /// fallback.
        ///
        /// `deserialize_with` only to make the field required: serde lets an `Option` be
        /// omitted entirely, and a client whose path came out undefined would then clear
        /// a wallpaper instead of being told it sent nonsense.
        #[serde(deserialize_with = "Option::deserialize")]
        path: Option<PathBuf>,
        /// Whether this choice outlives the daemon. False shows the image now and leaves
        /// the remembered one alone, so the next start goes back to it. Defaulted, so a
        /// client that has never heard of it still gets the behaviour everyone expects.
        #[serde(default = "yes")]
        save: bool,
    },
    /// The external blur signal, for whatever is drawing over the wallpaper.
    SetBlur {
        output: Option<OutputId>,
        on: bool,
    },
    ReloadConfig,
    Ping,
}

/// Default for `SetWallpaper::save`: remembering a choice is what a wallpaper setter is
/// for, so not remembering it has to be asked for.
fn yes() -> bool {
    true
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum Response {
    Done,
    Pong { version: u32 },
    State(StateSnapshot),
    Output(OutputSnapshot),
    Error { message: String },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StateSnapshot {
    pub version: u32,
    pub namespace: String,
    /// Frames presented since startup, across every output. An idle daemon leaves this
    /// still, so two readings a minute apart are the whole of the idle check.
    pub frames: u64,
    /// Video memory held by wallpaper textures, sharp and baked alike.
    pub texture_bytes: u64,
    /// From the first instruction of the process to the first frame on screen. `None`
    /// until something has actually been shown.
    pub startup_us: Option<Micros>,
    pub outputs: Vec<OutputSnapshot>,
}

/// What the GPU spent drawing one output.
///
/// Both are `None` when the driver has no usable timer, which is a property of the
/// driver rather than of this output: either every output reports or none does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GpuSnapshot {
    /// The most recent frame. Only frames are timed; a blur bake is not one.
    pub last_us: Option<Micros>,
    /// The most expensive frame since startup, which is the number the budget is about.
    pub peak_us: Option<Micros>,
}

/// A scalar mid-animation. `current` answers "what is on screen" and `target` "where is
/// it going"; both are reported because both have callers.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Tween {
    pub current: f32,
    pub target: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScrollSnapshot {
    pub vertical: Tween,
    pub horizontal: Tween,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct BlurSnapshot {
    pub amount: Tween,
    pub radius: u32,
    pub downscale: u32,
    pub tint: Rgba,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct IndexSnapshot {
    pub index: u32,
    pub count: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OutputSnapshot {
    pub name: OutputId,
    pub logical: LogicalSize,
    pub scale: f64,
    pub wallpaper: Option<PathBuf>,
    pub scroll: ScrollSnapshot,
    pub blur: BlurSnapshot,
    pub zoom: Tween,
    pub focused: bool,
    pub overview: bool,
    pub workspace: IndexSnapshot,
    pub column: IndexSnapshot,
    pub gpu: GpuSnapshot,
    /// False while something is still animating, which is also when frames are being
    /// submitted for this output.
    pub settled: bool,
}

impl OutputSnapshot {
    /// Projects live state into the wire form.
    ///
    /// `gpu` arrives as an argument because measuring it is the renderer's business, and
    /// the two crates cannot see each other. The root package joins them.
    pub fn new(state: &MonitorState, facts: &Facts, gpu: GpuSnapshot) -> Self {
        let output_facts = facts.output(&state.id);
        Self {
            name: state.id.clone(),
            logical: state.logical,
            scale: state.scale.as_f64(),
            wallpaper: state.wallpaper.current().map(|w| w.path().to_path_buf()),
            scroll: ScrollSnapshot {
                vertical: tween(&state.scroll.v),
                horizontal: tween(&state.scroll.h),
            },
            blur: BlurSnapshot {
                amount: tween(&state.blur.amount),
                radius: state.params.blur.radius,
                downscale: state.params.blur.downscale,
                tint: state.params.blur.effective_tint(),
            },
            zoom: tween(&state.zoom.factor),
            focused: facts.is_focused(&state.id),
            overview: facts.overview_active,
            workspace: IndexSnapshot {
                index: output_facts.workspace.idx,
                count: output_facts.workspace.count,
            },
            column: IndexSnapshot {
                index: output_facts.column.idx,
                count: output_facts.column.count,
            },
            gpu,
            settled: state.is_settled(),
        }
    }
}

fn tween(animated: &domain::Animated) -> Tween {
    Tween { current: animated.value(), target: animated.target() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(request: &Request) -> Request {
        let line = serde_json::to_string(request).unwrap();
        assert!(!line.contains('\n'), "a request must fit on one line: {line}");
        serde_json::from_str(&line).unwrap()
    }

    #[test]
    fn requests_survive_the_wire() {
        let requests = [
            Request::GetState,
            Request::GetOutput { output: OutputId::new("DP-1") },
            Request::SetWallpaper {
                output: None,
                path: Some(PathBuf::from("/srv/a.png")),
                save: true,
            },
            Request::SetWallpaper { output: Some(OutputId::new("DP-1")), path: None, save: true },
            Request::SetBlur { output: Some(OutputId::new("eDP-1")), on: true },
            Request::ReloadConfig,
            Request::Ping,
        ];
        for request in requests {
            assert_eq!(round_trip(&request), request);
        }
    }

    #[test]
    fn unit_requests_are_plain_strings() {
        assert_eq!(serde_json::to_string(&Request::GetState).unwrap(), "\"get-state\"");
        assert_eq!(serde_json::to_string(&Request::Ping).unwrap(), "\"ping\"");
    }

    #[test]
    fn fields_stay_snake_case_for_jq() {
        let line = serde_json::to_string(&Request::SetBlur {
            output: Some(OutputId::new("DP-1")),
            on: true,
        })
        .unwrap();
        assert_eq!(line, r#"{"set-blur":{"output":"DP-1","on":true}}"#);
    }

    #[test]
    fn a_client_that_omits_save_still_gets_its_wallpaper_remembered() {
        let line = r#"{"set-wallpaper":{"output":null,"path":"/srv/a.png"}}"#;
        assert_eq!(
            serde_json::from_str::<Request>(line).unwrap(),
            Request::SetWallpaper {
                output: None,
                path: Some(PathBuf::from("/srv/a.png")),
                save: true,
            }
        );
    }

    #[test]
    fn a_null_path_clears_but_a_missing_one_is_refused() {
        let line = r#"{"set-wallpaper":{"output":"DP-1","path":null}}"#;
        assert_eq!(
            serde_json::from_str::<Request>(line).unwrap(),
            Request::SetWallpaper { output: Some(OutputId::new("DP-1")), path: None, save: true }
        );
        assert!(
            serde_json::from_str::<Request>(r#"{"set-wallpaper":{"output":"DP-1"}}"#).is_err(),
            "a dropped field must not read as a wallpaper being cleared"
        );
    }

    #[test]
    fn a_misspelled_request_is_rejected_rather_than_ignored() {
        assert!(serde_json::from_str::<Request>("\"get-stat\"").is_err());
        assert!(serde_json::from_str::<Request>(r#"{"set-blur":{"output":null}}"#).is_err());
        assert!(
            serde_json::from_str::<Request>(r#"{"set-blur":{"on":true,"extra":1}}"#).is_err(),
            "unknown fields should not be silently dropped"
        );
    }

    #[test]
    fn an_unmeasured_gpu_is_null_rather_than_zero() {
        let line = serde_json::to_string(&GpuSnapshot::default()).unwrap();
        assert_eq!(line, r#"{"last_us":null,"peak_us":null}"#, "zero would read as a fast frame");
    }

    #[test]
    fn the_budget_survives_the_wire() {
        let response = Response::State(StateSnapshot {
            version: PROTOCOL_VERSION,
            namespace: "wallpaper".to_owned(),
            frames: 60,
            texture_bytes: 56_164_352,
            startup_us: Some(243_117),
            outputs: Vec::new(),
        });
        let line = serde_json::to_string(&response).unwrap();
        assert_eq!(serde_json::from_str::<Response>(&line).unwrap(), response);
    }

    #[test]
    fn responses_survive_the_wire() {
        let response = Response::Pong { version: PROTOCOL_VERSION };
        let line = serde_json::to_string(&response).unwrap();
        assert_eq!(serde_json::from_str::<Response>(&line).unwrap(), response);
    }
}
