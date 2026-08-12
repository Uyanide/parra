use std::path::Path;

use control::Request;

use crate::cmd;

#[derive(clap::Args)]
pub struct Args {
    /// Whether the wallpaper should be blurred regardless of window focus.
    #[arg(value_enum)]
    pub state: Signal,

    /// Limit to one output. Applies to every output when omitted, and a broadcast also
    /// clears any per-output requests.
    #[arg(long, value_name = "NAME")]
    pub output: Option<String>,
}

#[derive(Clone, Copy, clap::ValueEnum)]
pub enum Signal {
    On,
    Off,
}

pub fn run(args: &Args, socket: &Path) -> anyhow::Result<()> {
    let on = matches!(args.state, Signal::On);
    cmd::ask(socket, Request::SetBlur { output: cmd::output_id(&args.output), on })?;
    Ok(())
}
