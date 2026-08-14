use crate::anim::Motion;
use crate::blur::BlurState;
use crate::geometry::{UvRect, sample_rect};
use crate::output::{LogicalSize, OutputId, PixelSize, Scale};
use crate::params::OutputParams;
use crate::policy::Targets;
use crate::scroll::ScrollState;
use crate::wallpaper::{WallpaperRef, WallpaperSlot};
use crate::zoom::ZoomState;

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
    pub blur: BlurState,
    pub zoom: ZoomState,
}

impl MonitorState {
    /// `wallpaper` is snapped rather than faded in: coming into existence should not look
    /// like a transition.
    pub fn new(id: OutputId, params: OutputParams, wallpaper: Option<WallpaperRef>) -> Self {
        let mut state = Self {
            id,
            logical: LogicalSize::default(),
            scale: Scale::ONE,
            wallpaper: WallpaperSlot::new(),
            scroll: ScrollState::new(),
            blur: BlurState::new(),
            zoom: ZoomState::new(params.overview.zoom()),
            params,
        };
        state.wallpaper.set(wallpaper, &crate::params::TransitionParams::INSTANT);
        state
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
        )
    }

    /// Adopts a reloaded configuration.
    pub fn apply_params(&mut self, params: OutputParams) {
        self.params = params;
    }

    /// Sets the wallpaper. Returns whether it differs from what is already showing.
    pub fn set_wallpaper(&mut self, next: Option<WallpaperRef>) -> bool {
        self.wallpaper.set(next, &self.params.transition)
    }

    /// Starts easing toward freshly resolved targets.
    pub fn apply(&mut self, targets: &Targets) {
        let scroll = self.params.scroll;
        let blur = self.params.blur;
        let overview = self.params.overview;
        self.scroll.v.retarget(targets.scroll_v, scroll.vertical.tween);
        self.scroll.h.retarget(targets.scroll_h, scroll.horizontal.tween);
        self.blur.amount.retarget(targets.blur, blur.tween);
        self.zoom.factor.retarget(targets.zoom, overview.tween);
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
        let mut state = MonitorState::new(OutputId::new("DP-1"), OutputParams::default(), None);
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
        assert!(state.set_wallpaper(wallpaper.clone()));
        assert!(!state.set_wallpaper(wallpaper));
    }

    #[test]
    fn a_monitor_appears_already_showing_its_wallpaper() {
        let wallpaper = WallpaperRef::new("/tmp/a.png");
        let state = MonitorState::new(
            OutputId::new("DP-1"),
            OutputParams::default(),
            Some(wallpaper.clone()),
        );
        assert_eq!(state.wallpaper.current(), Some(&wallpaper));
        assert!(state.is_settled(), "appearing should not look like a transition");
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
        let mut state = monitor();
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
        let mut state = MonitorState::new(OutputId::new("DP-1"), params, None);
        state.set_wallpaper(Some(WallpaperRef::new("/tmp/a.png")));
        state.set_wallpaper(Some(WallpaperRef::new("/tmp/b.png")));

        assert!(state.wallpaper.outgoing().is_some());
        assert_eq!(state.tick(0.1), Motion::Running);
        assert_eq!(state.tick(10.0), Motion::Settled);
        assert!(state.wallpaper.outgoing().is_none());
    }
}
