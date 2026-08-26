# Usage

> [!NOTE]
> Please install the binary first, as the [install.md](install.md) describes.
> Everything below assumes `parra` is on your `$PATH`.

- [Usage](#usage)
  - [TL;DR](#tldr)
  - [Compositor integration](#compositor-integration)
    - [niri](#niri)
      - [Put within backdrop](#put-within-backdrop)
      - [Match animations](#match-animations)
      - [Autostart](#autostart)
    - [Hyprland](#hyprland)
      - [Workspace travel](#workspace-travel)
      - [Match animations](#match-animations-1)
      - [What Hyprland does not report](#what-hyprland-does-not-report)
      - [Start it at login](#start-it-at-login)
  - [Autostart via Systemd-Unit](#autostart-via-systemd-unit)
  - [Configuration](#configuration)
  - [Choosing a wallpaper](#choosing-a-wallpaper)
    - [Live switching](#live-switching)
    - [Temporary preview](#temporary-preview)

## TL;DR

parra works with any compostior that implements wlr-layer-shell. However,
the compositor-driven animation effects need a supported compositor. And so far
those are niri and Hyprland.

<details>
<summary>For niri</summary>

```bash
cat > "${XDG_CONFIG_HOME:-${HOME}/.config}/niri/parra.kdl" << 'EOF_PARRA_KDL'
layer-rule {
  match namespace="^parra$"
  place-within-backdrop true
}

layout {
  background-color "transparent"
}

overview {
  workspace-shadow {
    off
  }
}

animations {
  workspace-switch {
    duration-ms 300
    curve "ease-out-cubic"
  }

  overview-open-close {
    duration-ms 300
    curve "ease-out-cubic"
  }
}

spawn-at-startup "parra" "daemon"
EOF_PARRA_KDL

cat >> "${XDG_CONFIG_HOME:-${HOME}/.config}/niri/config.kdl" << 'EOF_CONFIG_KDL'
include "parra.kdl"
EOF_CONFIG_KDL
```

then restart niri, or start parra at once with:

```bash
niri msg action spawn -- parra daemon
```

Then set the wallpaper you like:

```bash
parra set /path/to/preferred/wallpaper.ext
```

</details>

<details>
<summary>For Hyprland</summary>

```bash
cat > "${XDG_CONFIG_HOME:-${HOME}/.config}/hypr/parra.lua" << 'EOF_PARRA_LUA'
hl.config({
    misc = {
        disable_hyprland_logo    = true,
        disable_splash_rendering = true,
    },
})

hl.curve("parra", { type = "bezier", points = { { 0.333, 1.0 }, { 0.667, 1.0 } } })

for _, leaf in ipairs({ "workspaces", "workspacesIn", "workspacesOut" }) do
    hl.animation({ leaf = leaf, enabled = true, speed = 3, bezier = "parra", style = "slide" })
end

hl.on("hyprland.start", function()
    hl.exec_cmd("parra daemon")
end)
EOF_PARRA_LUA

cat >> "${XDG_CONFIG_HOME:-${HOME}/.config}/hypr/hyprland.lua" << 'EOF_HYPRLAND_LUA'
require("parra")
EOF_HYPRLAND_LUA
```

Restart Hyprland, or start parra at once with:

```bash
hyprctl dispatch exec parra daemon
```

Then set the wallpaper you like:

```bash
parra set /path/to/preferred/wallpaper.ext
```

</details>

> [!IMPORTANT]
>
> Above is only a quick guide. If you run into any problems or wonder how these things
> work, please refer to the following sections and other documentations.

## Compositor integration

### niri

All of this goes in `$XDG_CONFIG_HOME/niri/config.kdl`, or `~/.config/niri/config.kdl`
if that variable is unset, or any file it includes.

#### Put within backdrop

The wallpaper layer is designed to be put in the backdrop (overview), rather
in the workspace background as niri's defaults.

> [!NOTE]
> The regex matches the layer-shell namespace, which defaults to `parra`, the
> program's own name. Change it if you set `[general] namespace` in parra's
> config

```kdl
layer-rule {
  match namespace="^parra$"
  place-within-backdrop true
}
```

Make the workspace background transparent so the backdrop shows through:

```kdl
layout {
  background-color "transparent"
}
```

_Optionally_, drop the workspace shadow for a cleaner overview:

```kdl
overview {
  workspace-shadow {
    off
  }
}
```

#### Match animations

Match niri's animations to parra's, or the wallpaper will lead or lag the windows it sits
behind. Three of niri's animations have a counterpart here:

| niri animation             | parra section         | Drives                                |
| -------------------------- | --------------------- | ------------------------------------- |
| `workspace-switch`         | `[scroll.vertical]`   | Vertical parallax                     |
| `horizontal-view-movement` | `[scroll.horizontal]` | Horizontal parallax, if you enable it |
| `overview-open-close`      | `[zoom]`              | Zoom                                  |

niri ships all three as springs, and parra has fixed-duration easings, so making the two
sides agree means writing niri's in the easing form:

```kdl
animations {
  workspace-switch {
    duration-ms 300
    curve "ease-out-cubic"
  }

  overview-open-close {
    duration-ms 300
    curve "ease-out-cubic"
  }
}
```

and for the horizontal parallax, case enabled:

```kdl
animations {
  horizontal-view-movement {
    duration-ms 300
    curve "ease-out-cubic"
  }
}
```

For reference, the curves the two have:

| parra        | niri           |
| ------------ | -------------- |
| linear       | linear         |
| out-quad     | ease-out-quad  |
| out-cubic    | ease-out-cubic |
| -            | ease-out-expo  |
| -            | cubic-bezier   |
| -            | spring         |
| in-out-quad  | -              |
| in-out-cubic | -              |
| out-quint    | -              |

niri's [animation documentation][niri-animations] has the full list.

[niri-animations]: https://github.com/niri-wm/niri/blob/main/docs/wiki/Configuration%3A-Animations.md

#### Autostart

Finally, start the daemon at login:

```kdl
spawn-at-startup "parra" "daemon"
```

> [!TIP]
>
> _Alternatively_, the daemon can be started with the shipped
> [Systemd-Unit](../examples/parra.service), case installed:
>
> ```bash
> systemctl --user add-wants niri.service parra.service
> ```
>
> For more information about this method, please refer to
> [this section](#autostart-via-systemd-unit) below.

### Hyprland

All of this goes in `$XDG_CONFIG_HOME/hypr/hyprland.lua`, or `~/.config/hypr/hyprland.lua`
if that variable is unset, or any file it requires.

#### Workspace travel

Parra follows Hyprland's live positive workspace ids automatically. For each monitor it
sorts that monitor's current ids numerically and spreads them evenly across the wallpaper
travel. Sparse global ids need no configuration: workspaces `1`, `3`, and `8` on one monitor
become its first, middle, and last stops regardless of ids owned by another monitor.

Creating, destroying, or moving a workspace updates the stops immediately. A workspace opened
by name sits centred, being outside the numeric row.

A special workspace is drawn over the workspace its monitor is showing rather than in place
of it, so opening one leaves the wallpaper where that workspace put it.

#### Match animations

Match Hyprland's animations to parra's, or the wallpaper will lead or lag the windows it
sits behind. Two of Hyprland's have a counterpart here:

| Hyprland node | parra section                                | Drives                  |
| ------------- | -------------------------------------------- | ----------------------- |
| `workspaces`  | `[scroll.horizontal]` or `[scroll.vertical]` | Parallax                |
| `fadeSwitch`  | `[blur]`                                     | Blur                    |
| none          | `[zoom]`                                     | Zoom, which never moves |

Hyprland measures `speed` in deciseconds, so a duration in milliseconds is `speed * 100`
and parra's 300 ms default is `speed = 3`. Larger is slower, despite the name. The curve
below is the exact bezier form of parra's `out-cubic`:

```lua
hl.curve("parra", { type = "bezier", points = { { 0.333, 1.0 }, { 0.667, 1.0 } } })

for _, leaf in ipairs({ "workspaces", "workspacesIn", "workspacesOut" }) do
    hl.animation({ leaf = leaf, enabled = true, speed = 3, bezier = "parra", style = "slide" })
end
```

A switch reads `workspacesIn` and `workspacesOut`, and falls back to the `workspaces` they
hang off only for what neither sets. The configuration Hyprland writes for a new session
sets both to `fade`, so the two children are worth stating.

The style decides which axis follows the workspace, e.g.

- `slide` and `slidefade` travel sideways, which is what parra's own defaults expect;
- `slidevert` and `slidefadevert` travel vertically;
- `fade` travels nowhere at all.

#### What Hyprland does not report

Two of parra's effects have nothing to drive them here:

- **The second scroll axis**. Although Hyprland supports `scrolling` layouts, this are
  some obstacles to obtaining column information via IPC. So only one scroll axis based
  on workspace switching is currently supported.
- **Overview Zoomin/out**. There is no builtin overview in Hyprland. So the zoom holds
  at whatevet `[zoom] crop-ratio` implies through the entire session.

#### Start it at login

Finally, start the daemon at login:

```lua
hl.on("hyprland.start", function()
    hl.exec_cmd("parra daemon")
end)
```

> [!TIP]
>
> _Alternatively_, the daemon can be started with the shipped
> [Systemd-Unit](../examples/parra.service), case the unit is installed and the Hyprland
> session is managed by [uwsm][uwsm]:
>
> ```bash
> systemctl --user add-wants wayland-wm@hyprland.service parra.service
> ```
>
> For more information about this method, please refer to
> [this section](#autostart-via-systemd-unit) below.

## Autostart via Systemd-Unit

> [!IMPORTANT]
>
> This section is **OPTIONAL**, parra can be started with a simple start-up shell command
> in the configuration file of the compositor, e.g.
> `spawn-at-startup "parra" "daemon"` for niri. Please refer to
> [Compositor integration](#compositor-integration) section above for instructions.

Install [examples/parra.service](../examples/parra.service) to a unit search path, e.g.
`~/.config/systemd/user/`, point `ExecStart` at wherever the binary lives, and reload:

```sh
systemctl --user daemon-reload
```

The unit carries no `[Install]` section for purpose. Rather than enabling it, bind it to
the compositor's own session unit, so it autostarts and dies together with that
compositor:

```sh
# under niri:
systemctl --user add-wants niri.service parra.service

# under a Hyprland session run through uwsm, whose instances are named after
# the ID uwsm was started with:
systemctl --user add-wants wayland-wm@hyprland.service parra.service
```

> [!TIP]
>
> Some compositors such as Hyprland does not start as systemd-unit by default. In such
> cases, [uwsm][uwsm] can be used to run and manage the session, providing
> `graphical-session.target` parra ties to and [session environments](environment.md)
> parra reads.

[uwsm]: https://github.com/Vladimir-csp/uwsm

To start parra right away, without waiting for the next login:

```sh
systemctl --user start parra.service
```

Undoing the binding is removing the symlink it created, e.g.

```sh
rm ~/.config/systemd/user/niri.service.wants/parra.service
```

## Configuration

> [!NOTE]
>
> The configuration file is _OPTIONAL_, a missing file is _NOT_ an error, as the
> built-in defaults are a working configuration. Only create the configuration file
> when one is needed.

parra reads one file per compositor, named after the backend: `niri.toml` under niri and
`hyprland.toml` under Hyprland. It looks in `$XDG_CONFIG_HOME/parra/`, falling back to
`~/.config/parra/`. Note the lower case, which does not match the `Hyprland` that
compositor puts in `$XDG_CURRENT_DESKTOP`.

```sh
mkdir -p ~/.config/parra
cp examples/niri.example.toml ~/.config/parra/niri.toml           # from the repository root
cp examples/hyprland.example.toml ~/.config/parra/hyprland.toml   # or this one, under Hyprland
```

Check a file before restarting anything:

```sh
parra daemon --check

# Optionally specify the backend
parra daemon --check niri   # or hyprland
```

Every key, its default and what a reload picks up are in [config.md](config.md).

## Choosing a wallpaper

### Live switching

Hand one to the running daemon:

```sh
parra set ~/pictures/wall.png               # every output
parra set ~/pictures/other.png --output eDP-1
parra set ~/pictures/passing.png --no-save  # this session only, not restored after restart
```

`set` returns immediately and the current image stays up until the new one is ready. A
file that turns out not to be an image is reported in the log; see
[environment.md](environment.md#logging).

The choice is remembered. It outlives a config reload, a monitor being unplugged and
plugged back in, and the daemon itself.

`unset` takes one back:

```sh
parra unset --output eDP-1     # eDP-1 goes back to whatever every output is on
parra unset                    # every output goes back to the config file
```

It uncovers the next wallpaper down, walking the order the daemon resolves in:

1. an output's own wallpaper
2. the one set for every output
3. an output's own `[wallpaper] fallback`, see [config.md](config.md#wallpaper)
4. `[wallpaper] fallback` for every output, see [config.md](config.md#wallpaper)
5. nothing, fully transparent

### Temporary preview

`--no-save` changes what is on screen now and leave the record untouched, so the next
start or `restore` cmd sets the wallpaper to what is recorded. This flag works
similarly for both `set` and `unset` cmd. e.g.

```sh
parra set ~/pictures/passing.png --no-save
parra restore                  # back to what is recorded, on every output
# or
parra restore --output eDP-1   # back to eDP-1's own recorded wallpaper
```
