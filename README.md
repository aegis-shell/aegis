# Aegis

Aegis is a Wayland compositor and desktop shell for Linux. It combines a
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
Aegis requires Rust 1.88 or later and the native build dependencies listed in
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

From the Aegis repository root, start the compositor:

```bash
cargo build --locked -p aegis-idle -p aegis-lock -p aegis-atspi
cargo run --locked -p aegis
```

`AEGIS_BACKEND=auto` is the default. A terminal with `WAYLAND_DISPLAY` set
opens a nested window; a login on a bare TTY selects direct DRM/KMS.

Source-tree Cargo commands do not install systemd, D-Bus, portal, desktop, or
icon metadata. The D-Bus-activated portal backend is built from the
independent
[xdg-desktop-portal-aegis repository](https://github.com/aegis-shell/xdg-desktop-portal-aegis)
and distributed as a compatibility-mapped optional package; the core
compositor runs without it. An installed core package can start the
compositor as a user service:

```bash
systemctl --user enable --now aegis.service
```

## Controls

| Action | Shortcut or command |
|--------|---------------------|
| Open Applications | Click Launchpad or press `Super` |
| Open System Settings | Press `Super+S` and select a settings tab |
| Lock the session | Press `Super+L` |
| Inspect compositor state | Run `aegis window` |

## Documentation

- [Documentation home](docs/index.md)
- [Getting Started](docs/tutorials/01-getting-started.md)
- [Daily-use guides](docs/how-to/index.md)
- [Configuration reference](docs/reference/config.md)
- [Architecture](docs/explanation/architecture.md)
- [Agent Workspaces](docs/how-to/ai-workspaces.md)

## License

Project source code is licensed under the [MIT License](LICENSE).

The bundled cursor theme under `assets/cursors/Bibata-Modern-Ice/` is derived
from [Bibata Cursor](https://github.com/ful1e5/Bibata_Cursor) and is licensed
under GPL-3.0-only; see its `LICENSE` and `NOTICE` in that directory. The
shipped binary therefore combines MIT (code) and GPL-3.0-only (bundled
cursor art).
