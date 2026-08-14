use std::path::{Path, PathBuf};

use crate::anim::{Animated, Motion};
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

    /// Starts showing `next`. Returns whether that changed anything, since asking for the
    /// wallpaper already in the slot is not a transition.
    pub fn set(&mut self, next: Option<WallpaperRef>, transition: &TransitionParams) -> bool {
        if self.current == next {
            return false;
        }
        let tween = transition.effective_tween();
        if tween.is_instant() || self.current.is_none() || next.is_none() {
            self.outgoing = None;
            self.current = next;
            self.fade.snap(1.0);
        } else {
            self.outgoing = self.current.take();
            self.current = next;
            self.fade.snap(0.0);
            self.fade.retarget(1.0, tween);
        }
        true
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
        assert!(slot.set(Some(WallpaperRef::at("/tmp/a.png", 1)), &TransitionParams::INSTANT));
        assert!(slot.set(Some(WallpaperRef::at("/tmp/a.png", 2)), &TransitionParams::INSTANT));
    }
}
