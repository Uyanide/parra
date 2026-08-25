# Configuration

One file per compositor, and exactly one is read per run. The daemon detects which
compositor it is under and reads `$XDG_CONFIG_HOME/parra/<compositor>.toml`, falling back
to `$HOME/.config/parra/`. Under niri that is `niri.toml`, and under Hyprland
`hyprland.toml` -- named after the backend, in lower case, and not after
`$XDG_CURRENT_DESKTOP`, which Hyprland sets to `Hyprland`. `--config PATH` overrides the
location.

A missing file is a working configuration: every key has a built-in default, and
[niri.example.toml](../examples/niri.example.toml) and
[hyprland.example.toml](../examples/hyprland.example.toml) list them all.

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

The one section whose keys differ between compositors. Under both it says which position
moves each parallax axis and when an output blurs, and under Hyprland also what that axis
travels through.

#### Under niri

| Key          | Default                              | Meaning                                                                    |
| ------------ | ------------------------------------ | -------------------------------------------------------------------------- |
| `vertical`   | `"workspace"`                        | `"workspace"`, `"column"` or `"none"`.                                     |
| `horizontal` | `"none"`                             | Same values. `"none"` leaves the axis pinned to its centre.                |
| `blur`       | `{ when = "non-empty", scope = "output", overview = "clear" }` | When an output blurs. See [When an output blurs](#when-an-output-blurs). |

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

#### Under Hyprland

| Key          | Default                                | Meaning                                                                    |
| ------------ | -------------------------------------- | -------------------------------------------------------------------------- |
| `vertical`   | `"none"`                               | `"workspace"` or `"none"`.                                                 |
| `horizontal` | `"workspace"`                          | Same values. `"none"` leaves the axis pinned to its centre.                |
| `span`       | `10`                                   | The workspaces the travel covers. See [The span](#the-span).               |
| `blur`       | `{ when = "non-empty", scope = "output" }` | When an output blurs. See [When an output blurs](#when-an-output-blurs). |

```toml
[compositor]
vertical = "none"
horizontal = "workspace"
span = 10
```

Sideways by default, because that is the way Hyprland moves a workspace switch. There is
no `"column"`: Hyprland's layouts report no position within a workspace, so a second axis
would have nothing to follow that the first does not already.

#### When an output blurs

`when` and `scope` are written the same way under either compositor:

| Key     | Default    | Meaning                                                                                |
| ------- | ---------- | -------------------------------------------------------------------------------------- |
| `when`  | `"non-empty"` | `"focused"`: the output holds the focused window. `"non-empty"`: the workspace it is showing holds at least one window, whether or not one is focused. |
| `scope` | `"output"` | `"output"`: each output answers for itself. `"global"`: every output blurs as soon as one of them answers yes. |

```toml
[compositor]
blur = { when = "non-empty", scope = "global" }
```

`"non-empty"` is what keeps a monitor blurred while the focus is somewhere else: on another
monitor, or on a launcher or other layer surface, which leaves no window focused at all.
An empty workspace is sharp either way.

Which windows count depends on the compositor only where the two differ in what they have.
Under niri a floating or fullscreen window counts, having a workspace but no place in the
scroll. Under Hyprland a special workspace drawn over the active one counts for nothing:
what is read is the workspace the monitor is showing, which a scratchpad does not replace.

`scope` reads what every output reached, so `"global"` blurs a second monitor that has
nothing on it because the first one qualifies.

Niri also takes an `overview` key, which Hyprland's `blur` does not:

| Key        | Default    | Meaning                                                                          |
| ---------- | ---------- | --------------------------------------------------------------------------------- |
| `overview` | `"clear"` | `"clear"`: sharp for as long as the overview is open. `"blur"`: blurred for as long as it is open. `"follow"`: the overview does not change what `when` and `scope` decided. |

```toml
[compositor]
blur = { when = "non-empty", scope = "output", overview = "blur" }
```

It reaches every output at once, the same state that drives [`[zoom]`](#zoom) back out to
the whole image.

Either of `when` and `scope` can be set per monitor, and each is read from the monitor it is
set on: `when` is the question that monitor answers, `scope` is how widely it reads the
answers. A monitor set to `"output"` still contributes its answer to what the others read,
so one set to `"global"` can blur because of it, and where two monitors answer different
`when`s, `"global"` blurs on either of them reaching its own. `overview` reads the same
state on every monitor, so setting it on one alone changes nothing by itself; it takes
effect paired with a `when`/`scope` override on that same monitor.

`[blur]` below says what a blur looks like and how long it takes to arrive; this says when
one happens. The external signal is a third thing again, ORed with both; see
[usage.md](usage.md#the-blur-signal).

#### Override per monitor

One monitor can differ, on either compositor:

```toml
[compositor]
horizontal = "column"
blur = { when = "non-empty", scope = "output" }

[output."eDP-1".compositor]
horizontal = "none"          # vertical stays "workspace"
blur = { when = "focused" }  # scope stays "output"
```

An object is merged key by key like any other part of the file, so an override names only
what it changes.

This section takes effect on the next start; see [Reloading](#reloading).
`scroll.<axis>.travel` changes how far an axis moves while the daemon runs, `0` included.

#### The span

Hyprland only names its workspaces. They are global rather than per monitor, and it creates
and destroys them as they are used, so there is no position it can report and no live count
worth reading: counting the workspaces that happen to exist would change the length of the
travel whenever one appeared or went away, moving the wallpaper with no user action behind
it. `span` declares the travel instead.

A number is shorthand for the workspaces named `"1"` through `"N"`:

```toml
[compositor]
span = 10
```

A list names them in the order they should be travelled through, which is what workspaces
carrying names need:

```toml
[compositor]
span = ["browser", "code", "mail"]
```

An entry written `"3-6"` is the range `"3"` to `"6"`, both ends included, and expands where
it stands. Written backwards it counts down, so `["9-7", "mail"]` travels `"9"`, `"8"`,
`"7"`, `"mail"`. A hyphen anywhere but between digits belongs to a name, which leaves
`"my-project"` and `"-1"` single workspaces.

The order is the list's own, not the names sorted: `["3", "1", "5"]` puts `"1"` in the
middle. It sets where each stop sits and nothing else, which is why the placement rules
below go by number.

No workspace may be listed twice, counting `"1"` and `"01"` as the same one, and a span
covers at most 1000 of them.

It is per output like everything else. Hyprland numbers workspaces across every monitor
rather than restarting on each, so a second monitor shows only part of the range and
travels through only part of its wallpaper. Give it the workspaces it actually shows:

```toml
[compositor]
span = 5                       # everywhere, unless said otherwise below

[output."HDMI-A-1".compositor]
span = ["6", "7", "8"]         # the three this one actually shows
```

A count is shorthand for `"1"` through `"N"` and nothing else, so `span = 3` there would
mean the workspaces `"1"`, `"2"` and `"3"`. It does not mean "three workspaces on this
monitor". Anything else has to be named.

The span is one coordinate space, and sharing it is what makes a monitor use only part of
its travel. Leave `"1"` through `"5"` global while one monitor only ever shows `"2"`, and
that monitor sits at 25% forever; a monitor showing `"1"`, `"3"` and `"4"` steps 0%, 50%,
75% rather than evenly. Declaring each monitor's own workspaces is what gives it the whole
travel and even steps between them.

Two monitors may name the same workspace, which is allowed and often right: Hyprland can
put it on either. Only the monitor actually showing it ever matches, so the two cannot
disagree.

A workspace the span does not list still has to land somewhere. Where every entry in the
span is a number it lands on the nearest of them by number, which also clamps anything past
either end. Two entries equally far off are settled by the workspace the monitor was showing
before, the nearer of the two to that one winning:

| `span`            | Was showing | Workspace | Sits at | Why                                 |
| ----------------- | ----------- | --------- | ------- | ----------------------------------- |
| `10`              | anything    | `"14"`    | `"10"`  | Past the last, so clamped to it     |
| `["1", "3", "6"]` | anything    | `"5"`     | `"6"`   | Nearer `"6"` than `"3"`             |
| `["3", "5"]`      | `"1"`       | `"4"`     | `"3"`   | Equally far; `"3"` is nearer `"1"`  |
| `["3", "5"]`      | `"10"`      | `"4"`     | `"5"`   | Equally far; `"5"` is nearer `"10"` |
| `["3", "5"]`      | nothing yet | `"4"`     | `"3"`   | Equally far, so the lower number    |

The last row covers the daemon's first update after it starts, and a monitor whose
earlier workspace has a name that no number describes.

Where the span carries names there is no distance to measure, so an unlisted workspace sits
centred instead. Centre is a position like any other: with `["2", "4", "6"]` it is exactly
where `"4"` sits. A workspace whose name is not a number sits centred either way.

### `[scroll.vertical]` and `[scroll.horizontal]`

The two parallax axes take the same five keys and are configured apart. What moves each
one is `[compositor]` above; these say how far, which way and how fast it moves. Both are
per output.

| Key           | Default       | Meaning                                                                                      |
| ------------- | ------------- | -------------------------------------------------------------------------------------------- |
| `travel`      | `1.0`         | Fraction of the available travel to use, `0..=1`, measured about the centre. `0` pins it.    |
| `invert`      | `false`       | Runs the axis the other way. See below.                                                      |
| `max-shift`   | `0.3`         | Furthest the image may move between two adjacent stops, in screens. `0` lifts it. See below. |
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
image moves between two **adjacent** stops -- the next workspace along, the next column,
or the next entry in a Hyprland [span](#the-span).

```toml
[scroll.vertical]
max-shift = 0.3   # one workspace along never moves the image more than a third of a screen
```

A jump across several stops at once, which a niri workspace switch or a jump across a
Hyprland span can be, moves that many times the cap.

More stops loosen the cap. One stop is `1 / (stops - 1)` of the travel, so the more stops
an axis has the shorter each one already is. On the wallpaper above, at `max-shift = 0.3`:

| workspaces | one switch, uncapped | with the cap | the image's total movement |
| ---------- | -------------------- | ------------ | -------------------------- |
| 2          | 2.23 screens         | 0.30         | 0.30                       |
| 3          | 1.11                 | 0.30         | 0.60                       |
| 5          | 0.56                 | 0.30         | 1.20                       |
| 6          | 0.45                 | 0.30         | 1.50                       |

Four more things it does:

- `travel` narrows the range a stop is taken from, so `travel = 0.5` halves the distance
  the cap measures and the cap does half as much.
- It is measured at the crop [`zoom.crop-ratio`](#zoom) fixes, which is the closest in the
  wallpaper ever sits. A stop moves the image less than this while the output is zoomed
  out.
- `0` lifts the cap. Pinning an axis is `travel = 0`, so one monitor asks for the whole
  travel back with:

  ```toml
  [scroll.vertical]
  max-shift = 0.3

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
| `tint`         | `"#101010"`      | `#rgb`, `#rgba`, `#rrggbb` or `#rrggbbaa`.                                                          |
| `tint-opacity` | `0.5`            | `0..=1`, multiplied into the tint's own alpha.                                                      |
| `duration-ms`  | `300`            |                                                                                                     |
| `easing`       | `"in-out-cubic"` | See [easing functions](#easing-functions).                                                          |

An output blurs when the compositor drives it to, or when the control socket has asked for
it; see [usage.md](usage.md#the-blur-signal). What the compositor drives it on is
[`[compositor] blur`](#when-an-output-blurs), which by default is whether the output's
active workspace holds any window.

`radius` is measured in texels of the wallpaper texture, which is decoded at the buffer
size times the deepest zoom. At rest one texel is one device pixel, so the configured
number is the blur's extent on screen, and one radius means the same thing on monitors at
different scales.

`tint` follows the wallpaper's own coverage, so it reaches a transparent part of an image
as far as that part is there; see [usage.md](usage.md#transparent-wallpapers).

### `[zoom]`

| Key           | Default       | Meaning                                                                                                                                                                                              |
| ------------- | ------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `crop-ratio`  | `0.9`         | Fraction of the image visible while zoomed in, `0.25..=1`. The remainder is the headroom the parallax travels through, so `1.0` leaves nothing to scroll unless the image is taller than the screen. |
| `duration-ms` | `300`         |                                                                                                                                                                                                      |
| `easing`      | `"out-cubic"` | See [easing functions](#easing-functions).                                                                                                                                                           |

An output zooms back out to the whole image when the compositor drives it to, which under
niri means the overview is open. Hyprland reports no wider view of an output, so nothing
ever drives it there: `crop-ratio` is a fixed crop and the two keys beside it never fire.
It is still doing its main job, which is leaving headroom for the parallax to travel
through.

The same overview state can also decide whether an output blurs; see niri's `overview` key
under [When an output blurs](#when-an-output-blurs).

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
| `at-start`    | `true`           | Whether a wallpaper arriving on an output that has none fades up from what is drawn below. |

A fade holds both wallpapers, and both of their blurs, until it finishes: about 19 MB
extra on a 2560x1440 output at the default `crop-ratio`, scaling with the same square as
the figure above. `mode = "none"` gives that memory back and swaps instantly.

`at-start` covers the daemon starting, a monitor being plugged in, and any other moment an
output goes from showing nothing to showing something. It uses the same `duration-ms` and
`easing`, and `mode = "none"` turns it off along with the crossfade. What shows through
while it runs is whatever the compositor draws below the layer surface; see
[usage.md](usage.md#transparent-wallpapers).

An output going the other way, from showing something to showing nothing, fades out over
the same `duration-ms` and `easing` and is left transparent.

Two cases snap whatever the mode says:

- A `parra set` part-way through a fade. Two wallpapers are held at once, so the new one
  displaces whichever of the two on screen is the less visible. What the frame as a whole
  is doing is not disturbed: an arrival goes on from where it had got to, and a wallpaper
  on its way out turns back from there.
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
