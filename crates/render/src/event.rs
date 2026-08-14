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
    /// Nothing could be shown for this wallpaper: neither a cached copy nor the source
    /// itself would load. Why is already in the log.
    WallpaperFailed {
        wallpaper: WallpaperRef,
    },
}
