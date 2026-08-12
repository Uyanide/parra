# parra

A wayland wallpaper daemon that supports:

- **vertical parallax scrolling** that follows the workspace
- **horizontal parallax scrolling** that follows the focusing column (disabled by default)
- **blurring and tinting** that follows window focus
- **zoom-in/out** that follows overview's close/open

Only niri is supported so far. Support for more compositors might be added
in the future.

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

- [Usage](docs/usage.md) - compositor integration and instructions for normal usage
- [Configuration](docs/config.md) — every key, and how per-output overrides inherit
- [Control protocol](docs/control-protocol.md) — the socket, the CLI, exit codes
- [Environment](docs/environment.md) — GPU selection, logging, and what is deliberately
  not configurable

development:

- [Architecture](docs/architecture.md) — what the crates are for and why they are split
