use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::anim::{Animated, Motion};
use crate::params::TransitionParams;

/// An image to display, identified by an absolute path. Paths arrive from the config
/// file or over the control socket; what chose one is out of scope.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WallpaperRef(PathBuf);

impl WallpaperRef {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self(path.into())
    }

    pub fn path(&self) -> &Path {
        &self.0
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

    /// Starts showing `next`. Returns whether anything changed, so callers can skip the
    /// decode and upload when the same path is set again.
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
