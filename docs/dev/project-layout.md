# Project Layout

Where code lives and where new files belong. For the conceptual design, see
[Architecture](../explanation/architecture.md).

## Source Tree

```text
aegis/
  Cargo.toml            workspace
  crates/
    aegis-core/          shared model: geometry, surface tree, outputs, focus, apps
    aegis-protocols/     shared generated Wayland protocol interface tables
    aegis-compositor/        Wayland server: socket, globals, object lifecycle
    aegis-backend/       presentation + input targets (nested, DRM/KMS + libinput + libseat)
    aegis-render/        compositing through flux
    aegis-shell/         compositor chrome host and contract through lens
    aegis-dock/          bottom-center dock chrome component
    aegis-prism/         compact application-search chrome component
    aegis-ai-workspaces/  Agent Realm lifecycle and authority UI
    aegis-settings/       standalone modular System Settings application
    aegis-hud/           display-only HUD status chips (system status, workspace dots, clock, SNI tray)
    aegis-command-panel/ full-screen modal command panel (quick settings, tray, notifications)
    aegis-wallpaper/     image and short-video background layer
    aegis-avatar/        user-avatar loading and rendering: still images and VRM models
    aegis-config/        TOML schema, typed atomic persistence, loader, live reload
    aegis-ipc/           versioned IPC and introspection over a unix socket
    aegis-ctl/       command-line driver for the IPC (reference external tool)
    aegis-fuji/          fuji in one crate: scoped MCP platform bridge + its own agent runtime
    aegis-desktop-entries/          freedesktop.org desktop-entry enumeration + icon lookup
    aegis-launcher/        detached, XDG-environment-aware app launching
    aegis/             the binary: wiring and event loop
  docs/                 documentation (see docs/index.md)
```

flux, lens, iris, and their Rust binding workspaces live in the
[Optics monorepo](https://github.com/ming2k/optics). The canonical dependency
graph uses the locked Optics Git release and system-installed C libraries.
Cross-repository development uses a
[worktree-isolated Cargo patch](cross-repository-development.md) for
`../optics/bindings` and discovers that checkout's uninstalled Meson tree.

## Modules

| Crate | Purpose | Design reference |
|-------|---------|------------------|
| [`aegis-core`](../../crates/aegis-core/README.md) | Backend- and renderer-agnostic types | [ADR-0001](../adr/0001-scope-and-responsibility-boundary.md) |
| [`aegis-protocols`](../../crates/aegis-protocols/README.md) | Shared generated Wayland protocol tables for client and server | [ADR-0002](../adr/0002-hand-rolled-wayland-server.md) |
| [`aegis-compositor`](../../crates/aegis-compositor/README.md) | Wayland server socket, globals, and object lifecycle | [ADR-0002](../adr/0002-hand-rolled-wayland-server.md) |
| [`aegis-backend`](../../crates/aegis-backend/README.md) | The `Backend` trait and its implementations | [ADR-0002](../adr/0002-hand-rolled-wayland-server.md), [ADR-0003](../adr/0003-nested-first-bring-up.md) |
| [`aegis-render`](../../crates/aegis-render/README.md) | Client buffers to flux textures, scene to output | [ADR-0004](../adr/0004-client-buffers-via-flux-dmabuf-import.md) |
| [`aegis-shell`](../../crates/aegis-shell/README.md) | Chrome host, `Chrome` contract, and shared components on lens | [ADR-0021](../adr/0021-chrome-component-trait.md) |
| [`aegis-dock`](../../crates/aegis-dock/README.md) | Bottom-center dock chrome component | [ADR-0019](../adr/0019-dock-as-bottom-center-overlay.md), [ADR-0021](../adr/0021-chrome-component-trait.md) |
| [`aegis-prism`](../../crates/aegis-prism/README.md) | Compact Spotlight-style application search component | [ADR-0021](../adr/0021-chrome-component-trait.md), [ADR-0044](../adr/0044-dock-and-control-center-crates.md) |
| [`aegis-ai-workspaces`](../../crates/aegis-ai-workspaces/README.md) | Compositor-owned Agent Realm lifecycle and authority UI | [ADR-0060](../adr/0060-statusbar-system-controls-and-live-system-ipc.md) |
| [`aegis-settings`](../../crates/aegis-settings/README.md) | Standalone modular System Settings application | [ADR-0069](../adr/0069-documentation-owned-installation-and-throwaway-development-staging.md) |
| [`aegis-hud`](../../crates/aegis-hud/README.md) | Display-only HUD status chips with the StatusNotifierItem tray row | [ADR-0080](../adr/0080-hud-status-chips-and-sao-command-panel.md), [ADR-0081](../adr/0081-hud-and-command-panel-naming.md) |
| [`aegis-command-panel`](../../crates/aegis-command-panel/README.md) | Full-screen modal command panel: quick settings, tray activation, and notifications | [ADR-0080](../adr/0080-hud-status-chips-and-sao-command-panel.md), [ADR-0081](../adr/0081-hud-and-command-panel-naming.md) |
| [`aegis-wallpaper`](../../crates/aegis-wallpaper/README.md) | Image and short-video background layer | [ADR-0018](../adr/0018-wallpaper-crate.md) |
| [`aegis-avatar`](../../crates/aegis-avatar/README.md) | User-avatar loading and rendering: still images and VRM models | [ADR-0080](../adr/0080-avatar-crate-xdg-conformant-vrm-aware.md) |
| [`aegis-config`](../../crates/aegis-config/README.md) | Versioned TOML schema, typed atomic persistence, loader, and mtime-based live reload | [ADR-0026](../adr/0026-configuration-system.md) |
| [`aegis-ipc`](../../crates/aegis-ipc/README.md) | Versioned schema and codec over a unix socket; the extension/automation surface | [ADR-0027](../adr/0027-ipc-and-introspection.md) |
| [`aegis-ctl`](../../crates/aegis-ctl/README.md) | Command-line driver for the IPC; the reference external tool | [ADR-0027](../adr/0027-ipc-and-introspection.md) |
| [`aegis-mcp`](../../crates/aegis-mcp/README.md) | The platform's scoped MCP bridge for any agent (`aegis-mcp`) | [ADR-0047](../adr/0047-neenee-agent-realm-platform-bridge.md), [ADR-0087](../adr/0087-aegis-mcp-standalone-platform-bridge-crate.md) |
| [`aegis-fuji`](../../crates/aegis-fuji/README.md) | fuji, the in-tree agent product: self-contained agent runtime (`fuji`) | [ADR-0050](../adr/0050-fuji-agent-product-and-bridge-rename.md) |
| [`aegis-desktop-entries`](../../crates/aegis-desktop-entries/README.md) | freedesktop.org desktop-entry enumeration and icon-theme lookup | [ADR-0022](../adr/0022-application-launcher.md) |
| [`aegis-launcher`](../../crates/aegis-launcher/README.md) | Detached, XDG-environment-aware launching of desktop applications | [ADR-0022](../adr/0022-application-launcher.md) |
| [`aegis`](../../crates/aegis/README.md) | Process entry point and frame loop | [Architecture](../explanation/architecture.md) |

## Placement Rules

- Code with no flux, lens, or Wayland dependency belongs in `aegis-core`.
- A new presentation or input target is a `Backend` implementation in
  `aegis-backend`, not a special case in the binary.
- Compositing and texture handling belong in `aegis-render`; the chrome
  contract and shared components belong in `aegis-shell`. A chrome component
  with its own state or dependency profile gets its own crate on the
  `aegis-shell` contract, registered by the binary
  ([ADR-0021](../adr/0021-chrome-component-trait.md),
  [ADR-0060](../adr/0060-statusbar-system-controls-and-live-system-ipc.md)).
- A persistent settings page belongs behind the `aegis-settings` module
  contract. The module emits typed settings intents; it does not write the
  configuration file or call its backing service. Compositor-owned settings
  use revisioned `aegis-ipc` transactions. System-owned settings use a separate
  authorized service adapter.
- TOML parsing, schema validation, comment-preserving typed edits, and atomic
  replacement belong in `aegis-config`. Authorization, live application, and
  serialization of concurrent edits belong in the compositor runtime.
- A rendering or texture capability missing from flux is added to flux, not
  worked around in aegis; see
  [ADR-0001](../adr/0001-scope-and-responsibility-boundary.md).
- Generic agent execution and product policy belong in
  `aegis-fuji`, fuji's self-contained runtime. Aegis-specific named-scope and
  Realm adaptation belongs in the separately launched `aegis-mcp`
  process, never in the compositor binary or the `fuji` binary.
- Cross-binding pointer casts (between the `flux` and `lens` `flux_*`
  types) stay localized at the call seam, not spread through the code.

## Documentation

Each workspace member has a short crate README that acts as its directory
landing page. Keep it focused on identity, responsibilities, boundaries,
runtime effect, and the shortest useful entry point. API details stay in
rustdoc, user-facing options stay under `docs/reference/`, and design rationale
stays in explanation documents or ADRs.

New documentation follows the
[documentation governance](documentation/index.md). Route content with the
governance's routing rules before writing.
