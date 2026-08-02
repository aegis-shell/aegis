# ADR-0095: Independent portal repository and component workspace

- Status: Superseded by [ADR-0099](0099-resource-authority-and-out-of-process-file-chooser.md)
- Date: 2026-08-02

## Context

[ADR-0075](0075-independent-portal-package-and-backend-contract.md) made
`aegis-portal` an optional, version-locked runtime package but kept it in the
Aegis Cargo workspace and source release. That arrangement preserved atomic
changes while the portal contract formed, but the implementation has since
become an independently testable product with its own D-Bus ABI, PipeWire
stack, encrypted vault, Secret Service compatibility surface, PAM helper,
activation metadata, and distribution dependencies.

Keeping the code in the compositor repository now makes every Aegis source
checkout and workspace operation carry portal-only dependencies. It also
obscures release ownership: the portal is already optional at runtime and in
distribution packaging, but not in source control or CI.

The portal implementation also contained a second boundary problem. Its
Secret subsystem combines persistent encrypted state, password-derived key
material, two D-Bus APIs, and a coordinated unlock lifecycle. Leaving that
security-sensitive subsystem as private modules inside the backend crate
made its dependency and audit boundary weaker than its runtime responsibility.

## Decision

Move the portal backend into the independent
`aegis-shell/xdg-desktop-portal-aegis` Git repository. Use
`xdg-desktop-portal-aegis` as the repository, distribution package, primary
Cargo package, and private executable name, following the conventional
`xdg-desktop-portal-<backend>` identity. Preserve the relevant Aegis history
rather than importing the current tree as a history-free snapshot. The new
repository owns:

- the private `xdg-desktop-portal-aegis` binary and its integration tests;
- D-Bus activation and xdg-desktop-portal discovery metadata;
- the optional `pam_aegis.so` module; and
- its own CI, canonical `Cargo.lock`, MIT license, and release artifacts.

The core Aegis repository retains the compositor's portal-scoped IPC
authority, protocol schemas, picker and consent chrome, the session-lock PAM
profile, and user-facing integration documentation.

**Pin compatibility explicitly.** Portal and Aegis use independent version
sequences. Each Portal release pins `aegis-core`, `aegis-ipc`, and
`aegis-logging` from one supported Aegis Git tag. Portal `v0.0.1` targets
Aegis `v0.0.9`. Distributions build the two source releases separately and
express the mapped Aegis version as an exact package dependency. A local
Cargo patch may point Portal development at an adjacent Aegis checkout, but
that path-resolved lockfile is never canonical or committed.

**Use an internal component workspace.** The portal repository contains:

- `xdg-desktop-portal-aegis`, the process composition root, D-Bus interface
  assembly, Aegis IPC adapters, and workers;
- `aegis-portal-runtime`, the Aegis-independent portal Request lifecycle
  shared by backend components;
- `aegis-portal-secret`, the encrypted vault, native Secret portal,
  transitional Secret Service compatibility API, and single unlock
  coordinator; and
- `aegis-pam`, the optional login-token producer.

`aegis-portal-secret` does not depend on Aegis IPC or the portal binary. The
composition root injects a narrow password-prompt capability. All components
remain statically linked into one `xdg-desktop-portal-aegis` process, so
[ADR-0085](0085-portal-secret-absorption-and-secret-service-compat.md)'s
single vault, unlock state, and D-Bus lifecycle remain unchanged.

**Split by dependency and trust boundary, not by interface count.** Small
portal interfaces remain modules in the process crate. A component becomes a
crate only when it has a distinct dependency profile, persistent or security
state, lifecycle, or independently valuable test surface. ScreenCast is the
next eligible component if its PipeWire lifecycle needs independent reuse or
release control.

The private executable path, advertised interfaces, scoped authority,
request/session lifecycle, fallback routing, and fail-closed behavior from
ADR-0075 remain binding. The protocol-facing backend ID remains `aegis`: the
D-Bus name, `aegis.portal`, `aegis-portals.conf`, and built-in
`aegis-portal` IPC scope do not inherit the executable's long-form name.

## Alternatives

- **Keep one source repository and only create more crates.** Rejected: this
  improves code organization but leaves portal-only dependencies, CI, and
  release ownership in the compositor repository.
- **Copy the current source without history.** Rejected: file history is
  valuable for security review, compatibility archaeology, and regression
  diagnosis.
- **Let the Secret crate depend on portal-private IPC and Request modules.**
  Rejected: that reverses the dependency direction and prevents independent
  testing. Shared Request behavior belongs in a neutral runtime crate, while
  password prompting crosses an injected capability.
- **Create one crate per portal interface.** Rejected: most interfaces are
  small adapters sharing the same process state and request machinery. The
  resulting package graph would encode file layout rather than real
  architectural boundaries.
- **Run Secret storage as a second daemon.** Rejected: a crate boundary is
  sufficient for dependency and audit isolation. A second process would
  duplicate activation and unlock coordination and would reverse ADR-0085's
  deliberate single-process decision.
- **Use a Git submodule inside Aegis.** Rejected: it retains nested-checkout
  and Cargo-workspace coupling while adding submodule synchronization and CI
  failure modes.

## Consequences

- A normal Aegis checkout and workspace build no longer require PipeWire or
  portal cryptography dependencies.
- Portal changes have an independent review, CI, lockfile, and release
  surface while remaining exactly versioned against Aegis IPC.
- Cross-repository protocol changes require coordinated commits and an
  explicit compatibility mapping instead of one atomic workspace commit.
- Distribution recipes consume two source archives and build roots. The
  installed package boundary and runtime behavior do not change.
- The Secret implementation gains a narrow public construction seam and an
  explicit security dependency boundary without moving keys or unlock state
  into another process.
