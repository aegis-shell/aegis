# ADR-0058: Independent Quick Settings and AI Workspaces applications

- Status: Superseded by ADR-0060
- Date: 2026-07-26

## Context

[ADR-0056](0056-system-settings-identity-and-boundary.md) established System
Settings as the standalone persistent-configuration application, but retained
`aegis-control-center` as a temporary in-process host for two unrelated
surfaces:

- Quick Settings changes immediate service and session state.
- AI Workspaces manages Realm lifecycle and interaction authority.

The temporary host no longer contained persistent settings, yet its name and
navigation still implied one user-facing product. It also left an obsolete
Control Center identity in the crate graph, localization catalog, built-in
application model, status surfaces, and runtime diagnostics. Quick Settings
had no stable launcher entry after the standalone settings application moved,
so the compatibility host preserved code without preserving a coherent
access path.

## Decision

The Control Center product and implementation identity are retired.

Quick Settings and AI Workspaces are independent compositor-owned
applications:

| Surface | Cargo crate | Built-in identity | Authority |
|---------|-------------|-------------------|-----------|
| Quick Settings | `aegis-quick-settings` | `BuiltInApplication::QuickSettings` | Immediate system and session actions |
| AI Workspaces | `aegis-ai-workspaces` | `BuiltInApplication::AiWorkspaces` | Realm lifecycle and interaction authority |

Both applications implement the shared `Chrome` contract and use a common
modal-application presentation primitive from `aegis-shell`. They have
separate open state, snapshots, typed intents, tests, launcher entries, and
source identities. Presenting one built-in application closes the other.

The status bar remains a compact status and notification surface. Its network
control opens Quick Settings, while its Fuji indicator opens AI Workspaces.
The launcher also exposes both built-in applications. Neither surface writes
configuration or belongs in the standalone `aegis-settings` process.

The `aegis-control-center` crate, `ControlCenter` type,
`BuiltInApplication::ControlCenter` variant, translation key, icon-cache key,
and runtime source identity are removed without compatibility aliases. This
decision amends the Control Center portion of
[ADR-0044](0044-dock-and-control-center-crates.md) and completes the temporary
retirement path recorded in ADR-0056. It does not change the canonical System
Settings namespace established by
[ADR-0057](0057-system-settings-canonical-namespace.md).

## Alternatives

- **Keep the temporary aggregate host.** Rejected because a temporary name
  had become part of current architecture and continued to obscure two
  different authority models.
- **Rename the aggregate crate.** Rejected because a neutral name such as
  `aegis-system-surfaces` would hide the same coupling without removing it.
- **Move both surfaces into System Settings.** Rejected because immediate
  service actions and Realm authority are not persistent configuration and
  do not share System Settings' backend or failure model.
- **Keep Quick Settings only in the status bar.** Rejected because the compact
  panel does not expose the complete connectivity, brightness, layout, and
  session control surface.
- **Duplicate modal layout in both crates.** Rejected because presentation
  mechanics belong at the shared shell seam, while product state and intents
  remain component-owned.

## Consequences

- No current crate, symbol, application identity, translation, diagnostic,
  or documentation surface uses Control Center as an active concept.
- Quick Settings regains a stable access path without becoming persistent
  configuration.
- AI Workspace code has a crate boundary aligned with its Realm authority
  model.
- The compositor gains two small component dependencies instead of one
  aggregate dependency.
- Historical ADRs retain their original Control Center text; their index
  statuses and this record identify the current decision.
