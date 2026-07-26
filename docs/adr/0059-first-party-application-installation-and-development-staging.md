# ADR-0059: First-party application installation and development staging

- Status: Accepted
- Date: 2026-07-26

## Context

[ADR-0057](0057-system-settings-canonical-namespace.md) established the
canonical identity of the standalone System Settings application. Its initial
development fallback synthesized an application-catalog entry inside the
compositor and read the icon directly from the source tree.

That fallback made the application visible without installing its XDG
metadata, but it did not build or place the `aegis-settings` executable on
`PATH`. It also gave a first-party external application a discovery path that
packaged applications never use. A development checkout could therefore pass
launcher presentation checks while still failing to launch or package the
application correctly.

System Settings is an ordinary application process even though it shares the
Aegis repository and release. Development must exercise the same desktop
entry, icon lookup, executable discovery, Wayland identity, and IPC boundary
used after installation.

## Decision

First-party external applications use standard XDG installation and
discovery. The compositor does not synthesize their catalog entries and does
not read their assets from source-tree paths.

The canonical System Settings identities remain:

| Surface | Identity |
|---------|----------|
| Cargo package and executable | `aegis-settings` |
| Wayland application id | `io.github.ming2k.aegis.Settings` |
| Desktop-file id | `io.github.ming2k.aegis.Settings.desktop` |
| Icon name | `io.github.ming2k.aegis.Settings` |

A production package installs the executable under the selected binary
prefix, the desktop file under `share/applications`, and the icon under the
matching hicolor application directory.

Development uses a private installation staging prefix under Cargo's target
directory. The development supervisor:

1. builds the compositor and every staged first-party application;
2. stages their executable and XDG metadata under `target/aegis-dev`;
3. prepends the staged `bin` directory to `PATH`;
4. prepends the staged `share` directory to `XDG_DATA_DIRS`; and
5. starts the compositor, which discovers the application through its normal
   XDG scanner.

The staging prefix never writes to the user's XDG data home or a system
installation prefix. Compositor-owned virtual surfaces, such as Quick
Settings and AI Workspaces, remain built-in catalog entries because they are
not external application processes.

## Alternatives

- **Keep the source-tree catalog fallback.** Rejected because it bypasses
  executable discovery and allows development behavior to diverge from
  packaging.
- **Install into `~/.local` for every development run.** Rejected because
  stale metadata leaks between checkouts, tests, and host sessions.
- **Link `aegis-settings` into the compositor.** Rejected because it removes
  the application's process, crash, Wayland, and IPC boundaries.
- **Move System Settings to a separate repository.** Rejected because
  repository ownership is independent of process and packaging boundaries.

## Consequences

- `cargo run -p aegis` runs only the compositor and does not register
  first-party external applications.
- The repository-owned development supervisor is the default integration
  path for Dock, launcher, icon, and application-startup testing.
- `cargo run -p aegis-settings` remains useful for direct UI testing but does
  not simulate application installation.
- Production packaging must install the binary, desktop file, and icon as one
  unit.
- Adding a first-party external application requires updating the staging
  manifest and its integration tests.
- Development and production exercise one XDG discovery and launch path.
