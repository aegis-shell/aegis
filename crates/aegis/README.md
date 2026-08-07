# aegis

`aegis` is the executable composition root for autonomous surface shell, a
Wayland compositor built on flux and lens.

## Responsibilities

- Initialize logging, configuration, the presentation backend, and the
  graphics stack.
- Select compositor mode or a native domain command before initializing the
  corresponding runtime.
- Construct the Wayland server, renderer, shell components, wallpaper, and
  IPC server.
- Run the frame loop and route input, window actions, application launches,
  configuration reloads, and IPC commands between the library crates.

## Boundaries

Reusable models and mechanisms do not belong in this crate. They live in the
corresponding `aegis-*` library so the executable remains wiring and lifecycle
code. The executable selects the presentation backend from `AEGIS_BACKEND`:
`auto` nests inside an existing session and drives DRM/KMS on a bare TTY.

## Build-Time Chrome Selection

The default `full-chrome` feature preserves the packaged desktop. Custom
product builds can disable it and select independently compiled chrome
components:

| Feature | Component | Additional service |
|---------|-----------|--------------------|
| `chrome-dock` | Persistent application Dock | None |
| `chrome-prism` | Compact application search | None |
| `chrome-hud` | Display-only status HUD | StatusNotifierItem tray |
| `chrome-command-panel` | Modal system command panel | StatusNotifierItem tray |

For example, build only the Dock and Prism on top of the shared shell:

```bash
cargo build -p aegis --no-default-features \
  --features chrome-dock,chrome-prism
```

Cargo features determine which component crates enter the binary. Runtime
configuration, such as `[hud] enabled`, controls a component only when its
crate was compiled in.

## Runtime Effect

Running `aegis` composites client surfaces and shell chrome into a nested
window or directly onto a KMS display, exposes a Wayland display to clients,
and serves the control socket at `$XDG_RUNTIME_DIR/aegis.sock`. Interaction Domain
application launch additionally requires the packaged systemd user service
with delegated `cpu`, `memory`, and `pids` controllers; other compositor
functions remain available when that preflight fails.

Running `aegis` with a resource subcommand instead connects to an existing
session without entering the compositor runtime. For example:

```bash
aegis display
aegis window
aegis workspace switch next
```

## Use

For cross-repository development, create a linked Aegis worktree next to the
Optics checkout, enable the local Cargo patch there, then build and run:

```bash
git worktree add -b dev ../aegis-dev main
cd ../aegis-dev
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
