use std::path::{Path, PathBuf};

use control::Request;

use crate::cmd;

#[derive(clap::Args)]
pub struct Args {
    /// Image file to display.
    pub path: PathBuf,

    /// Limit to one output. Applies to every output when omitted.
    #[arg(long, value_name = "NAME")]
    pub output: Option<String>,
}

pub fn run(args: &Args, socket: &Path) -> anyhow::Result<()> {
    let path = cmd::absolute(&args.path)?;
    cmd::ask(socket, Request::SetWallpaper { output: cmd::output_id(&args.output), path })?;
    Ok(())
}
