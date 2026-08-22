use std::io;
use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum RenderError {
    #[error("cannot connect to the Wayland compositor")]
    Connect {
        #[source]
        source: wayland_client::ConnectError,
    },
    #[error("cannot read the compositor's global registry")]
    Registry {
        #[source]
        source: wayland_client::globals::GlobalError,
    },
    #[error("the compositor does not offer {interface} version {version}")]
    MissingGlobal { interface: &'static str, version: u32 },
    #[error("lost the connection to the compositor")]
    Disconnected {
        #[source]
        source: wayland_client::backend::WaylandError,
    },
    #[error("the compositor rejected a Wayland request")]
    Protocol {
        #[source]
        source: wayland_client::DispatchError,
    },
    #[error("cannot create a native surface for {output}")]
    NativeSurface { output: String },

    #[error("EGL: {operation} failed")]
    Egl {
        operation: &'static str,
        #[source]
        source: khronos_egl::Error,
    },
    #[error("EGL: no configuration offers an sRGB window surface with a depth-less RGB8 buffer")]
    NoEglConfig,
    #[error("OpenGL: {0}")]
    Gl(String),
    #[error("OpenGL: {stage} shader did not compile: {log}")]
    ShaderCompile { stage: &'static str, log: String },
    #[error("OpenGL: the composite program did not link: {0}")]
    ProgramLink(String),
    #[error("OpenGL: cannot render into a blur level, framebuffer status {status:#06x}")]
    Framebuffer { status: u32 },

    #[error("cannot read {}", path.display())]
    ImageRead {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("cannot decode {}", path.display())]
    ImageDecode {
        path: PathBuf,
        #[source]
        source: image::ImageError,
    },
    #[error("cannot resize {}: {message}", path.display())]
    ImageResize { path: PathBuf, message: String },
    #[error("cannot premultiply an image: {0}")]
    Premultiply(String),
    #[error("cannot use the cached copy at {}: {message}", path.display())]
    Cache { path: PathBuf, message: String },
    #[error("cannot start the wallpaper decoder: {operation} failed")]
    Loader {
        operation: &'static str,
        #[source]
        source: io::Error,
    },
}

/// Renders an error together with what caused it.
///
/// Only the outermost message reaches a log line otherwise, and for anything to do with
/// files the cause is the half worth reading.
pub fn chain(error: &dyn std::error::Error) -> String {
    let mut text = error.to_string();
    let mut cause = error.source();
    while let Some(source) = cause {
        text.push_str(": ");
        text.push_str(&source.to_string());
        cause = source.source();
    }
    text
}
