# ass-shell

`ass-shell` hosts compositor chrome built on lens and exposes user interaction
as typed intents to the compositor loop.

## Responsibilities

- Own the lens UI context bound to the compositor's flux device.
- Host pluggable `Chrome` components in a defined render order.
- Draw decorations, the dock, launcher, workspace bar, and notification toasts.
- Consume window, workspace, application, notification, and input snapshots.
- Report focus, close, move, launch, workspace, and notification intents.

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
the dock explicitly remains available during that presentation.

## Use

Create one `Shell` from a live flux device pointer, register the required
`Chrome` components, and update its snapshots before rendering each frame.
After rendering, drain the typed intent accessors and apply them through the
owning crate. The unsafe device pointer passed to `Shell::new` must outlive the
shell.

## Related Documentation

- [Architecture](../../docs/explanation/architecture.md)
- [Chrome component decision](../../docs/adr/0021-chrome-component-trait.md)
- [Workspace layout](../../docs/dev/project-layout.md)
