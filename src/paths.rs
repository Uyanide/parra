use std::env;
use std::path::PathBuf;

use thiserror::Error;

/// Filename inside the config directory.
pub const CONFIG_FILE: &str = "config.toml";

/// Fallback display name.
const DEFAULT_WAYLAND_DISPLAY: &str = "wayland-0";

#[derive(Debug, Error)]
pub enum PathError {
    #[error("neither XDG_CONFIG_HOME nor HOME is set, so the config file cannot be located")]
    NoConfigHome,
    #[error("XDG_RUNTIME_DIR is not set, so the control socket cannot be located")]
    NoRuntimeDir,
}

/// Everywhere the program touches the filesystem, derived from the name it was given.
#[derive(Clone, Debug)]
pub struct Paths {
    pub config: PathBuf,
    pub socket: PathBuf,
}

impl Paths {
    /// Applies explicit overrides on top of the derived defaults.
    pub fn resolve(
        name: &str,
        config: Option<PathBuf>,
        socket: Option<PathBuf>,
    ) -> Result<Self, PathError> {
        Ok(Self {
            config: match config {
                Some(path) => path,
                None => config_file(name)?,
            },
            socket: match socket {
                Some(path) => path,
                None => socket_path(name)?,
            },
        })
    }
}

/// `$XDG_CONFIG_HOME/<name>/config.toml`, falling back to `$HOME/.config` as the XDG
/// specification prescribes.
pub fn config_file(name: &str) -> Result<PathBuf, PathError> {
    let base = env::var_os("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .ok_or(PathError::NoConfigHome)?;
    Ok(base.join(name).join(CONFIG_FILE))
}

/// `$XDG_RUNTIME_DIR/<name>-<display>.sock`.
pub fn socket_path(name: &str) -> Result<PathBuf, PathError> {
    let dir = env::var_os("XDG_RUNTIME_DIR")
        .filter(|value| !value.is_empty())
        .ok_or(PathError::NoRuntimeDir)?;
    let display = env::var("WAYLAND_DISPLAY").unwrap_or_default();
    let display = sanitize(&display);
    Ok(PathBuf::from(dir).join(format!("{name}-{display}.sock")))
}

/// `WAYLAND_DISPLAY` may be an absolute path, which cannot appear inside a filename.
fn sanitize(display: &str) -> String {
    if display.is_empty() {
        return DEFAULT_WAYLAND_DISPLAY.to_owned();
    }
    display
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '-' })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unset_display_matches_the_wayland_default() {
        assert_eq!(sanitize(""), DEFAULT_WAYLAND_DISPLAY);
    }

    #[test]
    fn a_display_path_cannot_escape_the_filename() {
        assert!(!sanitize("/run/user/1000/wayland-1").contains('/'));
        assert_eq!(sanitize("wayland-1"), "wayland-1");
    }
}
