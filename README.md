# Aegis

Aegis is a Wayland compositor and desktop shell for Linux. It combines a
Vulkan-first renderer, native shell surfaces, standalone System Settings,
and scoped AI-agent workspaces behind explicit process and security
boundaries.

## Capabilities

- Vulkan rendering through the
  [Optics](https://github.com/ming2k/optics) stack
- Native status bar, Dock, application launcher, notifications, and
  multi-workspace window management
- Nested Wayland development and direct DRM/KMS presentation
- Versioned local IPC with a command-line client and desktop portal backend
- Isolated Agent Realms with cgroup and capability boundaries

## Quick Start

The safest first run is a nested session inside an existing Wayland desktop.
Aegis requires Rust 1.88 or later and the native build dependencies listed in
the [Getting Started tutorial](docs/tutorials/01-getting-started.md).

Install the Optics `v0.0.3` native libraries from a distribution package or
build the matching source release:

```bash
git clone --branch v0.0.3 --depth 1 \
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
cargo run --locked -p aegis
```

`AEGIS_BACKEND=auto` is the default. A terminal with `WAYLAND_DISPLAY` set
opens a nested window; a login on a bare TTY selects direct DRM/KMS.

Source-tree Cargo commands do not install systemd, D-Bus, portal, desktop, or
icon metadata. An installed distribution package can start the compositor as
a user service:

```bash
systemctl --user enable --now aegis.service
```

## Controls

| Action | Shortcut or command |
|--------|---------------------|
| Open Applications | Click Launchpad or press `Super` |
| Open System Settings | Select it in Applications or run `aegis-settings` |
| Inspect compositor state | Run `aegis-ctl --help` |

## Documentation

- [Documentation home](docs/index.md)
- [Getting Started](docs/tutorials/01-getting-started.md)
- [Daily-use guides](docs/how-to/index.md)
- [Configuration reference](docs/reference/config.md)
- [Architecture](docs/explanation/architecture.md)
- [AI Workspaces](docs/how-to/ai-workspaces.md)

## License

[Apache-2.0](LICENSE)
