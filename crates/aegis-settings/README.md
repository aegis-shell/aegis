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
use the separate system-control model and appear in the command panel. AI
Workspace lifecycle is authority management and lives independently in
`aegis-agent-workspaces`.

Built-in modules are registered statically because Rust has no stable dynamic
library ABI. A future third-party module boundary must be a versioned process
protocol rather than an in-process Rust `.so`.

## Runtime Effect

The binary opens an `xdg_toplevel` with app id
`io.github.ming2k.aegis.Settings`. A worker thread connects to
`$XDG_RUNTIME_DIR/aegis.sock`; the UI receives authoritative snapshots and
coalesces unsent instant edits while a confirmed transaction is in flight.

The `display`, `touchpad`, and `appearance` modules are editable. Appearance
submits the complete desktop preference profile as one explicit-apply
transaction. `mouse`, `keyboard`, `power`, `users`, and `window-rules` keep
stable routes but remain unavailable until their backends exist.

## Use

Run the app from the workspace:

```bash
cargo run --locked -p aegis-settings
cargo run --locked -p aegis-settings -- --module display
cargo run --locked -p aegis-settings -- touchpad
```

These commands test the application directly but do not register it with
application launchers. Use the throwaway prefix in
[Setup](../../docs/dev/setup.md#build-and-run) for Dock and launcher
integration.

Production packages install
`contrib/io.github.ming2k.aegis.Settings.desktop` and the matching hicolor
icon alongside the binary.

## Related Documentation

- [System Settings Reference](../../docs/reference/settings.md)
- [Documentation-owned installation and throwaway development staging](../../docs/adr/0069-documentation-owned-installation-and-throwaway-development-staging.md)
- [First-Party Application Development](../../docs/dev/first-party-applications.md)
- [Status bar system controls](../../docs/adr/0060-statusbar-system-controls-and-live-system-ipc.md)
- [System Settings identity and boundary](../../docs/adr/0056-system-settings-identity-and-boundary.md)
