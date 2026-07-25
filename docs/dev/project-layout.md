# Project Layout

Where code lives and where new files belong. For the conceptual design, see
[Architecture](../explanation/architecture.md).

## Source Tree

```text
ass/
  Cargo.toml            workspace
  crates/
    aegis-core/          shared model: geometry, surface tree, outputs, focus, apps
    aegis-protocols/     shared generated Wayland protocol interface tables
    aegis-compositor/        Wayland server: socket, globals, object lifecycle
    aegis-backend/       presentation + input targets (nested, DRM/KMS + libinput + libseat)
    aegis-render/        compositing through flux
    aegis-shell/         compositor chrome host and contract through lens
    aegis-dock/          bottom-center dock chrome component
    aegis-ctl-center/  standalone modular settings app + compatibility chrome host
    aegis-statusbar/     top status bar chrome component (workspaces, tray, clock, system status, SNI tray)
    aegis-wallpaper/     image and short-video background layer
    aegis-config/        declarative configuration: TOML schema, loader, live reload
    aegis-ipc/           versioned IPC and introspection over a unix socket
    aegis-ctl/       command-line driver for the IPC (reference external tool)
    aegis-fuji/          fuji in one crate: scoped MCP platform bridge + its own agent runtime
    aegis-desktop-entries/          freedesktop.org desktop-entry enumeration + icon lookup
    aegis-launcher/        detached, XDG-environment-aware app launching
    ass/               the binary: wiring and event loop
  docs/                 documentation (see docs/index.md)
```

flux and lens live in the sibling `../optics` Meson project under
`libs/flux` and `libs/lens`. Their Rust bindings live under
`../optics/bindings/`; ass consumes them as path dependencies.

## Modules

| Crate | Purpose | Design reference |
|-------|---------|------------------|
| [`aegis-core`](../../crates/aegis-core/README.md) | Backend- and renderer-agnostic types | [ADR-0001](../adr/0001-scope-and-responsibility-boundary.md) |
| [`aegis-protocols`](../../crates/aegis-protocols/README.md) | Shared generated Wayland protocol tables for client and server | [ADR-0002](../adr/0002-hand-rolled-wayland-server.md) |
| [`aegis-compositor`](../../crates/aegis-compositor/README.md) | Wayland server socket, globals, and object lifecycle | [ADR-0002](../adr/0002-hand-rolled-wayland-server.md) |
| [`aegis-backend`](../../crates/aegis-backend/README.md) | The `Backend` trait and its implementations | [ADR-0002](../adr/0002-hand-rolled-wayland-server.md), [ADR-0003](../adr/0003-nested-first-bring-up.md) |
| [`aegis-render`](../../crates/aegis-render/README.md) | Client buffers to flux textures, scene to output | [ADR-0004](../adr/0004-client-buffers-via-flux-dmabuf-import.md) |
| [`aegis-shell`](../../crates/aegis-shell/README.md) | Chrome host, `Chrome` contract, and shared components on lens | [ADR-0021](../adr/0021-chrome-component-trait.md) |
| [`aegis-dock`](../../crates/aegis-dock/README.md) | Bottom-center dock chrome component | [ADR-0019](../adr/0019-dock-as-bottom-center-overlay.md), [ADR-0044](../adr/0044-dock-and-control-center-crates.md) |
| [`aegis-ctl-center`](../../crates/aegis-ctl-center/README.md) | Standalone modular settings application and temporary compatibility chrome host | [ADR-0049](../adr/0049-standalone-modular-control-center.md) |
| [`aegis-statusbar`](../../crates/aegis-statusbar/README.md) | Top status bar chrome component with the StatusNotifierItem tray | [ADR-0045](../adr/0045-statusbar-crate-and-sni-tray.md) |
| [`aegis-wallpaper`](../../crates/aegis-wallpaper/README.md) | Image and short-video background layer | [ADR-0018](../adr/0018-wallpaper-crate.md) |
| [`aegis-config`](../../crates/aegis-config/README.md) | Versioned TOML schema, loader, and mtime-based live reload | [ADR-0026](../adr/0026-configuration-system.md) |
| [`aegis-ipc`](../../crates/aegis-ipc/README.md) | Versioned schema and codec over a unix socket; the extension/automation surface | [ADR-0027](../adr/0027-ipc-and-introspection.md) |
| [`aegis-ctl`](../../crates/aegis-ctl/README.md) | Command-line driver for the IPC; the reference external tool | [ADR-0027](../adr/0027-ipc-and-introspection.md) |
| [`aegis-fuji`](../../crates/aegis-fuji/README.md) | fuji in one crate: scoped MCP bridge plus its self-contained agent runtime (`aegis-fuji-mcp`, `fuji`) | [ADR-0047](../adr/0047-neenee-agent-realm-platform-bridge.md), [ADR-0050](../adr/0050-fuji-agent-product-and-bridge-rename.md) |
| [`aegis-desktop-entries`](../../crates/aegis-desktop-entries/README.md) | freedesktop.org desktop-entry enumeration and icon-theme lookup | [ADR-0022](../adr/0022-application-launcher.md) |
| [`aegis-launcher`](../../crates/aegis-launcher/README.md) | Detached, XDG-environment-aware launching of desktop applications | [ADR-0022](../adr/0022-application-launcher.md) |
| [`ass`](../../crates/ass/README.md) | Process entry point and frame loop | [Architecture](../explanation/architecture.md) |

## Placement Rules

- Code with no flux, lens, or Wayland dependency belongs in `aegis-core`.
- A new presentation or input target is a `Backend` implementation in
  `aegis-backend`, not a special case in the binary.
- Compositing and texture handling belong in `aegis-render`; the chrome
  contract and shared components belong in `aegis-shell`. A chrome component
  with its own state or dependency profile gets its own crate on the
  `aegis-shell` contract, registered by the binary
  ([ADR-0021](../adr/0021-chrome-component-trait.md),
  [ADR-0044](../adr/0044-dock-and-control-center-crates.md)).
- A persistent settings page belongs behind the `aegis-ctl-center` module
  contract. The module emits typed settings intents; it does not write the
  configuration file or call its backing service. Compositor-owned settings
  use revisioned `aegis-ipc` transactions. System-owned settings use a separate
  authorized service adapter.
- A rendering or texture capability missing from flux is added to flux, not
  worked around in ass; see
  [ADR-0001](../adr/0001-scope-and-responsibility-boundary.md).
- Generic agent execution and product policy belong in the agent half of
  `aegis-fuji`, fuji's self-contained runtime. ASS-specific named-scope and
  Realm adaptation belongs in the separately launched `aegis-fuji-mcp`
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
