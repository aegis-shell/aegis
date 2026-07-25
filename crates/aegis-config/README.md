# aegis-config

`aegis-config` owns the versioned TOML configuration schema, loader, diagnostics,
and caller-driven live-reload detection for ass.

## Responsibilities

- Parse `$XDG_CONFIG_HOME/ass/config.toml` and validate its schema version.
- Reject unknown or malformed values with structured diagnostics.
- Resolve configuration entries into shared `aegis-core` models.
- Detect file modification-time changes through `ReloadWatcher`.

## Boundaries

This crate parses and validates configuration. It does not apply settings to
the compositor, own a filesystem-watcher thread, or render configuration UI.
The executable applies a successfully loaded snapshot to the relevant crates.

## Runtime Effect

Loading is side-effect-free apart from reading the requested file. A missing
file means built-in defaults; a failed reload leaves the caller's previous
configuration intact.

## Use

```rust
let path = ass_config::default_path().expect("a home directory is available");

if let Some(config) = ass_config::load(&path)? {
    let (keymap, diagnostics) = config.keymap();
    // Apply the validated snapshot and report diagnostics.
}
```

Poll `ReloadWatcher::changed` from the event loop before loading the file
again.

## Related Documentation

- [Configuration reference](../../docs/reference/config.md)
- [Configuration system decision](../../docs/adr/0026-configuration-system.md)
- [Workspace layout](../../docs/dev/project-layout.md)

