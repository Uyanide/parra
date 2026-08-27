use crate::anim::{Animated, Motion, Move, Tween};
use crate::blur::BlurState;
use crate::geometry::{Travel, UvRect, sample_rect};
use crate::output::{LogicalSize, OutputId, PixelSize, Scale};
use crate::params::OutputParams;
use crate::policy::Targets;
use crate::scroll::ScrollState;
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
    /// Travel the wallpaper on screen has at the deepest zoom, once something has been
    /// decoded to measure. Assigned from outside like the geometry above it.
    ///
    /// Kept rather than re-read per resolve so that an arriving wallpaper, whose texture
    /// lands a moment after it is chosen, does not resolve against nothing in between.
    pub travel: Option<Travel>,
    pub scroll: ScrollState,
    /// Where the wallpaper on its way out sits, frozen when its crossfade began.
    ///
    /// The share beside it is resolved against the wallpaper arriving, so without this the
    /// image leaving would be moved by a decode it has nothing to do with. `None` once
    /// there is only one image, which is when the share alone is the answer again.
    pub outgoing_scroll: Option<(f32, f32)>,
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
            travel: None,
            scroll: ScrollState::new(),
            outgoing_scroll: None,
            blur: BlurState::new(),
            zoom: ZoomState::new(params.zoom.factor()),
            params,
        }
    }

    /// Size of the buffer this output needs, in device pixels.
    pub fn buffer_size(&self) -> PixelSize {
        self.logical.to_pixels(self.scale)
    }

    /// Region of one wallpaper currently visible. The animated scroll is a share of the
    /// headroom, so current and outgoing wallpapers project it onto their own.
    pub fn sample_rect(&self, image: PixelSize) -> UvRect {
        sample_rect(
            image,
            self.buffer_size(),
            self.zoom.factor.value(),
            self.scroll.h.value(),
            self.scroll.v.value(),
        )
    }

    /// Adopts a reloaded configuration.
    pub fn apply_params(&mut self, params: OutputParams) {
        self.params = params;
    }

    /// Sets the wallpaper. Returns the swap it made, and `None` when the wallpaper asked
    /// for is the one already showing.
    pub fn set_wallpaper(&mut self, next: Option<WallpaperRef>) -> Option<Swap> {
        let was = self.wallpaper.outgoing().cloned();
        let swap = self.wallpaper.set(next, &self.params.transition);
        // Only when a different image is the one on its way out. Asking for the wallpaper
        // already there is not a transition, and a crossfade interrupted in its first half
        // goes on leaving the image it was already leaving: in both the live share belongs
        // to something else by now, so reading it would move a frame nothing happened to.
        if self.wallpaper.outgoing() != was.as_ref() {
            self.outgoing_scroll =
                self.wallpaper.outgoing().map(|_| (self.scroll.h.value(), self.scroll.v.value()));
        }
        swap
    }

    /// Region of the wallpaper leaving the screen, which holds the place it was last
    /// drawn in for as long as the crossfade lasts.
    pub fn sample_rect_outgoing(&self, image: PixelSize) -> UvRect {
        let (h, v) = self.outgoing_scroll.unwrap_or((self.scroll.h.value(), self.scroll.v.value()));
        sample_rect(image, self.buffer_size(), self.zoom.factor.value(), h, v)
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

    /// Places the scroll without animating, for when the image underneath it changed
    /// rather than the position of it: nothing moved, so nothing should be seen moving.
    ///
    /// Only the wallpaper arriving is placed. The one leaving keeps `outgoing_scroll`,
    /// which is why this cannot be seen: it happens before the arriving image is drawn.
    /// Reported all the same, as the move of no duration that it is, so that a subscriber
    /// following the values is never told less than the screen was.
    pub fn replace_geometry(&mut self, targets: &Targets) -> Moves {
        Moves {
            scroll_v: place(&mut self.scroll.v, targets.scroll_v),
            scroll_h: place(&mut self.scroll.h, targets.scroll_h),
            ..Moves::default()
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
        let motion = self.scroll.tick(dt)
            | self.blur.tick(dt)
            | self.zoom.tick(dt)
            | self.wallpaper.tick(dt);
        if self.wallpaper.outgoing().is_none() {
            self.outgoing_scroll = None;
        }
        motion
    }

    pub fn is_settled(&self) -> bool {
        self.scroll.v.is_settled()
            && self.scroll.h.is_settled()
            && self.blur.amount.is_settled()
            && self.zoom.factor.is_settled()
            && self.wallpaper.is_settled()
    }
}

/// Jumps one value and describes the jump, or reports nothing when it was already there.
fn place(value: &mut Animated, target: f32) -> Option<Move> {
    let from = value.value();
    if from == target {
        return None;
    }
    value.snap(target);
    Some(Move { from, to: target, tween: Tween::INSTANT })
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
        Targets { scroll_v: 0.0, scroll_h: 0.0, blur, zoom: 1.0 }
    }

    /// Tall enough that the two images below place one share very differently.
    const TALL: PixelSize = PixelSize { w: 2937, h: 4796 };
    const WIDE: PixelSize = PixelSize { w: 5120, h: 1440 };

    fn crossfading() -> MonitorState {
        let mut state = monitor();
        state.params.transition =
            TransitionParams { mode: TransitionMode::Fade, ..TransitionParams::default() };
        state.set_wallpaper(Some(WallpaperRef::new("/tmp/a.png")));
        state.scroll.v.snap(-0.45);
        state
    }

    /// What a decode landing mid-crossfade must not do: the image on its way out is where
    /// it was drawn, and a share resolved against the image replacing it is not about it.
    #[test]
    fn the_wallpaper_leaving_keeps_its_place_while_the_one_arriving_is_placed() {
        let mut state = crossfading();
        let before = state.sample_rect(TALL);

        state.set_wallpaper(Some(WallpaperRef::new("/tmp/b.png")));
        assert!(state.wallpaper.outgoing().is_some(), "the crossfade is running");
        assert_eq!(state.sample_rect_outgoing(TALL), before, "the one leaving has not moved");

        let placed = state.replace_geometry(&Targets { scroll_v: -0.08, ..targets(0.0) });
        assert_eq!(state.sample_rect_outgoing(TALL), before, "and still has not");
        assert_ne!(state.sample_rect(TALL), before, "the one arriving was placed");
        assert_eq!(placed.scroll_v.map(|placed| placed.tween.duration), Some(0.0));
        assert_eq!(placed.scroll_v.map(|placed| placed.from), Some(-0.45));
    }

    /// `apply_wallpapers` calls this for every output on every pass, so most calls resolve
    /// to the wallpaper already on screen and must leave the frame alone.
    #[test]
    fn asking_for_the_wallpaper_already_there_leaves_the_frozen_place_alone() {
        let mut state = crossfading();
        state.set_wallpaper(Some(WallpaperRef::new("/tmp/b.png")));
        let frozen = state.sample_rect_outgoing(TALL);

        // What a decode landing does: the live share moves to the arriving image's.
        state.replace_geometry(&Targets { scroll_v: -0.08, ..targets(0.0) });

        assert_eq!(state.set_wallpaper(Some(WallpaperRef::new("/tmp/b.png"))), None);
        assert_eq!(state.sample_rect_outgoing(TALL), frozen);
    }

    /// Two slots cannot hold three images, so the first half of a crossfade keeps the one
    /// already leaving. It kept its place too, and a third wallpaper does not change that.
    #[test]
    fn interrupting_a_crossfade_early_leaves_the_image_already_leaving_where_it_was() {
        let mut state = crossfading();
        state.set_wallpaper(Some(WallpaperRef::new("/tmp/b.png")));
        let frozen = state.sample_rect_outgoing(TALL);
        let leaving = state.wallpaper.outgoing().cloned();

        state.tick(0.05);
        assert!(state.wallpaper.fade() < 0.5, "still in the first half");
        state.replace_geometry(&Targets { scroll_v: -0.08, ..targets(0.0) });
        state.set_wallpaper(Some(WallpaperRef::new("/tmp/c.png")));

        assert_eq!(state.wallpaper.outgoing().cloned(), leaving, "the same image is leaving");
        assert_eq!(state.sample_rect_outgoing(TALL), frozen);
    }

    /// Past half way the image leaving is replaced, and the one taking its place is
    /// wherever it was last drawn, which is the live share.
    #[test]
    fn interrupting_a_crossfade_late_freezes_the_image_that_becomes_the_one_leaving() {
        let mut state = crossfading();
        state.set_wallpaper(Some(WallpaperRef::new("/tmp/b.png")));
        state.replace_geometry(&Targets { scroll_v: -0.08, ..targets(0.0) });

        while state.wallpaper.fade() < 0.5 {
            state.tick(1.0 / 60.0);
        }
        let drawn = state.sample_rect(TALL);
        state.set_wallpaper(Some(WallpaperRef::new("/tmp/c.png")));

        assert_eq!(
            state.wallpaper.outgoing().map(WallpaperRef::path),
            Some("/tmp/b.png".as_ref()),
            "the image that was arriving is now the one leaving"
        );
        assert_eq!(state.sample_rect_outgoing(TALL), drawn, "frozen where it was drawn");
    }

    /// A placement is not movement, so it reports the jump it is rather than starting an
    /// animation a subscriber would then be waiting to end.
    #[test]
    fn placing_reports_a_move_of_no_duration_and_settles_at_once() {
        let mut state = monitor();
        state.scroll.v.snap(-0.5);
        let placed = state.replace_geometry(&Targets { scroll_v: -0.0788, ..targets(0.0) });

        assert_eq!(placed.scroll_v.map(|placed| (placed.from, placed.to)), Some((-0.5, -0.0788)));
        assert_eq!(placed.blur, None, "only the scroll is placed");
        assert_eq!(placed.zoom, None);
        assert!(state.is_settled(), "nothing is left in flight");
        assert_eq!(state.scroll.v.value(), -0.0788);
    }

    #[test]
    fn placing_where_it_already_sits_reports_nothing() {
        let mut state = monitor();
        state.scroll.v.snap(-0.3);
        state.scroll.h.snap(0.0);
        let placed = state.replace_geometry(&Targets { scroll_v: -0.3, ..targets(0.0) });
        assert_eq!(placed, Moves::default());
    }

    /// Held only for as long as there are two images, so an output that has settled is
    /// back to one share answering for the whole frame.
    #[test]
    fn the_frozen_place_is_dropped_when_the_crossfade_ends() {
        let mut state = crossfading();
        state.set_wallpaper(Some(WallpaperRef::new("/tmp/b.png")));
        assert!(state.outgoing_scroll.is_some());

        state.tick(10.0);
        assert!(state.outgoing_scroll.is_none());
        assert_eq!(state.sample_rect_outgoing(WIDE), state.sample_rect(WIDE), "one share again");
    }

    /// An arrival has nothing to crossfade against, so there is no second place to keep.
    #[test]
    fn a_first_wallpaper_freezes_nothing() {
        let state = crossfading();
        assert!(state.outgoing_scroll.is_none());
        assert_eq!(state.sample_rect_outgoing(TALL), state.sample_rect(TALL));
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

    #[test]
    fn a_scroll_retarget_starts_at_the_current_sample() {
        let mut state = monitor();
        state.scroll.v.snap(-0.15);
        let before = state.sample_rect(PixelSize::new(1000, 2000));

        let moves = state.apply(&Targets { scroll_v: -0.3, ..targets(0.0) });
        assert!(moves.scroll_v.is_some());
        assert_eq!(state.sample_rect(PixelSize::new(1000, 2000)), before);

        state.tick(10.0);
        assert_ne!(state.sample_rect(PixelSize::new(1000, 2000)), before);
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
