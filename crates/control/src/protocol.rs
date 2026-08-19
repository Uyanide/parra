use std::fmt;
use std::path::PathBuf;

use domain::{Driven, Easing, LogicalSize, MonitorState, Move, OutputId, Rgba, Swap};
use serde::{Deserialize, Serialize};

/// Bumped whenever the wire format changes, including when it only gains a field.
/// `Ping` reports it, which is the only way to tell a stale daemon from an unreachable one.
pub const PROTOCOL_VERSION: u32 = 2;

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
    /// Turns this connection into a stream of [`Event`]s. Nothing is answered on it after
    /// the reply to this, so every line the daemon then sends is an event.
    Subscribe,
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

/// One animated property of one output. Named apart from the snapshot fields because a
/// stream has to say which value it is talking about, where a snapshot carries all of them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum Property {
    ScrollVertical,
    ScrollHorizontal,
    Blur,
    Zoom,
}

impl Property {
    pub const ALL: [Property; 4] =
        [Property::ScrollVertical, Property::ScrollHorizontal, Property::Blur, Property::Zoom];
}

impl fmt::Display for Property {
    /// The name it goes by on the wire, so that anything printing one for a human does not
    /// spell it a second time. A test keeps this and the serde name together.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Property::ScrollVertical => "scroll-vertical",
            Property::ScrollHorizontal => "scroll-horizontal",
            Property::Blur => "blur",
            Property::Zoom => "zoom",
        })
    }
}

/// One line pushed to a subscribed connection, encoded the way a request is.
///
/// What is reported is the daemon's own decisions, at the moment it takes them:
/// - Animations are reported once, whole, so a client can run the same curve rather than
///   sample this one.
/// - Driven channels, per-frame values and configured parameters are not here. `get-state`
///   has them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum Event {
    /// A move that has just begun. A later one for the same output and property replaces
    /// this, and `duration_us` of 0 means jump rather than animate.
    Animation {
        output: OutputId,
        property: Property,
        from: f32,
        to: f32,
        duration_us: Micros,
        easing: Easing,
    },
    /// `from` is the image leaving the screen and `to` the one arriving; either can be
    /// null. The transition is the one actually used, which is instant when there was
    /// nothing to crossfade against.
    WallpaperChanged {
        output: OutputId,
        from: Option<PathBuf>,
        to: Option<PathBuf>,
        duration_us: Micros,
        easing: Easing,
    },
    /// An image that will not decode, reported once for the image. The outputs waiting on
    /// it report their fallback separately.
    WallpaperFailed {
        path: PathBuf,
    },
    /// An output the daemon now holds state for, which is later than the compositor
    /// knowing the monitor exists. Also sent for every output that already exists when a
    /// connection subscribes, so a stream stands on its own.
    OutputReady {
        output: OutputId,
        wallpaper: Option<PathBuf>,
        values: Values,
    },
    OutputGone {
        output: OutputId,
    },
    /// The configuration file was re-read and adopted. A reload that changed nothing is
    /// not reported.
    ConfigReloaded,
}

/// Where an output's animated values start.
///
/// Reported because a monitor appearing snaps rather than animating, so no [`Event`] of
/// the animation kind will ever carry them.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Values {
    pub scroll_vertical: f32,
    pub scroll_horizontal: f32,
    pub blur: f32,
    pub zoom: f32,
}

impl Event {
    /// Projects one started move into the wire form.
    pub fn animation(output: &OutputId, property: Property, started: Move) -> Self {
        Self::Animation {
            output: output.clone(),
            property,
            from: started.from,
            to: started.to,
            duration_us: micros(started.tween.duration),
            easing: started.tween.easing,
        }
    }

    pub fn wallpaper_changed(output: &OutputId, swap: &Swap) -> Self {
        Self::WallpaperChanged {
            output: output.clone(),
            from: swap.from.as_ref().map(|from| from.path().to_path_buf()),
            to: swap.to.as_ref().map(|to| to.path().to_path_buf()),
            duration_us: micros(swap.tween.duration),
            easing: swap.tween.easing,
        }
    }

    /// Projects a newly created output into the wire form, values included since they were
    /// snapped rather than animated to.
    pub fn output_ready(state: &MonitorState) -> Self {
        Self::OutputReady {
            output: state.id.clone(),
            wallpaper: state.wallpaper.current().map(|w| w.path().to_path_buf()),
            values: Values {
                scroll_vertical: state.scroll.v.value(),
                scroll_horizontal: state.scroll.h.value(),
                blur: state.blur.amount.value(),
                zoom: state.zoom.factor.value(),
            },
        }
    }

    /// Which output this is about, for a listener that wants one of them.
    pub fn output(&self) -> Option<&OutputId> {
        match self {
            Self::Animation { output, .. }
            | Self::WallpaperChanged { output, .. }
            | Self::OutputReady { output, .. }
            | Self::OutputGone { output } => Some(output),
            Self::WallpaperFailed { .. } | Self::ConfigReloaded => None,
        }
    }
}

/// Seconds, as the animation layer counts them, in the unit the wire uses.
fn micros(seconds: f32) -> Micros {
    (seconds.max(0.0) * 1e6) as Micros
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

/// What the compositor is driving this output to, before any configuration is applied.
///
/// The animated values elsewhere in the snapshot are where those channels have got to;
/// these are what they are heading for.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChannelSnapshot {
    pub scroll_x: f32,
    pub scroll_y: f32,
    pub blur: bool,
    pub zoom_out: bool,
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
    pub channels: ChannelSnapshot,
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
    pub fn new(state: &MonitorState, driven: &Driven, gpu: GpuSnapshot) -> Self {
        let channels = driven.output(&state.id);
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
            channels: ChannelSnapshot {
                scroll_x: channels.scroll_x,
                scroll_y: channels.scroll_y,
                blur: channels.blur,
                zoom_out: channels.zoom_out,
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
    use domain::{OutputParams, WallpaperRef};

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
            Request::Subscribe,
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

    fn output() -> OutputId {
        OutputId::new("DP-1")
    }

    /// The animation kind of tween, which is not the wire's [`Tween`] of the same name.
    fn curve() -> domain::Tween {
        domain::Tween::new(0.4, Easing::OutCubic)
    }

    fn monitor() -> MonitorState {
        MonitorState::new(output(), OutputParams::default(), None)
    }

    #[test]
    fn events_survive_the_wire() {
        let events = [
            Event::animation(
                &output(),
                Property::Blur,
                Move { from: 0.0, to: 1.0, tween: curve() },
            ),
            Event::wallpaper_changed(
                &output(),
                &Swap {
                    from: Some(WallpaperRef::new("/srv/a.png")),
                    to: Some(WallpaperRef::new("/srv/b.png")),
                    tween: curve(),
                },
            ),
            Event::WallpaperFailed { path: PathBuf::from("/srv/broken.png") },
            Event::output_ready(&monitor()),
            Event::OutputGone { output: output() },
            Event::ConfigReloaded,
        ];
        for event in events {
            let line = serde_json::to_string(&event).unwrap();
            assert!(!line.contains('\n'), "an event must fit on one line: {line}");
            assert_eq!(serde_json::from_str::<Event>(&line).unwrap(), event);
        }
    }

    #[test]
    fn an_event_names_itself_the_way_a_request_does() {
        let started = Move { from: 0.0, to: 1.0, tween: curve() };
        assert_eq!(
            serde_json::to_string(&Event::animation(&output(), Property::ScrollVertical, started))
                .unwrap(),
            r#"{"animation":{"output":"DP-1","property":"scroll-vertical","from":0.0,"to":1.0,"duration_us":400000,"easing":"out-cubic"}}"#
        );
        assert_eq!(serde_json::to_string(&Event::ConfigReloaded).unwrap(), "\"config-reloaded\"");
    }

    #[test]
    fn a_swap_with_nothing_to_fade_against_reports_no_duration() {
        let swap = Swap {
            from: None,
            to: Some(WallpaperRef::new("/srv/a.png")),
            tween: domain::Tween::INSTANT,
        };
        let Event::WallpaperChanged { from, duration_us, .. } =
            Event::wallpaper_changed(&output(), &swap)
        else {
            panic!("that is what was built")
        };
        assert_eq!(from, None);
        assert_eq!(duration_us, 0, "a client is told to jump, not to fade from nothing");
    }

    #[test]
    fn an_output_arrives_with_the_values_no_animation_will_report() {
        let state = monitor();
        let Event::OutputReady { values, .. } = Event::output_ready(&state) else {
            panic!("that is what was built")
        };
        assert_eq!(values.zoom, state.zoom.factor.value(), "the zoom it was snapped to");
        assert_eq!(values.blur, state.blur.amount.value());
    }

    #[test]
    fn a_property_prints_the_name_it_goes_by_on_the_wire() {
        for property in Property::ALL {
            assert_eq!(format!("\"{property}\""), serde_json::to_string(&property).unwrap());
        }
    }

    #[test]
    fn only_the_events_about_one_output_name_one() {
        assert_eq!(Event::OutputGone { output: output() }.output(), Some(&output()));
        assert_eq!(Event::ConfigReloaded.output(), None);
        assert_eq!(Event::WallpaperFailed { path: PathBuf::new() }.output(), None);
    }
}
