# ADR-0057: System Settings canonical namespace

- Status: Superseded by ADR-0059
- Date: 2026-07-26

## Context

[ADR-0056](0056-system-settings-identity-and-boundary.md) established the
standalone System Settings product identity, but used `ming` as the GitHub
owner in its reverse-DNS application namespace. The repository owner is
`ming2k`. Desktop-file ids, Wayland application ids, and icon names must use
one namespace so launchers and task switchers can associate the installed
metadata, running window, and application icon.

The project has not published a tagged release, so the incorrect
development-only namespace can be replaced without a compatibility alias.

## Decision

System Settings uses these canonical identities:

| Surface | Identity |
|---------|----------|
| Cargo package and executable | `aegis-settings` |
| Wayland application id | `io.github.ming2k.aegis.Settings` |
| Desktop-file id | `io.github.ming2k.aegis.Settings.desktop` |
| Icon name | `io.github.ming2k.aegis.Settings` |

The desktop file and hicolor icon use the canonical names. Source-tree runs
resolve the same bundled icon when the application metadata has not been
installed into an XDG data directory.

No alias is provided for `io.github.ming.aegis.Settings` or its desktop file.

## Alternatives

- **Keep the `ming` namespace.** Rejected because it does not identify the
  repository owner and would publish incorrect application metadata.
- **Keep both namespaces.** Rejected because duplicate desktop entries and
  icon identities would make launcher and window grouping ambiguous before
  the first tagged release.
- **Use a non-reverse-DNS identifier.** Rejected because the desktop file,
  Wayland application id, and icon should share a conventional globally
  scoped identity.

## Consequences

- Packagers install
  `io.github.ming2k.aegis.Settings.desktop` and the matching hicolor icon.
- The Dock can use the same icon for installed and source-tree Settings
  entries.
- Development-only pins or window state recorded under the incorrect
  namespace do not migrate automatically.
