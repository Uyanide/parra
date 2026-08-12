use std::time::Instant;

use domain::PixelSize;
use glow::HasContext;
use tracing::debug;

use crate::error::RenderError;
use crate::gl::program::link;
use crate::gl::texture::Texture;

const VERTEX_SOURCE: &str = include_str!("shaders/bake.vert");
const DOWN_SOURCE: &str = include_str!("shaders/kawase_down.frag");
const UP_SOURCE: &str = include_str!("shaders/kawase_up.frag");

/// Deepest the level chain may go. Eight halvings take even a 4K wallpaper below 16
/// pixels, past which there is nothing left to blur.
const MAX_LEVELS: u32 = 8;

/// Band the tap offset is allowed to move in. The kernel weights were derived for taps
/// roughly this far out; much further and the five samples stop overlapping, which reads
/// as a grid rather than a blur.
const OFFSET: std::ops::RangeInclusive<f32> = 1.0..=4.0;

/// Per-axis variance of the downsample kernel at tap distance 1: four of its eight
/// weights sit one tap out along each axis, so `4 * 1^2 / 8`.
const DOWN_VARIANCE: f32 = 0.5;

/// Per-axis variance of the upsample kernel at tap distance 1. Its axis taps reach twice
/// as far as the diagonals, which is why an up pass spreads more than a down pass at the
/// same level: `(2 * 2^2 + 4 * 2 * 1^2) / 12`.
const UP_VARIANCE: f32 = 4.0 / 3.0;

/// Standard deviations spanned by the radius. A Gaussian is visually finished about three
/// of them out, so `radius` reads as the extent of the blur rather than its inflection.
const SIGMAS_PER_RADIUS: f32 = 3.0;

/// How far the bake walks down the level chain and where it stops on the way back up.
///
/// Level 0 is the source, level 1 is half its size, and so on. Derived separately from
/// the passes so the radius mapping can be tested without a GPU.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Plan {
    /// Deepest level reached. Always at least `keep`.
    pub levels: u32,
    /// Level the result is kept at, which is what `downscale` asked for.
    pub keep: u32,
    /// Tap distance in half-texels of each pass's destination, scaled by 256 so the plan
    /// stays comparable and hashable.
    offset_256: u32,
}

impl Plan {
    pub fn offset(self) -> f32 {
        self.offset_256 as f32 / 256.0
    }
}

/// Chooses a level chain for a radius given in texels of the source image.
///
/// `radius` is the half-width of the Gaussian this approximates, so the visible spread is
/// about that far either side of an edge.
pub fn plan(radius: u32, downscale: u32) -> Plan {
    // Capping `keep` as well as `levels` is what makes "the chain always climbs back to
    // the level it keeps" unconditional rather than a range the caller has to respect.
    let keep = ceil_log2(downscale).min(MAX_LEVELS);
    let wanted = radius as f32 / SIGMAS_PER_RADIUS;

    // Every level doubles what its taps can reach, so the shallowest chain that can pull
    // its offset back inside the band is also the cheapest one that covers the radius.
    let mut levels = keep.max(1);
    while levels < MAX_LEVELS && offset_for(wanted, levels, keep) > *OFFSET.end() {
        levels += 1;
    }

    let offset = offset_for(wanted, levels, keep).clamp(*OFFSET.start(), *OFFSET.end());
    Plan { levels, keep, offset_256: (offset * 256.0).round() as u32 }
}

/// Spread the whole chain produces at tap distance 1, in source texels.
///
/// Taps at level `i` reach `2^(i-1)` source texels, and independent passes add variance
/// rather than distance, so this is a sum of squares. The up passes are counted
/// separately because their kernel is the wider of the two.
fn chain_spread(levels: u32, keep: u32) -> f32 {
    let reach = |level: u32| (1u32 << level) as f32 / 2.0;
    let mut variance = 0.0;
    for level in 1..=levels {
        variance += DOWN_VARIANCE * reach(level).powi(2);
    }
    for level in keep..levels {
        variance += UP_VARIANCE * reach(level).powi(2);
    }
    variance.sqrt()
}

/// Tap distance that puts this chain at the wanted spread.
fn offset_for(sigma: f32, levels: u32, keep: u32) -> f32 {
    sigma / chain_spread(levels, keep)
}

/// Smallest `n` with `2^n >= value`.
fn ceil_log2(value: u32) -> u32 {
    match value {
        0 | 1 => 0,
        _ => u32::BITS - (value - 1).leading_zeros(),
    }
}

/// The two passes that bake a blurred copy of a wallpaper.
///
/// The wallpaper is the bottom layer, so nothing behind it can change and the blur is a
/// still image. Baking it once is the whole performance argument.
pub struct Kawase {
    down: Pass,
    up: Pass,
    framebuffer: glow::Framebuffer,
    vao: glow::VertexArray,
}

struct Pass {
    program: glow::Program,
    halfpixel: Option<glow::UniformLocation>,
    offset: Option<glow::UniformLocation>,
}

impl Kawase {
    pub fn new(gl: &glow::Context) -> Result<Self, RenderError> {
        unsafe {
            Ok(Self {
                down: Pass::new(gl, DOWN_SOURCE)?,
                up: Pass::new(gl, UP_SOURCE)?,
                framebuffer: gl.create_framebuffer().map_err(RenderError::Gl)?,
                vao: gl.create_vertex_array().map_err(RenderError::Gl)?,
            })
        }
    }

    /// Renders `source` through the level chain and returns the result at `plan.keep`.
    ///
    /// Every intermediate level is freed before returning, so the lasting cost is one
    /// texture at the configured downscale.
    pub fn bake(
        &self,
        gl: &glow::Context,
        source: &Texture,
        plan: Plan,
    ) -> Result<Texture, RenderError> {
        let started = Instant::now();
        let mut levels = Levels::allocate(gl, source.size(), plan)?;

        unsafe {
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.framebuffer));
            gl.bind_vertex_array(Some(self.vao));
            gl.active_texture(glow::TEXTURE0);

            for level in 1..=plan.levels {
                let from = if level == 1 { source } else { levels.get(level - 1) };
                self.run(gl, &self.down, from, levels.get(level), plan)?;
            }
            for level in (plan.keep..plan.levels).rev() {
                let from = levels.get(level + 1);
                // Reading level+1 while writing level: distinct textures, so the up chain
                // can overwrite what the down chain left without a second set of buffers.
                self.run(gl, &self.up, from, levels.get(level), plan)?;
            }

            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            gl.bind_vertex_array(None);
            gl.bind_texture(glow::TEXTURE_2D, None);
            // The bake happens once per wallpaper, so stalling here to time it honestly
            // costs nothing and the number is the only evidence for the frame budget.
            gl.finish();
        }

        let result = levels.take(plan.keep);
        levels.release(gl);
        debug!(
            levels = plan.levels,
            keep = plan.keep,
            offset = plan.offset(),
            width = result.size().w,
            height = result.size().h,
            elapsed = ?started.elapsed(),
            "baked blur"
        );
        Ok(result)
    }

    unsafe fn run(
        &self,
        gl: &glow::Context,
        pass: &Pass,
        from: &Texture,
        into: &Texture,
        plan: Plan,
    ) -> Result<(), RenderError> {
        let size = into.size();
        unsafe {
            gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(into.id()),
                0,
            );
            let status = gl.check_framebuffer_status(glow::FRAMEBUFFER);
            if status != glow::FRAMEBUFFER_COMPLETE {
                return Err(RenderError::Framebuffer { status });
            }

            gl.use_program(Some(pass.program));
            gl.bind_texture(glow::TEXTURE_2D, Some(from.id()));
            gl.viewport(0, 0, size.w as i32, size.h as i32);
            gl.uniform_2_f32(pass.halfpixel.as_ref(), 0.5 / size.w as f32, 0.5 / size.h as f32);
            gl.uniform_1_f32(pass.offset.as_ref(), plan.offset());
            gl.draw_arrays(glow::TRIANGLES, 0, 3);
        }
        Ok(())
    }
}

impl Pass {
    unsafe fn new(gl: &glow::Context, fragment: &str) -> Result<Self, RenderError> {
        unsafe {
            let program = link(gl, VERTEX_SOURCE, fragment)?;
            gl.use_program(Some(program));
            let source = gl.get_uniform_location(program, "u_source");
            gl.uniform_1_i32(source.as_ref(), 0);
            Ok(Self {
                halfpixel: gl.get_uniform_location(program, "u_halfpixel"),
                offset: gl.get_uniform_location(program, "u_offset"),
                program,
            })
        }
    }
}

/// The scratch textures the chain walks through, indexed by level. Slot 0 is allocated
/// only for a downscale of 1, where the result has to land back at full size.
struct Levels {
    slots: Vec<Option<Texture>>,
}

impl Levels {
    fn allocate(gl: &glow::Context, source: PixelSize, plan: Plan) -> Result<Self, RenderError> {
        let mut slots = Vec::with_capacity(plan.levels as usize + 1);
        for level in 0..=plan.levels {
            let needed = level > 0 || plan.keep == 0;
            let texture = needed.then(|| Texture::empty(gl, halve(source, level))).transpose();
            match texture {
                Ok(texture) => slots.push(texture),
                Err(error) => {
                    Self { slots }.release(gl);
                    return Err(error);
                }
            }
        }
        Ok(Self { slots })
    }

    fn get(&self, level: u32) -> &Texture {
        self.slots[level as usize].as_ref().expect("the chain only reads levels it allocated")
    }

    fn take(&mut self, level: u32) -> Texture {
        self.slots[level as usize].take().expect("the kept level is always allocated")
    }

    fn release(self, gl: &glow::Context) {
        for texture in self.slots.into_iter().flatten() {
            texture.destroy(gl);
        }
    }
}

/// Size after `level` halvings, never smaller than one pixel.
fn halve(size: PixelSize, level: u32) -> PixelSize {
    let shrink = |value: u32| (value >> level.min(u32::BITS - 1)).max(1);
    PixelSize::new(shrink(size.w), shrink(size.h))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Wider than any configuration can ask for, so the properties below are checked
    /// beyond the range the config crate will actually admit.
    const WIDEST_RADIUS: u32 = 1024;
    const DEEPEST_DOWNSCALE: u32 = 32;

    /// Spread the chain produces, in source texels. Monotonicity in this is the whole
    /// contract between the configured radius and what appears on screen.
    fn spread(plan: Plan) -> f32 {
        chain_spread(plan.levels, plan.keep) * plan.offset()
    }

    #[test]
    fn a_bigger_radius_never_blurs_less() {
        for downscale in 1..=DEEPEST_DOWNSCALE {
            let mut previous = 0.0;
            for radius in 1..=WIDEST_RADIUS {
                let now = spread(plan(radius, downscale));
                assert!(now >= previous, "radius {radius} spreads {now}, less than {previous}");
                previous = now;
            }
        }
    }

    #[test]
    fn a_radius_the_chain_can_reach_is_reproduced_closely() {
        // Below the band the offset floor blurs more than asked, and above it the level
        // cap blurs less; in between the plan is expected to land on the number.
        for radius in 24..=512 {
            let asked = radius as f32 / SIGMAS_PER_RADIUS;
            let got = spread(plan(radius, 4));
            let error = (got - asked).abs() / asked;
            assert!(error < 0.01, "radius {radius} asked {asked} and got {got}");
        }
    }

    #[test]
    fn too_small_a_radius_is_widened_rather_than_faked() {
        // A downscale of four has already thrown away the detail a two-texel blur would
        // have softened, so the floor is honest about producing more than was asked.
        assert!(spread(plan(1, 4)) > 1.0 / SIGMAS_PER_RADIUS);
        assert_eq!(plan(1, 4).offset(), *OFFSET.start());
    }

    #[test]
    fn the_result_lands_at_the_configured_downscale() {
        for (downscale, keep) in [(1, 0), (2, 1), (4, 2), (8, 3), (16, 4)] {
            assert_eq!(plan(32, downscale).keep, keep, "downscale {downscale}");
        }
    }

    #[test]
    fn the_chain_is_never_shallower_than_the_level_it_keeps() {
        for downscale in 1..=DEEPEST_DOWNSCALE {
            for radius in [1, 8, 32, 200, WIDEST_RADIUS] {
                let plan = plan(radius, downscale);
                assert!(plan.levels >= plan.keep, "{plan:?} cannot climb back to its own level");
            }
        }
    }

    #[test]
    fn even_the_widest_radius_stays_within_the_level_budget() {
        assert!(plan(WIDEST_RADIUS, 1).levels <= MAX_LEVELS);
        assert!(plan(WIDEST_RADIUS, DEEPEST_DOWNSCALE).levels <= MAX_LEVELS);
    }

    #[test]
    fn the_offset_stays_in_the_band_the_kernel_was_derived_for() {
        for downscale in 1..=DEEPEST_DOWNSCALE {
            for radius in 1..=WIDEST_RADIUS {
                let offset = plan(radius, downscale).offset();
                assert!(OFFSET.contains(&offset), "radius {radius} gave offset {offset}");
            }
        }
    }

    #[test]
    fn halving_bottoms_out_at_one_pixel_rather_than_zero() {
        let size = PixelSize::new(2560, 1440);
        for level in 0..=MAX_LEVELS + 4 {
            let halved = halve(size, level);
            assert!(halved.w >= 1 && halved.h >= 1, "level {level} gave {halved:?}");
        }
    }

    #[test]
    fn ceil_log2_rounds_up_to_the_next_power_of_two() {
        for (value, expected) in [(0, 0), (1, 0), (2, 1), (3, 2), (4, 2), (5, 3), (16, 4)] {
            assert_eq!(ceil_log2(value), expected, "value {value}");
        }
    }
}
