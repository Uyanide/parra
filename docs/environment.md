# Environment

Several things the daemon needs are decided outside it. It reads those and adds no control
of its own.

| What                           | Decided by                                             | What the daemon does                                                                                    |
| ------------------------------ | ------------------------------------------------------ | ------------------------------------------------------------------------------------------------------- |
| Rendering GPU and driver       | libglvnd and driver environment variables              | Nothing. It does not enumerate or select devices.                                                       |
| Which GPU buffers come from    | The compositor, via `zwp_linux_dmabuf_v1` feedback     | Nothing. It does not allocate dmabufs.                                                                  |
| Wayland connection             | `WAYLAND_DISPLAY`, `XDG_RUNTIME_DIR`                   | Connects with a null display name and lets libwayland resolve it.                                       |
| Compositor IPC socket          | Whatever the compositor exports: `NIRI_SOCKET` under niri, `HYPRLAND_INSTANCE_SIGNATURE` under Hyprland | Reads those variables. niri's names the socket outright; Hyprland's names a directory under `$XDG_RUNTIME_DIR/hypr/`, where the two socket names are the compositor's own and fixed. |
| Where every file it owns lives | XDG Base Directory                                     | Derives paths from `XDG_CONFIG_HOME`, `XDG_RUNTIME_DIR`, `XDG_STATE_HOME`, `XDG_CACHE_HOME` and `HOME`. |
| Log level and filtering        | `RUST_LOG`                                             | Reads it. There is no config key for verbosity.                                                         |

## Where things go

| Variable          | Falls back to        | Holds                                           |
| ----------------- | -------------------- | ----------------------------------------------- |
| `XDG_CONFIG_HOME` | `$HOME/.config`      | `parra/<compositor>.toml`                       |
| `XDG_RUNTIME_DIR` | nothing; required    | `parra-$WAYLAND_DISPLAY.sock`                   |
| `XDG_STATE_HOME`  | `$HOME/.local/state` | `parra/state.toml`, the wallpaper to restore    |
| `XDG_CACHE_HOME`  | `$HOME/.cache`       | `parra/*.qoi`, those wallpapers already resized |

`--config`, `--socket`, `--state` and `--cache-dir` override the four, and work on any
subcommand.

Two daemons on two Wayland displays share the state file and the cache unless `--state`
and `--cache-dir` separate them. A cached copy that does not fit is regenerated from the
original.

## Choosing a GPU

The daemon selects no device. It renders the way any ordinary GUI application does,
leaving device selection, buffer allocation and cross-GPU import to the EGL implementation
and the compositor, so a machine whose monitors hang off two different DRM devices needs
no configuration. See
[architecture.md](architecture.md#choosing-a-gpu-is-not-part-of-this).

To pin it somewhere specific, set the same variables you would for any other GUI
application. Some combination of:

| Variable                           | Effect                                     |
| ---------------------------------- | ------------------------------------------ |
| `__NV_PRIME_RENDER_OFFLOAD=1`      | Offload to the NVIDIA GPU.                 |
| `__GLX_VENDOR_LIBRARY_NAME=nvidia` | Select the NVIDIA GLX vendor.              |
| `__EGL_VENDOR_LIBRARY_FILENAMES`   | Select an EGL vendor ICD explicitly.       |
| `DRI_PRIME=1`                      | Select the non-default DRI device on Mesa. |
| `MESA_LOADER_DRIVER_OVERRIDE`      | Force a specific Mesa driver.              |

Set them where the daemon is started.

In a niri config:

```kdl
spawn-at-startup "sh" "-c" "DRI_PRIME=0 exec parra daemon"
```

In a Hyprland config:

```lua
hl.on("hyprland.start", function()
    hl.exec_cmd("DRI_PRIME=0 exec parra daemon")
end)
```

In a systemd user unit, set them with a drop-in rather than editing the unit;
[examples/parra.service](../examples/parra.service) provides a base:

```sh
systemctl --user edit parra.service
```

```ini
[Service]
Environment=DRI_PRIME=0
```

## Logging

`RUST_LOG` takes the usual filter syntax. Targets are module paths, so a crate can be
turned up on its own:

```sh
RUST_LOG=warn,render=debug parra daemon
```

The daemon defaults to `info` and the other subcommands to `warn`. Logs go to stderr, so
`state --json` on stdout stays parseable.
