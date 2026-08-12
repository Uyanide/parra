use crate::anim::{Animated, Motion};
use crate::blur::BlurState;
use crate::geometry::{UvRect, sample_rect};
use crate::output::{LogicalSize, OutputId, PixelSize, Scale};
use crate::params::OutputParams;
use crate::policy::Targets;
use crate::scroll::ScrollState;
use crate::wallpaper::{WallpaperRef, WallpaperSlot};

/// Everything the renderer needs about one output.
///
/// Each output owns its own animation clocks, so two monitors refreshing at different
/// rates advance independently and one of them settling stops its frames alone.
#[derive(Clone, Debug)]
pub struct MonitorState {
    pub id: OutputId,
    pub logical: LogicalSize,
    pub scale: Scale,
    pub params: OutputParams,
    pub wallpaper: WallpaperSlot,
    pub scroll: ScrollState,
    pub blur: BlurState,
    pub zoom: Animated,
    /// Last wallpaper the config asked for. Kept so that a reload can tell an edited
    /// path from one the control socket set, and leave the latter alone.
    config_wallpaper: Option<WallpaperRef>,
}

impl MonitorState {
    pub fn new(id: OutputId, params: OutputParams) -> Self {
        let mut state = Self {
            id,
            logical: LogicalSize::default(),
            scale: Scale::ONE,
            wallpaper: WallpaperSlot::new(),
            scroll: ScrollState::new(),
            blur: BlurState::new(),
            zoom: Animated::new(params.overview.zoom()),
            config_wallpaper: params.wallpaper.clone(),
            params,
        };
        let initial = state.config_wallpaper.clone();
        state.wallpaper.set(initial, &crate::params::TransitionParams::INSTANT);
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
            self.zoom.value(),
            self.scroll.h.value(),
            self.scroll.v.value(),
        )
    }

    /// Adopts a reloaded configuration. Returns whether the wallpaper changed, which is
    /// the only part that costs a decode.
    pub fn apply_params(&mut self, params: OutputParams) -> bool {
        let wallpaper = params.wallpaper.clone();
        let changed = wallpaper != self.config_wallpaper;
        self.config_wallpaper = wallpaper.clone();
        self.params = params;
        changed && self.set_wallpaper(wallpaper)
    }

    /// Sets the wallpaper directly, as the control socket does. Returns whether it
    /// differs from what is already showing, which is what costs a decode.
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
        self.zoom.retarget(targets.zoom, overview.tween);
    }

    /// Jumps to the targets. Used on the first resolve for an output, so that appearing
    /// on screen is not itself an animation.
    pub fn snap(&mut self, targets: &Targets) {
        self.scroll.v.snap(targets.scroll_v);
        self.scroll.h.snap(targets.scroll_h);
        self.blur.amount.snap(targets.blur);
        self.zoom.snap(targets.zoom);
    }

    pub fn tick(&mut self, dt: f32) -> Motion {
        self.scroll.tick(dt) | self.blur.tick(dt) | self.zoom.tick(dt) | self.wallpaper.tick(dt)
    }

    pub fn is_settled(&self) -> bool {
        self.scroll.v.is_settled()
            && self.scroll.h.is_settled()
            && self.blur.amount.is_settled()
            && self.zoom.is_settled()
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
    fn a_reload_leaves_a_socket_set_wallpaper_alone() {
        let params = OutputParams {
            wallpaper: Some(WallpaperRef::new("/tmp/from-config.png")),
            ..OutputParams::default()
        };
        let mut state = MonitorState::new(OutputId::new("DP-1"), params.clone());
        state.set_wallpaper(Some(WallpaperRef::new("/tmp/from-socket.png")));

        assert!(!state.apply_params(params), "an unchanged config should not reclaim the slot");
        assert_eq!(
            state.wallpaper.current().map(WallpaperRef::path),
            Some(std::path::Path::new("/tmp/from-socket.png"))
        );
    }

    #[test]
    fn a_reload_that_edits_the_path_does_take_effect() {
        let params = OutputParams {
            wallpaper: Some(WallpaperRef::new("/tmp/from-config.png")),
            ..OutputParams::default()
        };
        let mut state = MonitorState::new(OutputId::new("DP-1"), params);
        state.set_wallpaper(Some(WallpaperRef::new("/tmp/from-socket.png")));

        let edited = OutputParams {
            wallpaper: Some(WallpaperRef::new("/tmp/edited.png")),
            ..OutputParams::default()
        };
        assert!(state.apply_params(edited));
        assert_eq!(
            state.wallpaper.current().map(WallpaperRef::path),
            Some(std::path::Path::new("/tmp/edited.png"))
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
        let mut state = MonitorState::new(OutputId::new("DP-1"), params);
        state.set_wallpaper(Some(WallpaperRef::new("/tmp/a.png")));
        state.set_wallpaper(Some(WallpaperRef::new("/tmp/b.png")));

        assert!(state.wallpaper.outgoing().is_some());
        assert_eq!(state.tick(0.1), Motion::Running);
        assert_eq!(state.tick(10.0), Motion::Settled);
        assert!(state.wallpaper.outgoing().is_none());
    }
}
