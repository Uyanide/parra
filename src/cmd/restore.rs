use std::path::Path;

use control::Request;

use crate::cmd;

#[derive(clap::Args)]
pub struct Args {
    /// Limit to one output, restoring only its own recorded wallpaper. Omitted, it
    /// restores every slot, per-output ones included.
    #[arg(long, value_name = "NAME")]
    pub output: Option<String>,
}

pub fn run(args: &Args, socket: &Path) -> anyhow::Result<()> {
    cmd::ask(socket, Request::RestoreWallpaper { output: cmd::output_id(&args.output) })?;
    Ok(())
}
