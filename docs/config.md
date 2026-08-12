# Configuration

The daemon reads `$XDG_CONFIG_HOME/parra/config.toml`, falling back to
`$HOME/.config/parra/config.toml`. `--config PATH` overrides the location.

A missing file is not an error: the built-in defaults are a working configuration.

[config.example.toml](../config.example.toml) lists every key with its default.

Validate a file without starting anything:

```sh
parra daemon --check --config ./config.toml
```

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

Blur and scrolling are per output and independent: one monitor can be blurred and
scrolled while another is sharp and still.

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

| Key    | Default | Meaning                                                               |
| ------ | ------- | --------------------------------------------------------------------- |
| `path` | unset   | Image to display. Unset starts blank, waiting for the control socket. |

Paths may be absolute, start with `~/`, or be relative. A relative path resolves against
the config file's own directory, since a daemon's working directory is not something a
user can reason about.

The file is not opened at load time. A path that does not exist yet is a decode error
later, rather than a configuration error.

### `[scroll.vertical]` and `[scroll.horizontal]`

The two parallax axes take the same four keys and are configured apart, because a
compositor animates a workspace switch and a column move as two separate animations that
need not agree. The vertical axis follows the active workspace, the horizontal one the
column in the scrolling layout. Both are per output.

| Key           | Default                              | Meaning                                                                                                                                                             |
| ------------- | ------------------------------------ | --------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `enabled`     | `true` vertical, `false` horizontal  | When false the image is pinned to its centre on that axis. Enabling `[scroll.horizontal]` is all that turning on horizontal parallax takes.                          |
| `travel`      | `1.0`                                | Fraction of the available travel to use, `0..=1`, measured about the centre. `0.5` halves the excursion in both directions rather than biasing it toward one edge. |
| `duration-ms` | `400`                                | `0` makes the move instant. Capped at 60000.                                                                                                                        |
| `easing`      | `"out-cubic"`                        | See below.                                                                                                                                                          |

```toml
[scroll.vertical]
travel = 0.5

[scroll.horizontal]
enabled = true
duration-ms = 250   # travel and easing stay at their defaults
```

Each monitor scrolls by its own active workspace and that workspace's own column, so a
monitor without the focus holds its position rather than drifting to the centre. A
workspace nothing has been focused on yet sits centred, and so does a monitor whose
focused window is floating or fullscreen and therefore has no place in the scroll.

### `[blur]`

| Key            | Default          | Meaning                                                                                                                                                               |
| -------------- | ---------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `radius`       | `32`             | `0` disables blur entirely, including the bake. Capped at 512.                                                                                                        |
| `downscale`    | `4`              | Linear downscale of the baked blur texture, `1..=16`. Higher is cheaper in both VRAM and bake time; blur removes the detail that downsampling would have cost anyway. |
| `tint`         | `"#1e1e2e"`      | `#rgb`, `#rgba`, `#rrggbb` or `#rrggbbaa`.                                                                                                                            |
| `tint-opacity` | `0.5`            | `0..=1`, multiplied into the tint's own alpha.                                                                                                                        |
| `duration-ms`  | `400`            |                                                                                                                                                                       |
| `easing`       | `"in-out-cubic"` |                                                                                                                                                                       |

An output blurs when it holds the focused window, or when the control socket has asked
for it. Nothing focused anywhere leaves every output sharp.

### `[overview]`

| Key           | Default       | Meaning                                                                                                                                                                                                           |
| ------------- | ------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `crop-ratio`  | `0.9`         | Fraction of the image visible while the overview is closed, `0.01..=1`. The remainder is the headroom the parallax travels through, so `1.0` leaves nothing to scroll unless the image is taller than the screen. |
| `duration-ms` | `400`         |                                                                                                                                                                                                                   |
| `easing`      | `"out-cubic"` |                                                                                                                                                                                                                   |

Opening the overview zooms back out to show the whole image.

### `[transition]`

| Key           | Default          | Meaning                                                                                                      |
| ------------- | ---------------- | ------------------------------------------------------------------------------------------------------------ |
| `mode`        | `"none"`         | `"none"` or `"fade"`. `fade` parses and animates in the state model, but the renderer still swaps instantly. |
| `duration-ms` | `400`            |                                                                                                              |
| `easing`      | `"in-out-cubic"` |                                                                                                              |

## Easing functions

`linear`, `out-quad`, `in-out-quad`, `out-cubic`, `in-out-cubic`, `out-quint`.

## Errors

Unknown keys are rejected rather than ignored, with the line and the accepted names.
Out-of-range values are reported with their full key path, including the output table:

```
config.toml: output."DP-1".blur.radius: expected an integer in 0..=512
```

## What is not configurable

Rendering device, driver vendor, buffer allocation, log level and socket locations are
all controlled by mechanisms that already exist, so there is no key for any of them. See
[environment.md](environment.md).
