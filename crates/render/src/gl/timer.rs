use glow::HasContext;
use tracing::warn;

/// Without this there is no timer at all. Its EXT entry points are aliases of the core
/// ones on GLES 3.0, which is why the calls below are the plain ones.
pub const EXTENSION: &str = "GL_EXT_disjoint_timer_query";

/// `GL_GPU_DISJOINT_EXT`. Set when the GPU was interrupted during a query, which makes
/// that query's result meaningless. Reading it clears it.
const GPU_DISJOINT: u32 = 0x8FBB;

/// What the GPU spent drawing one output.
///
/// The query returns nanoseconds; microseconds are what leaves this crate, matching the
/// unit every duration crosses the control socket in. A frame costs hundreds of them, so
/// the division loses nothing worth keeping.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrameCost {
    last_ns: Option<u64>,
    peak_ns: Option<u64>,
}

impl FrameCost {
    fn record(&mut self, ns: u64) {
        self.last_ns = Some(ns);
        self.peak_ns = Some(self.peak_ns.unwrap_or(0).max(ns));
    }

    pub fn last_us(self) -> Option<u64> {
        self.last_ns.map(|ns| ns / 1_000)
    }

    /// The highest frame since startup. This is the number the budget is about: an
    /// average hides exactly the frame that missed.
    pub fn peak_us(self) -> Option<u64> {
        self.peak_ns.map(|ns| ns / 1_000)
    }
}

/// Times one output's frames on the GPU.
///
/// One query object and one flag, rather than a rotating pair: a result that is not ready
/// yet costs this frame's sample and nothing else. Reading it before it is ready would
/// wait for the GPU, which would destroy the very thing being measured.
pub struct Timer {
    query: glow::Query,
    /// A query has been ended and its result not taken yet.
    pending: bool,
    /// Cleared after the first bracket has been checked. Accepting `TIME_ELAPSED` at the
    /// core entry point is the extension's aliasing rule rather than a core guarantee, and
    /// a driver that refuses it does so silently, so it is checked once. Only once:
    /// `glGetError` is a round trip to the driver.
    unverified: bool,
    cost: FrameCost,
}

impl Timer {
    pub fn new(gl: &glow::Context) -> Option<Self> {
        let query = unsafe { gl.create_query() };
        match query {
            Ok(query) => {
                Some(Self { query, pending: false, unverified: true, cost: FrameCost::default() })
            }
            Err(error) => {
                warn!(%error, "no GPU timer for this output");
                None
            }
        }
    }

    /// Draws with the timer around it, and reports whether the timer survived.
    ///
    /// `draw` runs on every path, so measuring never decides what reaches the screen.
    pub fn measure(&mut self, gl: &glow::Context, draw: impl FnOnce()) -> bool {
        if self.pending {
            draw();
            return true;
        }

        if self.unverified {
            // So that the check below sees this bracket's error and not an older one.
            while unsafe { gl.get_error() } != glow::NO_ERROR {}
        }

        unsafe { gl.begin_query(glow::TIME_ELAPSED, self.query) };
        draw();
        unsafe { gl.end_query(glow::TIME_ELAPSED) };
        self.pending = true;

        !self.unverified || self.verify(gl)
    }

    /// Takes the result if the GPU has finished with it, and leaves it alone otherwise.
    pub fn collect(&mut self, gl: &glow::Context) {
        if !self.pending {
            return;
        }
        unsafe {
            if gl.get_query_parameter_u32(self.query, glow::QUERY_RESULT_AVAILABLE) == 0 {
                return;
            }
            let elapsed = gl.get_query_parameter_u64(self.query, glow::QUERY_RESULT);
            self.pending = false;
            if gl.get_parameter_i32(GPU_DISJOINT) == 0 {
                self.cost.record(elapsed);
            }
        }
    }

    pub fn cost(&self) -> FrameCost {
        self.cost
    }

    pub fn destroy(self, gl: &glow::Context) {
        unsafe { gl.delete_query(self.query) };
    }

    fn verify(&mut self, gl: &glow::Context) -> bool {
        self.unverified = false;
        let error = unsafe { gl.get_error() };
        if error == glow::NO_ERROR {
            return true;
        }
        warn!(
            error = format!("{error:#06x}"),
            "the driver refused a GPU timer query, so frame cost will not be reported"
        );
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_measured_reports_nothing() {
        let cost = FrameCost::default();
        assert_eq!(cost.last_us(), None);
        assert_eq!(cost.peak_us(), None, "a peak of zero would read as a measured zero");
    }

    #[test]
    fn the_peak_outlives_the_frame_that_set_it() {
        let mut cost = FrameCost::default();
        cost.record(400_000);
        cost.record(1_900_000);
        cost.record(410_000);
        assert_eq!(cost.last_us(), Some(410));
        assert_eq!(cost.peak_us(), Some(1_900));
    }

    #[test]
    fn a_sample_shorter_than_a_microsecond_is_not_a_missing_one() {
        let mut cost = FrameCost::default();
        cost.record(900);
        assert_eq!(cost.last_us(), Some(0));
    }
}
