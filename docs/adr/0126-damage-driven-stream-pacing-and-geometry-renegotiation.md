# ADR-0126: Damage-driven stream pacing and stream renegotiation

- Status: Accepted (Decision 1 superseded by [ADR-0130](0130-stream-paced-presentation-and-scanout-exclusion.md))
- Date: 2026-08-16

## Context

[ADR-0052](0052-scoped-output-frame-streaming.md) made a live stream a
presentation driver: its next due frame capped the main loop's idle wait,
and reaching that deadline forced a composite even when nothing on screen
had changed. That keeps a consumer fed at the negotiated `max_fps` cadence
on a static desktop, but it means a 60 fps portal stream renders 60
composites per second of an identical picture for the lifetime of a
recording — pure GPU, power, and thermal waste on the exact workload
(long screen recordings) the protocol exists to serve. The compositor's
pacing philosophy is otherwise damage-driven
([ADR-0038](0038-frame-pacing.md),
[ADR-0039](0039-damage-driven-shm-refresh.md)).

Three adjacent gaps had accumulated around the same machinery. Streams
were addressed only implicitly (the whole desktop, or one window), while
the portal needs per-monitor streams on multi-output desktops. A stream
whose target geometry changed — hotplug, a config mode change, or a
window resize — either kept delivering mismatched frames (the hotplug
`take_resize` path never notified streams at all) or was ended wholesale
with `StreamEnded` (the `begin_frame` rebuild path), although PipeWire
consumers can renegotiate cleanly when told. And the `max_fps` ceiling of
60 was becoming the limiter on high-refresh hardware.

## Decision

Protocol version 29 makes streams damage-driven, output-addressed, and
renegotiable.

1. **Streams capture opportunistically, never force at their cadence.**
   Whenever a real damage-driven composite presents, every stream whose
   `max_fps` interval elapsed captures from that frame — the shared
   readback binding and the post-present dmabuf blit are unchanged, they
   simply no longer cause the frame. A stream may force a presentation
   only at a one-second **liveness tick** (`LIVENESS_INTERVAL`) without a
   captured frame, and immediately after `StreamOutputStart` so the first
   frame never waits for damage. The main loop's idle wait tracks the
   next liveness deadline instead of the next due frame. On a static
   desktop a consumer now observes ~1 fps for roughly 1/60th of the
   former rendering cost; consumers such as OBS and PipeWire accept
   sparse frame streams by design. `max_fps` remains a pure upper
   throttle, and its clamp rises from 60 to 240 (default stays 30) so
   high-refresh captures are not protocol-limited. This paragraph
   supersedes the forced-cadence pacing of
   [ADR-0052](0052-scoped-output-frame-streaming.md); the transport,
   authorization, and backpressure decisions there stand.
2. **`EnumerateOutputs` → `Outputs` (query-gated, like `GetSettings`)**
   reports each connector's `primary` flag and physical-pixel rectangle
   in desktop coordinates, sorted by connector name. `StreamTarget::
   Output` gains an optional connector selector whose serde shape is
   backward compatible (`{"type":"Output"}` still means the whole
   desktop). A connector-addressed stream crops the SHM readback, or
   blits the output's sub-region of the presented dma-buf, and announces
   the output's size as its geometry. Window targets may now opt into the
   dmabuf transport at the protocol layer; the runtime still starts them
   as SHM with a warning until the window-dmabuf follow-up. `StreamOutput
   Start` also accepts an optional `cursor` mode (`hidden`/`embedded`),
   validated and stored on the stream state; compositing the cursor is a
   follow-up, so `embedded` currently matches `hidden`.
3. **`Event::StreamGeometryChanged { stream_id, width, height }` freezes,
   never silently mismatches.** Every surface-size change — the hotplug
   and config-mode `take_resize` path and the `begin_frame` rebuild —
   reconciles each live stream: a desktop or connector stream whose
   negotiated size changed is frozen and notified; a connector that
   disappeared ends with `StreamEnded`. Pure position moves are followed
   silently. Window streams keep their lazy delivery-time detection but
   now freeze on a size change instead of ending; a closed window still
   ends its stream. A frozen stream stays registered, produces no frames,
   and never forces presentation until the client restarts it with
   `StreamOutputStop` + `StreamOutputStart`, which re-registers at the
   new geometry (including a fresh dmabuf slot table). The event is sent
   only to connections that negotiated version 29; an older client simply
   observes frames stop.

## Alternatives

- **Pure damage-driven pacing with no liveness tick** would freeze a
  recording's picture for the entire duration of a static desktop. A
  recording consumer cannot distinguish that from a compositor stall, so
  a minimal liveness guarantee is kept — at one frame per second rather
  than at the negotiated cadence.
- **Keeping the forced cadence** wastes a full composite per stream tick
  on static content and, worse, disqualifies direct scanout whenever a
  stream is attached, so a fullscreen video would never reach the
  zero-GPU-cost path while being recorded.
- **`StreamEnded` on every geometry change** matches the old `begin_frame`
  path but forces consumers to tear down and re-consent their PipeWire
  sessions for a routine monitor mode change; an explicit freeze event
  carries the new geometry and keeps the stream's identity and
  authorization intact across a restart.
- **Per-output framebuffers** would make connector streams trivial but is
  a much larger architectural change; cropping the shared desktop frame
  answers the portal's need at a fraction of the intrusion.

## Consequences

- A live stream on a static desktop costs ~1 forced composite per second
  instead of up to 240; on a changing desktop every due stream still
  captures every presented frame up to its `max_fps`.
- Direct-scanout frames no longer feed streams (there is no composite to
  capture from); the liveness tick bounds the resulting sparseness.
- The portal backend can enumerate monitors, open per-monitor streams,
  negotiate cursor modes, and renegotiate geometry without reconnecting.
- Follow-up work this decision creates: window-target dmabuf streams,
  `embedded` cursor compositing, and damage translation into cropped
  stream coordinates (frames currently report full-crop damage).
