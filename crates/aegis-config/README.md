# aegis-config

`aegis-config` owns the versioned TOML configuration schema, loader,
diagnostics, typed document edits, atomic persistence, and caller-driven
live-reload detection for aegis.

## Responsibilities

- Parse `$XDG_CONFIG_HOME/aegis/config.toml` and validate its schema version.
- Reject unknown or malformed values with structured diagnostics.
- Resolve configuration entries into shared `aegis-model` models.
- Apply typed dock, touchpad, and output edits while preserving comments and
  unrelated keys.
- Validate the complete edited document and publish it with an atomic
  same-directory replacement.
- Explicitly migrate supported legacy schemas while preserving comments and
  creating a durable, non-overwriting owner-only backup.
- Detect file modification-time changes through `ReloadWatcher`.

## Boundaries

This crate owns the on-disk representation, not settings policy. It does not
authorize requests, apply live state to the compositor or backend, serialize
independent callers, own a filesystem-watcher thread, or render configuration
UI. The compositor validates settings transactions, serializes typed edits on
one worker, and applies successfully persisted state.

## Runtime Effect

Loading is side-effect-free apart from reading the requested file. A missing
file means built-in defaults; a failed reload leaves the caller's previous
configuration intact. `ConfigStore::apply` performs one
read-modify-validate-write transaction: invalid input never replaces the
existing file, and a successful write is flushed and atomically renamed into
place.

## Use

```rust
let path = aegis_config::default_path().expect("a config directory is available");
let store = aegis_config::ConfigStore::new(path);

if let Some(config) = store.load()? {
    let (keymap, diagnostics) = config.keymap();
    // Apply the validated snapshot and report diagnostics.
}
```

Poll `ReloadWatcher::changed` from the event loop before loading the file
again. Submit concurrent edits through one serialized owner before calling
`ConfigStore::apply`; the compositor's config-write worker is that owner in
the desktop runtime.

Use `aegis config validate [path]` for an offline validation and
`aegis config migrate [path]` for an explicit migration. The normal loader
never changes a file or guesses how to translate removed authority.

## Related Documentation

- [Configuration reference](../../docs/reference/config.md)
- [Configuration system decision](../../docs/adr/0026-configuration-system.md)
- [Workspace layout](../../docs/dev/project-layout.md)
