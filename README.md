# parra

A wlr-layer-shell wallpaper daemon that supports animation effects:

- **vertical parallax scrolling** that follows the workspace
- **horizontal parallax scrolling** that follows the focusing column (disabled
  by default)
- **blurring and tinting** that follows window focus
- **zoom-in/out** that follows overview's close/open

and acts exactly like what would be expected from such promgrams:

- **control via IPC** including setting wallpapers, setting blurring, quering
  status etc.
- **live wallpaper switching** without restarting or reinitialize the daemon
- **automatic wallpaper restoration** at the next start
- **configuration override** per monitor

> [!NOTE]
>
> Only niri is supported so far. Support for more compositors might be added
> in the (near) future.

## Build

This is a standard cargo project with a single binary output, so build with

```sh
cargo build --release --frozen
```

and (optionally) install in the way and the directory you like, e.g.

```sh
sudo install -Dm755 -t /usr/local/bin target/release/parra
```

## Documentation

common:

- [Usage](docs/usage.md) — compositor integration and instructions for normal usage
- [Configuration](docs/config.md) — every key, defaults, per-monitor inheritance
- [Control protocol](docs/control-protocol.md) — the socket, the CLI, exit codes
- [Environment](docs/environment.md) — GPU selection, logging, where files go, and what is
  deliberately not configurable

development:

- [Architecture](docs/architecture.md) — what the crates are for and why they are split
