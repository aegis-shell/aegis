# First-Party Application Development

Use this workflow for standalone applications that ship with Aegis but run as
ordinary Wayland clients. System Settings is the first application following
this model.

The architecture is defined by
[ADR-0069](../adr/0069-documentation-owned-installation-and-throwaway-development-staging.md).

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
models belong in library crates such as `aegis-model` and `aegis-ipc`.

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

Direct Cargo commands are the default development loop. Stage the complete
first-party application contract into a throwaway prefix only when a test
needs XDG discovery, so the user's `~/.local` stays clean. Follow the
canonical recipe in [Setup](setup.md#build-and-run).

The desktop entry's `Exec=aegis-settings` resolves through `PATH` and its
`Icon` through `XDG_DATA_DIRS`, so the launcher follows the same lookup path
as a packaged installation. Distribution installation also owns the systemd
unit, D-Bus service, and portal metadata; its complete manifest is in
[Distribution Packaging](packaging.md).

## One-Off Workflows

### Application UI

Run System Settings directly when only its UI, IPC behavior, or module state
needs testing:

```bash
cargo run --locked -p aegis-settings
```

This starts the independent application but does not register it with the
Dock or launcher. The command connects to the Aegis instance selected by the
current `XDG_RUNTIME_DIR` and `WAYLAND_DISPLAY`.

### XDG Integration

With the staging prefix exported above, start the compositor:

```bash
cargo run --locked -p aegis
```

Open Applications inside the nested session and launch System Settings. This
verifies desktop discovery, icon resolution, `TryExec`, detached launch,
Wayland application grouping, and IPC connectivity together. Inspect the
staged tree at any time with `find "$stage" -type f -print`.

## Test Selection

| Change | Minimum focused validation |
|--------|----------------------------|
| Settings module or UI | `cargo test --locked -p aegis-settings` |
| Shared application identity | `cargo test --locked -p aegis-model app::tests` |
| Desktop scanning or icon lookup | `cargo test --locked -p aegis-desktop-entries` |
| Dock and launcher integration | Stage the development prefix, start Aegis, and launch Settings from Applications |
| Production package | Install into a clean prefix and verify the same desktop id, icon, and executable |

Run the workspace suite before delivery:

```bash
cargo test --locked --workspace
```

## New First-Party Applications

When adding another external application:

1. Add a workspace binary with an independent process boundary.
2. Assign one reverse-DNS application id.
3. Add a desktop entry whose filename and `StartupWMClass` match that id.
4. Add a hicolor icon whose name matches the desktop entry's `Icon`.
5. Add the binary and metadata to the development staging recipe and
   production package manifest.
6. Test launching it from Applications instead of synthesizing a catalog
   entry.

Compositor-owned virtual surfaces are different: they do not have an
external executable and may remain explicit built-in catalog entries.

## Troubleshooting

| Symptom | Check |
|---------|-------|
| Settings is absent from Applications | Stage the development prefix and confirm the desktop file exists under `$XDG_DATA_DIRS` and `$aegis_stage/share/applications`. |
| Settings appears but does not launch | Confirm `aegis-settings` is executable on `PATH` and `TryExec=aegis-settings` resolves. |
| Settings uses a fallback glyph | Confirm the desktop `Icon` matches the installed SVG filename and `rsvg-convert` is available. |
| A running Settings window is a separate Dock item | Confirm `StartupWMClass`, the Wayland application id, and the desktop-file stem share the canonical identity. |
| A direct `cargo run --locked -p aegis` cannot find Settings | Stage the development prefix first; Cargo does not install desktop entries or icons. |
