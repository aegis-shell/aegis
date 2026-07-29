# aegis

`aegis` is the executable composition root for autonomous surface shell, a
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
corresponding `aegis-*` library so the executable remains wiring and lifecycle
code. The executable selects the presentation backend from `AEGIS_BACKEND`:
`auto` nests inside an existing session and drives DRM/KMS on a bare TTY.

## Runtime Effect

Running `aegis` composites client surfaces and shell chrome into a nested
window or directly onto a KMS display, exposes a Wayland display to clients,
and serves the control socket at `$XDG_RUNTIME_DIR/aegis.sock`. Realm
application launch additionally requires the packaged systemd user service
with delegated `cpu`, `memory`, and `pids` controllers; other compositor
functions remain available when that preflight fails.

## Use

For cross-repository development, create a linked Aegis worktree next to the
Optics checkout, enable the local Cargo patch there, then build and run:

```bash
git worktree add ../aegis-optics-dev -b feat/<topic> origin/main
cd ../aegis-optics-dev
cp .cargo/optics-local.toml .cargo/config.toml
git config core.hooksPath .githooks
meson compile -C ../optics/build
cargo check -p aegis
cargo run --locked -p aegis
```

The repository [Quick Start](../../README.md#quick-start) contains the full
dependency build sequence. Contributors editing both repositories should
follow
[Aegis and Optics Cross-Repository Development](../../docs/dev/cross-repository-development.md).

## Related Documentation

- [Architecture](../../docs/explanation/architecture.md)
- [Setup](../../docs/dev/setup.md)
- [Workspace layout](../../docs/dev/project-layout.md)
