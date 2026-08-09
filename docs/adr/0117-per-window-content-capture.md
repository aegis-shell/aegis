# ADR-0117: Per-window content capture

- Status: Accepted
- Date: 2026-08-10

## Context

An Agent operating on the human desktop could read window metadata
(`ObserveWindows`) and capture whole outputs through portal-held operations,
but it had no sanctioned way to see one window's content. The existing pixel
paths each answer a different question: `CaptureOutput` reads the focused
physical output including chrome and unrelated windows, `StreamTarget::Window`
crops a window's visible region out of the presented frame, and
`CaptureInteractionDomain` reads a directed virtual output the Agent itself
controls. None returns the real pixels of one arbitrary desktop window —
occluded, minimized, or living on another workspace.

The workaround, transferring the window into an Agent Interaction Domain and
capturing a region of its directed output, forces an authority transition just
to look at a window. Observation should not require ownership.

ADR-0102 established that semantic state is the primary observation path and
framebuffer pixels are a compatibility path carrying no action authority.
Per-window capture must preserve that ranking rather than reopen a
pixel-first interaction model.

## Decision

Aegis adds one fail-closed per-window content-capture operation across the
capability vocabulary, the IPC protocol, the compositor runtime, and the MCP
bridge:

- a new `ActorCapability::CaptureWindow`, agent-requestable and always
  runtime-gated, so the first use asks the user interactively however the
  ceiling was approved (ADR-0090);
- `Request::CaptureWindow { window }` → `Response::CaptureWindow` (protocol
  26), with the PNG transferred as a sealed memfd under the same blob rules
  as `CaptureOutput`;
- offscreen rendering of the window's complete surface tree — toplevel,
  subsurfaces, and popups — into a fresh per-capture readback target, so the
  capture carries true content whether the window is visible, occluded,
  minimized, or on another workspace; and
- an MCP `window_capture` tool that returns the image inline (capped at
  32 MiB) plus an owner-only compatibility file, without any observation
  token.

The captured image's origin is the toplevel's logical origin; popups
extending past the toplevel bounds are clipped, mirroring
`StreamTarget::Window` semantics. The capture scale follows the output the
window is currently visible on, falling back to the primary output and then
to 1000.

Authorization mirrors `CaptureOutput`: the connection needs `control`, a
live lease, and an explicit `CaptureWindow` decision for the exact window —
`ActorScope::decide_window_capture` requires the scope's `windows` axis to
permit the window and the operation to be named in `ops` (permit) or
`ask_ops` (runtime grant); it is never inherited through the unrestricted
default. The lock/VT gate refuses the request, invalidates in-flight pixels
through the capture security generation, and stays armed in the IPC writer,
which rechecks the live scope, the lease, and that the window still exists
immediately before sending the sealed descriptor. Pixel encoding runs in the
bounded capture worker like every other capture.

## Alternatives

- **Crop the window out of the output frame** (the `StreamTarget::Window`
  approach). Cheaper — no extra render pass — but it captures what the user
  sees, not what the window contains: occlusion, minimization, and workspace
  switches all destroy the content. Per-window capture exists precisely for
  the cases cropping cannot serve.
- **Reuse `CaptureInteractionDomain` by transferring the window first.**
  Conflating observation with authority transfer mutates the desktop to read
  it, disturbs the user's window placement, and cannot observe a window the
  human keeps controlling.
- **A generic region capture over the human desktop.** Simpler, but it leaks
  neighbouring content and chrome into every capture and gives the scope
  model no resource axis to bound. Naming one window keeps the grant
  semantically exact.
- **Filter minimized or occluded windows like presentation does.** The
  buffers stay live regardless of presentation, and authorization already
  happened at the IPC layer; presentation-state filters would only make the
  operation silently incomplete.

## Consequences

Agents can observe human-desktop windows without taking authority over them,
which unblocks visual verification workflows the semantic tree does not cover
yet (ADR-0102's semantic-first ranking is unchanged: these pixels issue no
observation token and feed no input path). The dangerous-set runtime grant
now covers one more privacy-sensitive operation, and the `windows` scope axis
gains a second consumer alongside input injection.

The compositor pays one offscreen render pass per capture and holds at most
one in-flight window readback, bounded by the existing capture-worker lane.
Built-in named scopes do not list `CaptureWindow`; paired agents reach it only
through the runtime grant. Every capture allocates a fresh target instead of
caching one per window, trading a small allocation cost for not retaining
per-window GPU surfaces indefinitely.
