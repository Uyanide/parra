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

## Requests

| Request                                                 | Meaning                                                                                                                      |
| ------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------- |
| `"get-state"`                                           | Every output.                                                                                                                |
| `{"get-output":{"output":"DP-1"}}`                      | One output.                                                                                                                  |
| `{"set-wallpaper":{"output":null,"path":"/srv/a.png"}}` | Show an image. `null` addresses every output.                                                                                |
| `{"set-blur":{"output":"DP-1","on":true}}`              | External blur signal. `null` broadcasts, and a broadcast also clears any per-output requests, so it is always authoritative. |
| `"reload-config"`                                       | Re-read the config file.                                                                                                     |
| `"ping"`                                                | Liveness, and the protocol version.                                                                                          |

`set-wallpaper` takes an absolute path: the daemon's working directory is not the
caller's. The CLI canonicalizes for you and fails early if the file is not readable, and
the daemon refuses anything that is not a file while the client is still on the line.
Decoding happens afterwards, on a thread of its own, so `set` returns in about two
milliseconds and whatever is on screen stays there until the new image is ready. A file
that turns out not to be an image is reported in the log, not to the client that asked.

A wallpaper set this way outlives a config reload. Reloading only reclaims the slot if
the config file's own path actually changed, so an edit elsewhere in the file does not
undo what something else set. It also outlives the monitor: one that is unplugged and
plugged back in comes back to what was asked for rather than to what the file says.

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
  "startup_us": 340107,
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
parra set PATH [--output NAME]
parra blur on|off [--output NAME]
parra state [--output NAME] [--json]
parra reload
parra ping
```

`--config PATH` and `--socket PATH` work on any subcommand.

`state --json` prints the reply verbatim, for anything that is not a human. Without it
you get a readable summary.

## Exit codes

| Code | Meaning                 |
| ---- | ----------------------- |
| 0    | Success.                |
| 1    | Failed.                 |
| 3    | No daemon is listening. |

Code 3 is separate so a script can tell "start it first" from "something is broken".

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
