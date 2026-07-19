# ass-shell

`ass-shell` hosts compositor chrome built on lens and exposes user interaction
as typed intents to the compositor loop.

## Responsibilities

- Own the lens UI context bound to the compositor's flux device.
- Host pluggable `Chrome` components in a defined render order.
- Own the shared chrome contract — `Chrome`, `ChromeEvents`, and the
  `AppCatalog` snapshot — consumed by in-crate components and by the
  separate `ass-dock` and `ass-control-center` component crates.
- Draw decorations, the launcher, workspace bar, notification toasts,
  Overview, and the Realm transfer shelf.
- Consume window, workspace, Realm, application, notification, and input
  snapshots.
- Report focus, minimize, close, move, launch, workspace, and notification
  intents plus optimistic Realm lifecycle and authority-transfer intents.

## Boundaries

Chrome components do not mutate Wayland state, discover or spawn applications,
or issue presentation commands. The executable drains their intents into
`ass-server` and `ass-launch`, while `ass-backend` and flux own presentation.

## Runtime Effect

Each shell frame draws overlays above client content, reports edge reservations
for tiled work areas, and tells the event loop whether chrome captures input or
has an animation in flight. The launcher additionally requests a backdrop blur;
the executable captures a live quarter-resolution desktop and applies a
fixed-cost multi-resolution filter while the shell renders the responsive app
grid above it.
Modal chrome suppresses covered components; the launcher opts in as modal and
persistent chrome such as the dock explicitly remains available during that
presentation.
Overview supports click-to-focus and drag-to-transfer. A window dropped on a
Realm emits one interaction-group transfer intent; the shell never mutates
Wayland or authority state directly.

## Use

Create one `Shell` from a live flux device pointer, register the required
`Chrome` components, and update its snapshots before rendering each frame.
After rendering, drain the typed intent accessors and apply them through the
owning crate. The unsafe device pointer passed to `Shell::new` must outlive the
shell.

## Related Documentation

- [Architecture](../../docs/explanation/architecture.md)
- [Dock and launcher operations](../../docs/how-to/dock-and-launcher.md)
- [Borderless window operations](../../docs/how-to/window-management.md)
- [AI Workspace operations](../../docs/how-to/ai-workspaces.md)
- [Chrome component decision](../../docs/adr/0021-chrome-component-trait.md)
- [Component crate split](../../docs/adr/0044-dock-and-control-center-crates.md)
