mod cmd;
mod daemon;
mod paths;

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::{Shell, generate};
use control::ClientError;
use tracing_subscriber::EnvFilter;

/// The only place the program's name is read. Everything else takes it as a parameter,
/// so the one literal stays in the root `Cargo.toml`.
const NAME: &str = env!("CARGO_PKG_NAME");

#[derive(Parser)]
#[command(version, about = "Wallpaper daemon with compositor-driven effects, for wlr-layer-shell")]
struct Cli {
    #[arg(
        long,
        global = true,
        value_name = "PATH",
        help = concat!("Config file [default: $XDG_CONFIG_HOME/", env!("CARGO_PKG_NAME"), "/<compositor>.toml]")
    )]
    config: Option<PathBuf>,

    #[arg(
        long,
        global = true,
        value_name = "PATH",
        help = concat!("Control socket [default: $XDG_RUNTIME_DIR/", env!("CARGO_PKG_NAME"), "-$WAYLAND_DISPLAY.sock]")
    )]
    socket: Option<PathBuf>,

    #[arg(
        long,
        global = true,
        value_name = "PATH",
        help = concat!("Remembered wallpaper [default: $XDG_STATE_HOME/", env!("CARGO_PKG_NAME"), "/state.toml]")
    )]
    state: Option<PathBuf>,

    #[arg(
        long,
        global = true,
        value_name = "PATH",
        help = concat!("Resized wallpapers [default: $XDG_CACHE_HOME/", env!("CARGO_PKG_NAME"), "]")
    )]
    cache_dir: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the wallpaper daemon.
    Daemon(cmd::daemon::Args),
    /// Show the wallpaper at PATH.
    Set(cmd::set::Args),
    /// Stop showing a wallpaper set earlier, falling back to what is underneath.
    Unset(cmd::set::UnsetArgs),
    /// Put the recorded wallpapers back, undoing a --no-save set or unset.
    Restore(cmd::restore::Args),
    /// Turn the external blur signal on or off.
    Blur(cmd::blur::Args),
    /// Report what the daemon is currently showing.
    State(cmd::state::Args),
    /// Follow what the daemon changes, one event per line, until it stops.
    Events(cmd::events::Args),
    /// Re-read the config file.
    Reload,
    /// Check that the daemon is responding.
    Ping,
    /// Print a completion script for SHELL to stdout.
    Completions {
        /// Shell to write the script for.
        shell: Shell,
    },
}

fn main() -> ExitCode {
    // First, so the cold start covers everything after it. What ran before `main` is the
    // loader's, and cannot be measured from in here.
    let started = Instant::now();
    let cli = Cli::parse();
    init_logging(matches!(cli.command, Command::Daemon(_)));

    match run(cli, started) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{NAME}: {error:#}");
            ExitCode::from(exit_code(&error))
        }
    }
}

fn run(cli: Cli, started: Instant) -> anyhow::Result<()> {
    // A completion script depends on nothing resolved below: print it to stdout and go.
    let command = match cli.command {
        Command::Completions { shell } => {
            generate(shell, &mut Cli::command(), NAME, &mut std::io::stdout());
            return Ok(());
        }
        command => command,
    };
    let overrides = paths::Overrides {
        config: cli.config,
        socket: cli.socket,
        state: cli.state,
        cache_dir: cli.cache_dir,
    };
    let paths = paths::Paths::resolve(NAME, overrides)?;
    match &command {
        Command::Daemon(args) => cmd::daemon::run(args, &paths, NAME, started),
        Command::Set(args) => cmd::set::run(args, &paths.socket),
        Command::Unset(args) => cmd::set::unset(args, &paths.socket),
        Command::Restore(args) => cmd::restore::run(args, &paths.socket),
        Command::Blur(args) => cmd::blur::run(args, &paths.socket),
        Command::State(args) => cmd::state::run(args, &paths.socket),
        Command::Events(args) => cmd::events::run(args, &paths.socket),
        Command::Reload => cmd::reload(&paths.socket),
        Command::Ping => cmd::ping(&paths.socket),
        Command::Completions { .. } => unreachable!(),
    }
}

/// Tell apart the failures with a known remedy from "broken".
fn exit_code(error: &anyhow::Error) -> u8 {
    match error.downcast_ref::<ClientError>() {
        Some(ClientError::NotRunning { .. }) => cmd::EXIT_NOT_RUNNING,
        Some(ClientError::Mismatch { .. }) => cmd::EXIT_PROTOCOL,
        _ => cmd::EXIT_FAILURE,
    }
}

/// Verbosity comes from `RUST_LOG` alone, so there is no config key for it.
fn init_logging(is_daemon: bool) {
    let default = if is_daemon { "info" } else { "warn" };
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default));
    tracing_subscriber::fmt().with_env_filter(filter).with_writer(std::io::stderr).init();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A global option and a subcommand's own argument sharing a name is not a conflict
    /// clap can see: it panics inside `parse`, on that subcommand alone and only once the
    /// two are read back as different types. Parsing every subcommand is what finds it.
    #[test]
    fn every_subcommand_parses_beside_the_global_options() {
        let globals = ["--config", "/c", "--socket", "/s", "--state", "/t", "--cache-dir", "/d"];
        for tail in [
            ["daemon"].as_slice(),
            &["set", "/srv/a.png"],
            &["unset"],
            &["restore"],
            &["blur", "on"],
            &["state"],
            &["events"],
            &["reload"],
            &["ping"],
            &["completions", "bash"],
        ] {
            let argv = [&[NAME][..], &globals, tail].concat();
            Cli::try_parse_from(argv).unwrap_or_else(|error| panic!("{}: {error}", tail[0]));
        }
    }
}
