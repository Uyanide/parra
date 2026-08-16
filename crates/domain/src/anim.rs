use std::ops::BitOr;

use serde::{Deserialize, Serialize};

/// Converts a config duration to the seconds the animation layer works in.
pub fn seconds_from_millis(millis: u32) -> f32 {
    millis as f32 / 1000.0
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Easing {
    Linear,
    OutQuad,
    InOutQuad,
    OutCubic,
    InOutCubic,
    OutQuint,
}

impl Easing {
    /// Maps normalized time to normalized progress. `t` is assumed clamped to `0..=1`.
    pub fn eval(self, t: f32) -> f32 {
        match self {
            Easing::Linear => t,
            Easing::OutQuad => 1.0 - (1.0 - t) * (1.0 - t),
            Easing::InOutQuad => {
                if t < 0.5 {
                    2.0 * t * t
                } else {
                    1.0 - (-2.0 * t + 2.0).powi(2) / 2.0
                }
            }
            Easing::OutCubic => 1.0 - (1.0 - t).powi(3),
            Easing::InOutCubic => {
                if t < 0.5 {
                    4.0 * t * t * t
                } else {
                    1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
                }
            }
            Easing::OutQuint => 1.0 - (1.0 - t).powi(5),
        }
    }
}

/// How long a move takes and the shape it takes it in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tween {
    pub duration: f32,
    pub easing: Easing,
}

impl Tween {
    /// No animation at all: the value is wherever it was last put.
    pub const INSTANT: Tween = Tween { duration: 0.0, easing: Easing::Linear };

    pub const fn new(duration: f32, easing: Easing) -> Self {
        Self { duration, easing }
    }

    pub fn is_instant(&self) -> bool {
        self.duration <= 0.0
    }
}

/// A move that has just been started, for whoever has to report one. A snap reports too,
/// with an instant tween, since a jump is what a client is meant to reproduce there.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Move {
    pub from: f32,
    pub to: f32,
    pub tween: Tween,
}

/// Whether anything is still moving.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Motion {
    Settled,
    Running,
}

impl Motion {
    pub fn is_running(self) -> bool {
        self == Motion::Running
    }
}

impl BitOr for Motion {
    type Output = Motion;

    fn bitor(self, rhs: Motion) -> Motion {
        if self.is_running() || rhs.is_running() { Motion::Running } else { Motion::Settled }
    }
}

/// A scalar easing toward a target over a fixed duration.
///
/// Holds no clock: elapsed time is supplied from outside via [`Animated::tick`].
#[derive(Clone, Copy, Debug)]
pub struct Animated {
    from: f32,
    to: f32,
    elapsed: f32,
    tween: Tween,
}

impl Animated {
    pub fn new(value: f32) -> Self {
        Self { from: value, to: value, elapsed: 0.0, tween: Tween::INSTANT }
    }

    pub fn value(&self) -> f32 {
        if self.tween.is_instant() {
            return self.to;
        }
        let t = (self.elapsed / self.tween.duration).clamp(0.0, 1.0);
        self.from + (self.to - self.from) * self.tween.easing.eval(t)
    }

    pub fn target(&self) -> f32 {
        self.to
    }

    pub fn is_settled(&self) -> bool {
        self.elapsed >= self.tween.duration
    }

    /// Jumps to `value` with no animation. Used for initial state and for instant modes.
    pub fn snap(&mut self, value: f32) {
        self.from = value;
        self.to = value;
        self.elapsed = 0.0;
        self.tween = Tween::INSTANT;
    }

    /// Aims at a new target starting from the *current* value, so redirecting mid-flight
    /// never jumps. Re-requesting the target already in flight is a no-op.
    ///
    /// Returns the move it started, which is `None` for that no-op. Deciding here rather
    /// than by comparing values afterwards keeps one answer to "did an animation begin".
    pub fn retarget(&mut self, to: f32, tween: Tween) -> Option<Move> {
        if self.to == to && self.tween.easing == tween.easing {
            return None;
        }
        let from = self.value();
        if tween.is_instant() || from == to {
            // Landing where it already rests is not a move, however the curve changed.
            // Landing there from mid-flight is one: whoever followed it must stop too.
            let moved = from != to || !self.is_settled();
            self.snap(to);
            return moved.then_some(Move { from, to, tween: Tween::INSTANT });
        }
        self.from = from;
        self.to = to;
        self.elapsed = 0.0;
        self.tween = tween;
        Some(Move { from, to, tween })
    }

    pub fn tick(&mut self, dt: f32) -> Motion {
        if self.is_settled() {
            return Motion::Settled;
        }
        self.elapsed = (self.elapsed + dt.max(0.0)).min(self.tween.duration);
        if self.is_settled() { Motion::Settled } else { Motion::Running }
    }
}

impl Default for Animated {
    fn default() -> Self {
        Self::new(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPS: f32 = 1e-5;

    #[test]
    fn easings_span_the_unit_interval() {
        for easing in [
            Easing::Linear,
            Easing::OutQuad,
            Easing::InOutQuad,
            Easing::OutCubic,
            Easing::InOutCubic,
            Easing::OutQuint,
        ] {
            assert!((easing.eval(0.0) - 0.0).abs() < EPS, "{easing:?} at 0");
            assert!((easing.eval(1.0) - 1.0).abs() < EPS, "{easing:?} at 1");
        }
    }

    #[test]
    fn easings_are_monotonic() {
        for easing in [Easing::OutQuad, Easing::InOutCubic, Easing::OutQuint] {
            let mut previous = f32::NEG_INFINITY;
            for step in 0..=100 {
                let value = easing.eval(step as f32 / 100.0);
                assert!(value >= previous, "{easing:?} dipped at {step}");
                previous = value;
            }
        }
    }

    #[test]
    fn a_fresh_value_is_already_settled() {
        let mut animated = Animated::new(0.5);
        assert!(animated.is_settled());
        assert_eq!(animated.tick(1.0), Motion::Settled);
        assert_eq!(animated.value(), 0.5);
    }

    #[test]
    fn reaches_the_target_exactly() {
        let mut animated = Animated::new(0.0);
        animated.retarget(1.0, Tween::new(0.4, Easing::OutCubic));
        assert_eq!(animated.tick(0.2), Motion::Running);
        assert_eq!(animated.tick(0.2), Motion::Settled);
        assert_eq!(animated.value(), 1.0);
    }

    #[test]
    fn overshooting_dt_does_not_overshoot_the_value() {
        let mut animated = Animated::new(0.0);
        animated.retarget(1.0, Tween::new(0.4, Easing::Linear));
        assert_eq!(animated.tick(99.0), Motion::Settled);
        assert_eq!(animated.value(), 1.0);
    }

    #[test]
    fn retargeting_mid_flight_starts_from_the_current_value() {
        let mut animated = Animated::new(0.0);
        animated.retarget(1.0, Tween::new(1.0, Easing::Linear));
        animated.tick(0.5);
        let midpoint = animated.value();
        assert!((midpoint - 0.5).abs() < EPS);

        animated.retarget(0.0, Tween::new(1.0, Easing::Linear));
        assert!((animated.value() - midpoint).abs() < EPS, "value jumped on retarget");
    }

    #[test]
    fn retargeting_to_the_active_target_does_not_restart() {
        let mut animated = Animated::new(0.0);
        animated.retarget(1.0, Tween::new(1.0, Easing::Linear));
        animated.tick(0.5);
        animated.retarget(1.0, Tween::new(1.0, Easing::Linear));
        assert!((animated.value() - 0.5).abs() < EPS);
    }

    #[test]
    fn zero_duration_snaps() {
        let mut animated = Animated::new(0.0);
        animated.retarget(1.0, Tween::new(0.0, Easing::OutCubic));
        assert!(animated.is_settled());
        assert_eq!(animated.value(), 1.0);
    }

    #[test]
    fn a_started_move_is_reported_whole() {
        let tween = Tween::new(0.4, Easing::OutCubic);
        let mut animated = Animated::new(0.25);
        assert_eq!(animated.retarget(1.0, tween), Some(Move { from: 0.25, to: 1.0, tween }));
    }

    #[test]
    fn a_retarget_that_changes_nothing_reports_nothing() {
        let tween = Tween::new(1.0, Easing::Linear);
        let mut animated = Animated::new(0.0);
        animated.retarget(1.0, tween);
        animated.tick(0.5);
        assert_eq!(animated.retarget(1.0, tween), None);
    }

    #[test]
    fn a_snap_reports_a_move_of_no_duration() {
        let mut animated = Animated::new(0.0);
        let started = animated.retarget(1.0, Tween::new(0.0, Easing::OutCubic));
        assert_eq!(started, Some(Move { from: 0.0, to: 1.0, tween: Tween::INSTANT }));
    }

    #[test]
    fn settling_where_it_already_rests_reports_nothing_even_on_a_new_curve() {
        let mut animated = Animated::new(1.0);
        assert_eq!(animated.retarget(1.0, Tween::new(0.4, Easing::OutQuint)), None);
    }

    #[test]
    fn a_move_cut_short_is_reported_so_a_follower_stops_too() {
        let mut animated = Animated::new(0.0);
        animated.retarget(1.0, Tween::new(1.0, Easing::Linear));
        animated.tick(0.5);

        let started = animated.retarget(animated.value(), Tween::INSTANT);
        assert_eq!(started.map(|started| started.tween), Some(Tween::INSTANT));
    }

    #[test]
    fn a_move_started_mid_flight_reports_where_it_actually_starts() {
        let tween = Tween::new(1.0, Easing::Linear);
        let mut animated = Animated::new(0.0);
        animated.retarget(1.0, tween);
        animated.tick(0.5);

        let Some(started) = animated.retarget(0.0, tween) else { panic!("that is a new move") };
        assert!(
            (started.from - 0.5).abs() < EPS,
            "reporting the previous origin would put a listener half an animation out"
        );
    }

    #[test]
    fn motion_merges_as_a_disjunction() {
        assert_eq!(Motion::Settled | Motion::Settled, Motion::Settled);
        assert_eq!(Motion::Settled | Motion::Running, Motion::Running);
        assert_eq!(Motion::Running | Motion::Settled, Motion::Running);
    }
}
