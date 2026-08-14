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

    /// Show it now but do not remember it, so the next start goes back to whatever was
    /// chosen before.
    #[arg(long)]
    pub no_save: bool,
}

/// `unset` takes everything `set` does except the image itself.
#[derive(clap::Args)]
pub struct UnsetArgs {
    /// Limit to one output, dropping only its own wallpaper. Omitted, it drops every
    /// wallpaper set this way, including the per-output ones.
    #[arg(long, value_name = "NAME")]
    pub output: Option<String>,

    /// Drop it for now but keep remembering it, so the next start brings it back.
    #[arg(long)]
    pub no_save: bool,
}

pub fn run(args: &Args, socket: &Path) -> anyhow::Result<()> {
    let path = cmd::absolute(&args.path)?;
    ask(socket, &args.output, Some(path), args.no_save)
}

pub fn unset(args: &UnsetArgs, socket: &Path) -> anyhow::Result<()> {
    ask(socket, &args.output, None, args.no_save)
}

fn ask(
    socket: &Path,
    output: &Option<String>,
    path: Option<PathBuf>,
    no_save: bool,
) -> anyhow::Result<()> {
    let output = cmd::output_id(output);
    cmd::ask(socket, Request::SetWallpaper { output, path, save: !no_save })?;
    Ok(())
}
