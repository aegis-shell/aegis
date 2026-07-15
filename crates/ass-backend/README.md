# ass-backend

`ass-backend` abstracts the compositor's presentation target and raw input
source.

## Responsibilities

- Define the `Backend` contract used by the compositor frame loop.
- Own backend-specific window, display, resize, and input state.
- Provide the nested Wayland backend used for development and normal nested
  operation.
- Expose the Vulkan surface information needed to create the flux output
  surface.

## Boundaries

A backend does not implement the Wayland server, render client buffers, draw
chrome, or choose window-management policy. Those concerns remain in
`ass-server`, `ass-render`, `ass-shell`, and `ass-core`.

## Runtime Effect

The active backend pumps host events into backend-neutral `InputEvent` values
and presents compositor frames to its target. Closing or resizing the target
is reported to the executable through the same interface.

## Use

The executable constructs a `nested::NestedHost`, then drives it through the
`Backend` trait:

```rust
use ass_backend::nested::NestedHost;
use ass_backend::Backend;

let mut backend = NestedHost::open("ass", 1280, 720)?;
while backend.dispatch() {
    let events = backend.take_input();
    // Route events and render the next frame.
}
```

New presentation targets implement `Backend` rather than adding conditional
paths to the executable.

## Related Documentation

- [Backend abstraction](../../docs/explanation/architecture.md#backend-abstraction)
- [Nested-first decision](../../docs/adr/0003-nested-first-bring-up.md)
- [Workspace layout](../../docs/dev/project-layout.md)

