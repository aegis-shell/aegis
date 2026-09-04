# tessera-model

`tessera-model` is the backend-, protocol-, and renderer-agnostic state and
deterministic model contract shared by Tessera components.

## Responsibilities

- Define logical geometry, output, input, window, workspace, Interaction
  Domain, seat, semantic object, and application types.
- Hold pure layout, key-binding, launcher, notification, and window-rule
  logic.
- Provide the stable models exchanged by the server, renderer, shell,
  configuration, and IPC crates.
- Enforce the single-controller, explicit-observer, and atomic interaction
  authority invariants shared by human and agent interaction domains.
- Optionally derive serialization through the `serde` feature.

## Boundaries

`tessera-model` performs no Wayland, Vulkan, flux, lens, filesystem, socket, or
process I/O. Mechanisms that require those dependencies belong in a more
specific crate. Being effect-free is necessary but not sufficient for
placement here: a helper with one owning component remains with that owner.

## Runtime Effect

The crate supplies state and deterministic transformations only. Keeping the
shared model pure makes window-management policy and interaction state
testable without a compositor process or graphics stack.

## Use

```rust
use tessera_model::{Point, Rect};

let work_area = Rect::new(0, 0, 1920, 1080);
assert!(work_area.contains(Point { x: 100, y: 100 }));
```

Enable the `serde` feature only for boundaries such as IPC that serialize the
shared model.

## Related Documentation

- [Architecture](../../docs/explanation/architecture.md)
- [Responsibility boundary decision](../../docs/adr/0001-scope-and-responsibility-boundary.md)
- [Workspace layout](../../docs/dev/project-layout.md)
- [Interaction Domain and seat decision](../../docs/adr/0040-realms-seats-and-transferable-interaction-authority.md)
- [Actor semantic observation decision](../../docs/adr/0102-actor-scoped-semantic-observation-and-transactional-actions.md)
