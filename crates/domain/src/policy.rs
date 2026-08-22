use std::collections::BTreeMap;

use crate::output::OutputId;
use crate::params::{AxisParams, OutputParams};
use crate::scroll::{ScrollState, Stop, Stride};
use crate::wallpaper::WallpaperRef;

/// What the compositor drives on one output, before any configuration is applied.
///
/// Positions are normalized to `0..=1` of the available travel, and an axis the
/// compositor does not drive sits centred.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Channels {
    pub x: Stop,
    pub y: Stop,
    pub blur: bool,
    pub zoom_out: bool,
}

impl Channels {
    /// The strides both axes were last reported at, which is all the geometry needs of
    /// them.
    pub fn stride(&self) -> Stride {
        Stride { v: self.y.stride, h: self.x.stride }
    }
}

/// Every output a compositor backend is driving.
#[derive(Clone, Debug, Default)]
pub struct Driven {
    pub outputs: BTreeMap<OutputId, Channels>,
}

impl Driven {
    /// An output nothing has been reported for reads as the neutral position.
    pub fn output(&self, id: &OutputId) -> Channels {
        self.outputs.get(id).copied().unwrap_or_default()
    }
}

/// Overrides asked for from outside, which the daemon has no way to observe for itself.
#[derive(Clone, Debug, Default)]
pub struct Signals {
    global_blur: bool,
    per_output_blur: BTreeMap<OutputId, bool>,
    global_wallpaper: Option<WallpaperRef>,
    per_output_wallpaper: BTreeMap<OutputId, WallpaperRef>,
}

impl Signals {
    /// `None` addresses every output and drops per-output requests, so a broadcast is
    /// always authoritative; `Some` records one output without touching the others.
    pub fn set_blur(&mut self, output: Option<OutputId>, on: bool) {
        match output {
            None => {
                self.global_blur = on;
                self.per_output_blur.clear();
            }
            Some(id) => {
                self.per_output_blur.insert(id, on);
            }
        }
    }

    pub fn blur(&self, id: &OutputId) -> bool {
        self.global_blur || self.per_output_blur.get(id).copied().unwrap_or(false)
    }

    /// Same addressing as the blur signal. Kept for outputs that do not exist yet, so one
    /// that comes back returns to what was asked for rather than to the fallback.
    ///
    /// `None` for the wallpaper empties the slot instead of filling it, which is not "show
    /// nothing": an emptied entry falls through to the next one [`wallpaper_for`] tries.
    pub fn set_wallpaper(&mut self, output: Option<OutputId>, wallpaper: Option<WallpaperRef>) {
        match output {
            None => {
                self.global_wallpaper = wallpaper;
                self.per_output_wallpaper.clear();
            }
            Some(id) => match wallpaper {
                Some(wallpaper) => {
                    self.per_output_wallpaper.insert(id, wallpaper);
                }
                None => {
                    self.per_output_wallpaper.remove(&id);
                }
            },
        }
    }

    pub fn wallpaper(&self, id: &OutputId) -> Option<&WallpaperRef> {
        self.per_output_wallpaper.get(id).or(self.global_wallpaper.as_ref())
    }

    /// Forgets one wallpaper wherever it was asked for, so that resolving an output again
    /// cannot put it straight back.
    pub fn forget_wallpaper(&mut self, wallpaper: &WallpaperRef) {
        if self.global_wallpaper.as_ref() == Some(wallpaper) {
            self.global_wallpaper = None;
        }
        self.per_output_wallpaper.retain(|_, asked| asked != wallpaper);
    }
}

/// Where every animated property should be heading. Everything is normalized to `0..=1`
/// except the zoom factor, which is a multiplier.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Targets {
    pub scroll_v: f32,
    pub scroll_h: f32,
    pub blur: f32,
    pub zoom: f32,
}

/// The one place driven channels, external signals and configuration become intent.
///
/// It lives here because some targets, blur among them, combine sources from several
/// crates at once, so none of those crates could resolve them alone.
pub fn resolve(
    output: &OutputId,
    driven: &Driven,
    signals: &Signals,
    params: &OutputParams,
) -> Targets {
    let channels = driven.output(output);
    let blur_on = params.blur.is_enabled() && (channels.blur || signals.blur(output));

    Targets {
        scroll_v: axis(channels.y.at, &params.scroll.vertical),
        scroll_h: axis(channels.x.at, &params.scroll.horizontal),
        blur: if blur_on { 1.0 } else { 0.0 },
        zoom: if channels.zoom_out { 1.0 } else { params.zoom.factor() },
    }
}

/// The wallpaper an output should show: whatever was set for it, otherwise the configured
/// fallback, otherwise nothing.
///
/// Stated once and here, beside the other place a signal is weighed against the
/// configuration, because a monitor appearing, a reload and a decode failure all have to
/// answer the same question the same way.
pub fn wallpaper_for<'a>(
    signals: &'a Signals,
    output: &OutputId,
    params: &'a OutputParams,
) -> Option<&'a WallpaperRef> {
    signals.wallpaper(output).or(params.fallback.as_ref())
}

/// Scales the excursion about the centre rather than the raw position, so a travel below
/// 1 shortens the movement symmetrically instead of biasing it toward one edge.
///
/// Inverting negates that same excursion, which leaves the centre a fixed point: an
/// undriven output does not move when the key is toggled.
fn axis(position: f32, params: &AxisParams) -> f32 {
    let offset = position - ScrollState::CENTRE;
    let travel = if params.invert { -params.travel } else { params.travel };
    (ScrollState::CENTRE + offset * travel).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::{BlurParams, ScrollParams};

    fn output(name: &str) -> OutputId {
        OutputId::new(name)
    }

    /// One output driven to the given channels, beside a second left at its neutral
    /// position for every case here to check against.
    fn driven(id: &OutputId, channels: Channels) -> Driven {
        let mut driven = Driven::default();
        driven.outputs.insert(id.clone(), channels);
        driven.outputs.insert(output("untouched"), Channels::default());
        driven
    }

    fn travel(travel: f32) -> AxisParams {
        AxisParams { travel, ..AxisParams::default() }
    }

    fn inverted(travel: f32) -> AxisParams {
        AxisParams { invert: true, ..self::travel(travel) }
    }

    #[test]
    fn an_undriven_output_sits_at_the_centre() {
        let channels = Channels::default();
        assert_eq!(channels.x, Stop::CENTRED);
        assert_eq!(channels.y, Stop::CENTRED);
        assert_eq!(Driven::default().output(&output("DP-1")), channels);
    }

    #[test]
    fn travel_shortens_the_movement_about_the_centre() {
        assert_eq!(axis(0.0, &travel(1.0)), 0.0);
        assert_eq!(axis(0.0, &travel(0.5)), 0.25);
        assert_eq!(axis(1.0, &travel(0.5)), 0.75);
        assert_eq!(axis(0.0, &travel(0.0)), 0.5, "no travel pins the axis to the centre");
    }

    #[test]
    fn inverting_swaps_the_two_ends_of_the_axis() {
        assert_eq!(axis(0.0, &inverted(1.0)), 1.0);
        assert_eq!(axis(1.0, &inverted(1.0)), 0.0);
    }

    /// So that toggling the key never makes an output nothing has reported for jump.
    #[test]
    fn the_centre_is_where_inverting_changes_nothing() {
        assert_eq!(axis(0.5, &inverted(1.0)), axis(0.5, &travel(1.0)));
    }

    #[test]
    fn inverting_shortens_about_the_centre_the_way_travel_does() {
        assert_eq!(axis(0.0, &inverted(0.5)), 0.75);
        assert_eq!(axis(1.0, &inverted(0.5)), 0.25);
    }

    #[test]
    fn an_inverted_axis_with_no_travel_is_still_pinned() {
        assert_eq!(axis(0.0, &inverted(0.0)), 0.5);
        assert_eq!(axis(1.0, &inverted(0.0)), 0.5);
    }

    #[test]
    fn each_axis_is_inverted_on_its_own() {
        let id = output("DP-1");
        let edges = Channels {
            x: Stop { at: 1.0, stride: 0.5 },
            y: Stop { at: 1.0, stride: 0.5 },
            ..Channels::default()
        };
        let driven = driven(&id, edges);
        let params = OutputParams {
            scroll: ScrollParams { vertical: inverted(1.0), horizontal: travel(1.0) },
            ..OutputParams::default()
        };
        let targets = resolve(&id, &driven, &Signals::default(), &params);

        assert_eq!(targets.scroll_v, 0.0, "the inverted axis runs the other way");
        assert_eq!(targets.scroll_h, 1.0, "the other one is untouched");
    }

    #[test]
    fn each_axis_follows_its_own_channel() {
        let id = output("DP-1");
        let edges = Channels {
            x: Stop { at: 1.0, stride: 0.5 },
            y: Stop { at: 0.0, stride: 0.5 },
            ..Channels::default()
        };
        let driven = driven(&id, edges);
        let targets = resolve(&id, &driven, &Signals::default(), &OutputParams::default());

        assert_eq!(targets.scroll_v, 0.0, "the vertical axis follows the y channel");
        assert_eq!(targets.scroll_h, 1.0, "the horizontal axis follows the x channel");
    }

    #[test]
    fn only_the_output_driven_to_blur_blurs() {
        let id = output("DP-1");
        let driven = driven(&id, Channels { blur: true, ..Channels::default() });
        let signals = Signals::default();
        let params = OutputParams::default();

        assert_eq!(resolve(&id, &driven, &signals, &params).blur, 1.0);
        assert_eq!(resolve(&output("untouched"), &driven, &signals, &params).blur, 0.0);
    }

    #[test]
    fn only_the_output_driven_to_zoom_out_zooms_out() {
        let id = output("DP-1");
        let driven = driven(&id, Channels { zoom_out: true, ..Channels::default() });
        let signals = Signals::default();
        let params = OutputParams::default();

        assert_eq!(resolve(&id, &driven, &signals, &params).zoom, 1.0);
        assert!(resolve(&output("untouched"), &driven, &signals, &params).zoom > 1.0);
    }

    #[test]
    fn nothing_driven_leaves_every_output_sharp() {
        let driven = Driven::default();
        let params = OutputParams::default();
        assert_eq!(resolve(&output("DP-1"), &driven, &Signals::default(), &params).blur, 0.0);
    }

    #[test]
    fn an_external_signal_blurs_an_undriven_output() {
        let driven = Driven::default();
        let mut signals = Signals::default();
        signals.set_blur(Some(output("eDP-1")), true);
        let params = OutputParams::default();

        assert_eq!(resolve(&output("eDP-1"), &driven, &signals, &params).blur, 1.0);
        assert_eq!(resolve(&output("DP-1"), &driven, &signals, &params).blur, 0.0);
    }

    #[test]
    fn a_zero_radius_disables_blur_entirely() {
        let id = output("DP-1");
        let driven = driven(&id, Channels { blur: true, ..Channels::default() });
        let mut signals = Signals::default();
        signals.set_blur(None, true);
        let params = OutputParams {
            blur: BlurParams { radius: 0, ..BlurParams::default() },
            ..Default::default()
        };

        assert_eq!(resolve(&id, &driven, &signals, &params).blur, 0.0);
    }

    #[test]
    fn a_broadcast_overrides_per_output_signals() {
        let mut signals = Signals::default();
        signals.set_blur(Some(output("DP-1")), true);
        signals.set_blur(None, false);
        assert!(!signals.blur(&output("DP-1")));

        signals.set_blur(None, true);
        assert!(signals.blur(&output("eDP-1")));
    }

    #[test]
    fn a_wallpaper_set_for_one_output_leaves_the_others_alone() {
        let mut signals = Signals::default();
        signals.set_wallpaper(Some(output("DP-1")), Some(WallpaperRef::new("/tmp/one.png")));

        assert_eq!(
            signals.wallpaper(&output("DP-1")).map(WallpaperRef::path),
            Some("/tmp/one.png".as_ref())
        );
        assert_eq!(signals.wallpaper(&output("eDP-1")), None);
    }

    #[test]
    fn a_broadcast_wallpaper_replaces_the_ones_set_singly() {
        let mut signals = Signals::default();
        signals.set_wallpaper(Some(output("DP-1")), Some(WallpaperRef::new("/tmp/one.png")));
        signals.set_wallpaper(None, Some(WallpaperRef::new("/tmp/all.png")));

        assert_eq!(
            signals.wallpaper(&output("DP-1")).map(WallpaperRef::path),
            Some("/tmp/all.png".as_ref())
        );
        assert_eq!(
            signals.wallpaper(&output("eDP-1")).map(WallpaperRef::path),
            Some("/tmp/all.png".as_ref())
        );
    }

    #[test]
    fn a_set_wallpaper_wins_over_the_configured_fallback() {
        let params = OutputParams {
            fallback: Some(WallpaperRef::new("/tmp/fallback.png")),
            ..OutputParams::default()
        };
        let mut signals = Signals::default();
        assert_eq!(
            wallpaper_for(&signals, &output("DP-1"), &params).map(WallpaperRef::path),
            Some("/tmp/fallback.png".as_ref())
        );

        signals.set_wallpaper(None, Some(WallpaperRef::at("/tmp/set.png", 1)));
        assert_eq!(
            wallpaper_for(&signals, &output("DP-1"), &params).map(WallpaperRef::path),
            Some("/tmp/set.png".as_ref())
        );
    }

    #[test]
    fn without_a_signal_or_a_fallback_an_output_shows_nothing() {
        let signals = Signals::default();
        let params = OutputParams::default();
        assert_eq!(wallpaper_for(&signals, &output("DP-1"), &params), None);
    }

    #[test]
    fn clearing_one_output_leaves_it_on_the_broadcast_one() {
        let all = WallpaperRef::at("/tmp/all.png", 1);
        let mut signals = Signals::default();
        signals.set_wallpaper(None, Some(all.clone()));
        signals.set_wallpaper(Some(output("DP-1")), Some(WallpaperRef::at("/tmp/one.png", 2)));

        signals.set_wallpaper(Some(output("DP-1")), None);

        assert_eq!(signals.wallpaper(&output("DP-1")), Some(&all), "not nothing: the global one");
        assert_eq!(signals.wallpaper(&output("eDP-1")), Some(&all));
    }

    #[test]
    fn clearing_one_output_that_has_no_entry_changes_nothing() {
        let all = WallpaperRef::at("/tmp/all.png", 1);
        let mut signals = Signals::default();
        signals.set_wallpaper(None, Some(all.clone()));

        signals.set_wallpaper(Some(output("DP-1")), None);

        assert_eq!(signals.wallpaper(&output("DP-1")), Some(&all));
    }

    #[test]
    fn clearing_the_broadcast_slot_clears_the_per_output_ones_too() {
        let mut signals = Signals::default();
        signals.set_wallpaper(None, Some(WallpaperRef::at("/tmp/all.png", 1)));
        signals.set_wallpaper(Some(output("DP-1")), Some(WallpaperRef::at("/tmp/one.png", 2)));

        signals.set_wallpaper(None, None);

        assert_eq!(signals.wallpaper(&output("DP-1")), None);
        assert_eq!(signals.wallpaper(&output("eDP-1")), None);
    }

    #[test]
    fn a_cleared_output_falls_all_the_way_to_the_configured_fallback() {
        let params = OutputParams {
            fallback: Some(WallpaperRef::new("/tmp/fallback.png")),
            ..OutputParams::default()
        };
        let mut signals = Signals::default();
        signals.set_wallpaper(Some(output("DP-1")), Some(WallpaperRef::at("/tmp/one.png", 1)));
        signals.set_wallpaper(Some(output("DP-1")), None);

        assert_eq!(
            wallpaper_for(&signals, &output("DP-1"), &params).map(WallpaperRef::path),
            Some("/tmp/fallback.png".as_ref())
        );
    }

    #[test]
    fn forgetting_a_wallpaper_clears_it_everywhere_it_was_asked_for() {
        let doomed = WallpaperRef::at("/tmp/gone.png", 1);
        let kept = WallpaperRef::at("/tmp/here.png", 2);
        let mut signals = Signals::default();
        signals.set_wallpaper(None, Some(doomed.clone()));
        signals.set_wallpaper(Some(output("DP-1")), Some(doomed.clone()));
        signals.set_wallpaper(Some(output("eDP-1")), Some(kept.clone()));

        signals.forget_wallpaper(&doomed);

        assert_eq!(signals.wallpaper(&output("DP-1")), None);
        assert_eq!(signals.wallpaper(&output("eDP-1")), Some(&kept));
    }
}
