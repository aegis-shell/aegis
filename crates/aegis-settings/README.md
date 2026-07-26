# aegis-settings

`aegis-settings` is the standalone modular System Settings application for
aegis.

## Responsibilities

- Host settings modules in a normal Wayland window through Iris, Lens, and
  Flux.
- Provide stable module metadata, navigation ids, categories, keywords,
  backend availability, and instant or explicit apply policy.
- Read revisioned settings snapshots and submit typed settings actions over
  `aegis-ipc` without blocking the UI thread.
- Expose honest unavailable pages for domains whose authoritative backend is
  not implemented.

## Boundaries

A settings module owns presentation and local draft state. It does not read or
write `config.toml`, probe hardware, call system services, or choose its
transport. The application routes compositor-owned settings through confirmed
IPC transactions. Future system-owned modules use authorized service adapters.

Live volume, brightness, radio, notification, and current-session controls
use the separate system-control model and appear in the status bar. AI
Workspace lifecycle is authority management and lives independently in
`aegis-ai-workspaces`.

Built-in modules are registered statically because Rust has no stable dynamic
library ABI. A future third-party module boundary must be a versioned process
protocol rather than an in-process Rust `.so`.

## Runtime Effect

The binary opens an `xdg_toplevel` with app id
`io.github.ming2k.aegis.Settings`. A worker thread connects to
`$XDG_RUNTIME_DIR/aegis.sock`; the UI receives authoritative snapshots and
coalesces unsent instant edits while a confirmed transaction is in flight.

The `display` and `touchpad` modules are editable. `mouse`, `keyboard`,
`appearance`, `power`, `users`, and `window-rules` keep stable routes but
remain unavailable until their backends exist.

## Use

Run the app from the workspace:

```bash
cargo run -p aegis-settings
cargo run -p aegis-settings -- --module display
cargo run -p aegis-settings -- touchpad
```

These commands test the application directly but do not register it with
application launchers. Use `scripts/dev-loop.sh` from the repository root to
build and stage the binary, desktop entry, and icon for Dock and launcher
integration.

Production packages install
`contrib/io.github.ming2k.aegis.Settings.desktop` and the matching hicolor
icon alongside the binary.

## Related Documentation

- [System Settings Reference](../../docs/reference/settings.md)
- [First-party application installation and staging](../../docs/adr/0059-first-party-application-installation-and-development-staging.md)
- [First-Party Application Development](../../docs/dev/first-party-applications.md)
- [Status bar system controls](../../docs/adr/0060-statusbar-system-controls-and-live-system-ipc.md)
- [System Settings identity and boundary](../../docs/adr/0056-system-settings-identity-and-boundary.md)
