# ass-backend

`ass-backend` abstracts the compositor's presentation target and raw input
source.

## Responsibilities

- Define the `Backend` contract used by the compositor frame loop.
- Own backend-specific window, display, resize, and input state.
- Provide the **nested** Wayland backend used for development inside an
  existing session, and the **DRM/KMS** backend that drives display hardware
  directly from a bare TTY with libinput input and libseat session
  management (VT switching, hotplug, explicit sync via `IN_FENCE_FD`).
- Expose the backend-specific device extensions and surface factory the
  flux output surface needs (`host::Host`).

## Boundaries

A backend does not implement the Wayland server, render client buffers, draw
chrome, or choose window-management policy. Those concerns remain in
`ass-server`, `ass-render`, `ass-shell`, and `ass-core`.

## Runtime Effect

The active backend pumps host events into backend-neutral `InputEvent` values
and presents compositor frames to its target. Resize, hotplug, and VT
suspend/resume are reported through the same interface; the nested backend
ignores the direct-display-only calls (VT switch, surface recreation).

## Use

The executable selects a target through `host::Host` (`--backend
auto|drm|nested` or `ASS_BACKEND`; `auto` nests when `$WAYLAND_DISPLAY` is
set and drives KMS on a TTY), then drives it through the `Backend` trait:

```rust
use ass_backend::host::{BackendKind, Host};
use ass_backend::Backend;

let mut host = Host::open(BackendKind::Auto, "ass", 1280, 720, Default::default())?;
let device = host.create_device()?;
let mut surface = host.create_surface(&device)?;
while host.dispatch_timeout(std::time::Duration::from_secs(1)) {
    let events = host.take_input();
    // Route events and render the next frame.
}
```

New presentation targets implement `Backend` rather than adding conditional
paths to the executable.

## Related Documentation

- [Backend abstraction](../../docs/explanation/architecture.md#backend-abstraction)
- [Nested-first decision](../../docs/adr/0003-nested-first-bring-up.md)
- [Bare-metal bring-up checklist](../../docs/how-to/bare-metal-drm.md)
- [Workspace layout](../../docs/dev/project-layout.md)
