# First-Party Application Development

Use this workflow for standalone applications that ship with Aegis but run as
ordinary Wayland clients. System Settings is the first application following
this model.

The architecture is defined by
[ADR-0059](../adr/0059-first-party-application-installation-and-development-staging.md).

## Process Boundary

Sharing one repository and release does not make an application part of the
compositor process:

```text
Aegis repository and release
├── aegis
│   └── Wayland compositor and IPC server
└── aegis-settings
    └── independent Wayland client and IPC client
```

Cargo workspace membership coordinates source and builds. It does not load
one binary into another, place sibling binaries on `PATH`, or register XDG
metadata.

System Settings reaches Aegis through two runtime boundaries:

- Wayland provides the window, input, rendering, and
  `io.github.ming2k.aegis.Settings` application identity.
- `$XDG_RUNTIME_DIR/aegis.sock` provides revisioned settings snapshots and
  actions.

Do not add `aegis-settings` as an `aegis` Rust dependency. Shared schemas and
models belong in library crates such as `aegis-core` and `aegis-ipc`.

## Installation Contract

A production package installs all parts of an application as one unit:

| Artifact | Path relative to the installation prefix |
|----------|------------------------------------------|
| System Settings executable | `bin/aegis-settings` |
| Desktop entry | `share/applications/io.github.ming2k.aegis.Settings.desktop` |
| Application icon | `share/icons/hicolor/scalable/apps/io.github.ming2k.aegis.Settings.svg` |

The desktop entry's `Exec`, `TryExec`, `Icon`, and `StartupWMClass` values
must agree with the installed executable, icon name, and Wayland application
id. Do not use an absolute build-directory path in packaged metadata.

Cargo does not install desktop entries or icons. Distribution packages and
installation tooling must install the files under `contrib/` alongside the
compiled binaries.

## Development Staging

Development simulates installation without writing to `~/.local` or `/usr`.
The private prefix defaults to:

```text
target/aegis-dev/
├── bin/
│   └── aegis-settings
└── share/
    ├── applications/
    │   └── io.github.ming2k.aegis.Settings.desktop
    └── icons/hicolor/scalable/apps/
        └── io.github.ming2k.aegis.Settings.svg
```

Run one integrated development session:

```bash
scripts/dev.sh
```

The runner:

1. builds `aegis` and `aegis-settings`;
2. stages the compiled application and its XDG metadata;
3. prepends `target/aegis-dev/bin` to `PATH`;
4. prepends `target/aegis-dev/share` to `XDG_DATA_DIRS`;
5. starts one nested compositor session; and
6. lets the normal XDG scanner discover System Settings.

The staging directory is a build artifact. It persists across development
runs for inspection and is safe to remove through the normal Cargo target
cleanup process.

## One-Off Workflows

### Application UI

Run System Settings directly when only its UI, IPC behavior, or module state
needs testing:

```bash
cargo run -p aegis-settings
```

This starts the independent application but does not register it with the
Dock or launcher. The command connects to the Aegis instance selected by the
current `XDG_RUNTIME_DIR` and `WAYLAND_DISPLAY`.

### XDG Integration

Use the runner for normal integration testing:

```bash
scripts/dev.sh
```

Open Applications inside the nested session and launch System Settings. This
path verifies desktop discovery, icon resolution, `TryExec`, detached launch,
Wayland application grouping, and IPC connectivity together.

### Staging Layout

The staging step is intentionally small. To inspect it without starting the
compositor, run:

```bash
install -Dm0755 target/debug/aegis-settings \
  target/aegis-dev/bin/aegis-settings
install -Dm0644 contrib/io.github.ming2k.aegis.Settings.desktop \
  target/aegis-dev/share/applications/io.github.ming2k.aegis.Settings.desktop
install -Dm0644 contrib/icons/hicolor/scalable/apps/io.github.ming2k.aegis.Settings.svg \
  target/aegis-dev/share/icons/hicolor/scalable/apps/io.github.ming2k.aegis.Settings.svg
find target/aegis-dev -type f -print
```

Adjust `target/debug` when using another Cargo target directory or profile.

## Test Selection

| Change | Minimum focused validation |
|--------|----------------------------|
| Settings module or UI | `cargo test -p aegis-settings` |
| Shared application identity | `cargo test -p aegis-core app::tests` |
| Desktop scanning or icon lookup | `cargo test -p aegis-desktop-entries` |
| Development build, staging, and environment behavior | `tests/dev-workflow.sh` |
| Dock and launcher integration | Run `scripts/dev.sh` and launch Settings from Applications |
| Production package | Install into a clean prefix and verify the same desktop id, icon, and executable |

Run the workspace suite before delivery:

```bash
cargo test --workspace
tests/dev-workflow.sh
```

## New First-Party Applications

When adding another external application:

1. Add a workspace binary with an independent process boundary.
2. Assign one reverse-DNS application id.
3. Add a desktop entry whose filename and `StartupWMClass` match that id.
4. Add a hicolor icon whose name matches the desktop entry's `Icon`.
5. Add the binary and metadata to the staging step in `scripts/dev.sh`.
6. Extend `tests/dev-workflow.sh`.
7. Add the artifacts to the production package manifest.
8. Test launching it from Applications instead of synthesizing a catalog
   entry.

Compositor-owned virtual surfaces are different: they do not have an
external executable and may remain explicit built-in catalog entries.

## Troubleshooting

| Symptom | Check |
|---------|-------|
| Settings is absent from Applications | Use `scripts/dev.sh`; confirm the staged desktop file exists and `XDG_DATA_DIRS` starts with the staged `share` directory. |
| Settings appears but does not launch | Confirm the staged binary is executable, staged `bin` is on `PATH`, and `TryExec=aegis-settings` resolves. |
| Settings uses a fallback glyph | Confirm the desktop `Icon` matches the staged SVG filename and `rsvg-convert` is available. |
| A running Settings window is a separate Dock item | Confirm `StartupWMClass`, the Wayland application id, and the desktop-file stem share the canonical identity. |
| Host applications disappear | Preserve `/usr/local/share:/usr/share` after the staged directory in `XDG_DATA_DIRS`. |
| A direct `cargo run -p aegis` cannot find Settings | This is expected; direct Cargo runs do not stage first-party applications. |
