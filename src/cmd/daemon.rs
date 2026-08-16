use std::time::Instant;

use tracing::{info, warn};

use crate::paths::Paths;
use crate::{cmd, daemon};

#[derive(clap::Args)]
pub struct Args {
    /// Load and validate the configuration, report what it resolved to, and exit.
    #[arg(long)]
    pub check: bool,
}

pub fn run(args: &Args, paths: &Paths, name: &str, started: Instant) -> anyhow::Result<()> {
    let loaded = config::load(&paths.config, name)?;

    if loaded.from_file {
        info!(path = %paths.config.display(), "loaded configuration");
    } else {
        warn!(path = %paths.config.display(), "no configuration file, using defaults");
    }

    let config = loaded.config;
    if args.check {
        return check(&config, paths);
    }

    info!(
        namespace = %config.surface.namespace,
        layer = ?config.surface.layer,
        "starting"
    );
    if let Some(backend) = compositor::backends::detect() {
        info!(backend = backend.name(), "compositor backend selected");
    } else {
        warn!("no compositor backend available, the effects it drives will not react");
    }

    daemon::run(config, paths, name, started)
}

/// Reports what the configuration resolved to and what the last run left behind, without
/// touching a graphics stack. Everything printed here is read-only.
fn check(config: &config::Config, paths: &Paths) -> anyhow::Result<()> {
    println!("{}", paths.config.display());
    println!("  namespace  {}", config.surface.namespace);
    println!("  layer      {:?}", config.surface.layer);
    println!("  socket     {}", paths.socket.display());
    println!(
        "  fallback   {}",
        cmd::path_or_none(config.global.fallback.as_ref().map(|w| w.path()))
    );
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
