use std::ops::RangeInclusive;

use wayland_client::globals::{BindError, GlobalList};
use wayland_client::protocol::wl_compositor::WlCompositor;
use wayland_client::{Dispatch, Proxy, QueueHandle};
use wayland_protocols::wp::fractional_scale::v1::client::wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1;
use wayland_protocols::wp::viewporter::client::wp_viewporter::WpViewporter;
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::ZwlrLayerShellV1;

use crate::error::RenderError;

/// Version of `wl_output` that reports connector names. Required: without one there is
/// no identity to key a config table or an output's state on.
pub const OUTPUT_VERSION: RangeInclusive<u32> = 4..=4;

/// Every global the renderer needs, bound once at startup.
///
/// All four are required. Degrading silently past a missing one would put a blurry or
/// misplaced wallpaper on screen with nothing to explain it.
pub struct Globals {
    pub compositor: WlCompositor,
    pub layer_shell: ZwlrLayerShellV1,
    pub viewporter: WpViewporter,
    pub fractional_scale: WpFractionalScaleManagerV1,
}

impl Globals {
    pub fn bind<D>(globals: &GlobalList, qh: &QueueHandle<D>) -> Result<Self, RenderError>
    where
        D: Dispatch<WlCompositor, ()>
            + Dispatch<ZwlrLayerShellV1, ()>
            + Dispatch<WpViewporter, ()>
            + Dispatch<WpFractionalScaleManagerV1, ()>
            + 'static,
    {
        Ok(Self {
            compositor: require(globals, qh, 4..=6)?,
            layer_shell: require(globals, qh, 1..=5)?,
            viewporter: require(globals, qh, 1..=1)?,
            fractional_scale: require(globals, qh, 1..=1)?,
        })
    }
}

fn require<I, D>(
    globals: &GlobalList,
    qh: &QueueHandle<D>,
    version: RangeInclusive<u32>,
) -> Result<I, RenderError>
where
    I: Proxy + 'static,
    D: Dispatch<I, ()> + 'static,
{
    let lowest = *version.start();
    globals.bind(qh, version, ()).map_err(|error| match error {
        BindError::NotPresent | BindError::UnsupportedVersion => {
            RenderError::MissingGlobal { interface: I::interface().name, version: lowest }
        }
    })
}
