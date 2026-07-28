# ADR-0068: Cargo-native development and environment-only backend selection

- Status: Superseded by ADR-0069
- Date: 2026-07-29

## Context

[ADR-0059](0059-first-party-application-installation-and-development-staging.md)
introduced a development-session shell wrapper. It built multiple binaries,
created a private runtime directory, staged XDG metadata, selected a backend,
and launched Aegis. A dedicated shell test then verified the wrapper.

The wrapper duplicated behavior already owned by Cargo, Aegis, and the
installation contract. Aegis already selects nested presentation when
`WAYLAND_DISPLAY` exists and direct DRM/KMS otherwise. Cargo already builds
and runs workspace binaries. The installation helper already installs the
desktop entry, icon, and executable required for XDG integration.

Backend choice also describes the process launch environment. Providing both
a command-line option and `AEGIS_BACKEND` created two control paths and a
precedence rule without adding a capability.

## Decision

`cargo run --locked -p aegis` is the standard source-tree development entry
point. Aegis accepts no runtime command-line options.

Backend selection uses only `AEGIS_BACKEND=auto|nested|drm`. The default is
`auto`: an inherited `WAYLAND_DISPLAY` selects nested presentation, while its
absence selects DRM/KMS. Explicit values remain available for focused tests
and service configuration.

The development-session wrapper and its shell-only test are removed. Direct
System Settings work uses `cargo run --locked -p aegis-settings`. Tests that
need desktop-entry discovery, icons, `TryExec`, and detached launch install
the development artifacts with `scripts/install.sh --user --debug` before
starting Aegis.

The production installation contract from ADR-0059 remains: packages install
the first-party executable and matching XDG metadata as one unit. This
decision supersedes ADR-0059's development workflow.

## Alternatives

- **Keep the wrapper as the documented default.** This preserves private
  runtime and staging behavior but hides the ordinary Cargo workflow and
  requires a separate test for orchestration code.
- **Keep both CLI and environment backend overrides.** This retains a
  redundant precedence rule and makes launch configuration harder to reason
  about.
- **Replace the shell wrapper with an `xtask`.** This moves the same
  orchestration into Rust without removing the duplicated workflow.
- **Remove the installation helper as well.** Cargo does not install systemd
  units, D-Bus services, desktop entries, portal metadata, or icons, so an
  installation mechanism remains necessary.

## Consequences

- Daily development uses standard Cargo commands without backend arguments.
- Backend configuration has one explicit override surface.
- XDG integration tests write to a chosen installation prefix instead of a
  private per-run staging prefix.
- Simultaneous Aegis processes under one `XDG_RUNTIME_DIR` still contend for
  the owner-only IPC socket; contributors stop the other process before IPC
  tests.
- Specialized Realm sandbox testing keeps its systemd/cgroup test command
  because Cargo alone cannot create that execution topology.
