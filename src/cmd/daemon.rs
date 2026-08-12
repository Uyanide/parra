use std::time::Instant;

use tracing::{info, warn};

use crate::daemon;
use crate::paths::Paths;

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
        println!("{}", paths.config.display());
        println!("  namespace  {}", config.surface.namespace);
        println!("  layer      {:?}", config.surface.layer);
        println!("  socket     {}", paths.socket.display());
        println!(
            "  wallpaper  {}",
            config
                .global
                .wallpaper
                .as_ref()
                .map_or("none".to_owned(), |w| w.path().display().to_string())
        );
        return Ok(());
    }

    info!(
        namespace = %config.surface.namespace,
        layer = ?config.surface.layer,
        "starting"
    );
    if let Some(backend) = compositor::backends::detect() {
        info!(backend = backend.name(), "compositor backend selected");
    } else {
        warn!("no compositor backend available, scroll and blur will not react");
    }

    daemon::run(config, paths, name, started)
}
