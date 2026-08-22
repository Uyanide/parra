use std::path::{Path, PathBuf};

use crate::anim::{Animated, Motion, Tween};
use crate::params::TransitionParams;

/// An image to display, identified by an absolute path and the moment it was chosen.
///
/// The epoch is part of the identity, so the same path chosen twice gives two wallpapers
/// that compare unequal. A path alone could not tell an image edited in place from itself.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct WallpaperRef {
    source: PathBuf,
    epoch: u64,
}

impl WallpaperRef {
    /// Epoch 0, for wallpapers that are replaced only when the path itself changes.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self::at(path, 0)
    }

    pub fn at(path: impl Into<PathBuf>, epoch: u64) -> Self {
        Self { source: path.into(), epoch }
    }

    pub fn path(&self) -> &Path {
        &self.source
    }

    pub fn epoch(&self) -> u64 {
        self.epoch
    }
}

/// One wallpaper replacing another, and the transition that carries it.
///
/// `from` is whatever was on screen, whether or not it was kept to fade out, so a report of
/// this describes what a viewer sees rather than which slots were used.
#[derive(Clone, Debug, PartialEq)]
pub struct Swap {
    pub from: Option<WallpaperRef>,
    pub to: Option<WallpaperRef>,
    pub tween: Tween,
}

/// Outgoing and incoming wallpaper, plus how far the crossfade between them has got.
///
/// With the transition mode off, `fade` snaps and `outgoing` is dropped on the same
/// call, so the second slot costs nothing.
#[derive(Clone, Debug, Default)]
pub struct WallpaperSlot {
    outgoing: Option<WallpaperRef>,
    current: Option<WallpaperRef>,
    /// 0 shows `outgoing`, 1 shows `current`.
    fade: Animated,
    /// How much of the frame is there at all. Below 1 only while a wallpaper is arriving
    /// with nothing behind it to cross into.
    opacity: Animated,
}

impl WallpaperSlot {
    pub fn new() -> Self {
        Self {
            outgoing: None,
            current: None,
            fade: Animated::new(1.0),
            opacity: Animated::new(1.0),
        }
    }

    pub fn current(&self) -> Option<&WallpaperRef> {
        self.current.as_ref()
    }

    pub fn outgoing(&self) -> Option<&WallpaperRef> {
        self.outgoing.as_ref()
    }

    pub fn fade(&self) -> f32 {
        self.fade.value()
    }

    /// How present the frame is, which the composite pass scales its whole output by.
    pub fn opacity(&self) -> f32 {
        self.opacity.value()
    }

    /// Starts showing `next`. Returns the swap it made, and `None` when nothing changed,
    /// since asking for the wallpaper already in the slot is not a transition.
    ///
    /// The tween reported is the one actually used, whether it carried a crossfade or an
    /// arrival, and only this knows which of the two happened.
    pub fn set(
        &mut self,
        next: Option<WallpaperRef>,
        transition: &TransitionParams,
    ) -> Option<Swap> {
        if self.current == next {
            return None;
        }
        // A slot with nothing in it has no second image to weigh against, so what arrives
        // fades the whole frame up instead of crossfading.
        let arriving = self.current.is_none() && next.is_some();
        // Nothing is drawn for a slot holding no wallpaper, so an emptied one has no frame
        // left to animate on.
        let tween = if next.is_none() || (arriving && !transition.at_start) {
            Tween::INSTANT
        } else {
            transition.effective_tween()
        };
        let crossfade = if arriving { Tween::INSTANT } else { tween };

        let previous = self.current.clone();
        if crossfade.is_instant() {
            self.outgoing = None;
            self.current = next.clone();
            self.fade.snap(1.0);
        } else {
            // Two slots cannot hold three images, so a restart drops one of them. Keeping
            // whichever is the more visible bounds the discontinuity at half.
            if self.fade.value() >= 0.5 {
                self.outgoing = self.current.take();
            }
            self.current = next.clone();
            self.fade.snap(0.0);
            self.fade.retarget(1.0, crossfade);
        }

        if arriving {
            self.opacity.snap(0.0);
            self.opacity.retarget(1.0, tween);
        } else if next.is_none() {
            // A slot with no wallpaper draws no frame, so an arrival left in flight here
            // would never be ticked to its end and the output would never settle.
            self.opacity.snap(1.0);
        }

        // What the viewer watches leave: the slot kept to fade out, or whatever was on
        // screen when none was kept.
        Some(Swap { from: self.outgoing.clone().or(previous), to: next, tween })
    }

    pub fn tick(&mut self, dt: f32) -> Motion {
        // The crossfade alone decides when the second slot is released. Testing the two
        // together would pin the outgoing image and its bake for the rest of an arrival
        // that a `set` interrupted, long after the crossfade needing them finished.
        let crossfade = self.fade.tick(dt);
        if !crossfade.is_running() {
            self.outgoing = None;
        }
        crossfade | self.opacity.tick(dt)
    }

    pub fn is_settled(&self) -> bool {
        self.fade.is_settled() && self.opacity.is_settled()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;
    use crate::anim::Easing;
    use crate::params::TransitionMode;

    fn fade() -> TransitionParams {
        TransitionParams { mode: TransitionMode::Fade, ..TransitionParams::default() }
    }

    /// A slot already showing `/tmp/a.png`, with no transition behind it.
    fn showing_a() -> WallpaperSlot {
        let mut slot = WallpaperSlot::new();
        slot.set(Some(WallpaperRef::new("/tmp/a.png")), &TransitionParams::INSTANT);
        slot
    }

    #[test]
    fn one_path_at_two_epochs_is_two_wallpapers() {
        let first = WallpaperRef::at("/tmp/a.png", 1);
        let second = WallpaperRef::at("/tmp/a.png", 2);
        assert_ne!(first, second);

        let keys: HashSet<WallpaperRef> = [first.clone(), second].into_iter().collect();
        assert_eq!(keys.len(), 2, "a cache keyed on this must see two entries");
        assert_eq!(first.path(), Path::new("/tmp/a.png"));
    }

    #[test]
    fn setting_the_same_path_at_a_new_epoch_reports_a_change() {
        let mut slot = WallpaperSlot::new();
        assert!(
            slot.set(Some(WallpaperRef::at("/tmp/a.png", 1)), &TransitionParams::INSTANT).is_some()
        );
        assert!(
            slot.set(Some(WallpaperRef::at("/tmp/a.png", 2)), &TransitionParams::INSTANT).is_some()
        );
    }

    #[test]
    fn setting_the_wallpaper_already_shown_reports_nothing() {
        let mut slot = showing_a();
        assert_eq!(slot.set(Some(WallpaperRef::new("/tmp/a.png")), &fade()), None);
    }

    #[test]
    fn the_first_wallpaper_fades_the_frame_up_rather_than_crossfading() {
        let mut slot = WallpaperSlot::new();
        let swap = slot.set(Some(WallpaperRef::new("/tmp/a.png")), &fade()).unwrap();

        assert_eq!(swap.from, None);
        assert_eq!(swap.to, Some(WallpaperRef::new("/tmp/a.png")));
        assert_eq!(swap.tween, fade().tween);
        assert_eq!(slot.opacity(), 0.0);
        assert_eq!(slot.fade(), 1.0, "there is nothing to crossfade against");
        assert!(slot.outgoing().is_none());
    }

    #[test]
    fn an_arrival_lands_exactly_on_a_fully_present_frame() {
        let mut slot = WallpaperSlot::new();
        slot.set(Some(WallpaperRef::new("/tmp/a.png")), &fade());
        assert_eq!(slot.tick(fade().tween.duration), Motion::Settled);
        // Compared exactly where the opaque region is decided, so anything short of full
        // would leave the surface blended for ever.
        assert_eq!(slot.opacity(), 1.0);
    }

    #[test]
    fn an_arrival_turned_off_appears_outright() {
        let params = TransitionParams { at_start: false, ..fade() };
        let mut slot = WallpaperSlot::new();
        let swap = slot.set(Some(WallpaperRef::new("/tmp/a.png")), &params).unwrap();

        assert!(swap.tween.is_instant());
        assert_eq!(slot.opacity(), 1.0);
        assert!(slot.is_settled());
    }

    #[test]
    fn a_mode_that_swaps_outright_arrives_outright_too() {
        let params = TransitionParams { mode: TransitionMode::None, ..TransitionParams::default() };
        let mut slot = WallpaperSlot::new();
        assert!(
            slot.set(Some(WallpaperRef::new("/tmp/a.png")), &params).unwrap().tween.is_instant()
        );
        assert_eq!(slot.opacity(), 1.0);
        assert!(slot.is_settled());
    }

    /// An arrival that is never drawn again would otherwise leave the output reporting
    /// motion for ever, since a slot with no wallpaper submits no frame to tick it on.
    #[test]
    fn emptying_a_slot_part_way_through_an_arrival_leaves_nothing_in_flight() {
        let mut slot = WallpaperSlot::new();
        slot.set(Some(WallpaperRef::new("/tmp/a.png")), &fade());
        slot.tick(fade().tween.duration * 0.25);
        slot.set(None, &fade());

        assert!(slot.is_settled());
    }

    /// The case that rules out reusing the crossfade weight for both: either branch of the
    /// half-way rule would reset the arrival or jump it to the end.
    #[test]
    fn a_set_part_way_through_an_arrival_leaves_it_running() {
        for elapsed in [0.25, 0.75] {
            let mut slot = WallpaperSlot::new();
            slot.set(Some(WallpaperRef::new("/tmp/a.png")), &fade());
            slot.tick(fade().tween.duration * elapsed);
            let arrived = slot.opacity();

            slot.set(Some(WallpaperRef::new("/tmp/b.png")), &fade());
            assert_eq!(slot.opacity(), arrived, "the frame moved at {elapsed} of the way in");
            assert_eq!(slot.fade(), 0.0, "the crossfade starts from the outgoing image");
        }
    }

    #[test]
    fn the_outgoing_slot_is_released_when_the_crossfade_ends_not_the_arrival() {
        let long = TransitionParams { tween: Tween::new(4.0, Easing::Linear), ..fade() };
        let mut slot = WallpaperSlot::new();
        slot.set(Some(WallpaperRef::new("/tmp/a.png")), &long);
        slot.tick(long.tween.duration * 0.75);
        slot.set(Some(WallpaperRef::new("/tmp/b.png")), &fade());

        assert!(slot.outgoing().is_some());
        assert_eq!(slot.tick(fade().tween.duration), Motion::Running, "the arrival goes on");
        assert!(slot.outgoing().is_none(), "the crossfade finished, so its second image can go");
    }

    #[test]
    fn emptying_the_slot_reports_what_left_and_no_transition() {
        let mut slot = showing_a();
        let swap = slot.set(None, &fade()).unwrap();

        assert_eq!(swap.from, Some(WallpaperRef::new("/tmp/a.png")));
        assert_eq!(swap.to, None);
        assert!(swap.tween.is_instant());
    }

    #[test]
    fn a_fade_reports_the_configured_tween() {
        let mut slot = showing_a();
        let swap = slot.set(Some(WallpaperRef::new("/tmp/b.png")), &fade()).unwrap();

        assert_eq!(swap.from, Some(WallpaperRef::new("/tmp/a.png")));
        assert_eq!(swap.tween, fade().tween);
    }

    #[test]
    fn a_restart_reports_the_image_actually_leaving_the_screen() {
        let mut slot = showing_a();
        slot.set(Some(WallpaperRef::new("/tmp/b.png")), &fade());
        let swap = slot.set(Some(WallpaperRef::new("/tmp/c.png")), &fade()).unwrap();

        assert_eq!(
            swap.from,
            Some(WallpaperRef::new("/tmp/a.png")),
            "b was barely visible, so it is a that fades out"
        );
    }

    #[test]
    fn a_restart_early_in_a_fade_keeps_the_image_still_on_screen() {
        let mut slot = showing_a();
        slot.set(Some(WallpaperRef::new("/tmp/b.png")), &fade());
        slot.set(Some(WallpaperRef::new("/tmp/c.png")), &fade());

        assert_eq!(slot.outgoing(), Some(&WallpaperRef::new("/tmp/a.png")));
        assert_eq!(slot.current(), Some(&WallpaperRef::new("/tmp/c.png")));
    }

    #[test]
    fn a_restart_late_in_a_fade_keeps_the_image_that_replaced_it() {
        let mut slot = showing_a();
        slot.set(Some(WallpaperRef::new("/tmp/b.png")), &fade());
        slot.tick(fade().tween.duration * 0.75);
        slot.set(Some(WallpaperRef::new("/tmp/c.png")), &fade());

        assert_eq!(slot.outgoing(), Some(&WallpaperRef::new("/tmp/b.png")));
        assert_eq!(slot.current(), Some(&WallpaperRef::new("/tmp/c.png")));
    }

    #[test]
    fn a_fade_lands_exactly_on_the_incoming_wallpaper() {
        let mut slot = showing_a();
        slot.set(Some(WallpaperRef::new("/tmp/b.png")), &fade());
        assert_eq!(slot.tick(fade().tween.duration), Motion::Settled);

        assert_eq!(slot.fade(), 1.0, "any residue would leave the outgoing image visible");
        assert!(slot.outgoing().is_none());
    }
}
