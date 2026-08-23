use std::path::Path;
use std::time::Instant;

use anyhow::anyhow;
use compositor::backends;
use tracing::{info, warn};

use crate::cmd;
use crate::daemon::{self, Startup};
use crate::paths::Paths;

#[derive(clap::Args)]
pub struct Args {
    /// Load and validate the configuration, report what it resolved to, and exit.
    #[arg(long)]
    pub check: bool,

    /// Which compositor to configure for, instead of detecting the one running.
    #[arg(
        long,
        value_name = "NAME",
        value_parser = clap::builder::PossibleValuesParser::new(backends::AVAILABLE)
    )]
    pub backend: Option<String>,
}

pub fn run(args: &Args, paths: &Paths, name: &str, started: Instant) -> anyhow::Result<()> {
    // Named by hand wins, so a file can be validated somewhere other than where it runs.
    let backend: Option<&str> = match args.backend.as_deref() {
        Some(name) => Some(name),
        None => backends::detect(),
    };
    let config_path = paths.config(backend);

    let (config, compositor) = match &config_path {
        Some(path) => {
            let loaded = config::load(path, name)?;
            if loaded.from_file {
                info!(path = %path.display(), "loaded configuration");
            } else {
                warn!(path = %path.display(), "no configuration file, using defaults");
            }
            let compositor =
                backend.map(|backend| settings(&loaded.config, backend, path)).transpose()?;
            (loaded.config, compositor)
        }
        None => {
            warn!(
                supported = ?backends::AVAILABLE,
                "no compositor backend for this session, so no configuration file applies; \
                 name one with --config to read it anyway"
            );
            (config::defaults(name, &paths.config_dir), None)
        }
    };

    if args.check {
        return check(&config, compositor.as_ref(), config_path.as_deref(), paths);
    }

    info!(
        namespace = %config.surface.namespace,
        layer = ?config.surface.layer,
        "starting"
    );
    if let Some(compositor) = &compositor {
        info!(backend = compositor.backend(), settings = %compositor, "driving the channels");
    }

    daemon::run(Startup { config, config_path, backend: compositor }, paths, name, started)
}

/// Reads the compositor's own section, which only that backend's schema can validate.
///
/// Each output that overrides something arrives already laid over the global section, so
/// the backend reads whole settings rather than a patch.
fn settings(
    config: &config::Config,
    backend: &str,
    path: &Path,
) -> anyhow::Result<backends::Settings> {
    let global = toml::Value::Table(config.compositor_table().clone());
    let mut settings = backends::Settings::deserialize(backend, global)
        .map_err(|error| bad_section(path, "[compositor]", &error))?;

    for (output, table) in config.compositor_overrides() {
        let section = toml::Value::Table(table.clone());
        settings.deserialize_output(output.clone(), section).map_err(|error| {
            bad_section(path, &format!("[output.{:?}.compositor]", output.as_str()), &error)
        })?;
    }
    Ok(settings)
}

/// Names the file and the table the key came from, which only this layer knows.
fn bad_section(path: &Path, section: &str, error: &toml::de::Error) -> anyhow::Error {
    // Deserializing from a value leaves the error with no span to point at, so only the
    // message survives. `toml` wraps it over several lines; flatten it back.
    let flattened = error.to_string().split_whitespace().collect::<Vec<_>>().join(" ");
    anyhow!("{}: {section}: {flattened}", path.display())
}

/// Reports what the configuration resolved to and what the last run left behind, without
/// touching a graphics stack. Everything printed here is read-only.
fn check(
    config: &config::Config,
    compositor: Option<&backends::Settings>,
    config_path: Option<&Path>,
    paths: &Paths,
) -> anyhow::Result<()> {
    match config_path {
        Some(path) => println!("{}", path.display()),
        None => println!("(no configuration file: no backend detected and none named)"),
    }
    println!("  namespace  {}", config.surface.namespace);
    println!("  layer      {:?}", config.surface.layer);
    println!("  socket     {}", paths.socket.display());
    println!(
        "  fallback   {}",
        cmd::path_or_none(config.global.fallback.as_ref().map(|w| w.path()))
    );
    match compositor {
        Some(settings) => {
            println!("  compositor {} {settings}", settings.backend());
            for (output, own) in settings.overrides() {
                println!("  {:<10} {own}", output.as_str());
            }
        }
        None => println!("  compositor none"),
    }
    println!("{}", paths.state.display());
    println!("  cache      {}", paths.cache_dir.display());

    let state = store::State::load(&paths.state)?;
    if state.entries().next().is_none() {
        println!("  remembered nothing");
    }
    for (output, entry) in state.entries() {
        let slot = output.map_or_else(|| "all".to_owned(), ToString::to_string);
        println!("  {slot:<9}  {}", entry.source.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every example file says its keys are shown with their defaults. `config` checks
    /// that for every section but `[compositor]`, whose keys only a backend can read.
    #[test]
    fn every_example_compositor_section_states_the_real_defaults() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples");
        for backend in backends::AVAILABLE {
            let path = dir.join(format!("{backend}.example.toml"));
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("{} should be readable: {e}", path.display()));
            let file: toml::Table = toml::from_str(&text).expect("the example should be valid");
            let section =
                file.get("compositor").cloned().unwrap_or_else(|| toml::Table::new().into());

            let parsed = backends::Settings::deserialize(backend, section)
                .unwrap_or_else(|e| panic!("examples/{backend}.example.toml: [compositor]: {e}"));
            assert_eq!(
                parsed,
                backends::Settings::default_for(backend).unwrap(),
                "examples/{backend}.example.toml has drifted from that backend's defaults"
            );
        }
    }
}
