# aegis-dock

`aegis-dock` is the macOS-style dock chrome component for the aegis compositor,
built on the `Chrome` contract from `aegis-shell` and the shared materials from
`aegis-design`.

## Responsibilities

- Draw a translucent bottom-center bar of pinned application icons.
- Fold running windows into their pinned tile by matching `app_id` against
  each entry's `Entry::match_keys`.
- Magnify the tile under the cursor and its neighbours with a damped-spring
  reflow wave, and grow new tiles in from a seed size.
- Offer the shared application context menu, including pin/unpin actions.
- Emit launch, focus, and pin-toggle intents through `ChromeEvents`.

## Boundaries

The dock owns presentation and interaction state only. The composition root
(the `aegis` binary) resolves the pinned entries and owns the decoded icon
textures; the dock receives both as borrowed snapshots pushed through
`Chrome::update_app_catalog`. It never mutates Wayland state, spawns
processes, or writes configuration.

## Runtime Effect

Each frame the dock reports its reserved bottom edge, backdrop-blur regions,
and whether its magnification springs still animate. Pin toggles are drained
by the main loop into the `[dock] pinned` configuration, which answers with a
refreshed application catalog.

## Use

Push the application catalog, then register one dock with the shell:

```rust
shell.set_app_catalog(catalog);
shell.add(Box::new(aegis_dock::Dock::new()));
```

`Shell::add` seeds newly registered components with the current catalog, and
`Shell::set_app_catalog` fans replacements out to every component.

## Related Documentation

- [Dock and launcher operations](../../docs/how-to/dock-and-launcher.md)
- [Chrome component contract](../../docs/adr/0021-chrome-component-trait.md)
- [Chrome component decision](../../docs/adr/0021-chrome-component-trait.md)
- [Design system decision](../../docs/adr/0046-design-system-crate.md)
