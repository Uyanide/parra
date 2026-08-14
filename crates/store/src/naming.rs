use domain::OutputId;

/// Extension of a cached wallpaper.
const EXTENSION: &str = "qoi";

/// Slot name for the wallpaper that every output without one of its own shows.
const GLOBAL: &str = "global";

/// Filename of one cached wallpaper.
///
/// The connector is only there to make the directory readable; uniqueness comes from the
/// epoch, which is allocated once per `set` across every slot. That is what stops a file
/// written for an earlier wallpaper being taken for the current one.
pub fn cache_file(output: Option<&OutputId>, epoch: u64) -> String {
    let slot = output.map_or_else(|| GLOBAL.to_owned(), |id| sanitize(id.as_str()));
    format!("{slot}-{epoch}.{EXTENSION}")
}

/// Reduces arbitrary text to something that can only ever be one path component.
///
/// Compositors report connector names verbatim, and nothing promises they are tame.
pub fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '-' })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn a_connector_name_cannot_escape_the_filename() {
        let name = cache_file(Some(&OutputId::new("../../etc/DP-1")), 3);
        assert_eq!(Path::new(&name).components().count(), 1, "{name}");
        assert!(!name.contains('/'), "{name}");
    }

    #[test]
    fn each_epoch_gets_its_own_file() {
        let output = OutputId::new("DP-1");
        assert_ne!(cache_file(Some(&output), 3), cache_file(Some(&output), 4));
        assert_ne!(cache_file(None, 3), cache_file(Some(&output), 3));
        assert_eq!(cache_file(None, 3), "global-3.qoi");
    }

    #[test]
    fn a_display_path_survives_as_one_component() {
        assert_eq!(sanitize("wayland-1"), "wayland-1");
        assert!(!sanitize("/run/user/1000/wayland-1").contains('/'));
    }
}
