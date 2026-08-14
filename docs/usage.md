# Usage

Build and install the binary first, as the [README#build](../README.md#build) describes.
Everything below assumes `parra` is on your `PATH`.

## Compositor integration

### niri

All of this goes in `$XDG_CONFIG_HOME/niri/config.kdl`, or `~/.config/niri/config.kdl`
if that variable is unset, or any file it includes.

#### Put within backdrop

Put the wallpaper layer in the backdrop. The regex matches the layer-shell namespace,
which defaults to the program's own name; change it if you set `[general] namespace` in
parra's config:

```kdl
layer-rule {
  match namespace="^parra$"
  place-within-backdrop true
}
```

Without this rule niri draws the wallpaper inside every workspace thumbnail rather than
behind the overview as a whole.

Make the workspace background transparent so the backdrop shows through:

```kdl
layout {
  background-color "transparent"
}
```

Optionally, drop the workspace shadow for a cleaner overview:

```kdl
overview {
  workspace-shadow {
    off
  }
}
```

#### Match animations

Match niri's animations to parra's, or the wallpaper will lead or lag the windows it sits
behind. Three of niri's animations have a counterpart here:

| niri animation             | parra section         | Drives                                |
| -------------------------- | --------------------- | ------------------------------------- |
| `workspace-switch`         | `[scroll.vertical]`   | Vertical parallax                     |
| `horizontal-view-movement` | `[scroll.horizontal]` | Horizontal parallax, if you enable it |
| `overview-open-close`      | `[overview]`          | Zoom                                  |

Each parallax axis carries its own `duration-ms` and `easing` for exactly this reason:
niri's two animations are separate, and their defaults differ.

niri ships all three as springs, and parra has no spring model, but only fixed-duration
easings. A niri animation is one or the other, never both, so making the two sides agree
means converting niri's to the easing form:

```kdl
animations {
  workspace-switch {
    duration-ms 400
    curve "ease-out-cubic"
  }

  overview-open-close {
    duration-ms 400
    curve "ease-out-cubic"
  }
}
```

and for the horizontal parallax, if you enable it:

```kdl
animations {
  horizontal-view-movement {
    duration-ms 400
    curve "ease-out-cubic"
  }
}
```

That is a real change to how niri feels. Their defaults are
`spring damping-ratio=1.0 stiffness=1000 epsilon=0.0001` for `workspace-switch` and the
same with `stiffness=800` for the other two.

The matching animation types between parra and niri:

| parra        | niri           |
| ------------ | -------------- |
| linear       | linear         |
| out-quad     | ease-out-quad  |
| out-cubic    | ease-out-cubic |
| -            | ease-out-expo  |
| -            | cubic-bezier   |
| -            | sping          |
| in-out-quad  | -              |
| in-out-cubic | -              |
| out-quint    | -              |

niri's [animation documentation][niri-animations] has the full list.

[niri-animations]: https://github.com/niri-wm/niri/blob/main/docs/wiki/Configuration%3A-Animations.md

#### Autostart

Finally, start the daemon at login:

```kdl
spawn-at-startup "parra" "daemon"
```

## Configuration

parra reads `$XDG_CONFIG_HOME/parra/config.toml`, falling back to
`~/.config/parra/config.toml`. A missing file is _NOT_ an error, as the built-in defaults
are a working configuration.

```sh
mkdir -p ~/.config/parra
cp config.example.toml ~/.config/parra/config.toml   # from the repository root
```

Every key and its default is in [config.md](config.md). Check a file before restarting
anything:

```sh
parra daemon --check
```

That prints the resolved namespace, layer, socket and fallback, where the remembered
wallpaper is kept and what it currently is, or names the offending key and its accepted
range. Unknown keys are rejected rather than ignored.

The daemon watches the config file, so an edit takes effect on save. `[general]
namespace` and `[general] layer` are the exception: a layer surface is given both when it
is created, so those two take effect on the next start.

## Choosing a wallpaper

Hand one to the running daemon:

```sh
parra set ~/pictures/wall.png                  # every output
parra set ~/pictures/other.png --output eDP-1
parra set ~/pictures/passing.png --no-save     # this session only
```

That choice is remembered. It outlives a config reload, outlives a monitor being
unplugged and plugged back in, and outlives the daemon: it comes back at the next start,
from a resized copy, so restarting costs a fraction of the first decode.

`unset` takes one back:

```sh
parra unset --output eDP-1     # eDP-1 goes back to whatever every output is on
parra unset                    # every output goes back to the config file
```

It reveals rather than blanks. An output that loses its own wallpaper shows the one set
for every output, and one that loses that shows `[wallpaper] fallback`. Only when there is
nothing left underneath does a screen go empty.

The config file only says what to show when nothing has been chosen yet:

```toml
[wallpaper]
fallback = "~/pictures/wall.png"
```

so the two never compete for the slot. Where the choice and the copies are kept is in
[config.md](config.md#state-and-cache).

Decoding happens on another thread, so `set` returns immediately and the current image
stays up until the new one is ready. A file that turns out not to be an image or fails
to decode is reported in the log, rather to the requester.

## Checking it works

```sh
parra ping     # protocol 1
parra state    # every output, what it shows, where its animations are
```

`parra state` should list each connector with a size, a wallpaper path and a set of
flags. If an output is missing, niri has not configured its layer surface yet.

`ping` exits 4 when the daemon reports a protocol other than the one this binary speaks,
which is a daemon still running from before the binary was replaced. Restart it.

For scripts, `--json` prints the reply verbatim:

```sh
parra state --json | jq '.state.outputs[] | {name, blur: .blur.amount.current}'
```

## Everyday commands

```sh
parra daemon                              # run it
parra set ~/pictures/wall.png             # change the wallpaper
parra unset --output DP-1                 # drop one monitor's own wallpaper
parra blur on --output DP-1               # external blur signal
parra state                               # what is on screen
parra state --json | jq                   # the same, for scripts
parra reload                              # re-read the config file
```

`--config PATH` and `--socket PATH` work on any subcommand.

The blur signal is for whatever else is on your screen: a bar or a sidebar can ask for
the wallpaper behind it to blur while it is up, and turn it off again afterwards. Blur is
otherwise driven by window focus alone. Omitting `--output` broadcasts, which also clears
any per-output requests, so a broadcast is always authoritative.

The full protocol, every request and response, and the exit codes are in
[control-protocol.md](control-protocol.md).

## Running under systemd instead

If you would rather use systemd units for autostart:

```ini
[Unit]
Description=parra wallpaper daemon
PartOf=graphical-session.target
After=graphical-session.target

[Service]
ExecStart=%h/.local/bin/parra daemon
Restart=on-failure

[Install]
WantedBy=graphical-session.target
```

The unit needs `WAYLAND_DISPLAY` and some other environment variables including those set
by the compositor. See [environment.md](environment.md) for the variables that matter and
for pinning the daemon to a particular GPU.

## Troubleshooting

See [environment.md#logging](./environment.md#logging) for logging.
