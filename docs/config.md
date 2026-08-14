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
back to if the chosen image will not load. See [state and cache](#state-and-cache).

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
| `duration-ms` | `400`                               | `0` makes the move instant. Capped at 60000.                                 |
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
| `duration-ms`  | `400`            |                                                                                                     |
| `easing`       | `"in-out-cubic"` | See [easing functions](#easing-functions).                                                          |

An output blurs when it holds the focused window, or when the control socket has asked
for it. Nothing focused anywhere leaves every output sharp.

### `[overview]`

| Key           | Default       | Meaning                                                                                                                                                                                                           |
| ------------- | ------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `crop-ratio`  | `0.9`         | Fraction of the image visible while the overview is closed, `0.25..=1`. The remainder is the headroom the parallax travels through, so `1.0` leaves nothing to scroll unless the image is taller than the screen. |
| `duration-ms` | `400`         |                                                                                                                                                                                                                   |
| `easing`      | `"out-cubic"` | See [easing functions](#easing-functions).                                                                                                                                                                        |

Opening the overview zooms back out to show the whole image.

The wallpaper is decoded at the size the deepest zoom needs, `monitor / crop-ratio` per
axis, so the lower this is the more texture memory the image costs: at `0.25` that is
sixteen times the area of the screen, around 370 MB for a 3200x1800 output. That is the
reason for the floor, along with an absolute clamp at whatever `GL_MAX_TEXTURE_SIZE` the
driver reports.

### `[transition]`

| Key           | Default          | Meaning                                                                                                      |
| ------------- | ---------------- | ------------------------------------------------------------------------------------------------------------ |
| `mode`        | `"none"`         | `"none"` or `"fade"`. `fade` parses and animates in the state model, but the renderer still swaps instantly. |
| `duration-ms` | `400`            |                                                                                                              |
| `easing`      | `"in-out-cubic"` |                                                                                                              |

## State and cache

Two more locations, neither of them meant to be edited by hand. `--state PATH` and
`--cache-dir PATH` override them.

| Location                           | Holds                                                                        |
| ---------------------------------- | ---------------------------------------------------------------------------- |
| `$XDG_STATE_HOME/parra/state.toml` | Which wallpaper each slot was last set to, so a restart restores it.         |
| `$XDG_CACHE_HOME/parra/*.qoi`      | Those wallpapers, already resized, so a restart skips decoding the original. |

`$HOME/.local/state` and `$HOME/.cache` are the fallbacks, as the XDG specification
prescribes.

The state file records the path you asked for, never the copy. It is rewritten by `parra
set` and `parra unset` and by nothing else, so an image that will not load stays recorded:
the daemon logs it, falls back for that session, and tries again on the next start.
Deleting either location is safe. The state file is what a restart shows; the cache is
only speed, and every file in it can be produced again from the original.

Do _NOT_ edit it by hand. The daemon reads it once at startup and rewrites it whole on
every change, so an edit made while it is running is overwritten with no warning. Use
`parra unset` to take back a wallpaper:

```sh
parra unset --output DP-1     # DP-1 goes back to whatever every output is on
parra unset                   # every output goes back to `fallback`
```

Clearing reveals rather than blanks, walking the same order the daemon resolves in: an
output's own wallpaper, then the one set for every output, then `fallback`, then nothing.
The copy of whatever is dropped is swept in the same breath.

A copy is kept at the size the largest monitor showing it needs. It is used again as long
as it still covers that, and re-made from the original when it does not, which is what a
rotation, a resolution change, a scale change or a smaller `crop-ratio` all amount to.
The old copy keeps drawing meanwhile, so nothing stalls. Copies no longer pointed at are
deleted when the daemon starts and after every `set` or `unset`.

`--no-save` works on both and means the same thing on each: change what is on screen now
and leave the file alone, so the next start goes back to what it records. On `set` that is
a wallpaper shown without being adopted; on `unset` it is one dropped without being
forgotten.

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
