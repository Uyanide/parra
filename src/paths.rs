use std::env;
use std::path::PathBuf;

use thiserror::Error;

/// Filename inside the config directory.
pub const CONFIG_FILE: &str = "config.toml";

/// Filename inside the state directory.
pub const STATE_FILE: &str = "state.toml";

/// Fallback display name.
const DEFAULT_WAYLAND_DISPLAY: &str = "wayland-0";

/// A location that could not be derived because the environment does not say where it
/// goes. One shape rather than one variant per location: every case is the same sentence
/// with two words changed, and no caller has ever needed to tell them apart.
#[derive(Debug, Error)]
#[error("{missing}, so {purpose} cannot be located")]
pub struct PathError {
    /// Which variables were consulted, phrased to read inside the message.
    missing: &'static str,
    purpose: &'static str,
}

impl PathError {
    fn new(missing: &'static str, purpose: &'static str) -> Self {
        Self { missing, purpose }
    }
}

/// Everywhere the program touches the filesystem, derived from the name it was given.
#[derive(Clone, Debug)]
pub struct Paths {
    pub config: PathBuf,
    pub socket: PathBuf,
    /// Which wallpaper each slot was last asked for.
    pub state: PathBuf,
    /// Resized copies of those wallpapers.
    pub cache_dir: PathBuf,
}

/// Explicit locations, each overriding what would otherwise be derived from the name.
#[derive(Clone, Debug, Default)]
pub struct Overrides {
    pub config: Option<PathBuf>,
    pub socket: Option<PathBuf>,
    pub state: Option<PathBuf>,
    pub cache_dir: Option<PathBuf>,
}

impl Paths {
    /// Applies explicit overrides on top of the derived defaults.
    pub fn resolve(name: &str, overrides: Overrides) -> Result<Self, PathError> {
        Ok(Self {
            config: match overrides.config {
                Some(path) => path,
                None => config_file(name)?,
            },
            socket: match overrides.socket {
                Some(path) => path,
                None => socket_path(name)?,
            },
            state: match overrides.state {
                Some(path) => path,
                None => state_file(name)?,
            },
            cache_dir: match overrides.cache_dir {
                Some(path) => path,
                None => cache_dir(name)?,
            },
        })
    }
}

/// `$XDG_CONFIG_HOME/<name>/config.toml`, falling back to `$HOME/.config`.
pub fn config_file(name: &str) -> Result<PathBuf, PathError> {
    let base = base_dir("XDG_CONFIG_HOME", ".config").ok_or_else(|| {
        PathError::new("neither XDG_CONFIG_HOME nor HOME is set", "the config file")
    })?;
    Ok(base.join(name).join(CONFIG_FILE))
}

/// `$XDG_STATE_HOME/<name>/state.toml`, falling back to `$HOME/.local/state`.
pub fn state_file(name: &str) -> Result<PathBuf, PathError> {
    let base = base_dir("XDG_STATE_HOME", ".local/state").ok_or_else(|| {
        PathError::new("neither XDG_STATE_HOME nor HOME is set", "the state file")
    })?;
    Ok(base.join(name).join(STATE_FILE))
}

/// `$XDG_CACHE_HOME/<name>`, falling back to `$HOME/.cache`.
pub fn cache_dir(name: &str) -> Result<PathBuf, PathError> {
    let base = base_dir("XDG_CACHE_HOME", ".cache").ok_or_else(|| {
        PathError::new("neither XDG_CACHE_HOME nor HOME is set", "cached wallpapers")
    })?;
    Ok(base.join(name))
}

/// `$XDG_RUNTIME_DIR/<name>-<display>.sock`. `$XDG_RUNTIME_DIR` must be set,
/// <display> falls back to [DEFAULT_WAYLAND_DISPLAY].
pub fn socket_path(name: &str) -> Result<PathBuf, PathError> {
    let dir = env::var_os("XDG_RUNTIME_DIR")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| PathError::new("XDG_RUNTIME_DIR is not set", "the control socket"))?;
    // `WAYLAND_DISPLAY` may be an absolute path, which cannot appear inside a filename.
    let display = match env::var("WAYLAND_DISPLAY").unwrap_or_default() {
        display if display.is_empty() => DEFAULT_WAYLAND_DISPLAY.to_owned(),
        display => store::naming::sanitize(&display),
    };
    Ok(PathBuf::from(dir).join(format!("{name}-{display}.sock")))
}

/// One XDG base directory, or the place under `$HOME` the specification names for it.
fn base_dir(variable: &str, under_home: &str) -> Option<PathBuf> {
    env::var_os(variable)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(under_home)))
}
