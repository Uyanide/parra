# Installation

- [Installation](#installation)
  - [Quick Install](#quick-install)
    - [AUR](#aur)
    - [Release Page](#release-page)
  - [Building from Source](#building-from-source)
    - [Dependencies](#dependencies)
    - [Build](#build)
    - [Install](#install)
  - [Optional Installations](#optional-installations)
    - [Systemd-Unit](#systemd-unit)
    - [Shell Completions](#shell-completions)

## Quick Install

### AUR

parra is available on AUR as [parra-bin][parra-bin]:

```bash
git clone https://aur.archlinux.org/parra-bin.git
cd parra-bin
makepkg -si
```

[parra-bin]: https://aur.archlinux.org/packages/parra-bin

### Release Page

Prebuilt banaries are available on the [release page][release-page]. You can
download it from there, verify its signature, and install it to one of the
locations on `$PATH`. e.g.

[release-page]: https://github.com/Uyanide/parra/releases

## Building from Source

### Dependencies

Native libraries:

- libwayland-egl
- libwayland-client
- libEGL
- libGLdispatch
- libffi

Buildtime:

- pkg-config
- rust toolchain (>=1.88)

Runtime:

- A supported compositor

These can be installed with apt on Debian/Ubuntu:

```bash
sudo apt install libwayland-dev libegl-dev pkgconf
```

or with pacman on Archlinux:

```bash
sudo pacman -S --needed wayland libglvnd mesa pkgconf
```

The rust toolchain can be installed with the distro's package manager (e.g.
`rustc` and `cargo` on Debian/Ubuntu), or with [rustup][rustup-home], which
often provides newer versions and more flexibility.

[rustup-home]: https://rustup.rs

### Build

This is a standard cargo project. It can be built normally with:

```bash
cargo build --release --locked
```

Optionally, run the tests with:

```bash
cargo test --release --locked --workspace
```

### Install

The compilation result consists of a single binary file. It can be installed to
any location on `$PATH`, e.g. `/usr/local/bin`:

```bash
sudo install -Dm755 -t /usr/local/bin target/release/parra
```

## Optional Installations

### Systemd-Unit

> [!NOTE]
>
> For usage of this unit, please refer to [usage.md](usage.md#autostart-via-systemd-unit).

parra provides a systemd-unit for auto-starting. It can be installed either to
a system-wide location e.g. `/usr/lib/systemd/user/`, or at user level,
e.g. `~/.config/systemd/user/`:

```bash
install -Dm644 -t "${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user" examples/parra.service
```

### Shell Completions

> [!NOTE]
>
> For usage of CLI, please refer to [cli.md](cli.md).

`parra completions SHELL` prints a completion script to stdout, for one of `bash`,
`zsh`, `fish`, `powershell`, or `elvish`. Where it goes depends on the shell:

```sh
# bash, once bash-completion is installed:
parra completions bash > ~/.local/share/bash-completion/completions/parra

# zsh, into a directory on $fpath:
parra completions zsh > "${fpath[1]}/_parra"

# fish:
parra completions fish > ~/.config/fish/completions/parra.fish
```

New shells pick the scripts up on their next start.
