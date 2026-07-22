# ADR-0049: Standalone modular Control Center with revisioned settings IPC

- Status: Accepted
- Date: 2026-07-22

## Context

[ADR-0044](0044-dock-and-control-center-crates.md) made Control Center a
separate crate but kept it inside the compositor process because the IPC
could not read or mutate persistent settings. That restriction no longer
matches the desired product boundary. Settings pages are ordinary
applications: they need independent crash containment, launcher identity,
deep links, and a lifecycle that does not extend the compositor's trusted
rendering path.

KDE System Settings provides a useful module model. A module owns one
settings domain and its editor state, while the host owns discovery,
navigation, and the apply/default lifecycle. Copying KDE's binary plugin ABI
would not work for this Rust codebase, and loading arbitrary Rust dynamic
libraries into a trusted process would create an unstable and unsafe ABI.

The settings domains also have different authorities. Display, touchpad, and
window-rule policy belong to ass. Power policy belongs to system services.
User accounts belong to the system account service and authorization policy.
Quick toggles such as volume and Wi-Fi are live service controls rather than
persistent compositor settings. A single generic persistence backend would
erase these boundaries.

## Decision

### Standalone application boundary

`ass-control-center` is an ordinary Wayland `xdg_toplevel` application hosted
by Iris, Lens, and Flux. It has the stable application id
`io.github.ming.ass.ControlCenter` and a matching desktop entry. Launcher and
status-bar activation spawn this process through the ordinary application
launch path.

The previous compositor-hosted surface remains temporarily as a compatibility
host for Quick Settings and AI Workspace management. It is not the canonical
persistent-settings application and receives no new settings modules.

### Module contract

The application uses a KCM-inspired module contract with:

- a stable module id used by navigation and deep links;
- localized title and icon metadata;
- category and search keywords;
- an instant or explicit apply policy;
- explicit backend availability;
- a settings-snapshot update hook; and
- typed settings intents returned to the host.

Modules do not read files, call system services, or select a transport. The
host routes each intent to the authoritative backend. Built-in Rust modules
are registered statically. A future third-party extension point must use a
versioned process protocol; ass does not expose a Rust `.so` ABI.

The stable catalog is:

| Module id | Domain owner | Apply policy | Current state |
|-----------|--------------|--------------|---------------|
| `display` | ass output model and DRM backend | Explicit | Editable |
| `mouse` | libinput device policy | Instant | Backend pending |
| `touchpad` | ass input policy and libinput backend | Instant | Editable |
| `keyboard` | keymap, repeat, compose, and shortcut policy | Explicit | Backend pending |
| `appearance` | ass UI policy and desktop theme services | Explicit | Backend pending |
| `power` | logind, UPower, and power-profile services | Instant | Backend pending |
| `users` | AccountsService and authorization policy | Explicit | Backend pending |
| `window-rules` | ass configuration and window-rule model | Explicit | Backend pending |

Backend-pending modules stay discoverable so routes and ownership remain
stable, but display an unavailable state instead of controls that appear to
succeed.

### Revisioned settings transport

IPC protocol version 4 adds one coherent `SettingsSnapshot`, a
`GetSettings` query, a `SettingsChanged` event, and typed `Settings` actions.
Every snapshot carries a monotonic revision. A mutation may include the
revision the editor observed; a stale revision refuses the mutation instead
of overwriting a newer edit.

Settings mutations require the `session` capability and a live lease. The
IPC response is synchronous: `SettingsApplied` is returned only after the
compositor main loop validates, persists, and applies the action. The same
commit path serves the temporary in-process host, so there is one authority
and one mutation journal regardless of presentation.

The standalone application performs blocking IPC on a worker thread. The UI
coalesces unsent instant edits by module kind, submits them in order, and
refreshes the authoritative snapshot after each confirmation.

### Separate non-settings surfaces

Quick Settings remains compositor chrome for immediate volume, brightness,
radio, notification, and session controls. AI Workspace lifecycle remains a
Realm-management surface. Neither is a settings module or part of the
revisioned persistent-settings snapshot.

## Alternatives

- **Keep all pages in compositor chrome.** Rejected because a settings UI
  crash would remain a compositor crash, launcher identity would stay
  synthetic, and every new backend would enlarge the trusted process.
- **Copy KDE's dynamic-library plugin mechanism.** Rejected because Rust has
  no stable dynamic ABI and an in-process plugin would cross the compositor's
  trust boundary. Static built-ins and a future process protocol preserve
  modularity without that risk.
- **Let each page write `config.toml` directly.** Rejected because it races
  live reload, bypasses hardware validation and audit, and cannot confirm
  that a modeset or libinput change took effect.
- **Put Quick Settings and account management in the same settings
  transaction.** Rejected because their authorities and failure models are
  different. Live service toggles are not compositor configuration, and
  account changes require system authorization.
- **Hide every unfinished module.** Rejected because routes and domain
  ownership would shift as backends land. An explicit unavailable page is a
  truthful, stable contract.

## Consequences

- Control Center failure no longer takes down the compositor, and the app can
  be launched, grouped, and deep-linked like any other Wayland application.
- Display and touchpad editors share one module implementation between the
  standalone app and the temporary compatibility host.
- Persistent settings gain optimistic concurrency, confirmed application,
  subscription invalidation, and journal coverage.
- IPC version 3 clients must upgrade to version 4; the major-version handshake
  rejects mismatched peers.
- New modules must first identify their authoritative service and typed state
  contract. A page is not marked editable until that backend exists.
- Removing the compatibility host requires a separate external route for AI
  Workspace management and completion of the Quick Settings migration.
