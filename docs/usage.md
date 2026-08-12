# Usage

Build and install the binary first, as the [README](../README.md) describes. Everything
below assumes `parra` is on your `PATH`.

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

| niri animation             | parra section          | Drives                                |
| -------------------------- | ---------------------- | ------------------------------------- |
| `workspace-switch`         | `[scroll.vertical]`    | Vertical parallax                     |
| `horizontal-view-movement` | `[scroll.horizontal]`  | Horizontal parallax, if you enable it |
| `overview-open-close`      | `[overview]`           | Zoom                                  |

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
`~/.config/parra/config.toml`. A missing file is *NOT* an error, as the built-in defaults
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

That prints the resolved namespace, layer, socket and wallpaper, or names the offending
key and its accepted range. Unknown keys are rejected rather than ignored.

The daemon watches the config file, so an edit takes effect on save. `[general]
namespace` and `[general] layer` are the exception: a layer surface is given both when it
is created, so those two take effect on the next start.

## Choosing a wallpaper

Either set one in the config file:

```toml
[wallpaper]
path = "~/pictures/wall.png"
```

or hand one to the running daemon:

```sh
parra set ~/pictures/wall.png              # every output
parra set ~/pictures/other.png --output eDP-1
```

A wallpaper set over the socket outlives a config reload and survives a monitor being
unplugged and plugged back in. The config file reclaims the slot only if its own
`path` actually changed.

Decoding happens on another thread, so `set` returns immediately and the current image
stays up until the new one is ready. A file that turns out not to be an image is reported
in the log, not to whoever asked.

## Checking it works

```sh
parra ping     # protocol 2
parra state    # every output, what it shows, where its animations are
```

`parra state` should list each connector with a size, a wallpaper path and a set of
flags. If an output is missing, niri has not configured its layer surface yet.

For scripts, `--json` prints the reply verbatim:

```sh
parra state --json | jq '.state.outputs[] | {name, blur: .blur.amount.current}'
```

## Everyday commands

```sh
parra daemon                              # run it
parra set ~/pictures/wall.png             # change the wallpaper
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

If you would rather not use `spawn-at-startup`:

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

The unit needs `WAYLAND_DISPLAY` and `NIRI_SOCKET` in its environment, which niri
normally exports through `systemctl --user import-environment` or a
`systemd.environment-generator`. See [environment.md](environment.md) for the variables
that matter and for pinning the daemon to a particular GPU.

## Troubleshooting

See [environment.md#logging](./environment.md#logging) for logging.
