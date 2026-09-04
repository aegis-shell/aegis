# Project Layout

Where code lives and where new files belong. For the conceptual design, see
[Architecture](../explanation/architecture.md).

## Source Tree

```text
tessera/
  Cargo.toml            workspace
  crates/
    tessera-model/        shared effect-free state and deterministic model rules
    tessera-security/      Actor authority and integrity-checked audit modules
    tessera-semantic/      validated application accessibility trees and semantic action routing
    tessera-wayland-protocols/     shared generated Wayland protocol interface tables
    tessera-compositor/        Wayland server: socket, globals, object lifecycle
    tessera-backend/       presentation + input targets (nested, DRM/KMS + libinput + libseat)
    tessera-render/        compositing through flux
    tessera-shell/         chrome host plus feature-gated persona domain
    tessera-dock/          bottom-center dock chrome component
    tessera-prism/         compact application-search chrome component
    tessera-settings/       settings module library: contract, registry, and built-in pages
    tessera-atspi/          supervised out-of-process AT-SPI semantic adapter
    tessera-hud/           display-only HUD status chips (system status, workspace dots, clock, SNI tray)
    tessera-command-panel/ full-screen modal command panel (quick settings, settings modules, tray, notifications)
    tessera-wallpaper/     image, video, 3D, and parallax background layer
    tessera-config/        TOML schema, typed atomic persistence, loader, live reload
    tessera-ipc/           Actor capability broker and introspection over a unix socket
    tessera-commands/    domain command parser and IPC dispatcher (lib-only)

    tessera-desktop-entries/          freedesktop.org desktop-entry enumeration + icon lookup
    tessera-launcher/        detached, XDG-environment-aware app launching
    tessera/             the binary: wiring and event loop
  docs/                 documentation (see docs/index.md)
```

flux, lens, iris, and their Rust binding workspaces live in the
[Optics monorepo](https://github.com/ming2k/optics). The canonical dependency
graph uses the locked Optics Git release and system-installed C libraries.
Cross-repository development uses a
[worktree-isolated Cargo patch](cross-repository-development.md) for
`../optics/bindings` and discovers that checkout's uninstalled Meson tree.

The optional
[xdg-desktop-portal-atrium repository](https://github.com/aegis-shell/xdg-desktop-portal-atrium)
owns the private backend, its encrypted Secret implementation, PipeWire
bridge, PAM helper, D-Bus activation files, and portal metadata. It depends
on `tessera-model`, `tessera-ipc`, and `tessera-logging` from its declared
compatible Tessera tag; none of its crates are workspace members here. See
[ADR-0095](../adr/0095-independent-portal-repository-and-component-workspace.md).

## Modules

| Crate | Purpose | Design reference |
|-------|---------|------------------|
| [`tessera-model`](../../crates/tessera-model/README.md) | Shared effect-free state, identities, geometry, and deterministic model rules | [ADR-0110](../adr/0110-shared-model-crate-naming-and-placement.md), [ADR-0001](../adr/0001-scope-and-responsibility-boundary.md) |
| [`tessera-security`](../../crates/tessera-security/README.md) | Transport-neutral Actor authority and integrity-checked audit mechanisms | [ADR-0109](../adr/0109-module-first-security-and-presentation-identity-boundaries.md) |
| [`tessera-semantic`](../../crates/tessera-semantic/README.md) | Bounded accessibility-tree validation, window-local identity namespacing, provider ownership, and semantic action routing | [ADR-0104](../adr/0104-actor-sessions-resource-grants-and-accessibility-adapter.md) |
| [`tessera-wayland-protocols`](../../crates/tessera-wayland-protocols/README.md) | Shared generated Wayland protocol tables for client and server | [ADR-0002](../adr/0002-hand-rolled-wayland-server.md) |
| [`tessera-compositor`](../../crates/tessera-compositor/README.md) | Wayland server socket, globals, and object lifecycle | [ADR-0002](../adr/0002-hand-rolled-wayland-server.md) |
| [`tessera-backend`](../../crates/tessera-backend/README.md) | The `Backend` trait and its implementations | [ADR-0002](../adr/0002-hand-rolled-wayland-server.md), [ADR-0003](../adr/0003-nested-first-bring-up.md) |
| [`tessera-render`](../../crates/tessera-render/README.md) | Client buffers to flux textures, scene to output | [ADR-0004](../adr/0004-client-buffers-via-flux-dmabuf-import.md) |
| [`tessera-shell`](../../crates/tessera-shell/README.md) | Chrome host, `Chrome` contract, shared components, and the feature-gated `persona` profile/portrait domain | [ADR-0021](../adr/0021-chrome-component-trait.md), [ADR-0111](../adr/0111-persona-as-shell-domain-with-feature-gated-portrait-runtime.md) |
| [`tessera-dock`](../../crates/tessera-dock/README.md) | Bottom-center dock chrome component | [ADR-0019](../adr/0019-dock-as-bottom-center-overlay.md), [ADR-0021](../adr/0021-chrome-component-trait.md) |
| [`tessera-prism`](../../crates/tessera-prism/README.md) | Compact Spotlight-style application search component | [ADR-0021](../adr/0021-chrome-component-trait.md), [ADR-0044](../adr/0044-dock-and-control-center-crates.md) |
| [`tessera-settings`](../../crates/tessera-settings/README.md) | Settings module library: the `SettingsModule` contract, registry, and built-in pages hosted by the command panel | [ADR-0114](../adr/0114-panel-hosted-settings-and-hud-command-panel.md) |
| [`tessera-atspi`](../../crates/tessera-atspi/README.md) | Supervised out-of-process AT-SPI semantic observation and action adapter | [ADR-0104](../adr/0104-actor-sessions-resource-grants-and-accessibility-adapter.md) |
| [`tessera-hud`](../../crates/tessera-hud/README.md) | Display-only HUD status chips with the StatusNotifierItem tray row | [ADR-0081](../adr/0081-hud-and-command-panel-naming.md), [ADR-0083](../adr/0083-frameless-transient-toasts-and-hud-consolidation.md) |
| [`tessera-command-panel`](../../crates/tessera-command-panel/README.md) | Full-screen modal command panel: quick settings, hosted settings modules, tray activation, and notifications | [ADR-0081](../adr/0081-hud-and-command-panel-naming.md), [ADR-0114](../adr/0114-panel-hosted-settings-and-hud-command-panel.md) |
| [`tessera-wallpaper`](../../crates/tessera-wallpaper/README.md) | Image, video, 3D, and parallax background layer | [ADR-0018](../adr/0018-wallpaper-crate.md), [ADR-0092](../adr/0092-explicit-wallpaper-modes-and-continuous-parallax.md) |
| [`tessera-config`](../../crates/tessera-config/README.md) | Versioned TOML schema, typed atomic persistence, loader, and mtime-based live reload | [ADR-0026](../adr/0026-configuration-system.md) |
| [`tessera-ipc`](../../crates/tessera-ipc/README.md) | Versioned Actor identity, capability, semantic observation, action, and audit protocol over a unix socket | [ADR-0027](../adr/0027-ipc-and-introspection.md), [ADR-0102](../adr/0102-actor-scoped-semantic-observation-and-transactional-actions.md) |
| [`tessera-commands`](../../crates/tessera-commands/README.md) | Domain command parser, IPC dispatcher, and output formatter; no installed binary | [ADR-0093](../adr/0093-unified-domain-oriented-tessera-command-surface.md) |
| [`tessera-mcp`](../../crates/tessera-mcp/README.md) | The platform's scoped MCP bridge for any agent (`tessera-mcp`) | [ADR-0047](../adr/0047-neenee-agent-realm-platform-bridge.md), [ADR-0087](../adr/0087-tessera-mcp-standalone-platform-bridge-crate.md) |

| [`tessera-desktop-entries`](../../crates/tessera-desktop-entries/README.md) | freedesktop.org desktop-entry enumeration and icon-theme lookup | [ADR-0022](../adr/0022-application-launcher.md) |
| [`tessera-launcher`](../../crates/tessera-launcher/README.md) | Detached, XDG-environment-aware launching of desktop applications | [ADR-0022](../adr/0022-application-launcher.md) |
| [`tessera`](../../crates/tessera/README.md) | Unified process entry point, native command entry, and compositor frame loop | [Architecture](../explanation/architecture.md) |

## Placement Rules

- State, value types, and deterministic invariants shared across components
  belong in `tessera-model` when they require no concrete I/O, protocol,
  renderer, or toolkit mechanism. An effect-free helper with one clear owner
  remains in that owner's module; absence of flux, lens, or Wayland alone is
  not a reason to place code in the shared model.
- A new presentation or input target is a `Backend` implementation in
  `tessera-backend`, not a special case in the binary.
- Compositing and texture handling belong in `tessera-render`; the chrome
  contract and shared components belong in `tessera-shell`. A chrome component
  with its own state or dependency profile gets its own crate on the
  `tessera-shell` contract, registered by the binary
  ([ADR-0021](../adr/0021-chrome-component-trait.md),
  [ADR-0060](../adr/0060-statusbar-system-controls-and-live-system-ipc.md)).
- Personalized shell profile conventions belong in `tessera-shell::persona`.
  Its lightweight profile contract is always available; consumers enable the
  `persona` feature only when they need still/VRM content, motion, or live
  reload. Authentication and Actor principals do not enter this module.
- A persistent settings page belongs behind the `tessera-settings` module
  contract. The module emits typed settings intents; it does not write the
  configuration file or call its backing service. The command panel hosts
  the modules in-process and routes their intents into the compositor's
  commit path; external clients reach the same path through revisioned
  `tessera-ipc` transactions. System-owned settings use a separate
  authorized service adapter.
- TOML parsing, schema validation, explicit comment-preserving migrations,
  typed edits, and atomic replacement belong in `tessera-config`.
  Authorization, live application, and serialization of concurrent edits
  belong in the compositor runtime.
- A rendering or texture capability missing from flux is added to flux, not
  worked around in tessera; see
  [ADR-0001](../adr/0001-scope-and-responsibility-boundary.md).
- Agent runtimes and agent products live out of tree. Tessera-specific MCP
  exposure lives in the separately launched `tessera-mcp` process, never in
  the compositor binary.
- Actor capability vocabulary, sessions, exact resource grants, observation
  leases, action preconditions, and generic durable audit mechanisms belong
  in the corresponding `tessera-security` modules. Untrusted
  accessibility graph validation belongs in `tessera-semantic`; AT-SPI and
  toolkit calls belong in `tessera-atspi`; wire framing belongs in `tessera-ipc`;
  and effect commit belongs at the compositor boundary. Agent plans and
  long-term memory do not enter any of those crates.
- Cross-binding pointer casts (between the `flux` and `lens` `flux_*`
  types) stay localized at the call seam, not spread through the code.

## Dependency Direction

The workspace enforces the lowest-level dependency boundaries in CI.
`tessera-model` does not depend on another Tessera crate.
Security, semantic, configuration, IPC, backend, render, and compositor
crates use explicit downward allowlists. `tessera-commands` remains a lib-only
client layer over `tessera-model`, `tessera-config`, and `tessera-ipc`; it does not
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
