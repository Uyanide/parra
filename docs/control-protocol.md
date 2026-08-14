# Control protocol

The daemon listens on `$XDG_RUNTIME_DIR/parra-$WAYLAND_DISPLAY.sock`. The display
name is part of the filename so two compositors in one login session get one daemon
each. `--socket PATH` overrides it.

One JSON value per line, request and response alike. Every subcommand except `daemon` is
a single round trip on this socket and nothing else, which is why they start without
touching a graphics stack.

## How the daemon answers

The socket is bound before the first frame, so a client that starts alongside the daemon
either finds it or finds nothing. A path left behind by a daemon that did not get to clean
up is taken over; a path something still answers on is refused, because two daemons
sharing one socket would each get an arbitrary half of the requests.

Every connection is served on a thread of its own, and each request crosses to the event
loop and its answer crosses back, so no request can stall a frame. A line that is not a
request is answered with an error rather than by closing the connection.

## Conventions

Variant names are kebab-case, so they read as commands. Field names are snake_case, so
`jq` paths need no quoting. A request with no fields is a bare string.

Unknown fields are rejected rather than ignored: a typo fails loudly instead of being
silently dropped.

## Protocol version

`ping` reports it and every snapshot carries it. It is bumped whenever the wire format
changes, including when it only gains a field, so a client can tell a stale daemon from an
unreachable one and can tell whether a field it wants exists at all.

Rejecting unknown fields means a skew cannot pass unnoticed: a request from a newer client,
or a reply to an older one, fails to parse rather than half working. The client turns that
into a plain answer — on any refusal or unparseable reply it pings, compares, and reports
`the daemon speaks protocol N, this build speaks M; restart the daemon` with exit code 4.
Only a request that already failed pays for the extra round trip.

This is the partial upgrade: a new binary talking to a daemon still running from before it
was replaced. Restarting the daemon is the whole fix.

## Requests

| Request                                                 | Meaning                                                                                                                      |
| ------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------- |
| `"get-state"`                                           | Every output.                                                                                                                |
| `{"get-output":{"output":"DP-1"}}`                      | One output.                                                                                                                  |
| `{"set-wallpaper":{"output":null,"path":"/srv/a.png"}}` | Show an image and remember it. A `null` path empties the slot instead; `"save":false` does not remember either. |
| `{"set-blur":{"output":"DP-1","on":true}}`              | External blur signal. `null` broadcasts, and a broadcast also clears any per-output requests, so it is always authoritative. |
| `"reload-config"`                                       | Re-read the config file.                                                                                                     |
| `"ping"`                                                | Liveness, and the protocol version.                                                                                          |

`set-wallpaper` takes an absolute path: the daemon's working directory is not the
caller's. The CLI canonicalizes for you and fails early if the file is not readable, and
the daemon refuses anything that is not a file while the client is still on the line.
Decoding happens afterwards, on a thread of its own, so `set` returns in about two
milliseconds and whatever is on screen stays there until the new image is ready. A file
that turns out not to be an image is reported in the log, not to the client that asked.

A wallpaper set this way outlives a config reload, outlives the monitor being unplugged
and plugged back in, and outlives the daemon: it is written to
`$XDG_STATE_HOME/parra/state.toml` and restored at the next start, from a resized copy
under `$XDG_CACHE_HOME/parra` so the start does not pay for the decode again.
`[wallpaper] fallback` in the config file is only what to show when nothing has been set,
so the two never compete for the slot. See [config.md](config.md#state-and-cache).

`save` defaults to true. `false` shows the image for this session only, leaving the
recorded one alone, so the next start goes back to it.

Setting the same path twice is not a no-op: every set is a distinct wallpaper, so an
image edited in place takes effect.

A `null` path empties the addressed slot rather than blanking the screen. What that
output shows is then resolved again from the top, so clearing one monitor's own wallpaper
reveals the one every other monitor is on, and clearing that reveals `[wallpaper]
fallback`. `null` for the output empties every slot at once, per-output ones included,
since a broadcast is authoritative here as everywhere else. `parra unset` sends this.

The field is required even though it is nullable. A client whose path came out undefined
should be told it sent nonsense rather than have a wallpaper quietly cleared.

An image that will not load is reported in the log, that output falls back to
`[wallpaper] fallback`, and what was recorded is left alone so the next start tries it
again. A drive that was not mounted yet therefore recovers on its own.

`reload-config` re-reads the file and answers with the parse error if there is one, and
the daemon keeps running on the configuration it already had. The namespace and layer are
the exception: a layer surface is given both when it is created, so a change to either is
reported in the log and takes effect on the next start.

The daemon also watches the configuration file, so an edit takes effect without anyone
sending this request at all. It watches the containing directory, since an editor writes
a temporary file and renames it over the original.

## Responses

| Response                      | Meaning                      |
| ----------------------------- | ---------------------------- |
| `"done"`                      | The request was carried out. |
| `{"pong":{"version":1}}`      | Protocol version.            |
| `{"state":{...}}`             | A `StateSnapshot`.           |
| `{"output":{...}}`            | One `OutputSnapshot`.        |
| `{"error":{"message":"..."}}` | The request was refused.     |

### Snapshot shape

```json
{
  "version": 1,
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
      "focused": true,
      "overview": false,
      "workspace": { "index": 2, "count": 4 },
      "column": { "index": 1, "count": 3 },
      "gpu": { "last_us": 142, "peak_us": 185 },
      "settled": true
    }
  ]
}
```

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

## Command line

```
parra daemon [--check]
parra set PATH [--output NAME] [--no-save]
parra unset [--output NAME] [--no-save]
parra blur on|off [--output NAME]
parra state [--output NAME] [--json]
parra reload
parra ping
```

`--config PATH`, `--socket PATH`, `--state PATH` and `--cache-dir PATH` work on any
subcommand.

`state --json` prints the reply verbatim, for anything that is not a human. Without it
you get a readable summary.

## Exit codes

| Code | Meaning                             |
| ---- | ----------------------------------- |
| 0    | Success.                            |
| 1    | Failed.                             |
| 3    | No daemon is listening.             |
| 4    | The daemon speaks another protocol. |

Codes 3 and 4 are separate because each has a remedy a script can act on: start the daemon,
or restart it. Everything else is 1.

## Examples

```sh
parra state --json | jq '.state.outputs[] | {name, blur: .blur.amount.current}'

# Is it really idle? Two readings, one minute apart, that agree.
parra state --json | jq '.state.frames'

# Blur while a sidebar is up, on the monitor it is up on.
parra blur on --output DP-1
parra blur off --output DP-1

parra set ~/pictures/other.png --output eDP-1
```
