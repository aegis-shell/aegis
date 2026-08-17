# Screen Capture and Streaming

How pixels travel from a recording or casting application to compositor
frames, and why aegis deliberately does not expose the standard Wayland
capture protocols. The frame path spans two repositories; this page is the
compositor-side view. The portal backend's own reference is
[its documentation](https://github.com/aegis-shell/xdg-desktop-portal-aegis).

## The Four Processes

A capture session involves four processes, and the boundary between them
differs from the standard XDG desktop in one place only:

| Process | Role |
|---------|------|
| Consumer (OBS, a browser, a remote-desktop agent) | Talks ScreenCast D-Bus and PipeWire only |
| `xdg-desktop-portal` | Frontend: source picker, consent, PipeWire node id |
| `xdg-desktop-portal-aegis` | Backend: a PipeWire stream node **and** a compositor IPC client |
| aegis compositor | The only frame source; owns pacing, capture, and authorization |

PipeWire never talks to the compositor. The portal backend is the bridge,
playing the same two roles `xdg-desktop-portal-wlr` plays on wlroots — a
Wayland-side client of the compositor and a PipeWire node for the consumer.
The difference is the client side: a scoped aegis IPC connection instead of
`wlr-screencopy-unstable-v1` or `ext-image-copy-capture-v1`.

```text
[consumer] --D-Bus--> [xdg-desktop-portal] --(backend API)--> [portal-aegis]
     |                                                            |  scoped IPC
     |                     PipeWire (distribution bus only)      |
     +------------------------- frames <-------------------------+----------+
                                                                |
                                                    [aegis compositor]
```

## Frame Source and Pacing

The compositor is the only clock in the pipeline
([ADR-0130](../adr/0130-stream-paced-presentation-and-scanout-exclusion.md)).
A live output stream paces presentation at its negotiated `max_fps`:

- A stream whose `frame_interval` has elapsed — including its first frame
  after start — may **force** a presentation, so a consumer receives frames
  at the negotiated rate even on a static desktop.
- Every damage-driven composite additionally serves each due stream between
  forced frames.
- Direct scanout is disqualified while any output stream lives: a
  page-flipped frame never passes through the compositor, so a fullscreen
  video being recorded is composited — and therefore captured — instead.
- Cursor motion presented on the hardware cursor plane would otherwise never
  composite; cadence forcing carries it into the stream.

When the last stream stops, presentation returns to pure damage-driven
pacing and scanout becomes eligible again. Window streams never force
presentation; they render offscreen on their own schedule
([ADR-0127](../adr/0127-occlusion-safe-window-streams-and-cursor-compositing.md)).

## Transports

Two transports share the pacing and cursor machinery, selected by consumer
capability at PipeWire negotiation:

| Transport | Path | Notes |
|-----------|------|-------|
| Zero-copy dmabuf | Compositor blits the presented frame into a 3-slot exportable capture ring; the consumer's pool buffers bind slots by index | Explicit opt-in, IPC protocol 25; a delivered slot stays consumer-owned until `StreamBufferRelease` |
| Sealed memfd (SHM) | Full-frame readback bound to the presentation frame, converted on a worker, delivered as a sealed memfd | Backend-agnostic; works on nested and DRM/KMS hosts ([ADR-0041](../adr/0041-sealed-file-descriptor-pixel-transport.md)) |

Frames carry per-frame damage in the stream's own coordinate space
(conservatively: a forced frame or a moved crop origin reports one
full-frame rectangle) and a cumulative drop count; delivery runs over a
bounded two-frame lane per stream, and excess frames are dropped and
counted rather than queued ([ADR-0052](../adr/0052-scoped-output-frame-streaming.md)).

## Authorization and Privacy

Capture is a scoped capability, not a bindable global
([ADR-0037](../adr/0037-scoped-pixel-capture-over-ipc.md), superseded by
[ADR-0041](../adr/0041-sealed-file-descriptor-pixel-transport.md)):

- Streams run under fail-closed named IPC scopes with per-operation
  allowlists; built-in scopes are bound to the claiming peer's identity
  ([ADR-0128](../adr/0128-peer-identity-bound-built-in-scopes-and-capture-indicator.md)).
- Each stream holds a lease with a TTL, renewed at half-life.
- A capture indicator is exposed while any stream is live.
- A locked or inactive session produces no frames; the stream survives and
  resumes.

This is the primary reason the standard capture protocols are not exposed:
a bindable protocol global bypasses the named-scope model, and retrofitting
consent onto it means hand-rolling bind gates in the compositor.

## Geometry and Lifecycle

Streams are addressed to the whole desktop, one connector, or one window. A
geometry change — hotplug, mode change, window resize — freezes the stream
with `StreamGeometryChanged` carrying the new size instead of delivering
mismatched frames or tearing the session down; the client restarts the
stream at the new geometry ([ADR-0126](../adr/0126-damage-driven-stream-pacing-and-geometry-renegotiation.md),
Decisions 2 and 3). A disappeared connector or a closed window ends its
stream.

## Why Not the Standard Capture Protocols

The deliberate trade, recorded in
[ADR-0037](../adr/0037-scoped-pixel-capture-over-ipc.md) and
[ADR-0052](../adr/0052-scoped-output-frame-streaming.md):

- **Gained**: authorization bound to peer identity, negotiated frame rate,
  drop accounting, geometry renegotiation, zero-copy slots, capture
  telemetry — none expressible in `wlr-screencopy` or
  `ext-image-copy-capture-v1` — and scheduling integrated with the frame
  loop (ADR-0130's stream-paced presentation has no protocol equivalent).
- **Cost**: capture tools that only speak the standard protocols (grim,
  wf-recorder, wayvnc) do not work against aegis, and the two repositories
  evolve the shared IPC contract in lockstep.

Adopting `ext-image-copy-capture-v1` later as an additional, scope-gated
gateway into the same stream machinery is compatible with this design; it
would widen the ecosystem without giving up the private capabilities.
