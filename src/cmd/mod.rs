pub mod blur;
pub mod daemon;
pub mod events;
pub mod set;
pub mod state;

use std::path::Path;

use anyhow::Context as _;
use control::{Client, ClientError, Micros, PROTOCOL_VERSION, Request, Response};
use domain::OutputId;

pub const EXIT_FAILURE: u8 = 1;
pub const EXIT_NOT_RUNNING: u8 = 3;
pub const EXIT_PROTOCOL: u8 = 4;

/// Sends one request and returns the reply, which is all every subcommand but `daemon`
/// does.
pub fn ask(socket: &Path, request: Request) -> anyhow::Result<Response> {
    let mut client = Client::connect(socket)?;
    Ok(client.request(&request)?)
}

pub fn reload(socket: &Path) -> anyhow::Result<()> {
    ask(socket, Request::ReloadConfig)?;
    Ok(())
}

pub fn ping(socket: &Path) -> anyhow::Result<()> {
    match ask(socket, Request::Ping)? {
        Response::Pong { version } => {
            // Reported before the verdict: a mismatch is exactly when the number is worth
            // reading, so it belongs on stdout either way.
            println!("protocol {version}");
            if version == PROTOCOL_VERSION {
                Ok(())
            } else {
                Err(ClientError::Mismatch { daemon: version, ours: PROTOCOL_VERSION }.into())
            }
        }
        other => Err(unexpected(&other)),
    }
}

pub fn output_id(name: &Option<String>) -> Option<OutputId> {
    name.as_ref().map(|name| OutputId::new(name.clone()))
}

/// A duration as the wire carries it, in what a person reads.
pub fn millis(us: Micros) -> String {
    format!("{:.2} ms", us as f64 / 1_000.0)
}

/// A path where there might be none, which every readable output spells the same way.
pub fn path_or_none(path: Option<&Path>) -> String {
    path.map_or_else(|| "none".to_owned(), |path| path.display().to_string())
}

pub fn unexpected(response: &Response) -> anyhow::Error {
    anyhow::anyhow!("unexpected reply from the daemon: {response:?}")
}

/// Makes a path meaningful to a process with a different working directory, and fails
/// early when the file is not there.
pub fn absolute(path: &Path) -> anyhow::Result<std::path::PathBuf> {
    std::fs::canonicalize(path).with_context(|| format!("cannot read {}", path.display()))
}
