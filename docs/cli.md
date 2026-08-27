# CLI

`parra` is the only binary.

Every subcommand except `daemon` sends one request to the daemon's control socket and
prints the reply; none of them touch a graphics stack. The wire format is in
[control-protocol.md](control-protocol.md).

- [CLI](#cli)
  - [Global flags](#global-flags)
  - [Commands](#commands)
    - [`parra daemon [--check] [--backend NAME]`](#parra-daemon---check---backend-name)
    - [`parra set PATH [--output NAME] [--no-save]`](#parra-set-path---output-name---no-save)
    - [`parra unset [--output NAME] [--no-save]`](#parra-unset---output-name---no-save)
    - [`parra restore [--output NAME]`](#parra-restore---output-name)
    - [`parra blur on|off [--output NAME]`](#parra-blur-onoff---output-name)
    - [`parra state [--output NAME] [--json]`](#parra-state---output-name---json)
    - [`parra events [--output NAME] [--json]`](#parra-events---output-name---json)
    - [`parra reload`](#parra-reload)
    - [`parra ping`](#parra-ping)
  - [The blur signal](#the-blur-signal)
  - [Exit codes](#exit-codes)
  - [Examples](#examples)

## Global flags

`--config PATH`, `--socket PATH`, `--state PATH` and `--cache-dir PATH` work on any
subcommand, and override the XDG-derived locations described in
[environment.md](environment.md#where-things-go).

## Commands

```
parra daemon [--check] [--backend NAME]
parra set PATH [--output NAME] [--no-save]
parra unset [--output NAME] [--no-save]
parra restore [--output NAME]
parra blur on|off [--output NAME]
parra state [--output NAME] [--json]
parra events [--output NAME] [--json]
parra reload
parra ping
```

### `parra daemon [--check] [--backend NAME]`

Runs the wallpaper daemon. `--check` loads and validates the configuration, reports the
resolved namespace, layer, socket, fallback, compositor settings and remembered wallpaper,
and exits without touching a graphics stack.

`--backend NAME` reads that compositor's configuration file instead of detecting the one
running, so `--check` can validate a file on any machine.

### `parra set PATH [--output NAME] [--no-save]`

Canonicalizes `PATH` and fails when the file is not readable, then hands it to the daemon.
`--output` limits the choice to one output; omitted, it applies to every output.
`--no-save` shows the wallpaper now without recording it, so the next start goes back to
the previously recorded one.

### `parra unset [--output NAME] [--no-save]`

Drops a wallpaper set with `parra set`. With `--output`, drops only that output's own
wallpaper; omitted, drops every wallpaper set this way, per-output ones included.
`--no-save` drops it now but keeps it recorded, so the next start brings it back. What an
output shows once a wallpaper is dropped is in [usage.md](usage.md#choosing-a-wallpaper).

### `parra restore [--output NAME]`

Puts the recorded wallpapers back, which is the way out of a `--no-save` set or unset
without restarting the daemon. It addresses slots the way `set` does: `--output` restores
only that output's own recorded wallpaper and leaves the one every output is on alone;
omitted, it restores every slot, per-output ones included.

Restoring what is already showing changes nothing. An image that would not load is offered
again, exactly as the next start would offer it.

### `parra blur on|off [--output NAME]`

Turns the external blur signal on or off. Omitted `--output` broadcasts, and a broadcast
also clears any per-output requests.

### `parra state [--output NAME] [--json]`

Reports the daemon's current state. `--output` reports one output instead of all of them.
`--json` prints the daemon's reply verbatim; without it you get a readable summary.
An output missing from the report is one whose layer surface the compositor has not
configured yet.

### `parra events [--output NAME] [--json]`

Whatever else you run on your screen can follow the wallpaper here instead of polling for
it; listening costs the daemon no frames. The daemon streams what it changes, one event
per line, until it stops. The outputs that already exist arrive first. `--output` reports
only what concerns one output, plus the events that name none. `--json` prints each event
as the daemon sent it.

The full list of events and the rules they follow is in
[control-protocol.md](control-protocol.md#events).

The readable form leads with the same name the JSON uses:

```
output-ready DP-1 /srv/a.png scroll -0.420/0.000 blur 1.000 zoom 1.111
animation DP-1 blur 0.000 -> 1.000  over 300.00 ms
wallpaper-changed DP-1 /srv/a.png -> /srv/b.png  over 800.00 ms
wallpaper-failed /srv/broken.png
config-reloaded
```

It exits 1 when the daemon goes away, and 0 when whatever it was piped into stops reading.

### `parra reload`

Asks the daemon to re-read its config file.

### `parra ping`

Checks that the daemon is responding. It prints `protocol N` and exits 0 when the daemon's
protocol version matches this binary; when they differ it still prints the daemon's version
and exits 4.
A mismatch means a daemon still running from before the binary was replaced; restart it.

## The blur signal

A bar or a sidebar can ask for the wallpaper behind it to blur while it is up, and turn it
off again afterwards:

```sh
parra blur on --output DP-1
parra blur off --output DP-1
```

_'output blurs'_ = _'the compositor drives this output to blur'_ **OR** _'blur signal is set for this output'_

What the compositor drives it on is [`[compositor] blur`](config.md#when-an-output-blurs),
by default whether the output's active workspace holds any window.

## Exit codes

| Code | Meaning                                        | Remedy              |
| ---- | ---------------------------------------------- | ------------------- |
| 0    | Success.                                       |                     |
| 1    | Failed, and for `events` the daemon went away. |                     |
| 3    | No daemon is listening.                        | Start the daemon.   |
| 4    | The daemon speaks another protocol.            | Restart the daemon. |

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
