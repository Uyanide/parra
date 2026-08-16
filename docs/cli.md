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
`--no-save` drops it now but keeps it recorded, so the next start brings it back.
Unsetting reveals rather than blanks; see [usage.md](usage.md#choosing-a-wallpaper).

### `parra blur on|off [--output NAME]`

Turns the external blur signal on or off. Omitted `--output` broadcasts, and a broadcast
also clears any per-output requests, so it is always authoritative.

### `parra state [--output NAME] [--json]`

Reports the daemon's current state. `--output` reports one output instead of all of them.
`--json` prints the daemon's reply verbatim, for anything that is not a human. Without it
you get a readable summary.

### `parra reload`

Asks the daemon to re-read its config file.

### `parra ping`

Checks that the daemon is responding. It prints `protocol N` and exits 0 when the
daemon's protocol version matches this binary; when they differ it still prints the
daemon's version and exits 4.

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
