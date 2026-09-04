# tessera-prism

`tessera-prism` is the compact, Spotlight-style application search component
for the Tessera compositor. The default `Super+Space` shortcut opens it without
expanding the full application library.

## Responsibilities

- Render a centered search field and a bounded list of application matches.
- Search names, generic names, descriptions, desktop IDs, and keywords.
- Support pointer activation and keyboard navigation with the arrow keys,
  `Enter`, and `Escape`.
- Focus a matching running window or emit an intent to start the selected
  application.
- Report its glass backdrop region, modal input ownership, and animation
  state through the `tessera-shell` `Chrome` contract.

## Boundaries

Prism owns presentation and interaction state only. The `tessera` binary owns
application discovery, decoded icon textures, detached process launch, and
Wayland focus. Prism receives immutable catalog snapshots and borrowed icon
handles from the shell.

## Use

Register Prism before persistent foreground chrome such as the Dock:

```rust
shell.add(Box::new(tessera_prism::Prism::new()));
```

The shell seeds the current application catalog during registration. Calling
`Shell::toggle_prism` opens or closes the component.

## Related Documentation

- [Dock and launcher operations](../../docs/how-to/dock-and-launcher.md)
- [System shortcuts](../../docs/reference/keyboard-shortcuts.md)
- [Chrome component contract](../../docs/adr/0021-chrome-component-trait.md)
