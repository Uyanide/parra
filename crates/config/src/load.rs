use std::path::{Path, PathBuf};
use std::{fs, io};

use thiserror::Error;

use crate::merge::{self, Config, Context, Invalid};
use crate::schema::ConfigFile;

/// Each variant names the file and leaves the underlying cause to the error chain.
#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("cannot read {}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("invalid configuration in {}", path.display())]
    Parse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("{}: {key}: {message}", path.display())]
    Invalid { path: PathBuf, key: String, message: String },
}

pub struct Loaded {
    pub config: Config,
    /// False when no file existed and the built-in defaults were used. A missing config
    /// is not an error: the defaults are a working configuration.
    pub from_file: bool,
}

/// The configuration a run with no file at all uses.
///
/// Reached when no compositor backend was detected, so no file applied.
pub fn defaults(default_namespace: &str, base_dir: &Path) -> Config {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let ctx = Context { default_namespace, base_dir, home: home.as_deref() };
    merge::resolve(&ConfigFile::default(), &ctx)
        .expect("a file with no keys in it cannot hold an invalid value")
}

pub fn load(path: &Path, default_namespace: &str) -> Result<Loaded, ConfigError> {
    let text = match fs::read_to_string(path) {
        Ok(text) => Some(text),
        Err(source) if source.kind() == io::ErrorKind::NotFound => None,
        Err(source) => {
            return Err(ConfigError::Read { path: path.to_owned(), source });
        }
    };

    let file = match &text {
        Some(text) => toml::from_str::<ConfigFile>(text)
            .map_err(|source| ConfigError::Parse { path: path.to_owned(), source })?,
        None => ConfigFile::default(),
    };

    let home = std::env::var_os("HOME").map(PathBuf::from);
    let ctx = Context {
        default_namespace,
        base_dir: path.parent().unwrap_or(Path::new(".")),
        home: home.as_deref(),
    };
    let config = merge::resolve(&file, &ctx).map_err(|Invalid { key, message }| {
        ConfigError::Invalid { path: path.to_owned(), key, message }
    })?;

    Ok(Loaded { config, from_file: text.is_some() })
}
