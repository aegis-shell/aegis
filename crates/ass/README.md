# ass

`ass` is the executable composition root for autonomous surface shell, a
Wayland compositor built on flux and lens.

## Responsibilities

- Initialize logging, configuration, the nested backend, and the graphics
  stack.
- Construct the Wayland server, renderer, shell components, wallpaper, and
  IPC server.
- Run the frame loop and route input, window actions, application launches,
  configuration reloads, and IPC commands between the library crates.

## Boundaries

Reusable models and mechanisms do not belong in this crate. They live in the
corresponding `ass-*` library so the executable remains wiring and lifecycle
code. The current executable runs through the nested backend; direct DRM/KMS
operation belongs in `ass-backend` when implemented.

## Runtime Effect

Running `ass` creates a nested compositor window, exposes a Wayland display to
clients, draws client surfaces and shell chrome, and exposes the control socket
at `$XDG_RUNTIME_DIR/ass.sock`.

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

