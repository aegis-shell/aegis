# ass-control-center

`ass-control-center` is the compositor-owned Control Center chrome component
for the ass compositor, built on the `Chrome` contract from `ass-shell`.

## Responsibilities

- Present system controls — audio, brightness, displays, touchpad, radios,
  and notifications — as a paged modal surface.
- Consume the normalized `SystemStatus` and Realm authority snapshots pushed
  by the shell.
- Emit typed `SystemAction` mutations and `RealmIntent` lifecycle requests
  through `ChromeEvents`.
- Resolve its header icon from the pushed application catalog's `IconSet`.

## Boundaries

The Control Center is a trusted in-process surface: it renders through
optics/lens and emits typed intents, but never invokes host commands or
probes the system itself. The composition root (the `ass` binary) owns the
system probing, the icon textures, and the catalog pushed through
`Chrome::update_app_catalog`.

## Runtime Effect

While open, the Control Center is modal chrome: it captures pointer and
keyboard input, requests a backdrop blur behind its panel, and suppresses
covered components. Applying display settings emits one
`SystemAction::SetDisplay`; the main loop owns the actual modeset.

## Use

Register one Control Center with the shell:

```rust
shell.add(Box::new(ass_control_center::ControlCenter::new()));
```

`Shell::add` seeds newly registered components with the current catalog, and
`Shell::set_app_catalog` fans replacements out to every component.

## Related Documentation

- [Component crate split](../../docs/adr/0044-dock-and-control-center-crates.md)
- [Chrome component decision](../../docs/adr/0021-chrome-component-trait.md)
- [AI Workspace operations](../../docs/how-to/ai-workspaces.md)
