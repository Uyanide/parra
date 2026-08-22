use crate::anim::{Motion, Move};
use crate::blur::BlurState;
use crate::geometry::{Limit, Limits, UvRect, sample_rect};
use crate::output::{LogicalSize, OutputId, PixelSize, Scale};
use crate::params::OutputParams;
use crate::policy::Targets;
use crate::scroll::{ScrollState, Stride};
use crate::wallpaper::{Swap, WallpaperRef, WallpaperSlot};
use crate::zoom::ZoomState;

/// The moves one call to [`MonitorState::apply`] started, per animated property.
///
/// Shaped like [`Targets`] on purpose: the two name the same four properties, and a second
/// set of names for them would be a second thing to keep in step.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Moves {
    pub scroll_v: Option<Move>,
    pub scroll_h: Option<Move>,
    pub blur: Option<Move>,
    pub zoom: Option<Move>,
}

/// Live state of one output: its geometry, what it is showing, and the animations
/// currently in flight on it.
///
/// Every clock here belongs to this output alone, so outputs refreshing at different
/// rates advance and settle independently.
#[derive(Clone, Debug)]
pub struct MonitorState {
    pub id: OutputId,
    pub logical: LogicalSize,
    pub scale: Scale,
    pub params: OutputParams,
    pub wallpaper: WallpaperSlot,
    pub scroll: ScrollState,
    /// How far one stop of each axis is, as the compositor last reported it. Assigned from
    /// outside like the geometry is, because it describes what drives the axis rather than
    /// anything the axis is animating toward.
    pub stride: Stride,
    pub blur: BlurState,
    pub zoom: ZoomState,
}

impl MonitorState {
    /// Arrives showing nothing. The caller sets the wallpaper, which is what gives the
    /// arrival the configured transition rather than a second rule for it here.
    pub fn new(id: OutputId, params: OutputParams) -> Self {
        Self {
            id,
            logical: LogicalSize::default(),
            scale: Scale::ONE,
            wallpaper: WallpaperSlot::new(),
            scroll: ScrollState::new(),
            stride: Stride::default(),
            blur: BlurState::new(),
            zoom: ZoomState::new(params.zoom.factor()),
            params,
        }
    }

    /// Size of the buffer this output needs, in device pixels.
    pub fn buffer_size(&self) -> PixelSize {
        self.logical.to_pixels(self.scale)
    }

    /// Region of the wallpaper currently visible, given where the animations have got to.
    pub fn sample_rect(&self, image: PixelSize) -> UvRect {
        sample_rect(
            image,
            self.buffer_size(),
            self.zoom.factor.value(),
            self.scroll.h.value(),
            self.scroll.v.value(),
            self.limits(),
        )
    }

    /// What caps each axis, which is the configured shift beside the stride it applies to.
    ///
    /// `travel` has already narrowed the range the position is driven over by the time it
    /// reaches here, so a stop covers that much less of it. Folding the two together is
    /// what keeps `travel` applied in one place.
    fn limits(&self) -> Limits {
        let axis = |stride: f32, params: &crate::params::AxisParams| Limit {
            stride: stride * params.travel,
            max_shift: params.max_shift,
        };
        Limits {
            h: axis(self.stride.h, &self.params.scroll.horizontal),
            v: axis(self.stride.v, &self.params.scroll.vertical),
        }
    }

    /// Adopts a reloaded configuration.
    pub fn apply_params(&mut self, params: OutputParams) {
        self.params = params;
    }

    /// Sets the wallpaper. Returns the swap it made, and `None` when the wallpaper asked
    /// for is the one already showing.
    pub fn set_wallpaper(&mut self, next: Option<WallpaperRef>) -> Option<Swap> {
        self.wallpaper.set(next, &self.params.transition)
    }

    /// Starts easing toward freshly resolved targets, reporting whichever of them moved.
    pub fn apply(&mut self, targets: &Targets) -> Moves {
        let scroll = self.params.scroll;
        let blur = self.params.blur;
        let zoom = self.params.zoom;
        Moves {
            scroll_v: self.scroll.v.retarget(targets.scroll_v, scroll.vertical.tween),
            scroll_h: self.scroll.h.retarget(targets.scroll_h, scroll.horizontal.tween),
            blur: self.blur.amount.retarget(targets.blur, blur.tween),
            zoom: self.zoom.factor.retarget(targets.zoom, zoom.tween),
        }
    }

    /// Jumps to the targets, for when arriving at them should not itself be an animation.
    pub fn snap(&mut self, targets: &Targets) {
        self.scroll.v.snap(targets.scroll_v);
        self.scroll.h.snap(targets.scroll_h);
        self.blur.amount.snap(targets.blur);
        self.zoom.factor.snap(targets.zoom);
    }

    pub fn tick(&mut self, dt: f32) -> Motion {
        self.scroll.tick(dt) | self.blur.tick(dt) | self.zoom.tick(dt) | self.wallpaper.tick(dt)
    }

    pub fn is_settled(&self) -> bool {
        self.scroll.v.is_settled()
            && self.scroll.h.is_settled()
            && self.blur.amount.is_settled()
            && self.zoom.factor.is_settled()
            && self.wallpaper.is_settled()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::{TransitionMode, TransitionParams};

    fn monitor() -> MonitorState {
        let mut state = MonitorState::new(OutputId::new("DP-1"), OutputParams::default());
        state.logical = LogicalSize::new(2560, 1440);
        state
    }

    fn targets(blur: f32) -> Targets {
        Targets { scroll_v: 0.5, scroll_h: 0.5, blur, zoom: 1.0 }
    }

    #[test]
    fn a_fresh_monitor_is_settled_and_submits_nothing() {
        let mut state = monitor();
        assert!(state.is_settled());
        assert_eq!(state.tick(1.0), Motion::Settled);
    }

    #[test]
    fn snapping_does_not_start_an_animation() {
        let mut state = monitor();
        state.snap(&targets(1.0));
        assert!(state.is_settled());
        assert_eq!(state.blur.amount.value(), 1.0);
    }

    #[test]
    fn applying_targets_runs_until_they_are_reached() {
        let mut state = monitor();
        state.apply(&targets(1.0));
        assert_eq!(state.tick(0.1), Motion::Running);
        assert_eq!(state.tick(10.0), Motion::Settled);
        assert_eq!(state.blur.amount.value(), 1.0);
    }

    /// Much taller than the screen, so the default cap has something to bite on.
    const TALL: PixelSize = PixelSize { w: 2937, h: 4796 };

    /// How far the wallpaper moves between the two ends of the vertical axis, in screen
    /// heights.
    fn excursion(state: &mut MonitorState) -> f32 {
        state.scroll.v.snap(0.0);
        let top = state.sample_rect(TALL);
        state.scroll.v.snap(1.0);
        let bottom = state.sample_rect(TALL);
        (bottom.v0 - top.v0) / top.height()
    }

    #[test]
    fn an_output_no_stride_was_reported_for_is_not_capped() {
        let mut state = monitor();
        assert_eq!(state.stride, Stride::default());
        assert!(excursion(&mut state) > 2.0, "the whole travel, cap or no cap");
    }

    #[test]
    fn the_reported_stride_is_what_the_cap_measures() {
        let mut state = monitor();
        state.stride = Stride { v: 1.0, h: 0.0 };
        assert!((excursion(&mut state) - 0.5).abs() < 1e-4, "one stop held to max-shift");

        state.stride = Stride { v: 0.25, h: 0.0 };
        assert!((excursion(&mut state) - 2.0).abs() < 1e-4, "four stops of it");
    }

    /// `travel` has already narrowed the range the position is driven over, so a stop
    /// covers that much less of it and the cap has correspondingly less to do.
    #[test]
    fn travel_shortens_the_stop_the_cap_measures() {
        let mut state = monitor();
        state.stride = Stride { v: 1.0, h: 0.0 };
        state.params.scroll.vertical.travel = 0.5;
        assert!((excursion(&mut state) - 1.0).abs() < 1e-4);
    }

    /// What a workspace opening does: every position stays where it was and the rect
    /// still moves, so a redraw cannot be inferred from the animations alone.
    #[test]
    fn a_stride_change_moves_the_rect_with_the_position_standing_still() {
        let mut state = monitor();
        state.scroll.v.snap(0.0);

        state.stride = Stride { v: 1.0, h: 0.0 };
        let two_stops = state.sample_rect(TALL);
        state.stride = Stride { v: 0.5, h: 0.0 };
        let three_stops = state.sample_rect(TALL);

        assert!(two_stops.v0 > three_stops.v0, "the looser cap reaches further up");
    }

    #[test]
    fn lifting_the_cap_gives_the_whole_travel_back() {
        let mut state = monitor();
        state.stride = Stride { v: 1.0, h: 0.0 };
        state.params.scroll.vertical.max_shift = None;
        assert!(excursion(&mut state) > 2.0);
    }

    #[test]
    fn the_buffer_follows_the_fractional_scale() {
        let mut state = monitor();
        state.logical = LogicalSize::new(2048, 1280);
        state.scale = Scale::from_120ths(150);
        assert_eq!(state.buffer_size(), PixelSize::new(2560, 1600));
    }

    #[test]
    fn setting_the_same_wallpaper_twice_reports_no_change() {
        let mut state = monitor();
        let wallpaper = Some(WallpaperRef::new("/tmp/a.png"));
        assert!(state.set_wallpaper(wallpaper.clone()).is_some());
        assert_eq!(state.set_wallpaper(wallpaper), None);
    }

    #[test]
    fn applying_targets_reports_only_what_moved() {
        let mut state = monitor();
        state.snap(&targets(0.0));

        let moves = state.apply(&targets(1.0));
        assert_eq!(moves.blur.map(|blur| (blur.from, blur.to)), Some((0.0, 1.0)));
        assert_eq!(moves.scroll_v, None, "the targets it was already on did not move");
        assert_eq!(moves.scroll_h, None);
        assert_eq!(moves.zoom, None);
    }

    #[test]
    fn applying_the_targets_already_held_reports_nothing() {
        let mut state = monitor();
        state.snap(&targets(1.0));
        assert_eq!(state.apply(&targets(1.0)), Moves::default());
    }

    #[test]
    fn a_monitor_appears_with_its_wallpaper_arriving() {
        let wallpaper = WallpaperRef::new("/tmp/a.png");
        let mut state = monitor();
        state.snap(&targets(1.0));
        state.set_wallpaper(Some(wallpaper.clone()));

        assert_eq!(state.wallpaper.current(), Some(&wallpaper));
        assert_eq!(state.wallpaper.opacity(), 0.0);
        // Only the wallpaper animates. The four driven values still snap, or a monitor
        // appearing would scroll and zoom its way to where it already is.
        assert_eq!(state.blur.amount.value(), 1.0);
        assert!(state.blur.amount.is_settled());
    }

    #[test]
    fn a_reload_leaves_the_wallpaper_to_the_caller() {
        let mut state = monitor();
        state.set_wallpaper(Some(WallpaperRef::new("/tmp/from-socket.png")));

        state.apply_params(OutputParams {
            fallback: Some(WallpaperRef::new("/tmp/from-config.png")),
            ..OutputParams::default()
        });

        assert_eq!(
            state.wallpaper.current().map(WallpaperRef::path),
            Some(std::path::Path::new("/tmp/from-socket.png"))
        );
    }

    #[test]
    fn without_a_transition_the_outgoing_slot_is_never_held() {
        let params = OutputParams {
            transition: TransitionParams {
                mode: TransitionMode::None,
                ..TransitionParams::default()
            },
            ..OutputParams::default()
        };
        let mut state = MonitorState::new(OutputId::new("DP-1"), params);
        state.set_wallpaper(Some(WallpaperRef::new("/tmp/a.png")));
        state.set_wallpaper(Some(WallpaperRef::new("/tmp/b.png")));
        assert!(state.wallpaper.outgoing().is_none());
        assert_eq!(state.wallpaper.fade(), 1.0);
    }

    #[test]
    fn a_fade_holds_the_outgoing_slot_until_it_completes() {
        let params = OutputParams {
            transition: TransitionParams {
                mode: TransitionMode::Fade,
                ..TransitionParams::default()
            },
            ..OutputParams::default()
        };
        let mut state = MonitorState::new(OutputId::new("DP-1"), params);
        state.set_wallpaper(Some(WallpaperRef::new("/tmp/a.png")));
        state.set_wallpaper(Some(WallpaperRef::new("/tmp/b.png")));

        assert!(state.wallpaper.outgoing().is_some());
        assert_eq!(state.tick(0.1), Motion::Running);
        assert_eq!(state.tick(10.0), Motion::Settled);
        assert!(state.wallpaper.outgoing().is_none());
    }
}
