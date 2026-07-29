# ADR-0067: Remote Optics dependencies with local development overrides

- Status: Superseded by ADR-0071
- Date: 2026-07-28

## Context

[ADR-0023](0023-split-flux-lens-stack.md) selected out-of-tree Rust bindings
for the rendering stack and made sibling path dependencies the workspace
default. Optics later consolidated its C libraries and binding workspaces in
one versioned monorepo.

The sibling layout is useful when a rendering change and its Aegis consumer
must be edited together. It is not a valid canonical dependency contract:
an independent Aegis checkout cannot resolve the workspace, distribution
build directories do not contain `../optics`, and CI must reproduce a
particular dependency release rather than whichever sibling commit happens
to be present.

The native binding crates already support the required binary boundary. Their
build scripts discover an uninstalled Meson tree for development or use
system-installed `.pc` files through `pkg-config`.

## Decision

Aegis declares the Optics Rust binding crates as Git dependencies on a tagged
Optics release. `Cargo.lock` pins the resolved commit. The native `flux`,
`flux-scene-graph`, `lens`, and `iris` libraries remain external system
dependencies discovered through `pkg-config`.

Cross-repository development uses a committed, opt-in Cargo configuration
that overrides the binding sources with paths under `../optics/bindings`.
Developers activate it as the ignored project Cargo configuration, so
ordinary Cargo commands use the sibling sources. Removing the ignored file
returns the checkout to the canonical dependency graph.

CI checks the downstream contract. It checks out the matching Optics tag,
builds and installs the C libraries, verifies the installed `.pc` files, and
then tests and builds Aegis with the locked remote Rust dependency graph.
Failures in any full-workspace step fail the workflow.

Distribution package preparation fetches or vendors the locked Rust Git
sources and provides the matching Optics native development package. Package
builds do not synthesize a sibling source layout.

This decision supersedes [ADR-0023](0023-split-flux-lens-stack.md).

## Alternatives

- **Keep sibling paths in the workspace manifest.** This preserves the
  shortest local edit loop but prevents independent checkout, CI, and
  conventional package builds.
- **Build Optics as a bundled Aegis subproject.** This hides the native ABI
  boundary, duplicates Optics release work, and prevents a package manager
  from owning the shared libraries.
- **Require installed Rust binding crates only.** The binding crates are not
  yet published to a registry. A locked Git source provides reproducibility
  without blocking local path overrides or a future registry release.
- **Use the local override in CI.** That tests the monorepo source layout, not
  the installed native-library contract that downstream users and packagers
  rely on.

## Consequences

- Aegis can be checked out and resolved without a sibling repository.
- Local changes across Aegis and Optics remain immediately testable through
  the opt-in path override and uninstalled Meson tree.
- The Optics tag in the workspace manifest, CI checkout, native package, and
  lockfile must move together during an upgrade.
- CI must install the Optics C libraries and dynamic-loader metadata before
  running full-workspace Cargo commands.
- Offline package builds must vendor the locked Git dependencies during
  source preparation.
