# ass

`ass` is the executable composition root for autonomous surface shell, a
Wayland compositor built on flux and lens.

## Responsibilities

- Initialize logging, configuration, the presentation backend, and the
  graphics stack.
- Construct the Wayland server, renderer, shell components, wallpaper, and
  IPC server.
- Run the frame loop and route input, window actions, application launches,
  configuration reloads, and IPC commands between the library crates.

## Boundaries

Reusable models and mechanisms do not belong in this crate. They live in the
corresponding `ass-*` library so the executable remains wiring and lifecycle
code. The executable selects the presentation backend (`--backend
auto|drm|nested` or `ASS_BACKEND`): nested inside an existing session for
development, direct DRM/KMS on a bare TTY via `ass-backend`.

## Runtime Effect

Running `ass` composites client surfaces and shell chrome into a nested
window or directly onto a KMS display, exposes a Wayland display to clients,
and serves the control socket at `$XDG_RUNTIME_DIR/ass.sock`. Realm
application launch additionally requires the packaged systemd user service
with delegated `cpu`, `memory`, and `pids` controllers; other compositor
functions remain available when that preflight fails.

## Use

Build the sibling optics libraries first, then run from the repository root:

```bash
source scripts/env.sh
cargo run -p ass
```

The repository [Quick Start](../../README.md#quick-start) contains the full
dependency build sequence.

## Related Documentation

- [Architecture](../../docs/explanation/architecture.md)
- [Setup](../../docs/dev/setup.md)
- [Workspace layout](../../docs/dev/project-layout.md)
