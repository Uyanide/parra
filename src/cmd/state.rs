use std::path::Path;

use control::{OutputSnapshot, Request, Response, StateSnapshot, Tween};

use crate::cmd;

/// Gap below which current and target print as one number rather than as a tween.
const SETTLED: f32 = 5e-4;

#[derive(clap::Args)]
pub struct Args {
    /// Report one output instead of all of them.
    #[arg(long, value_name = "NAME")]
    pub output: Option<String>,

    /// Print the daemon's reply as JSON, for anything that is not a human.
    #[arg(long)]
    pub json: bool,
}

pub fn run(args: &Args, socket: &Path) -> anyhow::Result<()> {
    let request = match cmd::output_id(&args.output) {
        Some(output) => Request::GetOutput { output },
        None => Request::GetState,
    };
    let response = cmd::ask(socket, request)?;

    if args.json {
        println!("{}", serde_json::to_string(&response)?);
        return Ok(());
    }

    match &response {
        Response::State(state) => {
            if state.outputs.is_empty() {
                println!("no outputs");
            }
            for output in &state.outputs {
                print_output(output);
                println!();
            }
            print_budget(state);
            Ok(())
        }
        Response::Output(output) => {
            print_output(output);
            Ok(())
        }
        other => Err(cmd::unexpected(other)),
    }
}

fn print_output(output: &OutputSnapshot) {
    let mut flags = Vec::new();
    if output.channels.blur {
        flags.push("blurred");
    }
    if output.channels.zoom_out {
        flags.push("zoomed-out");
    }
    flags.push(if output.settled { "settled" } else { "animating" });

    println!(
        "{}  {}x{} scale {}  [{}]",
        output.name,
        output.logical.w,
        output.logical.h,
        output.scale,
        flags.join(" ")
    );
    println!("  wallpaper  {}", cmd::path_or_none(output.wallpaper.as_deref()));
    println!(
        "  driven     x {:.3}  y {:.3}  (stride {:.3}, {:.3})",
        output.channels.x.at,
        output.channels.y.at,
        output.channels.x.stride,
        output.channels.y.stride
    );
    println!(
        "  scroll     v {}  h {}",
        tween(output.scroll.vertical),
        tween(output.scroll.horizontal)
    );
    println!(
        "  blur       {}  (radius {}, downscale {}, tint {})",
        tween(output.blur.amount),
        output.blur.radius,
        output.blur.downscale,
        output.blur.tint
    );
    println!("  zoom       {}", tween(output.zoom));
    if let (Some(last), Some(peak)) = (output.gpu.last_us, output.gpu.peak_us) {
        println!("  gpu        {} per frame, {} at worst", cmd::millis(last), cmd::millis(peak));
    }
}

/// What the whole daemon costs, as opposed to what one output is doing.
fn print_budget(state: &StateSnapshot) {
    print!("{} frames  {:.1} MB in textures", state.frames, megabytes(state.texture_bytes));
    match state.startup_us {
        Some(startup) => println!("  {} to the first frame", cmd::millis(startup)),
        None => println!("  nothing shown yet"),
    }
}

fn megabytes(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

fn tween(value: Tween) -> String {
    if (value.current - value.target).abs() < SETTLED {
        format!("{:.3}", value.current)
    } else {
        format!("{:.3} -> {:.3}", value.current, value.target)
    }
}
