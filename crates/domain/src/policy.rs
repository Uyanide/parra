use std::collections::BTreeMap;

use crate::output::OutputId;
use crate::params::{AxisParams, OutputParams};
use crate::scroll::ScrollState;
use crate::wallpaper::WallpaperRef;

/// A position within a one-based sequence the compositor exposes, such as the active
/// workspace among that output's workspaces. `count == 0` means "not reported yet".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Index {
    pub idx: u32,
    pub count: u32,
}

impl Index {
    pub const UNKNOWN: Index = Index { idx: 0, count: 0 };

    pub fn new(idx: u32, count: u32) -> Self {
        Self { idx, count }
    }

    /// Maps the position onto `0..=1`. A lone or not-yet-known position sits centred,
    /// which is the only neutral answer when there is nothing to travel between.
    pub fn progress(self) -> f32 {
        if self.count <= 1 || self.idx == 0 {
            return ScrollState::CENTRE;
        }
        let position = self.idx.min(self.count) - 1;
        position as f32 / (self.count - 1) as f32
    }
}

/// What the compositor reports about one output.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OutputFacts {
    pub workspace: Index,
    /// Position in the scrolling layout.
    pub column: Index,
}

// TODO: This is exclusively niri-shaped and NOT normalized across backends.
/// The compositor's view of the world.
#[derive(Clone, Debug, Default)]
pub struct Facts {
    /// Output holding the focused window, if any. Focus is global, so at most one.
    pub focused_output: Option<OutputId>,
    pub overview_active: bool,
    pub outputs: BTreeMap<OutputId, OutputFacts>,
}

impl Facts {
    pub fn output(&self, id: &OutputId) -> OutputFacts {
        self.outputs.get(id).copied().unwrap_or_default()
    }

    pub fn is_focused(&self, id: &OutputId) -> bool {
        self.focused_output.as_ref() == Some(id)
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

// TODO: Refactor into a more flexible system that does not hardcode the bindings between
//       facts, signals and params.
/// The one place where facts, external signals and configuration become intent.
///
/// It lives here because some targets, blur among them, combine signals from several
/// crates at once, so none of those crates could resolve them alone.
pub fn resolve(
    output: &OutputId,
    facts: &Facts,
    signals: &Signals,
    params: &OutputParams,
) -> Targets {
    let output_facts = facts.output(output);
    let blur_on = params.blur.is_enabled() && (facts.is_focused(output) || signals.blur(output));

    Targets {
        scroll_v: axis(output_facts.workspace, &params.scroll.vertical),
        scroll_h: axis(output_facts.column, &params.scroll.horizontal),
        blur: if blur_on { 1.0 } else { 0.0 },
        zoom: if facts.overview_active { 1.0 } else { params.overview.zoom() },
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

/// Scales the excursion about the centre rather than the raw progress, so a strength
/// below 1 shortens the travel symmetrically instead of biasing it toward one edge.
fn axis(index: Index, params: &AxisParams) -> f32 {
    if !params.enabled {
        return ScrollState::CENTRE;
    }
    let offset = index.progress() - ScrollState::CENTRE;
    (ScrollState::CENTRE + offset * params.travel).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::BlurParams;

    const EPS: f32 = 1e-6;

    fn output(name: &str) -> OutputId {
        OutputId::new(name)
    }

    #[test]
    fn progress_spans_the_sequence() {
        assert_eq!(Index::new(1, 4).progress(), 0.0);
        assert!((Index::new(2, 4).progress() - 1.0 / 3.0).abs() < EPS);
        assert_eq!(Index::new(4, 4).progress(), 1.0);
    }

    #[test]
    fn a_lone_or_unknown_position_is_centred() {
        assert_eq!(Index::new(1, 1).progress(), 0.5);
        assert_eq!(Index::UNKNOWN.progress(), 0.5);
        assert_eq!(Index::new(0, 5).progress(), 0.5);
    }

    #[test]
    fn an_out_of_range_index_stays_in_bounds() {
        assert_eq!(Index::new(9, 4).progress(), 1.0);
    }

    fn travel(travel: f32) -> AxisParams {
        AxisParams { travel, ..AxisParams::default() }
    }

    #[test]
    fn strength_shortens_the_travel_about_the_centre() {
        assert_eq!(axis(Index::new(1, 3), &travel(1.0)), 0.0);
        assert_eq!(axis(Index::new(1, 3), &travel(0.5)), 0.25);
        assert_eq!(axis(Index::new(3, 3), &travel(0.5)), 0.75);
        assert_eq!(axis(Index::new(1, 3), &travel(0.0)), 0.5);
    }

    #[test]
    fn disabling_an_axis_pins_it_to_the_centre() {
        let params = AxisParams { enabled: false, ..AxisParams::default() };
        assert_eq!(axis(Index::new(1, 3), &params), 0.5);
    }

    #[test]
    fn the_horizontal_axis_is_off_until_it_is_asked_for() {
        let mut facts = Facts::default();
        facts.outputs.insert(
            output("DP-1"),
            OutputFacts { column: Index::new(1, 3), ..OutputFacts::default() },
        );
        let signals = Signals::default();
        let mut params = OutputParams::default();

        assert_eq!(resolve(&output("DP-1"), &facts, &signals, &params).scroll_h, 0.5);

        params.scroll.horizontal.enabled = true;
        assert_eq!(resolve(&output("DP-1"), &facts, &signals, &params).scroll_h, 0.0);
    }

    #[test]
    fn only_the_focused_output_blurs() {
        let mut facts = Facts { focused_output: Some(output("DP-1")), ..Facts::default() };
        facts.outputs.insert(output("DP-1"), OutputFacts::default());
        facts.outputs.insert(output("eDP-1"), OutputFacts::default());
        let signals = Signals::default();
        let params = OutputParams::default();

        assert_eq!(resolve(&output("DP-1"), &facts, &signals, &params).blur, 1.0);
        assert_eq!(resolve(&output("eDP-1"), &facts, &signals, &params).blur, 0.0);
    }

    #[test]
    fn nothing_focused_leaves_every_output_sharp() {
        let facts = Facts::default();
        let signals = Signals::default();
        let params = OutputParams::default();
        assert_eq!(resolve(&output("DP-1"), &facts, &signals, &params).blur, 0.0);
    }

    #[test]
    fn an_external_signal_blurs_an_unfocused_output() {
        let facts = Facts::default();
        let mut signals = Signals::default();
        signals.set_blur(Some(output("eDP-1")), true);
        let params = OutputParams::default();

        assert_eq!(resolve(&output("eDP-1"), &facts, &signals, &params).blur, 1.0);
        assert_eq!(resolve(&output("DP-1"), &facts, &signals, &params).blur, 0.0);
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

    #[test]
    fn a_zero_radius_disables_blur_entirely() {
        let facts = Facts { focused_output: Some(output("DP-1")), ..Facts::default() };
        let mut signals = Signals::default();
        signals.set_blur(None, true);
        let params = OutputParams {
            blur: BlurParams { radius: 0, ..BlurParams::default() },
            ..Default::default()
        };

        assert_eq!(resolve(&output("DP-1"), &facts, &signals, &params).blur, 0.0);
    }

    #[test]
    fn the_overview_zooms_back_out() {
        let params = OutputParams::default();
        let signals = Signals::default();

        let closed = Facts::default();
        assert!(resolve(&output("DP-1"), &closed, &signals, &params).zoom > 1.0);

        let open = Facts { overview_active: true, ..Facts::default() };
        assert_eq!(resolve(&output("DP-1"), &open, &signals, &params).zoom, 1.0);
    }

    #[test]
    fn each_output_scrolls_by_its_own_workspace() {
        let mut facts = Facts::default();
        facts.outputs.insert(
            output("DP-1"),
            OutputFacts { workspace: Index::new(1, 3), ..OutputFacts::default() },
        );
        facts.outputs.insert(
            output("eDP-1"),
            OutputFacts { workspace: Index::new(3, 3), ..OutputFacts::default() },
        );
        let signals = Signals::default();
        let params = OutputParams::default();

        assert_eq!(resolve(&output("DP-1"), &facts, &signals, &params).scroll_v, 0.0);
        assert_eq!(resolve(&output("eDP-1"), &facts, &signals, &params).scroll_v, 1.0);
    }
}
