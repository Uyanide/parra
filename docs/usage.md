# Usage

Build and install the binary first, as the [README#build](../README.md#build) describes.
Everything below assumes `parra` is on your `PATH`.

## TL;DR

parra works with any compositor that supports wlr-layer-shell. Animated effects --
scrolling, blurring, zooming -- need a supported compositor, and so far that is niri.

For niri, run:

```bash
cat > "${XDG_CONFIG_HOME:-${HOME}/.config}/niri/parra.kdl" << 'EOF_PARRA_KDL'
layer-rule {
  match namespace="^parra$"
  place-within-backdrop true
}

layout {
  background-color "transparent"
}

overview {
  workspace-shadow {
    off
  }
}

animations {
  workspace-switch {
    duration-ms 300
    curve "ease-out-cubic"
  }

  overview-open-close {
    duration-ms 300
    curve "ease-out-cubic"
  }
}

spawn-at-startup "parra" "daemon"
EOF_PARRA_KDL

cat >> "${XDG_CONFIG_HOME:-${HOME}/.config}/niri/config.kdl" << 'EOF_CONFIG_KDL'
include "parra.kdl"
EOF_CONFIG_KDL
```

then restart niri, or start parra at once with:

```bash
niri msg action spawn -- parra daemon
```

and set the wallpaper you like:

```bash
parra set /path/to/preferred/wallpaper.ext
```

> [!IMPORTANT]
>
> Above is only a quick guide. If you run into any troubles or wonder how these things
> work, please refer to the following sections and other documentations.

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

Without this rule niri draws the wallpaper inside every workspace thumbnail.

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
| `overview-open-close`      | `[zoom]`              | Zoom                                  |

niri ships all three as springs, and parra has fixed-duration easings, so making the two
sides agree means writing niri's in the easing form:

```kdl
animations {
  workspace-switch {
    duration-ms 300
    curve "ease-out-cubic"
  }

  overview-open-close {
    duration-ms 300
    curve "ease-out-cubic"
  }
}
```

and for the horizontal parallax, if you enable it:

```kdl
animations {
  horizontal-view-movement {
    duration-ms 300
    curve "ease-out-cubic"
  }
}
```

That is a real change to how niri feels. Their defaults are
`spring damping-ratio=1.0 stiffness=1000 epsilon=0.0001` for `workspace-switch` and the
same with `stiffness=800` for the other two.

The curves the two have in common:

| parra        | niri           |
| ------------ | -------------- |
| linear       | linear         |
| out-quad     | ease-out-quad  |
| out-cubic    | ease-out-cubic |
| -            | ease-out-expo  |
| -            | cubic-bezier   |
| -            | spring         |
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

> [!NOTE]
>
> The configuration file is **OPTIONAL**, a missing file is _NOT_ an error, as the
> built-in defaults are a working configuration. Only create the configuration file
> when one is needed.

parra reads one file per compositor, and under niri that is
`$XDG_CONFIG_HOME/parra/niri.toml`, falling back to `~/.config/parra/niri.toml`.

```sh
mkdir -p ~/.config/parra
cp niri.example.toml ~/.config/parra/niri.toml   # from the repository root
```

Check a file before restarting anything:

```sh
parra daemon --check
```

Every key, its default and what a reload picks up are in [config.md](config.md).

## Choosing a wallpaper

Hand one to the running daemon:

```sh
parra set ~/pictures/wall.png               # every output
parra set ~/pictures/other.png --output eDP-1
parra set ~/pictures/passing.png --no-save  # this session only, not restored after restart
```

`set` returns immediately and the current image stays up until the new one is ready. A
file that turns out not to be an image is reported in the log; see
[environment.md](environment.md#logging).

That choice is remembered. It outlives a config reload, a monitor being unplugged and
plugged back in, and the daemon itself.

`unset` takes one back:

```sh
parra unset --output eDP-1     # eDP-1 goes back to whatever every output is on
parra unset                    # every output goes back to the config file
```

It uncovers the next wallpaper down, walking the order the daemon resolves in:

1. an output's own wallpaper
2. the one set for every output
3. `[wallpaper] fallback`
4. nothing.

`--no-save` works on both: change what is on screen now and leave the record alone, so the
next start goes back to what it says. On `set` that is a wallpaper shown without being
adopted; on `unset` it is one dropped while staying recorded.

`restore` is the way back without waiting for that next start:

```sh
parra set ~/pictures/passing.png --no-save
parra restore                  # back to what is recorded, on every output
parra restore --output eDP-1   # back to eDP-1's own recorded wallpaper
```

It empties the slots it addresses before re-applying the record, so it undoes a `--no-save`
set and a `--no-save` unset alike. Restoring what is already showing does nothing at all.

The config file says what to show when nothing has been chosen yet:

```toml
[wallpaper]
fallback = "~/pictures/wall.png"
```

### Transparent wallpapers

An image with an alpha channel is drawn with it. Where the wallpaper is not fully opaque,
whatever the compositor draws below the layer surface shows through.

While a wallpaper has any translucent pixel, the surface is no longer declared opaque and
the compositor blends it. An image whose alpha channel is present but says nothing
everywhere keeps being treated as opaque.

## State and cache

Two more locations, **neither** of them meant to be edited by hand. `--state PATH` and
`--cache-dir PATH` override them.

| Location                           | Holds                                                                        |
| ---------------------------------- | ---------------------------------------------------------------------------- |
| `$XDG_STATE_HOME/parra/state.toml` | Which wallpaper each slot was last set to, so a restart restores it.         |
| `$XDG_CACHE_HOME/parra/*.qoi`      | Those wallpapers, already resized, so a restart skips decoding the original. |

`$HOME/.local/state` and `$HOME/.cache` are the fallbacks.

The state file records the path you asked for, and `parra set` and `parra unset` rewrite
it. Do _NOT_ edit it by hand: the daemon reads it once at startup and rewrites it whole on
every change, so an edit made while it is running is overwritten with no warning.

A cached copy is kept at the size the largest monitor showing it needs. It is used again
as long as it still covers that, and re-made from the original when it does not, which is
what a rotation, a resolution change, a scale change or a smaller `crop-ratio` all amount
to. Copies no longer pointed at are deleted when the daemon starts and after every `set`
or `unset`.

## Checking it works

```sh
parra ping     # protocol {version}
parra state    # every output, what it shows, where its animations are
```

`parra state` should list each connector with a size, a wallpaper path and a set of flags.
If an output is missing, niri has not configured its layer surface yet.

`ping` exits 4 when the daemon speaks a different protocol from this binary, which means a
daemon still running from before the binary was replaced. Restart it.

For scripts, `--json` prints the reply verbatim:

```sh
parra state --json | jq '.state.outputs[] | {name, blur: .blur.amount.current}'
```

## Listening for changes

Whatever else you run on your screen can follow the wallpaper instead of polling it:

```sh
parra events                   # readable, one line per change
parra events --json            # for scripts
parra events --output DP-1     # only what concerns one monitor
```

The stream opens with a line per monitor describing what it shows and where its values
are, then reports what the daemon decides: a wallpaper changing, an image that would not
decode, a monitor arriving or leaving, the config file being adopted, and every animation
as it starts.

An animation carries where it is going, how long it takes and which curve it uses, which
is enough to run the same curve on your own clock:

```sh
parra events --json --output DP-1 \
  | jq -c --unbuffered 'select(.animation?.property == "blur") | .animation'
```

```json
{ "output": "DP-1", "property": "blur", "from": 0.0, "to": 1.0, "duration_us": 300000, "easing": "in-out-cubic" }
```

Listening costs the daemon no frames. The stream ends when the daemon does, and `parra
events` then exits 1, so a supervisor can restart it. Every event and the rules it follows
is in [control-protocol.md](control-protocol.md#events).

## The blur signal

A bar or a sidebar can ask for the wallpaper behind it to blur while it is up, and turn it
off again afterwards:

```sh
parra blur on --output DP-1
parra blur off --output DP-1
```

_'output blurs'_ = _'the focused window is on this output'_ **OR** _'blur signal is set for this output'_

Command syntax and exit codes are in [cli.md](cli.md). The full protocol, every request
and response, is in [control-protocol.md](control-protocol.md).

## Troubleshooting

See [environment.md#logging](./environment.md#logging) for logging.
