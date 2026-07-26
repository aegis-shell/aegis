# ADR-0056: System Settings identity and Control Center boundary

- Status: Superseded by ADR-0057
- Date: 2026-07-26

## Context

[ADR-0049](0049-standalone-modular-control-center.md) moved persistent
settings into an ordinary Wayland application but kept the Control Center
product identity from the earlier compositor-hosted surface. The resulting
crate mixed two different responsibilities:

- a standalone persistent-settings application with module navigation,
  revisioned snapshots, and confirmed edits; and
- compositor-owned Quick Settings and AI Workspace lifecycle controls.

Quick Settings changes live service state, and AI Workspaces manage Realm
authority. Neither is persistent configuration. Calling the standalone
application Control Center obscures that boundary and gives its launcher
identity a broader meaning than its actual behavior.

The project has not published a tagged release. The application identity can
therefore change without maintaining a released desktop-file or executable
compatibility contract.

## Decision

The standalone persistent-settings product is **System Settings**. Its stable
technical identities are:

| Surface | Identity |
|---------|----------|
| Cargo package and executable | `aegis-settings` |
| Wayland application id | `io.github.ming.aegis.Settings` |
| Desktop-file id | `io.github.ming.aegis.Settings.desktop` |
| English display name | `System Settings` |
| Simplified Chinese display name | `系统设置` |

The `aegis-settings` crate owns the module contract, built-in settings pages,
standalone application host, and settings IPC worker. Deep links continue to
use stable module ids through positional arguments or `--module`.

The `aegis-control-center` crate remains a temporary compositor-owned chrome
component. It contains only Quick Settings and AI Workspace lifecycle
management. It does not host persistent-settings modules, and new modules
must not be added to it.

ADR-0049's module isolation, authoritative-backend, revisioned-transaction,
and worker-thread decisions remain in effect as part of this decision. This
record replaces ADR-0049 because its former application and crate identities
are no longer current.

No compatibility executable or duplicate desktop file is provided for the
former `aegis-control-center` application binary, stale `aegis-ctl-center`
fallback command, or `io.github.ming.aegis.ControlCenter` application id.
Packaging must remove obsolete desktop files when upgrading a development
installation.

## Alternatives

- **Rename only the launcher label.** Rejected because the package,
  executable, application id, desktop-file id, fallback catalog entry, and
  documentation would continue to express the wrong product boundary.
- **Rename the whole mixed crate without splitting it.** Rejected because a
  crate named settings would still own Quick Settings and Realm lifecycle
  code, making the new name inaccurate at the implementation boundary.
- **Move Quick Settings and AI Workspaces into System Settings.** Rejected
  because their authorities, persistence, and failure models differ from
  persistent settings.
- **Ship aliases for the old identity.** Rejected because there is no tagged
  release to support, and aliases would preserve duplicate launcher and icon
  identities before the canonical name is established.

## Consequences

- Launcher search, task grouping, desktop packaging, executable names, and
  source-tree commands use one System Settings identity.
- Persistent-settings code has a crate boundary independent of compositor
  chrome, so the compositor does not depend on the standalone application.
- Control Center has a narrower, accurate temporary role and can be removed
  after Quick Settings and AI Workspaces gain permanent presentation routes.
- Development installations that contain
  `io.github.ming.aegis.ControlCenter.desktop` must delete that obsolete file
  during upgrade.
- Existing development-only dock pins or window-grouping state that reference
  the old application id do not migrate automatically.
