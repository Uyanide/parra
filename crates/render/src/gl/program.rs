use domain::{Rgba, UvRect};
use glow::HasContext;

use crate::error::RenderError;
use crate::gl::texture::Texture;

const VERTEX_SOURCE: &str = include_str!("shaders/composite.vert");
const FRAGMENT_SOURCE: &str = include_str!("shaders/composite.frag");

/// Texture units, fixed so the sampler uniforms can be set once at link time.
const UNIT_SHARP: u32 = 0;
const UNIT_BLURRED: u32 = 1;
const UNIT_SHARP_PREVIOUS: u32 = 2;
const UNIT_BLURRED_PREVIOUS: u32 = 3;

/// What to draw for one output in one frame.
pub struct Frame<'a> {
    pub sharp: &'a Texture,
    /// The baked blur, or the sharp texture where nothing has been baked. `blur` is then
    /// zero and the shader never samples it.
    pub blurred: &'a Texture,
    pub previous: Option<(&'a Texture, &'a Texture)>,
    pub uv: UvRect,
    pub uv_previous: UvRect,
    pub blur: f32,
    /// 0 shows the outgoing wallpaper, 1 the current one.
    pub mix: f32,
    pub tint: Rgba,
}

/// The one program that draws a wallpaper.
///
/// A crossfade is the same pass with a second pair of textures bound. Both layers share
/// the effect uniforms, which describe the output rather than the image.
pub struct Composite {
    program: glow::Program,
    vao: glow::VertexArray,
    uv: Option<glow::UniformLocation>,
    uv_previous: Option<glow::UniformLocation>,
    blur: Option<glow::UniformLocation>,
    mix: Option<glow::UniformLocation>,
    tint: Option<glow::UniformLocation>,
}

impl Composite {
    pub fn new(gl: &glow::Context) -> Result<Self, RenderError> {
        unsafe {
            let program = link(gl, VERTEX_SOURCE, FRAGMENT_SOURCE)?;
            gl.use_program(Some(program));
            for (name, unit) in [
                ("u_sharp", UNIT_SHARP),
                ("u_blurred", UNIT_BLURRED),
                ("u_sharp_p", UNIT_SHARP_PREVIOUS),
                ("u_blurred_p", UNIT_BLURRED_PREVIOUS),
            ] {
                let location = gl.get_uniform_location(program, name);
                gl.uniform_1_i32(location.as_ref(), unit as i32);
            }

            // GLES 3.0 draws from gl_VertexID with no attributes, but still wants a
            // vertex array object bound.
            let vao = gl.create_vertex_array().map_err(RenderError::Gl)?;

            Ok(Self {
                uv: gl.get_uniform_location(program, "u_uv"),
                uv_previous: gl.get_uniform_location(program, "u_uv_p"),
                blur: gl.get_uniform_location(program, "u_blur"),
                mix: gl.get_uniform_location(program, "u_mix"),
                tint: gl.get_uniform_location(program, "u_tint"),
                program,
                vao,
            })
        }
    }

    pub fn draw(&self, gl: &glow::Context, frame: &Frame<'_>) {
        let (previous_sharp, previous_blurred) =
            frame.previous.unwrap_or((frame.sharp, frame.blurred));

        unsafe {
            gl.use_program(Some(self.program));
            gl.bind_vertex_array(Some(self.vao));

            frame.sharp.bind(gl, UNIT_SHARP);
            frame.blurred.bind(gl, UNIT_BLURRED);
            previous_sharp.bind(gl, UNIT_SHARP_PREVIOUS);
            previous_blurred.bind(gl, UNIT_BLURRED_PREVIOUS);

            set_rect(gl, self.uv.as_ref(), frame.uv);
            set_rect(gl, self.uv_previous.as_ref(), frame.uv_previous);
            gl.uniform_1_f32(self.blur.as_ref(), frame.blur);
            gl.uniform_1_f32(self.mix.as_ref(), frame.mix);
            let tint = frame.tint;
            gl.uniform_4_f32(self.tint.as_ref(), tint.r, tint.g, tint.b, tint.a);

            gl.draw_arrays(glow::TRIANGLES, 0, 3);
            gl.bind_vertex_array(None);
        }
    }
}

fn set_rect(gl: &glow::Context, location: Option<&glow::UniformLocation>, rect: UvRect) {
    unsafe { gl.uniform_4_f32(location, rect.u0, rect.v0, rect.u1, rect.v1) };
}

pub(super) unsafe fn link(
    gl: &glow::Context,
    vertex_source: &str,
    fragment_source: &str,
) -> Result<glow::Program, RenderError> {
    unsafe {
        let vertex = compile(gl, glow::VERTEX_SHADER, "vertex", vertex_source)?;
        let fragment = compile(gl, glow::FRAGMENT_SHADER, "fragment", fragment_source)?;

        let program = gl.create_program().map_err(RenderError::Gl)?;
        gl.attach_shader(program, vertex);
        gl.attach_shader(program, fragment);
        gl.link_program(program);

        // The shaders are owned by the program once attached; detaching lets the driver
        // release their sources immediately.
        gl.detach_shader(program, vertex);
        gl.detach_shader(program, fragment);
        gl.delete_shader(vertex);
        gl.delete_shader(fragment);

        if !gl.get_program_link_status(program) {
            let log = gl.get_program_info_log(program);
            gl.delete_program(program);
            return Err(RenderError::ProgramLink(log));
        }
        Ok(program)
    }
}

unsafe fn compile(
    gl: &glow::Context,
    kind: u32,
    stage: &'static str,
    source: &str,
) -> Result<glow::Shader, RenderError> {
    unsafe {
        let shader = gl.create_shader(kind).map_err(RenderError::Gl)?;
        gl.shader_source(shader, source);
        gl.compile_shader(shader);
        if !gl.get_shader_compile_status(shader) {
            let log = gl.get_shader_info_log(shader);
            gl.delete_shader(shader);
            return Err(RenderError::ShaderCompile { stage, log });
        }
        Ok(shader)
    }
}
