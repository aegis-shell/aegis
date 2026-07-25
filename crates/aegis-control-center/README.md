# aegis-ctl-center

`aegis-ctl-center` is the standalone modular settings application for ass.
The crate also retains the temporary in-process host for Quick Settings and
AI Workspace management during migration.

## Responsibilities

- Host settings modules in a normal Wayland window through Iris, Lens, and
  Flux.
- Provide stable module metadata, navigation ids, categories, keywords,
  backend availability, and instant or explicit apply policy.
- Read revisioned settings snapshots and submit typed settings actions over
  `aegis-ipc` without blocking the UI thread.
- Share display and touchpad module implementations with the compositor
  compatibility host.
- Expose honest unavailable pages for domains whose authoritative backend is
  not implemented yet.

## Boundaries

A settings module owns presentation and local draft state. It does not read
or write `config.toml`, probe hardware, call system services, or choose its
transport. The standalone host routes compositor-owned settings through
confirmed IPC transactions. Future system-owned modules use their authorized
service adapters instead.

Quick Settings is not persistent configuration. Volume, brightness, radios,
notifications, and session actions remain compositor chrome. Realm lifecycle
is authority management and remains in the compatibility host.

Built-in modules are registered statically because Rust has no stable dynamic
library ABI. A future third-party module boundary must be a versioned process
protocol rather than an in-process Rust `.so`.

## Runtime Effect

The binary opens an `xdg_toplevel` with app id
`io.github.ming.ass.ControlCenter`. A worker thread connects to
`$XDG_RUNTIME_DIR/aegis.sock`; the UI receives authoritative snapshots and
coalesces unsent instant edits while a confirmed transaction is in flight.

The `display` and `touchpad` modules are editable. `mouse`, `keyboard`,
`appearance`, `power`, `users`, and `window-rules` keep stable routes but
remain unavailable until their backends exist.

## Use

Run the app from the workspace:

```bash
cargo run -p aegis-ctl-center
cargo run -p aegis-ctl-center -- --module display
cargo run -p aegis-ctl-center -- touchpad
```

Package `contrib/io.github.ming.ass.ControlCenter.desktop` beside the binary
to expose it through application launchers.

The compatibility host remains available to the composition root:

```rust
shell.add(Box::new(ass_control_center::ControlCenter::new()));
```

## Related Documentation

- [Control Center reference](../../docs/reference/control-center.md)
- [Standalone modular Control Center decision](../../docs/adr/0049-standalone-modular-control-center.md)
- [Component crate split](../../docs/adr/0044-dock-and-control-center-crates.md)
- [Design system decision](../../docs/adr/0046-design-system-crate.md)
