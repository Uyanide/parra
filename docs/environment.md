# Environment

Several things the daemon needs are already decided by mechanisms outside it. It reads
those and adds no control of its own, since a second control point would be a second
source of truth.

| What                           | Decided by                                             | What the daemon does                                                                                    |
| ------------------------------ | ------------------------------------------------------ | ------------------------------------------------------------------------------------------------------- |
| Rendering GPU and driver       | libglvnd and driver environment variables              | Nothing. It does not enumerate or select devices.                                                       |
| Which GPU buffers come from    | The compositor, via `zwp_linux_dmabuf_v1` feedback     | Nothing. It does not allocate dmabufs.                                                                  |
| Wayland connection             | `WAYLAND_DISPLAY`, `XDG_RUNTIME_DIR`                   | Connects with a null display name and lets libwayland resolve it.                                       |
| Compositor IPC socket          | Whatever the compositor exports, such as `NIRI_SOCKET` | Reads that variable. No path is hardcoded.                                                              |
| Where every file it owns lives | XDG Base Directory                                     | Derives paths from `XDG_CONFIG_HOME`, `XDG_RUNTIME_DIR`, `XDG_STATE_HOME`, `XDG_CACHE_HOME` and `HOME`. |
| Log level and filtering        | `RUST_LOG`                                             | Reads it. There is no config key for verbosity.                                                         |

## Where things go

| Variable          | Falls back to        | Holds                                           |
| ----------------- | -------------------- | ----------------------------------------------- |
| `XDG_CONFIG_HOME` | `$HOME/.config`      | `parra/config.toml`                             |
| `XDG_RUNTIME_DIR` | nothing; required    | `parra-$WAYLAND_DISPLAY.sock`                   |
| `XDG_STATE_HOME`  | `$HOME/.local/state` | `parra/state.toml`, the wallpaper to restore    |
| `XDG_CACHE_HOME`  | `$HOME/.cache`       | `parra/*.qoi`, those wallpapers already resized |

`--config`, `--socket`, `--state` and `--cache-dir` override the four, and work on any
subcommand. Two daemons on two Wayland displays share the last two unless told otherwise;
the recovery is regeneration from the original, so the cost of not separating them is a
slow start rather than a wrong wallpaper.

## Choosing a GPU

The renderer builds its display with `eglGetPlatformDisplay(EGL_PLATFORM_WAYLAND_KHR,
...)` on the `wl_display` the compositor gave it, and its surfaces with
`wl_egl_window_create`. Device selection, buffer allocation and cross-GPU import are the
EGL implementation's job on that path, and the compositor's dmabuf feedback tells it
which device each surface should use. This is what ordinary GUI applications do, and it
is why a machine whose monitors hang off two different DRM devices works without the
daemon knowing anything about it. Hand-rolled dmabuf allocation would mean reimplementing
that feedback handling, badly.

To pin it somewhere specific, set the same variables you would for any other GUI
application. Some combination of:

| Variable                           | Effect                                     |
| ---------------------------------- | ------------------------------------------ |
| `__NV_PRIME_RENDER_OFFLOAD=1`      | Offload to the NVIDIA GPU.                 |
| `__GLX_VENDOR_LIBRARY_NAME=nvidia` | Select the NVIDIA GLX vendor.              |
| `__EGL_VENDOR_LIBRARY_FILENAMES`   | Select an EGL vendor ICD explicitly.       |
| `DRI_PRIME=1`                      | Select the non-default DRI device on Mesa. |
| `MESA_LOADER_DRIVER_OVERRIDE`      | Force a specific Mesa driver.              |

Set them where the daemon is started, not in its configuration.

In a niri config:

```kdl
spawn-at-startup "sh" "-c" "DRI_PRIME=0 exec parra daemon"
```

In a systemd user unit:

```ini
[Service]
Environment=DRI_PRIME=0
ExecStart=%h/.local/bin/parra daemon
```

## Logging

`RUST_LOG` takes the usual filter syntax. Targets are module paths, so a crate can be
turned up on its own:

```sh
RUST_LOG=warn,render=debug parra daemon
```

The daemon defaults to `info` and the other subcommands to `warn`. Logs go to stderr, so
`state --json` on stdout stays parseable.
