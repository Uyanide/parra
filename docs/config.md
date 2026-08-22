# Configuration

One file per compositor, and exactly one is read per run. The daemon detects which
compositor it is under and reads `$XDG_CONFIG_HOME/parra/<compositor>.toml`, falling back
to `$HOME/.config/parra/`. Under niri that is `niri.toml`. `--config PATH` overrides the
location.

A missing file is a working configuration: every key has a built-in default, and
[niri.example.toml](../niri.example.toml) lists them all.

Nothing is shared between two compositors' files. Anyone running two writes their
wallpaper, blur and transition settings in each, and there is no include mechanism.

Validate a file without starting anything, on any machine:

```sh
parra daemon --check --backend niri --config ./niri.toml
```

## Inheritance

Global sections set the value for every output. A `[output."<connector>"]` table overrides
individual keys for one monitor, and anything it leaves out is inherited. Connector names
are matched exactly as the compositor reports them, `DP-1` and `eDP-1` being the usual
shapes.

```toml
[blur]
radius = 48
downscale = 2

[output."DP-1"]
blur.radius = 16   # downscale stays 2
```

`[output."<connector>".compositor]` overrides the file's own `[compositor]` section key by
key, on the same rule.

## Keys

### `[general]`

Applies to every output; there is no per-output form.

| Key         | Default            | Meaning                                                       |
| ----------- | ------------------ | ------------------------------------------------------------- |
| `namespace` | the program's name | Layer-shell namespace, for compositor rules that match on it. |
| `layer`     | `"background"`     | `"background"` or `"bottom"`.                                 |

Both take effect on the next start; see [Reloading](#reloading).

### `[wallpaper]`

| Key        | Default | Meaning                                                                              |
| ---------- | ------- | ------------------------------------------------------------------------------------ |
| `fallback` | unset   | What to show when nothing has been set over the control socket. Unset shows nothing. |

`parra set` takes precedence and is remembered across restarts, so `fallback` is what a
monitor shows before anything has been chosen for it, and what it falls back to if the
chosen image will not load. See [usage.md](usage.md#choosing-a-wallpaper).

Paths may be absolute, start with `~/`, or be relative; a relative path resolves against
the config file's own directory. The path is not opened at load time, so one that does not
exist yet passes `--check` and is reported as a decode error when the image is first
shown.

### `[compositor]`

The one section whose keys differ between compositors. Under niri it says which position
moves each parallax axis:

| Key          | Default       | Meaning                                                     |
| ------------ | ------------- | ----------------------------------------------------------- |
| `vertical`   | `"workspace"` | `"workspace"`, `"column"` or `"none"`.                      |
| `horizontal` | `"none"`      | Same values. `"none"` leaves the axis pinned to its centre. |

```toml
[compositor]
vertical = "workspace"
horizontal = "column"   # turn on horizontal parallax
```

`"workspace"` follows the active workspace among that output's own workspaces; `"column"`
follows the focused column of that output's active workspace. Each monitor scrolls by its
own active workspace and that workspace's own column, so a monitor without the focus holds
its position. A workspace nothing has been focused on yet sits centred, and so does a
monitor whose focused window is floating or fullscreen.

One monitor can differ:

```toml
[compositor]
horizontal = "column"

[output."eDP-1".compositor]
horizontal = "none"   # vertical stays "workspace"
```

This section takes effect on the next start; see [Reloading](#reloading).
`scroll.<axis>.travel` changes how far an axis moves while the daemon runs, `0` included.

### `[scroll.vertical]` and `[scroll.horizontal]`

The two parallax axes take the same five keys and are configured apart. What moves each
one is `[compositor]` above; these say how far, which way and how fast it moves. Both are
per output.

| Key           | Default       | Meaning                                                                                      |
| ------------- | ------------- | -------------------------------------------------------------------------------------------- |
| `travel`      | `1.0`         | Fraction of the available travel to use, `0..=1`, measured about the centre. `0` pins it.    |
| `invert`      | `false`       | Runs the axis the other way. See below.                                                      |
| `max-shift`   | `0.5`         | Furthest the image may move between two adjacent stops, in screens. `0` lifts it. See below. |
| `duration-ms` | `300`         | `0` makes the move instant. Capped at 60000.                                                 |
| `easing`      | `"out-cubic"` | See [easing functions](#easing-functions).                                                   |

```toml
[scroll.vertical]
travel = 0.5

[scroll.horizontal]
duration-ms = 250   # travel and easing stay at their defaults
```

#### Running an axis the other way

`invert` mirrors the axis, so the first workspace or column shows the bottom or right of
the wallpaper and the last one shows the top or left.

```toml
[scroll.vertical]
invert = true   # workspace 1 starts at the bottom of the image
```

- An axis at its centre stays where it is when the key is toggled, so an output the
  compositor has reported no position for keeps its place.
- `travel` still applies: `travel = 0.5` covers the same half of the travel, backwards,
  and `travel = 0` stays pinned.
- `max-shift` measures the length of a stop, so it applies unchanged.
- It is inherited apart from `travel`, so one monitor can change how far an axis moves
  while keeping the direction:

  ```toml
  [scroll.vertical]
  invert = true

  [output."DP-1".scroll.vertical]
  travel = 0.5   # still inverted
  ```

#### A maximum shift

`travel` is a fraction of the **available travel**, which is whatever the cover fit and
[`zoom`](#zoom) leave outside the screen. That is a different distance on every wallpaper:
a 2937x4796 image on a 2560x1440 screen has 2.2 screen heights of it, so with three
workspaces one switch drags the image over a screen height in 300 ms.

`max-shift` states that distance in units the screen supplies. It is measured in screen
heights on the vertical axis and screen widths on the horizontal one, and caps how far the
image moves between two **adjacent** stops -- the next workspace along, or the next
column.

```toml
[scroll.vertical]
max-shift = 0.5   # one workspace along never moves the image more than half a screen
```

A jump across several stops at once, which a niri workspace switch can be, moves that many
times the cap.

More stops loosen the cap. One stop is `1 / (stops - 1)` of the travel, so the more stops
an axis has the shorter each one already is. On the wallpaper above, at `max-shift = 0.5`:

| workspaces | one switch, uncapped | with the cap | the image's total movement |
| ---------- | -------------------- | ------------ | -------------------------- |
| 2          | 2.23 screens         | 0.50         | 0.50                       |
| 3          | 1.11                 | 0.50         | 1.00                       |
| 5          | 0.56                 | 0.50         | 2.00                       |
| 6          | 0.45                 | 0.45         | 2.23, the whole image      |

Three more things it does:

- `travel` narrows the range a stop is taken from, so `travel = 0.5` halves the distance
  the cap measures and the cap does half as much.
- `0` lifts the cap. Pinning an axis is `travel = 0`, so one monitor asks for the whole
  travel back with:

  ```toml
  [scroll.vertical]
  max-shift = 0.5

  [output."DP-1".scroll.vertical]
  max-shift = 0     # the big monitor may use the whole image
  ```

- It works within the travel an axis already has. A wallpaper the shape of the screen has
  almost none, and the default never reaches it.

A compositor that pans the wallpaper continuously has no adjacent stop to measure, and
`max-shift` does nothing there.

### `[blur]`

| Key            | Default          | Meaning                                                                                             |
| -------------- | ---------------- | --------------------------------------------------------------------------------------------------- |
| `radius`       | `32`             | `0` disables blur entirely, including the bake. Capped at 512.                                      |
| `downscale`    | `4`              | Linear downscale of the baked blur texture, `1..=16`. Higher is cheaper in both VRAM and bake time. |
| `tint`         | `"#1e1e2e"`      | `#rgb`, `#rgba`, `#rrggbb` or `#rrggbbaa`.                                                          |
| `tint-opacity` | `0.5`            | `0..=1`, multiplied into the tint's own alpha.                                                      |
| `duration-ms`  | `300`            |                                                                                                     |
| `easing`       | `"in-out-cubic"` | See [easing functions](#easing-functions).                                                          |

An output blurs when the compositor drives it to, which under niri means it holds the
focused window, or when the control socket has asked for it; see
[usage.md](usage.md#the-blur-signal). Nothing focused anywhere leaves every output sharp.

`radius` is measured in texels of the wallpaper texture, which is decoded at the buffer
size times the deepest zoom. At rest one texel is one device pixel, so the configured
number is the blur's extent on screen, and one radius means the same thing on monitors at
different scales.

### `[zoom]`

| Key           | Default       | Meaning                                                                                                                                                                                              |
| ------------- | ------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `crop-ratio`  | `0.9`         | Fraction of the image visible while zoomed in, `0.25..=1`. The remainder is the headroom the parallax travels through, so `1.0` leaves nothing to scroll unless the image is taller than the screen. |
| `duration-ms` | `300`         |                                                                                                                                                                                                      |
| `easing`      | `"out-cubic"` | See [easing functions](#easing-functions).                                                                                                                                                           |

An output zooms back out to the whole image when the compositor drives it to, which under
niri means the overview is open.

The wallpaper is decoded at the size the deepest zoom needs, `monitor / crop-ratio` per
axis, so a lower ratio costs more texture memory: at `0.25` that is sixteen times the area
of the screen, around 370 MB for a 3200x1800 output. An absolute clamp at whatever
`GL_MAX_TEXTURE_SIZE` the driver reports applies on top.

### `[transition]`

| Key           | Default          | Meaning                                                                                    |
| ------------- | ---------------- | ------------------------------------------------------------------------------------------ |
| `mode`        | `"fade"`         | `"fade"` crossfades the outgoing wallpaper into the incoming one. `"none"` swaps outright. |
| `duration-ms` | `800`            |                                                                                            |
| `easing`      | `"in-out-cubic"` |                                                                                            |

A fade holds both wallpapers, and both of their blurs, until it finishes: about 19 MB
extra on a 2560x1440 output at the default `crop-ratio`, scaling with the same square as
the figure above. `mode = "none"` gives that memory back and swaps instantly.

Three cases snap whatever the mode says:

- A monitor appearing, at startup or when it is plugged in.
- A `parra set` part-way through a fade. Two wallpapers are held at once, so the new one
  displaces whichever of the two on screen is the less visible.
- An outgoing wallpaper with no baked blur at the level the frame needs. It leaves the
  frame and the swap becomes instant.

## Reloading

The daemon watches the configuration file, so an edit takes effect on save. The file is
read once the save has settled, 50 ms after the last thing that happened to it, so an
editor that writes a temporary file and renames it over the original causes one reload. A
configuration file or directory that is a symlink is followed to wherever it leads,
including after the link is re-pointed. `parra reload` asks for a re-read at any time.

Two sections are read once and take effect on the next start:

- `[general] namespace` and `[general] layer`, which a layer surface is given when it is
  created.
- `[compositor]`, which the backend is given when it connects.

Removing the directory the file lives in goes unnoticed, since the watch belongs to that
directory. `parra reload` picks the file up again, and so does restarting the daemon.

## Easing functions

`linear`, `out-quad`, `in-out-quad`, `out-cubic`, `in-out-cubic`, `out-quint`.

## Errors

Unknown keys are rejected with the line and the accepted names. Out-of-range values are
reported with their full key path:

```
parra: niri.toml: scroll.vertical.duration-ms: expected at most 60000 ms
```

## What is not configurable

Rendering device, driver vendor, buffer allocation, log level and every file location are
set through the environment instead; see [environment.md](environment.md).
