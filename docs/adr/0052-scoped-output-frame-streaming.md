# ADR-0052: Scoped output frame streaming

- Status: Proposed
- Date: 2026-07-24

## Context

[ADR-0051](0051-portal-backend-dbus-bridge.md) shipped the portal backend
with one-shot capture only and deferred its Phase 2: `ScreenCast` needs a
continuous frame stream, which the single-shot sealed-memfd transfer of
[ADR-0041](0041-sealed-file-descriptor-pixel-transport.md) does not answer.
Both records kept the boundary fixed: pixels leave the compositor only over
the scoped, fail-closed IPC — never through a Wayland capture protocol — and
every delivery re-checks capability, lease, named scope, and the lock/VT
gate.

Streaming differs from one-shot capture in three ways. It is damage-driven
and long-lived, so per-request authorization becomes per-frame policy
enforcement. It moves far more bytes, so base64 or PNG-per-frame is not
viable. And it runs concurrently with presentation, so a slow consumer must
never stall the frame loop.

The compositor's frame loop is event-driven: a frame is rendered only when
something changed ([ADR-0038](0038-frame-pacing.md)). A presentation
therefore is the damage signal — no presentation means no new frame, and a
stream consumer simply receives nothing while the desktop is idle.

## Decision

Protocol version 5 adds continuous physical-output streaming inside the
existing scoped model:

1. **`Request::StreamOutputStart { max_fps }` →
   `Response::StreamOutputStarted { stream_id, width, height, format }`.**
   Authorization mirrors `CaptureOutput` exactly: the `control` capability,
   a live lease, and an explicit `StreamOutput` entry in the connection's
   named scope `ops` — never inherited through `None`-means-all. Starting
   while the session is locked or the seat is inactive is refused.
2. **Frames are pushed as `Event::StreamFrame`** carrying the stream id, a
   monotonically increasing sequence number, physical width/height/stride,
   the pixel format (`Bgra8`/`Rgba8`, memory byte order), a conservative
   damage rectangle list, a cumulative dropped-frame count, and the byte
   length — followed immediately by one sealed `memfd` of raw, tightly
   packed pixels transferred with `SCM_RIGHTS`, reusing the ADR-0041 blob
   channel and its 288 MiB bound unchanged. **`Event::StreamEnded
   { stream_id, reason }`** terminates a stream from the server side.
3. **`Request::StreamOutputStop { stream_id }`** stops a stream owned by the
   requesting connection. Disconnecting stops every stream the connection
   owns.

A frame is produced only for a presented frame, throttled per stream to
`max_fps` (default 30, clamped to 1–60). One GPU readback is bound per
presentation and is shared with one-shot capture; a pending `CaptureOutput`
or screenshot wins the arbitration and the stream skips that frame. The
readback's single staging slot therefore becomes a two-variant pending slot
(one-shot or stream), not a per-frame allocation. One detached readback fans
out to every due stream after a single CPU copy on the capture worker.

Backpressure is drop-oldest at bounded lanes, never blocking presentation:
the main-loop → worker lane holds one frame, and each stream's delivery
channel holds two. A full lane drops the frame and increments the stream's
`dropped` counter; the IPC forwarder coalesces to the newest frame when the
socket writer falls behind. The presentation thread only ever `try_send`s.

Live policy is enforced per frame, mirroring ADR-0041's delivery-time
re-checks. The sole IPC writer re-resolves the named scope and checks the
lock/VT gate before attaching each frame's descriptor: a revoked or narrowed
scope ends the stream with `StreamEnded`; a locked session or inactive seat
pauses delivery (frames are dropped, the stream survives). In-flight frames
carry the capture security generation, so a lock-then-quick-unlock boundary
invalidates pixels captured before it. Lease expiry ends the stream from the
connection's forwarder.

First version uses the existing CPU readback path. The pixel format is what
the readback already yields (8-bit RGBA-channel order, premultiplied alpha
resolved during the single CPU copy), and the damage list is one
conservative full-frame rectangle.

## Alternatives

- **PipeWire inside the compositor.** Rejected: it would link a second
  session service and its main loop into the display server, against
  ADR-0051's out-of-process bridge model and the compositor's dependency
  red lines. PipeWire belongs in the portal backend, which consumes this
  stream.
- **Wayland capture protocols (`wlr-screencopy`, ext-image-copy).** Rejected
  again per ADR-0037: bindable globals bypass the named-scope model.
- **A persistent shared ring buffer instead of per-frame memfds.** Deferred:
  a ring would avoid per-frame descriptor traffic but needs cross-process
  synchronization and weakens the immutability guarantee (a sealed memfd per
  frame can never be rewritten after validation). Per-frame sealed blobs
  keep ADR-0041's receiver checks verbatim; the 30–60 fps descriptor rate
  is well within `SCM_RIGHTS` costs.
- **Zero-copy dmabuf export.** Deferred, not precluded: flux can export the
  presented frame's dmabuf with stride and modifier, which a PipeWire
  consumer could import directly. The format/damage metadata is designed to
  grow a dmabuf variant later; CPU readback keeps version 1 backend-agnostic
  (nested and DRM/KMS behave identically) and unblocks the portal now.
- **Fine-grained damage rects.** Deferred: the compositor's damage tracking
  is per-surface commit damage, not a composed output region; computing the
  latter adds bookkeeping to the frame loop. Consumers must treat the
  full-frame rectangle as "every pixel may have changed", which is correct
  if not minimal.

## Consequences

- The IPC speaks protocol version 5; `docs/reference/ipc.md` gains a
  streaming section and its "streaming is future work" note is resolved.
- The built-in `aegis-portal` scope gains `StreamOutput` next to
  `CaptureOutput`, so the portal backend can serve `ScreenCast`; nothing
  else changes for existing scoped clients (internally tagged messages make
  the addition backward-compatible for peers that recompile against v5).
- A stream pins at most three frames of memory per consumer (one in the
  worker lane, two in the delivery channel), bounding the cost of a stalled
  or malicious consumer; a consumer that never reads is disconnected by the
  socket's backpressure rather than exhausting compositor memory.
- Screenshots and `CaptureOutput` keep priority over streaming; a screencast
  may drop a frame when a screenshot is taken, never the reverse.
- Follow-up work this decision creates: dmabuf zero-copy export, fine
  output-damage rectangles, per-output selection when the compositor drives
  several outputs, and portal-side persist modes (restore tokens) — the
  last landed with ScreenCast v2 in
  [ADR-0053](0053-portal-session-services-and-grants.md); the rest are
  documented in the IPC reference and the portal how-to.
