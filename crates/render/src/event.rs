use domain::{LogicalSize, OutputId, PixelSize, Scale, WallpaperRef};

/// What the display side tells the daemon. Everything else stays inside this crate.
///
/// Two of these come from the compositor and two from the decode thread, which is why
/// they live here rather than under `wayland`: a wallpaper that would not load is as much
/// something the daemon has to react to as a monitor that went away.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RenderEvent {
    /// A monitor appeared, or its geometry changed. Carries the current values, so the
    /// daemon never has to ask.
    OutputReady {
        id: OutputId,
        logical: LogicalSize,
        scale: Scale,
    },
    OutputGone {
        id: OutputId,
    },
    /// The compositor is ready for another frame on this output.
    FrameDue {
        id: OutputId,
    },
    /// A resized copy of this wallpaper was written, for a request of this size.
    WallpaperStored {
        wallpaper: WallpaperRef,
        asked: PixelSize,
    },
    /// This wallpaper now has resident dimensions. Reported by `Renderer::sync`, which
    /// runs before anything is drawn, so an output showing it can measure `max-shift`
    /// against it and place the image before the first frame that contains it.
    WallpaperReady {
        wallpaper: WallpaperRef,
    },
    /// Nothing could be shown for this wallpaper: neither a cached copy nor the source
    /// itself would load. Why is already in the log.
    WallpaperFailed {
        wallpaper: WallpaperRef,
    },
}
