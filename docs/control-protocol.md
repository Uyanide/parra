# Control protocol

The daemon listens on `$XDG_RUNTIME_DIR/parra-$WAYLAND_DISPLAY.sock`. The display
name is part of the filename so two compositors in one login session get one daemon
each. `--socket PATH` overrides it.

One JSON value per line, request and response alike. The `parra` subcommands that send
these are documented in [cli.md](cli.md).

## How the daemon answers

The socket is bound before the first frame, so a client that starts alongside the daemon
either finds it or finds nothing. A path left behind by a daemon that did not get to clean
up is taken over; a path something still answers on is refused, because two daemons
sharing one socket would each get an arbitrary half of the requests.

No request can stall a frame, and a connection that opens and then says nothing holds up
nobody. A line that is not a request is answered with an error rather than by closing the
connection.

## Conventions

Variant names are kebab-case. Field names are snake_case, so `jq` paths need no quoting. A
request with no fields is a bare string.

A request carrying a field the daemon does not know is refused.

## Protocol version

`ping` reports it and every snapshot carries it. It is bumped whenever the wire format
changes, including when it only gains a field, so a client can tell a stale daemon from an
unreachable one and can tell whether a field it wants exists at all.

Rejecting unknown fields means a skew cannot pass unnoticed: a request from a newer client,
or a reply to an older one, fails to parse rather than half working. A client that answers
a refusal by comparing `ping` against the version it was built for can report the skew
instead. `parra` prints `the daemon speaks protocol N, this build speaks M; restart the
daemon` and exits 4.

This is the partial upgrade: a new binary talking to a daemon still running from before it
was replaced. Restarting the daemon is the whole fix.

## Requests

| Request                                                 | Meaning                                                                                                                      |
| ------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------- |
| `"get-state"`                                           | Every output.                                                                                                                |
| `{"get-output":{"output":"DP-1"}}`                      | One output.                                                                                                                  |
| `{"set-wallpaper":{"output":null,"path":"/srv/a.png"}}` | Show an image and remember it. A `null` path empties the slot instead; `"save":false` does not remember either.              |
| `{"set-blur":{"output":"DP-1","on":true}}`              | External blur signal. `null` broadcasts, and a broadcast also clears any per-output requests. |
| `"reload-config"`                                       | Re-read the config file.                                                                                                     |
| `"subscribe"`                                           | Turn this connection into a stream of [events](#events).                                                                     |
| `"ping"`                                                | Liveness, and the protocol version.                                                                                          |

`set-wallpaper` takes an absolute path: the daemon's working directory is not the
caller's. The daemon refuses anything that is not a file while the client is still on the
line; the `set` command canonicalizes the path first (see [cli.md](cli.md)).
Decoding happens after the reply, so the request is answered immediately and whatever is
on screen stays there until the new image is ready. A file that turns out not to be an
image is reported in the log, not to the client that asked.

A wallpaper set this way is remembered across restarts, and `[wallpaper] fallback` in the
config file is only what to show when nothing has been set. How the two relate, and where
the choice is kept, is in [usage.md](usage.md#choosing-a-wallpaper).

`save` defaults to true. `false` shows the image for this session only, leaving the
recorded one alone, so the next start goes back to it.

Setting the same path twice is not a no-op: every set is a distinct wallpaper, so an
image edited in place takes effect.

A `null` path empties the addressed slot. What that output shows is then resolved again
from the top, so clearing one monitor's own wallpaper reveals the one every other monitor
is on, and clearing that reveals `[wallpaper] fallback`. `null` for the output empties
every slot at once, per-output ones included. The `unset` command sends this (see
[cli.md](cli.md)).

The field is required even though it is nullable, so a client whose path came out
undefined is refused rather than clearing a wallpaper.

An image that will not load is reported in the log, that output falls back to
`[wallpaper] fallback`, and what was recorded is left alone so the next start tries it
again. A drive that was not mounted yet therefore recovers on its own.

`reload-config` re-reads the file and answers with the parse error if there is one, and
the daemon keeps running on the configuration it already had. The namespace and layer are
the exception: a change to either is reported in the log and takes effect on the next
start. The daemon also watches the configuration file on its own; see
[config.md](config.md#reloading).

## Responses

| Response                      | Meaning                      |
| ----------------------------- | ---------------------------- |
| `"done"`                      | The request was carried out. |
| `{"pong":{"version":2}}`      | Protocol version.            |
| `{"state":{...}}`             | A `StateSnapshot`.           |
| `{"output":{...}}`            | One `OutputSnapshot`.        |
| `{"error":{"message":"..."}}` | The request was refused.     |

### Snapshot shape

```json
{
  "version": 2,
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
        "scroll_x": 0.5,
        "scroll_y": 0.333,
        "blur": true,
        "zoom_out": false
      },
      "gpu": { "last_us": 142, "peak_us": 185 },
      "settled": true
    }
  ]
}
```

`channels` is what the compositor is driving this output to, before any configuration is
applied: two scroll positions normalized to `0..=1`, and whether the output should be
blurred or zoomed out. The animated values elsewhere are where those have got to.

Animated values report both ends. `current` answers "what is on screen", `target`
answers "where is it going"; a widget that wants to move with the wallpaper needs the
first, one that wants to predict needs the second.

`settled` is false exactly while frames are being submitted for that output.

`tint` already has `tint-opacity` folded into its alpha, so it is the colour actually
used rather than the two numbers that produced it.

### The measurements

Every duration is in microseconds. One unit throughout, so nothing has to be read twice
to find out which.

`frames` counts every frame presented on every output. An idle daemon submits none, so
two readings a minute apart are the whole of the idle check. `texture_bytes` is the video
memory held by wallpapers, sharp and baked together. `startup_us` runs from the first
instruction of the process to the first frame on a screen, and is `null` until there has
been one.

`gpu` is what the GPU spent on that output's last frame, and on its most expensive one so
far. Both are `null` where the driver has no usable timer, which is a property of the
driver rather than of the output: either all of them report or none do.

`peak_us` is elapsed time, not occupancy. Nothing is submitted while everything is
settled, so the GPU drops to its lowest clock and the first frame of the next animation
is measured at that clock: about 9000 microseconds here against a steady state of 150.
A peak several times the typical frame is therefore the normal reading.

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

The reply comes first and the outputs that already exist follow, so a stream describes the
whole daemon without a second request and cannot race one. Nothing can happen in between:
both are done on the thread that owns the state, before it goes back to answering anything.

Whatever the client writes after subscribing is never read. Every line going the other way
is an event now, and a reply among them could not be told apart from one. Ask on a second
connection instead; `state` and `set` cost one round trip each.

A subscriber that stops reading is dropped rather than waited on: the daemon queues a
bounded number of events for it and closes the connection when that fills. A client that
sees the stream end reconnects and is described again from scratch, which is why the gap
is a closed connection rather than a missing line.

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
`easing` uses the names the config file uses, and durations are in microseconds like every
other duration here.

### Animations are reported once, not sampled

An animation is described when it starts and never again, so a client can run the same
curve against its own clock rather than follow this one frame by frame. That is what keeps
an idle daemon idle: a listener costs no frames, and a bar that blurs alongside the
wallpaper needs no polling to stay in step.

Four rules make that work:

- A later event for the same output and property replaces the earlier one, and its `from`
  is the value mid-flight, so a redirected animation never has to be guessed at.
- `duration_us` of `0` means jump rather than animate. That covers a zero-duration tween,
  `transition.mode = "none"`, and a wallpaper slot that was empty or is being emptied.
- Nothing is reported when nothing changed, including re-resolving to the value an output
  already rests at.
- There is no settled event. The start already says when it ends.

`output-ready` carries where the values start because a monitor appearing snaps rather than
animating, so no animation event will ever report them. It says that this daemon now has
state for that output, which is later than the compositor knowing the monitor exists: the
layer surface has to be configured first.

`wallpaper-failed` is the one thing a client cannot learn any other way. `set-wallpaper` is
answered before the decode, so the client that asked was told `done`; the failure arrives
here, followed by the `wallpaper-changed` of each output falling back.

### What is not reported

- Where an animation has got to, and anything else per frame. `get-state` reads those.
- What the compositor is driving. Those are its to announce, and it does so earlier and
  more precisely than this could. `get-state` reports them as `channels`.
- Blur `radius`, `downscale` and `tint`, and every other configured parameter. They change
  only with the config file, so `config-reloaded` is the signal to read `get-state` again.
