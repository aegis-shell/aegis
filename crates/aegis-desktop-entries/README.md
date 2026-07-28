# aegis-desktop-entries

`aegis-desktop-entries` discovers launchable desktop applications and resolves their icons
using freedesktop.org desktop-entry and icon-theme conventions.

## Responsibilities

- Scan `XDG_DATA_HOME` and `XDG_DATA_DIRS` for regular and exported-symlink
  `.desktop` files.
- Parse application entries, localized names, desktop visibility rules, and
  `Exec` fields.
- Resolve icons through scale-aware theme metadata, recursive inheritance,
  the `hicolor` theme, and unthemed pixmap fallback.
- Produce the shared `aegis_core::app::Entry` model.

## Boundaries

This crate discovers and parses applications. It does not render launcher UI
or icons, spawn processes, or manage windows. Those responsibilities belong to
`aegis-shell`, `aegis-launcher`, and `aegis-compositor` respectively.

## Runtime Effect

Discovery is synchronous and returns a snapshot of the applications visible
to the current XDG environment. Invalid, hidden, desktop-specific, or
unusable entries are excluded from the result.

## Use

```rust
let applications = aegis_apps::enumerate();

for application in applications {
    println!("{}: {}", application.id, application.name);
}
```

Use `resolve_icon` when a caller needs an icon path for a selected entry. Use
`resolve_icon_scaled` for a HiDPI-aware lookup, and use `aegis-launcher` to start
the resulting application.

## Related Documentation

- [Application launcher decision](../../docs/adr/0022-application-launcher.md)
- [Workspace layout](../../docs/dev/project-layout.md)
