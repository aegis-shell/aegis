# ADR-0001: Scope and responsibility boundary

- Status: Accepted
- Date: 2026-06-04

## Context

ass is a Wayland compositor written in Rust. It builds on two existing C
libraries developed alongside it: flux, a Vulkan-first 2D/3D graphics
library, and flux-ui, an immediate-mode UI library that draws through
flux's canvas. Both are client-side rendering libraries: flux renders into
a caller-supplied `VkSurfaceKHR` and has no windowing or Wayland code, and
flux-ui consumes input as a data snapshot and emits draw calls. Neither
implements the Wayland server protocol, input, output, or session
management.

A compositor needs all of that server-side machinery in addition to
rendering. The project must decide which component owns which
responsibility so that gaps are filled in the right place.

## Decision

ass owns everything that is not rendering or UI: the Wayland server
protocol and its globals, input, output, session and seat management,
window management, and the surface/scene model. flux owns GPU rendering and
the import of client buffers as textures. flux-ui owns the compositor's own
chrome.

When a capability ass needs is missing from a dependency, the fix is
placed by responsibility: if it is a rendering or texture concern it is
added to flux; if it is a UI concern it is added to flux-ui; otherwise ass
implements it itself.

## Alternatives

- **Fold compositor concerns into flux.** Rejected. flux is deliberately a
  client-side renderer with no platform or protocol surface; adding server
  responsibilities would distort its scope.
- **Adopt an existing compositor toolkit and treat flux as optional.**
  Rejected for the rendering path because flux and flux-ui are the
  project's chosen, co-developed graphics stack. The server-side toolkit
  question is decided separately in
  [ADR-0002](0002-hand-rolled-wayland-server.md).

## Consequences

- ass carries the full Wayland server implementation and the backend
  stack; see [ADR-0002](0002-hand-rolled-wayland-server.md) and
  [ADR-0003](0003-nested-first-bring-up.md).
- Rendering gaps are resolved by extending flux rather than working around
  it in ass; the first such extension is dmabuf import
  ([ADR-0004](0004-client-buffers-via-flux-dmabuf-import.md)).
- ass consumes flux and flux-ui from Rust, which requires Rust bindings to
  flux's core and Vulkan seam
  ([ADR-0005](0005-flux-core-binding-crate-in-flux-repo.md)).
- The shared model that does not belong to flux or flux-ui (surfaces,
  outputs, focus) lives in ass and can later grow the semantic surface the
  AI-adaptation phase needs.
