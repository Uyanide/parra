use std::io::{self, StdoutLock, Write};
use std::path::Path;

use control::{Client, ClientError, Event, Micros, Values};
use domain::OutputId;

use crate::cmd;

#[derive(clap::Args)]
pub struct Args {
    /// Report only what concerns one output, plus the events that name none.
    #[arg(long, value_name = "NAME")]
    pub output: Option<String>,

    /// Print each event as the daemon sent it, for anything that is not a human.
    #[arg(long)]
    pub json: bool,
}

pub fn run(args: &Args, socket: &Path) -> anyhow::Result<()> {
    let events = Client::connect(socket)?.subscribe()?;
    let wanted = cmd::output_id(&args.output);
    let mut out = io::stdout().lock();

    for event in events {
        let event = event?;
        if !concerns(&event, wanted.as_ref()) {
            continue;
        }
        let line = if args.json { serde_json::to_string(&event)? } else { describe(&event) };
        if !print(&mut out, &line)? {
            return Ok(());
        }
    }
    // The stream runs out only when the daemon does, which is worth an exit code.
    Err(ClientError::Closed.into())
}

/// Whether this event is one the caller asked for. An event that names no output concerns
/// every caller, there being no other output it could belong to.
fn concerns(event: &Event, wanted: Option<&OutputId>) -> bool {
    match (wanted, event.output()) {
        (Some(wanted), Some(output)) => wanted == output,
        _ => true,
    }
}

/// Writes one line, reporting whether the stream should carry on. A closed pipe is how
/// `head` ends a stream rather than a failure to report.
fn print(out: &mut StdoutLock, line: &str) -> anyhow::Result<bool> {
    match writeln!(out, "{line}") {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(false),
        Err(error) => Err(error.into()),
    }
}

/// One line per event, led by the name it goes by on the wire so that the readable form
/// teaches the other one. The easing is in `--json` alone: whatever reproduces a curve
/// reads that, and naming it here would be a second place that spells `out-cubic`.
fn describe(event: &Event) -> String {
    match event {
        Event::Animation { output, property, from, to, duration_us, .. } => {
            format!("animation {output} {property} {from:.3} -> {to:.3}  {}", over(*duration_us))
        }
        Event::WallpaperChanged { output, from, to, duration_us, .. } => {
            format!(
                "wallpaper-changed {output} {} -> {}  {}",
                cmd::path_or_none(from.as_deref()),
                cmd::path_or_none(to.as_deref()),
                over(*duration_us)
            )
        }
        Event::WallpaperFailed { path } => format!("wallpaper-failed {}", path.display()),
        Event::OutputReady { output, wallpaper, values } => {
            format!(
                "output-ready {output} {} {}",
                cmd::path_or_none(wallpaper.as_deref()),
                amounts(*values)
            )
        }
        Event::OutputGone { output } => format!("output-gone {output}"),
        Event::ConfigReloaded => "config-reloaded".to_owned(),
    }
}

fn amounts(values: Values) -> String {
    format!(
        "scroll {:.3}/{:.3} blur {:.3} zoom {:.3}",
        values.scroll_vertical, values.scroll_horizontal, values.blur, values.zoom
    )
}

/// How long the move takes, or that there is no move to make.
fn over(duration_us: Micros) -> String {
    if duration_us == 0 {
        "instant".to_owned()
    } else {
        format!("over {}", cmd::millis(duration_us))
    }
}
