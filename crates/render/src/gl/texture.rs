use domain::PixelSize;
use glow::HasContext;

use crate::error::RenderError;

/// An RGBA8 image living in video memory.
///
/// No mipmaps: images are resized on the CPU to roughly the size they will be drawn at,
/// so there is no minification to filter and a mip chain would cost a third more VRAM.
pub struct Texture {
    id: glow::Texture,
    size: PixelSize,
}

impl Texture {
    pub fn upload(gl: &glow::Context, size: PixelSize, rgba: &[u8]) -> Result<Self, RenderError> {
        let expected = size.area() as usize * 4;
        if rgba.len() < expected {
            return Err(RenderError::Gl(format!(
                "image buffer holds {} bytes, {expected} needed for {}x{}",
                rgba.len(),
                size.w,
                size.h
            )));
        }
        Self::allocate(gl, size, Some(&rgba[..expected]))
    }

    /// Storage with no contents, for a pass that is about to render into it.
    pub fn empty(gl: &glow::Context, size: PixelSize) -> Result<Self, RenderError> {
        Self::allocate(gl, size, None)
    }

    fn allocate(
        gl: &glow::Context,
        size: PixelSize,
        rgba: Option<&[u8]>,
    ) -> Result<Self, RenderError> {
        let size = size.max_one();
        unsafe {
            let id = gl.create_texture().map_err(RenderError::Gl)?;
            gl.bind_texture(glow::TEXTURE_2D, Some(id));
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
            // Clamping is what keeps the blur's taps from wrapping the opposite edge into
            // the image, which would show as a bright seam all the way round.
            let clamp = glow::CLAMP_TO_EDGE as i32;
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, clamp);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, clamp);
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 4);
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA8 as i32,
                size.w as i32,
                size.h as i32,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(rgba),
            );
            gl.bind_texture(glow::TEXTURE_2D, None);
            Ok(Self { id, size })
        }
    }

    pub(super) fn id(&self) -> glow::Texture {
        self.id
    }

    pub fn size(&self) -> PixelSize {
        self.size
    }

    /// Bytes of video memory this occupies, for the budget report.
    pub fn footprint(&self) -> u64 {
        self.size.area() * 4
    }

    pub fn bind(&self, gl: &glow::Context, unit: u32) {
        unsafe {
            gl.active_texture(glow::TEXTURE0 + unit);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.id));
        }
    }

    pub fn destroy(self, gl: &glow::Context) {
        unsafe { gl.delete_texture(self.id) };
    }
}
