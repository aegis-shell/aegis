# tessera-shell

`tessera-shell` hosts compositor chrome built on lens and exposes user interaction
as typed intents to the compositor loop.

## Responsibilities

- Own the lens UI context bound to the compositor's flux device.
- Host pluggable `Chrome` components in a defined render order.
- Own the shared chrome contract — `Chrome`, `ChromeCommand`, `ChromeUpdate`,
  `ChromeEvents`, and the `AppCatalog` snapshot — consumed by in-crate
  components and by the separate `tessera-dock`, `tessera-prism`, `tessera-hud`,
  and `tessera-command-panel` component crates. Persistent settings modules
  live in the `tessera-settings` library crate, hosted by the command panel.
- Provide the shared live-preview card model, hit-testing, optical focus,
  brightness hierarchy, and foreground materials used by Dock previews and
  the window switcher.
- Provide the shared modal-application layout and material primitive used by
  compositor-owned application components.
- Own `persona::Profile`, the lightweight personalized-profile convention.
  The optional `persona` feature adds the shared still/VRM portrait, motion,
  and transactional-reload runtime used by the lock and command panel.
- Draw the launcher, notification toasts, Overview, the Interaction Domain transfer shelf,
  and trusted Agent operation feedback.
- Consume window, workspace, Interaction Domain, application, notification, and input
  snapshots.
- Report focus, minimize, close, launch, workspace, and notification intents
  plus optimistic Interaction Domain lifecycle and authority-transfer intents.

## Boundaries

Chrome components do not mutate Wayland state, discover or spawn applications,
or issue presentation commands. The executable drains their intents into
`tessera-compositor` and `tessera-launcher`, while `tessera-backend` and flux own presentation.
Product-specific tokens and materials come from `tessera-design`; generic UI
behavior remains in lens.

Persona fields are display content, never authenticated Actor identity. The
portrait backend chooses and prepares content; each host owns its camera,
geometry, fallback style, and motion triggers. See the
[Persona Reference](../../docs/reference/persona.md).

## Runtime Effect

Each shell frame draws overlays above client content, reports edge reservations
for tiled work areas, and tells the event loop whether chrome captures input or
has an animation in flight. The launcher and Prism request backdrop blur; the
executable captures a live quarter-resolution desktop and applies a fixed-cost
multi-resolution filter while the shell renders the catalog surfaces above it.
Modal chrome suppresses covered components; the launcher opts in as modal and
persistent chrome such as the dock explicitly remains available during that
presentation.
Overview supports click-to-focus and drag-to-transfer. A window dropped on a
Interaction Domain emits one interaction-group transfer intent; the shell never mutates
Wayland or authority state directly.
Successfully applied Interaction Domain input is projected through a separate,
non-interactive Agent crosshair and operation label over read-only human
mirrors. Positionless or hidden-target activity uses a background pill. The
layer never changes the physical XDG cursor, never exposes key contents, and is
outside directed Interaction Domain capture.

## Use

Create one `Shell` from a live flux device pointer, register the required
`Chrome` components, and update its snapshots before rendering each frame.
After rendering, drain the typed intent accessors and apply them through the
owning crate. The unsafe device pointer passed to `Shell::new` must outlive the
shell.

## Related Documentation

- [Architecture](../../docs/explanation/architecture.md)
- [Dock, launcher, and Prism operations](../../docs/how-to/dock-and-launcher.md)
- [Borderless window operations](../../docs/how-to/window-management.md)
- [Agent Workspace operations](../../docs/how-to/ai-workspaces.md)
- [Chrome component decision](../../docs/adr/0021-chrome-component-trait.md)
- [Status bar system controls](../../docs/adr/0060-statusbar-system-controls-and-live-system-ipc.md)
- [Design system decision](../../docs/adr/0046-design-system-crate.md)
- [Agent operation feedback decision](../../docs/adr/0048-compositor-owned-agent-operation-feedback.md)
- [Persona module decision](../../docs/adr/0111-persona-as-shell-domain-with-feature-gated-portrait-runtime.md)
