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
}

impl WallpaperSlot {
    pub fn new() -> Self {
        Self { outgoing: None, current: None, fade: Animated::new(1.0) }
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

    /// Starts showing `next`. Returns the swap it made, and `None` when nothing changed,
    /// since asking for the wallpaper already in the slot is not a transition.
    ///
    /// The tween reported is the one actually used: an empty slot, an emptied one and a
    /// mode that swaps outright are all instant, and only this knows which happened.
    pub fn set(
        &mut self,
        next: Option<WallpaperRef>,
        transition: &TransitionParams,
    ) -> Option<Swap> {
        if self.current == next {
            return None;
        }
        let mut tween = transition.effective_tween();
        // An empty slot and an emptied one swap outright whatever the mode asks for, since
        // there is nothing to crossfade against.
        if self.current.is_none() || next.is_none() {
            tween = Tween::INSTANT;
        }

        let previous = self.current.clone();
        if tween.is_instant() {
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
            self.fade.retarget(1.0, tween);
        }

        // What the viewer watches leave: the slot kept to fade out, or whatever was on
        // screen when none was kept.
        Some(Swap { from: self.outgoing.clone().or(previous), to: next, tween })
    }

    pub fn tick(&mut self, dt: f32) -> Motion {
        let motion = self.fade.tick(dt);
        if !motion.is_running() {
            self.outgoing = None;
        }
        motion
    }

    pub fn is_settled(&self) -> bool {
        self.fade.is_settled()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;
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
    fn the_first_wallpaper_arrives_without_a_transition() {
        let mut slot = WallpaperSlot::new();
        let swap = slot.set(Some(WallpaperRef::new("/tmp/a.png")), &fade()).unwrap();

        assert_eq!(swap.from, None);
        assert_eq!(swap.to, Some(WallpaperRef::new("/tmp/a.png")));
        assert!(swap.tween.is_instant(), "there is nothing to crossfade against");
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
