# CLI

`parra` is the only binary. Every subcommand except `daemon` sends one request to the
daemon's control socket and prints the reply; none of them touch a graphics stack. The
wire format is in [control-protocol.md](control-protocol.md).

## Global flags

`--config PATH`, `--socket PATH`, `--state PATH` and `--cache-dir PATH` work on any
subcommand, and override the XDG-derived locations described in
[environment.md](environment.md#where-things-go).

## Commands

```
parra daemon [--check]
parra set PATH [--output NAME] [--no-save]
parra unset [--output NAME] [--no-save]
parra blur on|off [--output NAME]
parra state [--output NAME] [--json]
parra events [--output NAME] [--json]
parra reload
parra ping
```

### `parra daemon [--check]`

Runs the wallpaper daemon. `--check` loads and validates the configuration, reports the
resolved namespace, layer, socket, fallback and remembered wallpaper, and exits without
touching a graphics stack.

### `parra set PATH [--output NAME] [--no-save]`

Canonicalizes `PATH` first, so the daemon does not depend on the caller's working
directory, and fails early if the file is not readable. `--output` limits the choice to
one output; omitted, it applies to every output. `--no-save` shows the wallpaper now
without recording it, so the next start goes back to the previously recorded one.

### `parra unset [--output NAME] [--no-save]`

Drops a wallpaper set with `parra set`. With `--output`, drops only that output's own
wallpaper; omitted, drops every wallpaper set this way, per-output ones included.
`--no-save` drops it now but keeps it recorded, so the next start brings it back. What an
output shows once a wallpaper is dropped is in [usage.md](usage.md#choosing-a-wallpaper).

### `parra blur on|off [--output NAME]`

Turns the external blur signal on or off. Omitted `--output` broadcasts, and a broadcast
also clears any per-output requests, so it is always authoritative.

### `parra state [--output NAME] [--json]`

Reports the daemon's current state. `--output` reports one output instead of all of them.
`--json` prints the daemon's reply verbatim, for anything that is not a human. Without it
you get a readable summary.

### `parra events [--output NAME] [--json]`

Follows what the daemon changes, one event per line, until the daemon stops. The outputs
that already exist arrive first, so the stream stands on its own without a `state` call
beside it. `--output` reports only what concerns one output, plus the events that name
none. `--json` prints each event as the daemon sent it.

The full list of events and the rules they follow is in
[control-protocol.md](control-protocol.md#events).

The readable form leads with the same name the JSON uses:

```
output-ready DP-1 /srv/a.png scroll 0.500/0.500 blur 1.000 zoom 1.111
animation DP-1 blur 0.000 -> 1.000  over 300.00 ms
wallpaper-changed DP-1 /srv/a.png -> /srv/b.png  over 800.00 ms
wallpaper-failed /srv/broken.png
config-reloaded
```

It exits 1 when the daemon goes away, since that is the only thing that ends a stream, and
0 when whatever it was piped into stops reading.

### `parra reload`

Asks the daemon to re-read its config file.

### `parra ping`

Checks that the daemon is responding. It prints `protocol N` and exits 0 when the
daemon's protocol version matches this binary; when they differ it still prints the
daemon's version and exits 4.

## Exit codes

| Code | Meaning                                        |
| ---- | ---------------------------------------------- |
| 0    | Success.                                       |
| 1    | Failed, and for `events` the daemon went away. |
| 3    | No daemon is listening.                        |
| 4    | The daemon speaks another protocol.            |

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

# React to the wallpaper changing, without polling for it. The `?` matters: an event with
# no fields is a bare string, which jq will not index.
parra events --json | jq -r --unbuffered '.["wallpaper-changed"]?.to // empty'

# Follow one monitor's blur, with the curve it is using.
parra events --json --output DP-1 \
  | jq -c --unbuffered 'select(.animation?.property == "blur") | .animation | {to, duration_us, easing}'
```
