# ADR-0005: flux core binding crate in the flux repo

- Status: Superseded by [ADR-0023](0023-split-flux-lens-stack.md)
- Date: 2026-06-04

## Context

ass is written in Rust and drives flux from Rust. flux-ui already ships
Rust bindings in its own repository (`flux-ui-sys` raw plus `flux-ui`
safe), built from the in-tree headers. Those bindings cover the surface
reachable from `<flux/ui.h>` — flux core, math, and canvas — but not
`<flux/vulkan.h>`, the raw-Vulkan seam holding the handle accessors,
bindless registration, graphics pipeline, sampler, and dynamic-rendering
pass. A compositor needs that seam: to create a `VkSurfaceKHR` on flux's
instance and to composite client textures.

There were no Rust bindings to flux core as an independent surface.

## Decision

flux gains its own Rust binding crates in its repository, mirroring the
flux-ui binding layout: `flux-sys` (bindgen over the core, math, canvas,
and `vulkan.h` headers) and `flux` (a safe wrapper). The bindings are
generated from the in-tree headers and build against flux's meson build
tree, so they stay lock-stepped with the C library. ass consumes them as
path dependencies.

## Alternatives

- **Extend flux-ui's existing `wrapper.h` to also include `vulkan.h`.**
  Rejected: it places flux core bindings inside the flux-ui package,
  confusing ownership.
- **Keep the FFI layer inside the ass repository.** Rejected: the bindings
  would drift from flux's version and duplicate the generation strategy
  flux-ui already proved.

## Consequences

- flux carries its own core bindings, consistent with flux-ui owning its
  bindings. Adding this subtree to flux is a change to that repository and
  is recorded here from ass's side; flux maintainers track it under their
  own decision process.
- The `flux` crate's `flux_device` is a distinct bindgen type from
  flux-ui's `flux_device`. Pointers cross between them by cast, since both
  are opaque and ABI-identical; ass localizes these casts.
- Each terminal binary re-emits the rpaths the `-sys` crates publish, since
  `rustc-link-arg` does not propagate across crates.
- The bindings expand demand-first; ass adds wrappers as milestones need
  them.
