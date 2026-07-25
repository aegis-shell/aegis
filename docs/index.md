# ass Documentation

ass is a Wayland compositor for Linux, written in Rust on
[flux](../../optics/libs/flux) and [lens](../../optics/libs/lens). Start with
the [README](../README.md) for the project pitch and the shortest run path.

## Sections

| Section | Purpose |
|---------|---------|
| [Tutorials](tutorials/01-getting-started.md) | Learning-oriented step-by-step walkthroughs |
| [How-to guides](how-to/index.md) | Task-oriented instructions for daily use |
| [Explanation](explanation/index.md) | Architecture and conceptual background |
| [Reference](reference/index.md) | Configuration keys, schemas, and option tables |
| [Architecture Decision Records](adr/index.md) | Durable technical decisions |
| [Contributor docs](dev/index.md) | Setup, layout, and project maintenance |

## Orientation

- First-time walkthrough: read [Getting Started Tutorial](tutorials/01-getting-started.md).
- New to the project: read [Architecture](explanation/architecture.md), then
  [Vision and Scope](explanation/vision.md).
- Looking for where ass is headed: read [Roadmap](explanation/roadmap.md).
- Looking for how ass compares to GNOME, KDE, sway, river, niri, macOS, and
  Xfce: read [Comparative Survey](explanation/comparative-survey.md).
- Looking for a config key or option: read the
  [Configuration Reference](reference/config.md).
- Looking for Control Center module routes and backend availability: read the
  [Control Center Reference](reference/control-center.md).
- Starting applications or using app-level window actions: read
  [How to Use the Dock and Launcher](how-to/dock-and-launcher.md).
- Managing a borderless window: read
  [How to Manage Borderless Windows](how-to/window-management.md).
- Isolating agent input and applications: read
  [How to Use AI Workspaces](how-to/ai-workspaces.md).
- Connecting the fuji agent to scoped desktop and Realm tools: read
  [Connect fuji to ASS](how-to/fuji.md).
- Booting from a TTY and smoke-testing real hardware: read
  [How to Run ass on Bare Metal (DRM/KMS)](how-to/bare-metal-drm.md).
- Enabling portal-aware and Flatpak apps (settings, screenshots): read
  [How to Install and Verify the Portal Backend](how-to/portals.md).
- Setting up a build: read [Setup](dev/setup.md).
- Iterating on compositor code inside an existing Wayland session: read
  [Nested Backend Development](dev/nested-backend.md).
- Looking for why a choice was made: scan the [ADR index](adr/index.md).
