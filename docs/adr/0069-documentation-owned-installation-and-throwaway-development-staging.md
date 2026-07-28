# ADR-0069: Documentation-owned installation and throwaway development staging

- Status: Accepted
- Date: 2026-07-29

## Context

[ADR-0068](0068-cargo-native-development-and-environment-backend-selection.md)
removed the development-session wrapper and made `cargo run --locked -p aegis`
the standard development entry point. It also retained
`scripts/install.sh` as the mechanism for tests that need desktop-entry
discovery, icon resolution, `TryExec`, and detached launch, and it documented
that path in the contributor and tutorial material.

The installer mixed two incompatible audiences in one script. For a
distribution packager it hid the real install contract behind shell logic and
under-installed it: the script staged `aegis`, `aegis-settings`, `aegis-ctl`,
and `aegis-fuji-mcp` but omitted `aegis-portal`, `fuji`, and the portal
configuration. For a contributor it wrote development artifacts into the
user's `~/.local` prefix, so stale metadata leaked between checkouts, branches,
and host sessions — exactly the property
[ADR-0059](0059-first-party-application-installation-and-development-staging.md)
introduced its private staging prefix to avoid.

Cargo itself does not install systemd units, D-Bus services, desktop entries,
portal metadata, or icons, so an installation contract still has to live
somewhere. The question is where, not whether.

## Decision

The installation contract lives in documentation, not in a repository script.
The single `scripts/install.sh` is removed and its two audiences are split:

- **Distribution packaging** is fully specified in
  [Distribution Packaging](../dev/packaging.md): the complete binary and
  metadata manifest, `DESTDIR` staging under a `/usr` logical prefix,
  `cargo build --locked --release --workspace`, Optics system dependencies, and
  package-integration hooks. The documented manifest includes `aegis-portal`,
  `fuji`, and the portal configuration the script omitted.
- **Development XDG integration** uses a throwaway staging prefix created with
  `mktemp`, documented in [Setup](../dev/setup.md). A test that needs Dock,
  launcher, icon, and `TryExec` discovery builds the debug binary, copies it
  and its `contrib/` metadata into the temporary prefix, prepends that prefix
  to `PATH` and `XDG_DATA_DIRS` for the current shell, runs the compositor with
  `cargo run --locked -p aegis`, and removes the prefix when finished. The
  user's `~/.local` is never written.

The cargo-native development entry point and environment-only backend
selection from ADR-0068 remain in force: `cargo run --locked -p aegis` is the
standard development entry point, Aegis accepts no runtime command-line
options, and backend selection uses only `AEGIS_BACKEND=auto|nested|drm`.

This decision supersedes [ADR-0068](0068-cargo-native-development-and-environment-backend-selection.md).

## Alternatives

- **Keep `scripts/install.sh` as documented in ADR-0068.** Rejected because it
  serves two audiences with one under-complete script, hides the packaging
  contract, and pollutes `~/.local` for development runs.
- **Add the missing binaries and portal metadata to the script.** This fixes
  the under-installation but keeps a redundant artifact and the `~/.local`
  pollution problem.
- **Replace the script with an `xtask`.** This moves the same orchestration
  into Rust without removing the duplicated workflow or the audience conflict.
- **Install into `~/.local` for every development run.** Rejected in
  ADR-0059 because stale metadata leaks between checkouts and host sessions.
- **Restore the ADR-0059 development supervisor under `target/aegis-dev`.**
  This restores the private staging principle but reintroduces a
  repository-owned orchestration layer that duplicates Cargo, which ADR-0068
  removed for good reason. The throwaway `mktemp` prefix reaches the same
  property with ordinary shell commands.

## Consequences

- A single contributor command never both builds and installs; daily
  development uses Cargo directly, and XDG integration tests stage into a
  throwaway prefix.
- The distribution install manifest is owned by `docs/dev/packaging.md`, so a
  packager reads one complete, correct list instead of inferring it from a
  script.
- `~/.local` is never a development target; the no-leakage property from
  ADR-0059 is restored.
- Adding a first-party external application requires updating the documented
  manifests (the development staging recipe and the packaging install table),
  not a script.
- Specialized Realm sandbox testing keeps its systemd/cgroup script because
  Cargo alone cannot create that execution topology.
- ADR-0068's installation consequences are no longer authoritative; its
  cargo-native and backend-selection consequences stand, restated above.
