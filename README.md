# Tessera

Tessera is a Wayland compositor and desktop shell for Linux. It combines a
Vulkan-first renderer, native shell surfaces, panel-hosted system settings,
and scoped AI-agent workspaces behind explicit process and security
boundaries.

## Capabilities

- Vulkan rendering through the
  [Optics](https://github.com/ming2k/optics) stack
- Native status bar, Dock, full application launcher, Prism search,
  notifications, and multi-workspace window management
- Multi-output session lock, staged idle policy, lock-before-sleep, and
  physical display power management
- Nested Wayland development and direct DRM/KMS presentation
- Versioned local IPC with a command-line client and desktop portal backend
- Isolated Agent Interaction Domains with cgroup and capability boundaries
- Structured semantic observation through a supervised out-of-process AT-SPI
  adapter, observation-bound actions, and hash-chained Actor audit

## Quick Start

The safest first run is a nested session inside an existing Wayland desktop.
Tessera requires Rust 1.88 or later and the native build dependencies listed in
the [Getting Started tutorial](docs/tutorials/01-getting-started.md).

Install the Optics `v<OPTICS_VERSION>` native libraries from a distribution package or
build the matching source release:

```bash
git clone --branch "v<OPTICS_VERSION>" --depth 1 \
  https://github.com/ming2k/optics.git ../optics
meson setup ../optics/build ../optics \
  -Dtests=false -Dbuildtype=debugoptimized
meson compile -C ../optics/build
sudo meson install -C ../optics/build
sudo ldconfig
pkg-config --modversion flux flux-scene-graph lens iris
```

From the Tessera repository root, start the compositor:

```bash
cargo build --locked -p tessera-idle -p tessera-lock -p tessera-atspi
cargo run --locked -p tessera
```

`TESSERA_BACKEND=auto` is the default. A terminal with `WAYLAND_DISPLAY` set
opens a nested window; a login on a bare TTY selects direct DRM/KMS.
When developing nested inside an active Tessera session, specify an isolated
`XDG_DATA_HOME` (e.g. `XDG_DATA_HOME=/tmp/tessera-dev XDG_DATA_DIRS=$HOME/.local/share:/usr/local/share:/usr/share`)
to prevent audit store lock contention with the host compositor.

Source-tree Cargo commands do not install systemd, D-Bus, portal, desktop, or
icon metadata. The D-Bus-activated portal backend is built from the
independent
[xdg-desktop-portal-atrium repository](https://github.com/aegis-shell/xdg-desktop-portal-atrium)
and distributed as a compatibility-mapped optional package; the core
compositor runs without it. An installed core package can start the
compositor as a user service:

```bash
systemctl --user start --wait tessera.service
```

Greeters and TTY logins should use the `tessera-session` wrapper instead; it
additionally manages the login environment and the `graphical-session.target`
lifecycle.

## Controls

| Action | Shortcut or command |
|--------|---------------------|
| Open Applications | Click Launchpad or press `Super` |
| Open System Settings | Press `Super+S` and select a settings tab |
| Lock the session | Press `Super+L` |
| Inspect compositor state | Run `tessera window` |

## Documentation

- [Documentation home](docs/index.md)
- [Getting Started](docs/tutorials/01-getting-started.md)
- [Daily-use guides](docs/how-to/index.md)
- [Configuration reference](docs/reference/config.md)
- [Architecture](docs/explanation/architecture.md)
- [Agent Workspaces](docs/how-to/ai-workspaces.md)

## License

Project source code is licensed under the [MIT License](LICENSE). The
bundled cursor theme under `assets/cursors/Tessera/` is original art generated
in-tree by `scripts/prepare-tessera-cursors.py` and is MIT-licensed like the
rest of the project.
