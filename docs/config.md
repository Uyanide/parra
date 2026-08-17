# Configuration

The daemon reads `$XDG_CONFIG_HOME/parra/config.toml`, falling back to
`$HOME/.config/parra/config.toml`. `--config PATH` overrides the location.

A missing file is not an error: the built-in defaults are a working configuration.
[config.example.toml](../config.example.toml) lists every key with its default.

Validate a file without starting anything with `parra daemon --check --config ./config.toml`;
see [cli.md](cli.md).

## Inheritance

Global sections set the value for every output. A `[output."<connector>"]` table
overrides individual keys for one monitor; anything it leaves out is inherited. Connector
names are matched exactly as the compositor reports them, `DP-1` and `eDP-1` being the
usual shapes.

```toml
[blur]
radius = 48
downscale = 2

[output."DP-1"]
blur.radius = 16   # downscale stays 2
```

## Keys

### `[general]`

Not overridable per output.

| Key         | Default            | Meaning                                                       |
| ----------- | ------------------ | ------------------------------------------------------------- |
| `namespace` | the program's name | Layer-shell namespace, for compositor rules that match on it. |
| `layer`     | `"background"`     | `"background"` or `"bottom"`.                                 |

Both are read when a layer surface is created, so a reload accepts them but they take
effect on the **next start**.

### `[wallpaper]`

| Key        | Default | Meaning                                                                              |
| ---------- | ------- | ------------------------------------------------------------------------------------ |
| `fallback` | unset   | What to show when nothing has been set over the control socket. Unset shows nothing. |

`parra set` is remembered across restarts and takes precedence over this, so `fallback`
is what a monitor shows before anything has ever been chosen for it, and what it falls
back to if the chosen image will not load. See [usage.md](usage.md#state-and-cache).

Paths may be absolute, start with `~/`, or be relative. A relative path resolves against
the config file's own directory, since a daemon's working directory is not something a
user can reason about.

The file is not opened at load time. A path that does not exist yet is a decode error
later, rather than a configuration error.

### `[scroll.vertical]` and `[scroll.horizontal]`

The two parallax axes take the same four keys and are configured apart. The vertical axis
follows the active workspace, the horizontal one the column in the scrolling layout. Both
are per output.

| Key           | Default                             | Meaning                                                                      |
| ------------- | ----------------------------------- | ---------------------------------------------------------------------------- |
| `enabled`     | `true` vertical, `false` horizontal | When false the image is pinned to its centre on that axis.                   |
| `travel`      | `1.0`                               | Fraction of the available travel to use, `0..=1`, measured about the centre. |
| `duration-ms` | `300`                               | `0` makes the move instant. Capped at 60000.                                 |
| `easing`      | `"out-cubic"`                       | See [easing functions](#easing-functions).                                   |

```toml
[scroll.vertical]
travel = 0.5

[scroll.horizontal]
enabled = true
duration-ms = 250   # travel and easing stay at their defaults
```

Each monitor scrolls by its own active workspace and that workspace's own column, so a
monitor without the focus holds its position rather than drifting to other positions. A
workspace nothing has been focused on yet sits centred, and so does a monitor whose
focused window is floating or fullscreen and therefore has no place in the scroll.

### `[blur]`

| Key            | Default          | Meaning                                                                                             |
| -------------- | ---------------- | --------------------------------------------------------------------------------------------------- |
| `radius`       | `32`             | `0` disables blur entirely, including the bake. Capped at 512.                                      |
| `downscale`    | `4`              | Linear downscale of the baked blur texture, `1..=16`. Higher is cheaper in both VRAM and bake time. |
| `tint`         | `"#1e1e2e"`      | `#rgb`, `#rgba`, `#rrggbb` or `#rrggbbaa`.                                                          |
| `tint-opacity` | `0.5`            | `0..=1`, multiplied into the tint's own alpha.                                                      |
| `duration-ms`  | `300`            |                                                                                                     |
| `easing`       | `"in-out-cubic"` | See [easing functions](#easing-functions).                                                          |

An output blurs when it holds the focused window, or when the control socket has asked
for it. Nothing focused anywhere leaves every output sharp.

`radius` is measured in texels of the wallpaper texture, which is decoded at the buffer
size times the deepest zoom. At rest one texel is one device pixel, so the configured
number is the blur's extent on screen, and one radius means the same thing on monitors at
different scales.

### `[overview]`

| Key           | Default       | Meaning                                                                                                                                                                                                           |
| ------------- | ------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `crop-ratio`  | `0.9`         | Fraction of the image visible while the overview is closed, `0.25..=1`. The remainder is the headroom the parallax travels through, so `1.0` leaves nothing to scroll unless the image is taller than the screen. |
| `duration-ms` | `300`         |                                                                                                                                                                                                                   |
| `easing`      | `"out-cubic"` | See [easing functions](#easing-functions).                                                                                                                                                                        |

Opening the overview zooms back out to show the whole image.

The wallpaper is decoded at the size the deepest zoom needs, `monitor / crop-ratio` per
axis, so the lower this is the more texture memory the image costs: at `0.25` that is
sixteen times the area of the screen, around 370 MB for a 3200x1800 output. That is the
reason for the floor, along with an absolute clamp at whatever `GL_MAX_TEXTURE_SIZE` the
driver reports.

### `[transition]`

| Key           | Default          | Meaning                                                                       |
| ------------- | ---------------- | ----------------------------------------------------------------------------- |
| `mode`        | `"fade"`         | `"fade"` crossfades the outgoing wallpaper into the incoming one. `"none"` swaps outright. |
| `duration-ms` | `800`            | Longer than the other sections, since replacing the image is a larger event than moving it. |
| `easing`      | `"in-out-cubic"` |                                                                               |

A fade holds both wallpapers, and both of their blurs, until it finishes. That is about
19 MB extra on a 2560x1440 output at the default `crop-ratio`, and it scales with the
same square as the figure above, so it is only worth thinking about alongside a
`crop-ratio` near its floor. `mode = "none"` gives that memory back and swaps instantly.

A monitor appearing, whether at startup or when it is plugged in, always snaps.

Two more cases a `"fade"` does not cover cleanly:

- A `parra set` part-way through a fade. Only two wallpapers are held at once, so the new
  one displaces whichever of the two on screen is the less visible.
- An outgoing wallpaper with no baked blur at the level the frame needs. It leaves the
  frame and the swap becomes instant, which looks better than crossfading a sharp half
  against a blurred one.

## Reloading

The daemon watches the configuration file, so an edit takes effect without anyone
sending `reload-config`. An editor that saves by writing a temporary file and renaming it
over the original is handled too. `[general] namespace` and
`[general] layer` are the exception: a layer surface is given both when it is created, so
those two take effect on the next start.

## Easing functions

`linear`, `out-quad`, `in-out-quad`, `out-cubic`, `in-out-cubic`, `out-quint`.

## Errors

Unknown keys are rejected rather than ignored, with the line and the accepted names.
Out-of-range values are reported with their full key path:

```
parra: config.toml: scroll.vertical.duration-ms: expected at most 60000 ms
```

## What is not configurable

Rendering device, driver vendor, buffer allocation, log level and every file location are
all controlled by mechanisms that already exist, so there is no key for any of them. See
[environment.md](environment.md).
