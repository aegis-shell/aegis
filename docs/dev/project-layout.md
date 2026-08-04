# Project Layout

Where code lives and where new files belong. For the conceptual design, see
[Architecture](../explanation/architecture.md).

## Source Tree

```text
aegis/
  Cargo.toml            workspace
  crates/
    aegis-model/        shared effect-free state and deterministic model rules
    aegis-security/      Actor authority and integrity-checked audit modules
    aegis-semantic/      validated application accessibility trees and semantic action routing
    aegis-wayland-protocols/     shared generated Wayland protocol interface tables
    aegis-compositor/        Wayland server: socket, globals, object lifecycle
    aegis-backend/       presentation + input targets (nested, DRM/KMS + libinput + libseat)
    aegis-render/        compositing through flux
    aegis-shell/         chrome host plus feature-gated persona domain
    aegis-dock/          bottom-center dock chrome component
    aegis-prism/         compact application-search chrome component
    aegis-agent-workspaces/     Agent Workspaces lifecycle and authority UI
    aegis-settings/       standalone modular System Settings application
    aegis-atspi/          supervised out-of-process AT-SPI semantic adapter
    aegis-hud/           display-only HUD status chips (system status, workspace dots, clock, SNI tray)
    aegis-command-panel/ full-screen modal command panel (quick settings, tray, notifications)
    aegis-wallpaper/     image, video, 3D, and parallax background layer
    aegis-config/        TOML schema, typed atomic persistence, loader, live reload
    aegis-ipc/           Actor capability broker and introspection over a unix socket
    aegis-commands/    domain command parser and IPC dispatcher (lib-only)
    aegis-agent/         the in-tree agent runtime CLI (`aegis-agent`), internal persona: fuji (宓姬)

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

The optional
[xdg-desktop-portal-aegis repository](https://github.com/aegis-shell/xdg-desktop-portal-aegis)
owns the private backend, its encrypted Secret implementation, PipeWire
bridge, PAM helper, D-Bus activation files, and portal metadata. It depends
on `aegis-model`, `aegis-ipc`, and `aegis-logging` from its declared
compatible Aegis tag; none of its crates are workspace members here. See
[ADR-0095](../adr/0095-independent-portal-repository-and-component-workspace.md).

## Modules

| Crate | Purpose | Design reference |
|-------|---------|------------------|
| [`aegis-model`](../../crates/aegis-model/README.md) | Shared effect-free state, identities, geometry, and deterministic model rules | [ADR-0110](../adr/0110-shared-model-crate-naming-and-placement.md), [ADR-0001](../adr/0001-scope-and-responsibility-boundary.md) |
| [`aegis-security`](../../crates/aegis-security/README.md) | Transport-neutral Actor authority and integrity-checked audit mechanisms | [ADR-0109](../adr/0109-module-first-security-and-presentation-identity-boundaries.md) |
| [`aegis-semantic`](../../crates/aegis-semantic/README.md) | Bounded accessibility-tree validation, window-local identity namespacing, provider ownership, and semantic action routing | [ADR-0104](../adr/0104-actor-sessions-resource-grants-and-accessibility-adapter.md) |
| [`aegis-wayland-protocols`](../../crates/aegis-wayland-protocols/README.md) | Shared generated Wayland protocol tables for client and server | [ADR-0002](../adr/0002-hand-rolled-wayland-server.md) |
| [`aegis-compositor`](../../crates/aegis-compositor/README.md) | Wayland server socket, globals, and object lifecycle | [ADR-0002](../adr/0002-hand-rolled-wayland-server.md) |
| [`aegis-backend`](../../crates/aegis-backend/README.md) | The `Backend` trait and its implementations | [ADR-0002](../adr/0002-hand-rolled-wayland-server.md), [ADR-0003](../adr/0003-nested-first-bring-up.md) |
| [`aegis-render`](../../crates/aegis-render/README.md) | Client buffers to flux textures, scene to output | [ADR-0004](../adr/0004-client-buffers-via-flux-dmabuf-import.md) |
| [`aegis-shell`](../../crates/aegis-shell/README.md) | Chrome host, `Chrome` contract, shared components, and the feature-gated `persona` profile/portrait domain | [ADR-0021](../adr/0021-chrome-component-trait.md), [ADR-0111](../adr/0111-persona-as-shell-domain-with-feature-gated-portrait-runtime.md) |
| [`aegis-dock`](../../crates/aegis-dock/README.md) | Bottom-center dock chrome component | [ADR-0019](../adr/0019-dock-as-bottom-center-overlay.md), [ADR-0021](../adr/0021-chrome-component-trait.md) |
| [`aegis-prism`](../../crates/aegis-prism/README.md) | Compact Spotlight-style application search component | [ADR-0021](../adr/0021-chrome-component-trait.md), [ADR-0044](../adr/0044-dock-and-control-center-crates.md) |
| [`aegis-agent-workspaces`](../../crates/aegis-agent-workspaces/README.md) | Compositor-owned Agent Workspaces lifecycle and authority UI | [ADR-0108](../adr/0108-agent-workspaces-presentation-naming.md) |
| [`aegis-settings`](../../crates/aegis-settings/README.md) | Standalone modular System Settings application | [ADR-0069](../adr/0069-documentation-owned-installation-and-throwaway-development-staging.md) |
| [`aegis-atspi`](../../crates/aegis-atspi/README.md) | Supervised out-of-process AT-SPI semantic observation and action adapter | [ADR-0104](../adr/0104-actor-sessions-resource-grants-and-accessibility-adapter.md) |
| [`aegis-hud`](../../crates/aegis-hud/README.md) | Display-only HUD status chips with the StatusNotifierItem tray row | [ADR-0080](../adr/0080-hud-status-chips-and-sao-command-panel.md), [ADR-0081](../adr/0081-hud-and-command-panel-naming.md) |
| [`aegis-command-panel`](../../crates/aegis-command-panel/README.md) | Full-screen modal command panel: quick settings, tray activation, and notifications | [ADR-0080](../adr/0080-hud-status-chips-and-sao-command-panel.md), [ADR-0081](../adr/0081-hud-and-command-panel-naming.md) |
| [`aegis-wallpaper`](../../crates/aegis-wallpaper/README.md) | Image, video, 3D, and parallax background layer | [ADR-0018](../adr/0018-wallpaper-crate.md), [ADR-0092](../adr/0092-explicit-wallpaper-modes-and-continuous-parallax.md) |
| [`aegis-config`](../../crates/aegis-config/README.md) | Versioned TOML schema, typed atomic persistence, loader, and mtime-based live reload | [ADR-0026](../adr/0026-configuration-system.md) |
| [`aegis-ipc`](../../crates/aegis-ipc/README.md) | Versioned Actor identity, capability, semantic observation, action, and audit protocol over a unix socket | [ADR-0027](../adr/0027-ipc-and-introspection.md), [ADR-0102](../adr/0102-actor-scoped-semantic-observation-and-transactional-actions.md) |
| [`aegis-commands`](../../crates/aegis-commands/README.md) | Domain command parser, IPC dispatcher, and output formatter; no installed binary | [ADR-0093](../adr/0093-unified-domain-oriented-aegis-command-surface.md) |
| [`aegis-mcp`](../../crates/aegis-mcp/README.md) | The platform's scoped MCP bridge for any agent (`aegis-mcp`) | [ADR-0047](../adr/0047-neenee-agent-realm-platform-bridge.md), [ADR-0087](../adr/0087-aegis-mcp-standalone-platform-bridge-crate.md) |
| [`aegis-agent`](../../crates/aegis-agent/README.md) | Aegis agent runtime: self-contained agent CLI (`aegis-agent`), internal persona fuji | [ADR-0050](../adr/0050-fuji-agent-product-and-bridge-rename.md), [ADR-0089](../adr/0089-aegis-agent-product-and-fuji-identity-rename.md) |

| [`aegis-desktop-entries`](../../crates/aegis-desktop-entries/README.md) | freedesktop.org desktop-entry enumeration and icon-theme lookup | [ADR-0022](../adr/0022-application-launcher.md) |
| [`aegis-launcher`](../../crates/aegis-launcher/README.md) | Detached, XDG-environment-aware launching of desktop applications | [ADR-0022](../adr/0022-application-launcher.md) |
| [`aegis`](../../crates/aegis/README.md) | Unified process entry point, native command entry, and compositor frame loop | [Architecture](../explanation/architecture.md) |

## Placement Rules

- State, value types, and deterministic invariants shared across components
  belong in `aegis-model` when they require no concrete I/O, protocol,
  renderer, or toolkit mechanism. An effect-free helper with one clear owner
  remains in that owner's module; absence of flux, lens, or Wayland alone is
  not a reason to place code in the shared model.
- A new presentation or input target is a `Backend` implementation in
  `aegis-backend`, not a special case in the binary.
- Compositing and texture handling belong in `aegis-render`; the chrome
  contract and shared components belong in `aegis-shell`. A chrome component
  with its own state or dependency profile gets its own crate on the
  `aegis-shell` contract, registered by the binary
  ([ADR-0021](../adr/0021-chrome-component-trait.md),
  [ADR-0060](../adr/0060-statusbar-system-controls-and-live-system-ipc.md)).
- Personalized shell profile conventions belong in `aegis-shell::persona`.
  Its lightweight profile contract is always available; consumers enable the
  `persona` feature only when they need still/VRM content, motion, or live
  reload. Authentication and Actor principals do not enter this module.
- A persistent settings page belongs behind the `aegis-settings` module
  contract. The module emits typed settings intents; it does not write the
  configuration file or call its backing service. Compositor-owned settings
  use revisioned `aegis-ipc` transactions. System-owned settings use a separate
  authorized service adapter.
- TOML parsing, schema validation, explicit comment-preserving migrations,
  typed edits, and atomic replacement belong in `aegis-config`.
  Authorization, live application, and serialization of concurrent edits
  belong in the compositor runtime.
- A rendering or texture capability missing from flux is added to flux, not
  worked around in aegis; see
  [ADR-0001](../adr/0001-scope-and-responsibility-boundary.md).
- Generic agent execution and product policy belong in `aegis-agent`, the
  self-contained runtime whose internal persona remains fuji. Aegis-specific
  capability borrowing and Interaction Domain adaptation belong in the separately
  launched `aegis-mcp` process, never in the agent or compositor binary.
- Actor capability vocabulary, sessions, exact resource grants, observation
  leases, action preconditions, and generic durable audit mechanisms belong
  in the corresponding `aegis-security` modules. Untrusted
  accessibility graph validation belongs in `aegis-semantic`; AT-SPI and
  toolkit calls belong in `aegis-atspi`; wire framing belongs in `aegis-ipc`;
  and effect commit belongs at the compositor boundary. Agent plans and
  long-term memory do not enter any of those crates.
- Cross-binding pointer casts (between the `flux` and `lens` `flux_*`
  types) stay localized at the call seam, not spread through the code.

## Dependency Direction

The workspace enforces the lowest-level dependency boundaries in CI.
`aegis-model` and `aegis-agent` do not depend on another Aegis crate.
Security, semantic, configuration, IPC, backend, render, and compositor
crates use explicit downward allowlists. `aegis-commands` remains a lib-only
client layer over `aegis-model`, `aegis-config`, and `aegis-ipc`; it does not
depend on the compositor, backend, renderer, or shell.

Run the same check locally from the repository root:

```bash
scripts/check-crate-boundaries.sh --locked
```

## Documentation

Each workspace member has a short crate README that acts as its directory
landing page. Keep it focused on identity, responsibilities, boundaries,
runtime effect, and the shortest useful entry point. API details stay in
rustdoc, user-facing options stay under `docs/reference/`, and design rationale
stays in explanation documents or ADRs.

New documentation follows the
[documentation governance](documentation/index.md). Route content with the
governance's routing rules before writing.
