# Aegis

**Aegis** is a modern Wayland desktop environment and compositor for Linux, built with high-performance Vulkan rendering and built-in AI agent collaboration.

---

## Highlights

- 🚀 **Vulkan-Powered Performance**: Fast, smooth rendering driven by a modern Vulkan graphics engine.
- 🎨 **Complete Desktop Shell**: Integrated status bar, dock, application launcher, and System Settings app out of the box.
- 🤖 **AI Agent Workspaces**: Native `fuji` agent integration for desktop automation without slowing down system performance.
- 🛡️ **Sandboxed AI Realms**: Isolated execution environments using Linux cgroup v2 for safety and control.

---

## Quick Start

### Prerequisites

Aegis builds against the `optics` rendering libraries in a sibling directory. Make sure system dependencies (`meson`, `ninja`, `libwayland-dev`, `libvulkan-dev`, `libxkbcommon-dev`, `libinput-dev`, `libseat-dev`) are installed.

### 1. Build Rendering Engine

```bash
meson setup ../optics/build ../optics -Dtests=false -Dbuildtype=debugoptimized
meson compile -C ../optics/build
```

### 2. Install Aegis

Run the automated installer to build and install Aegis binaries and system integration files to `~/.local`:

```bash
./scripts/install.sh --user
```

### 3. Run Aegis

- **Try Nested Session** (inside your current desktop):
  ```bash
  scripts/dev.sh
  ```

- **Run as Systemd Service** (on login / bare metal):
  ```bash
  systemctl --user daemon-reload
  systemctl --user enable --now aegis.service
  ```

---

## Usage & Controls

| Action | Shortcut / Command |
| ------ | ------------------ |
| **App Launcher** | Click Launcher icon or press `Super` |
| **System Settings** | Open System Settings from Launcher or run `aegis-settings` |
| **CLI Control** | `aegis-ctl --help` |

---

## Documentation

- [User Guides & Daily Use](docs/how-to/index.md)
- [Configuration Reference](docs/reference/config.md)
- [Architecture & Design](docs/explanation/architecture.md)
- [AI Workspaces & Sandboxing](docs/how-to/ai-workspaces.md)

---

## License

[Apache-2.0](LICENSE)
