# Control protocol

The daemon listens on `$XDG_RUNTIME_DIR/parra-$WAYLAND_DISPLAY.sock`. The display name is
part of the filename, so two compositors in one login session get one daemon each.
`--socket PATH` overrides it.

One JSON value per line, request and response alike. The `parra` subcommands that send
these are documented in [cli.md](cli.md).

## Conventions

- Variant names are kebab-case. Field names are snake_case, so `jq` paths need no quoting.
- A request with no fields is a bare string.
- A request carrying a field the daemon does not know is refused.
- A line that is not a request is answered with an error, and the connection stays open.
- A socket left behind by a daemon that did not clean up is taken over. A socket something
  still answers on is refused.

## Protocol version

`ping` reports it and every snapshot carries it. It is bumped whenever the wire format
changes, including when it only gains a field, so a client can tell a stale daemon from an
unreachable one and can tell whether a field it wants exists at all.

A version skew fails to parse. `parra` prints `the daemon speaks protocol N, this build
speaks M; restart the daemon` and exits 4. Restarting the daemon is the whole fix.

## Requests

| Request                                                 | Meaning                                                                                                         |
| ------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------- |
| `"get-state"`                                           | Every output.                                                                                                   |
| `{"get-output":{"output":"DP-1"}}`                      | One output.                                                                                                     |
| `{"set-wallpaper":{"output":null,"path":"/srv/a.png"}}` | Show an image and remember it. A `null` path empties the slot instead; `"save":false` does not remember either. |
| `{"restore-wallpaper":{"output":null}}`                 | Put the recorded wallpapers back. `null` addresses every slot.                                                  |
| `{"set-blur":{"output":"DP-1","on":true}}`              | External blur signal. `null` broadcasts, and a broadcast also clears any per-output requests.                   |
| `"reload-config"`                                       | Re-read the config file.                                                                                        |
| `"subscribe"`                                           | Turn this connection into a stream of [events](#events).                                                        |
| `"ping"`                                                | Liveness, and the protocol version.                                                                             |

`output` and `path` are required fields even where they accept `null`.

### `set-wallpaper`

Takes an absolute path; the `set` command canonicalizes one first (see [cli.md](cli.md)).
The daemon checks that the path is a file while the client is still on the line, and
decodes after the reply, so the request is answered immediately and whatever is on screen
stays there until the new image is ready. An image that will not decode is reported in the
log and on the [event stream](#events), and that output falls back to `[wallpaper]
fallback` with the record left alone, so a drive that was not mounted yet recovers on its
own.

`save` defaults to true. `false` shows the image for this session only, leaving the
recorded one alone.

Setting the same path twice takes effect again, so an image edited in place is picked up.

A `null` path empties the addressed slot, and that output is resolved again from the top:
clearing one monitor's own wallpaper reveals the one every other monitor is on, and
clearing that reveals `[wallpaper] fallback`. With nothing under it either, the output ends
up showing nothing: what was on screen fades out over the configured transition and the
surface is left transparent. A `null` output empties every slot at once, per-output ones
included. The `unset` command sends this.

A wallpaper set this way is remembered across restarts; `[wallpaper] fallback` applies
when nothing has been set. See [usage.md](usage.md#choosing-a-wallpaper).

### `restore-wallpaper`

The counterpart to `"save":false`: it changes the screen back to what the record says.
Every slot it addresses is emptied first, so a wallpaper set over the socket since is
dropped whether or not the record has anything to put in its place. `null` addresses every
slot and drops the per-output requests too; naming an output restores that output's own
slot and leaves the broadcast one alone.

The record keeps each wallpaper's identity as well as its path, so restoring what is
already on screen is a no-op and reports nothing. A recorded image that will not load is
offered again.

### `reload-config`

Re-reads the file. A parse error is answered to the client and the daemon keeps running on
the configuration it already had. `[general]` and `[compositor]` changes are reported in
the log and take effect on the next start. The daemon also watches the file on its own;
see [config.md](config.md#reloading).

## Responses

| Response                      | Meaning                      |
| ----------------------------- | ---------------------------- |
| `"done"`                      | The request was carried out. |
| `{"pong":{"version":4}}`      | Protocol version.            |
| `{"state":{...}}`             | A `StateSnapshot`.           |
| `{"output":{...}}`            | One `OutputSnapshot`.        |
| `{"error":{"message":"..."}}` | The request was refused.     |

### Snapshot shape

```json
{
  "version": 4,
  "namespace": "...",
  "frames": 442,
  "texture_bytes": 56173364,
  "startup_us": 139963,
  "outputs": [
    {
      "name": "DP-1",
      "logical": { "w": 2560, "h": 1440 },
      "scale": 1.0,
      "wallpaper": "/srv/a.png",
      "scroll": {
        "vertical": { "current": 0.33, "target": 0.66 },
        "horizontal": { "current": 0.5, "target": 0.5 }
      },
      "blur": {
        "amount": { "current": 1.0, "target": 1.0 },
        "radius": 32,
        "downscale": 4,
        "tint": "#1e1e2e80"
      },
      "zoom": { "current": 1.111, "target": 1.111 },
      "channels": {
        "x": { "at": 0.5, "stride": 0.0 },
        "y": { "at": 0.333, "stride": 0.25 },
        "blur": true,
        "zoom_out": false
      },
      "gpu": { "last_us": 142, "peak_us": 185 },
      "settled": true
    }
  ]
}
```

- `channels` is what the compositor is driving this output to, before any configuration is
  applied: two scroll axes, and whether the output should be blurred or zoomed out. The
  animated values elsewhere are where those have got to.
- Each axis is an `at` normalized to `0..=1` and the `stride` one of its stops covers in
  the same units -- `1 / (stops - 1)`, or `0` for an axis that pans continuously or has
  nothing to travel between. `stride` turns [`max-shift`](config.md#a-maximum-shift) from a
  distance in screens into a fraction, and changes when workspaces or columns open and
  close.
- Animated values report both ends. `current` is what is on screen, `target` is where it
  is going.
- `settled` is false exactly while frames are being submitted for that output.
- `tint` already has `tint-opacity` folded into its alpha.

### The measurements

Every duration is in microseconds.

- `frames` counts every frame presented on every output. An idle daemon submits none, so
  two readings a minute apart are the whole of the idle check.
- `texture_bytes` is the video memory held by wallpapers, sharp and baked together.
- `startup_us` runs from the first instruction of the process to the first frame on a
  screen, and is `null` until there has been one. That frame is the first of the arrival
  rather than the finished wallpaper, so the number marks when the pipeline was ready, not
  when anything was visible.
- `gpu` is what the GPU spent on that output's last frame, and on its most expensive one so
  far. Both are `null` where the driver has no usable timer, which is a property of the
  driver: either all outputs report or none do.

`peak_us` is elapsed time. Nothing is submitted while everything is settled, so the GPU
drops to its lowest clock and the first frame of the next animation is measured there:
about 9000 microseconds here against a steady state of 150. A peak several times the
typical frame is the normal reading.

## Events

`subscribe` turns the connection into a one-way stream. The `parra events` command that
does this is documented in [cli.md](cli.md).

```
-> "subscribe"
<- "done"                      the subscription was accepted
<- {"output-ready":{...}}      one per output that already exists
<- {"output-ready":{...}}
   ...one line per event, until the daemon stops...
```

The daemon reads nothing more from a subscribed connection. Use a second connection for
other requests; `state` and `set` cost one round trip each.

A subscriber that stops reading is dropped: the daemon queues a bounded number of events
for it and closes the connection when that fills. A client that sees the stream end
reconnects and is described again from scratch.

### What is reported

| Event                                 | Meaning                                                                                 |
| ------------------------------------- | --------------------------------------------------------------------------------------- |
| `{"animation":{...}}`                 | One animated value has started moving.                                                  |
| `{"wallpaper-changed":{...}}`         | An output is showing something else.                                                    |
| `{"wallpaper-failed":{"path":"..."}}` | An image will not decode. Reported once for the image.                                  |
| `{"output-ready":{...}}`              | The daemon now holds state for an output.                                               |
| `{"output-gone":{"output":"DP-1"}}`   | It no longer does.                                                                      |
| `"config-reloaded"`                   | The config file was re-read and adopted. A reload that changed nothing is not reported. |

```json
{"animation":{"output":"DP-1","property":"blur","from":0.0,"to":1.0,"duration_us":300000,"easing":"in-out-cubic"}}
{"wallpaper-changed":{"output":"DP-1","from":"/srv/a.png","to":"/srv/b.png","duration_us":800000,"easing":"in-out-cubic"}}
{"output-ready":{"output":"eDP-1","wallpaper":"/srv/a.png","values":{"scroll_vertical":0.0,"scroll_horizontal":0.5,"blur":0.0,"zoom":1.1111112}}}
```

`property` is one of `scroll-vertical`, `scroll-horizontal`, `blur` or `zoom`, in the units
a snapshot reports them in: `0..=1` for the first three, a multiplier for the zoom.
`easing` uses the names the config file uses.

`output-ready` carries where the values start, since a monitor appearing snaps. It fires
once the layer surface is configured, which is later than the compositor knowing the
monitor exists. Its wallpaper is the one thing that does not snap, so a `wallpaper-changed`
with `from` of `null` follows it describing the arrival.

A `to` of `null` is the same event the other way round: the slot is empty from that moment,
and the duration is how long what was on it takes to leave the screen.

`wallpaper-failed` is the one thing a client cannot learn any other way, since
`set-wallpaper` is answered before the decode. It is followed by the `wallpaper-changed` of
each output falling back.

### Animation rules

An animation is described when it starts and never again, so a client runs the same curve
against its own clock. Four rules make that work:

- A later event for the same output and property replaces the earlier one, and its `from`
  is the value mid-flight, so a redirected animation is fully described.
- `duration_us` of `0` means jump. That covers a zero-duration tween,
  `transition.mode = "none"` and `transition.at-start = false`.
- Nothing is reported when nothing changed, including re-resolving to the value an output
  already rests at.
- The start event says when it ends, and no settled event follows.

### What is not reported

- Where an animation has got to, and anything else per frame. `get-state` reads those.
- What the compositor is driving. `get-state` reports it as `channels`.
- Blur `radius`, `downscale` and `tint`, and every other configured parameter. They change
  only with the config file, so `config-reloaded` is the signal to read `get-state` again.
