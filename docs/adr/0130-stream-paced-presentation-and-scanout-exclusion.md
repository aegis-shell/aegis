# ADR-0130: Stream-paced presentation and scanout exclusion for output streams

- Status: Accepted
- Date: 2026-08-17

## Context

[ADR-0126](0126-damage-driven-stream-pacing-and-geometry-renegotiation.md)
made output streams capture opportunistically: a stream rides
damage-driven composites up to its `max_fps`, and on a static screen only
a one-second liveness tick forces a presentation. That traded consumer
frame rate for idle power, on the theory that PipeWire consumers accept
sparse frame streams.

In practice the trade fails the workloads streaming exists for. A Flatpak
OBS recording of a quiet desktop — terminal, browser, occasional window
switches — delivered about 2.5 distinct frames per second (the 30 fps
container was OBS duplicating the last received frame), so every
interaction plays back as a slideshow. Two structural holes compound the
sparseness. Direct scanout bypasses the compositor entirely, so a
fullscreen video or game being recorded produces **no** capturable frames
at all — only the liveness tick — exactly when the content changes at
full refresh rate. And cursor motion presented through the hardware
cursor plane never composites either, so the cursor in a recording jumps
once per liveness tick instead of moving smoothly. Sparse delivery is
only acceptable to a consumer when the picture is truly static; these
cases are not static, they are merely composite-free.

## Decision

A live output stream paces presentation at its negotiated cadence again,
superseding Decision 1 of
[ADR-0126](0126-damage-driven-stream-pacing-and-geometry-renegotiation.md)
(its output addressing and geometry renegotiation, Decisions 2 and 3,
stand).

1. **A due output stream forces a presentation.** The forcing condition
   is the stream's own `frame_interval` — its first frame, or one full
   interval without a captured frame — replacing the one-second
   `LIVENESS_INTERVAL` test for output streams. The main loop's idle
   wait wakes at the next frame deadline (`next_stream_wake_in`), so a
   stream on a static screen still receives frames at its `max_fps`. The
   opportunistic binding is unchanged and still serves due streams from
   damage-driven composites between forced frames. `LIVENESS_INTERVAL`
   remains only for window streams
   ([ADR-0127](0127-occlusion-safe-window-streams-and-cursor-compositing.md)),
   which render offscreen and never force presentation.
2. **Direct scanout is disqualified while any output stream lives.** The
   primary-plane plan's capture disqualifier now includes a live-stream
   fact, so a fullscreen dmabuf client composites — and is therefore
   captured — for the duration of a stream instead of page-flipping past
   the compositor. Scanout returns automatically when the last output
   stream stops.

## Alternatives

- **Keep ADR-0126 pacing and only close the scanout hole** would capture
  moving content correctly while it damages, but cursor-plane-only motion
  would still reach consumers at one frame per second, and any animation
  below the damage threshold of a composite — a blinking caret, a subtle
  spinner — would stutter the same way. A consumer that negotiated 60 fps
  has no use for a one-second floor.
- **Re-deliver the last captured frame on static screens** would feed the
  cadence without re-compositing, but duplicate pixels carry no
  information: PipeWire consumers (OBS included) already repeat the last
  buffer locally. Extra copies would only waste readback bandwidth and
  per-stream memory.
- **Alternate scanout and composite at the stream's cadence** would keep
  some zero-GPU frames, but ping-ponging the primary plane forces a full
  redraw on every composite, churns plane assignments, and spams scanout
  telemetry; a steady composite is simpler and correct.
- **Leave scanout active and skip capture during it** is the ADR-0126
  status quo: fullscreen recordings are permanently stuck at the liveness
  rate. Rejected.

## Consequences

- A recording or cast consumer observes frames at the negotiated
  `max_fps` whenever they are due, including on a static desktop and
  including cursor-only motion; the observed rate no longer depends on
  how the frame happened to be produced.
- The ADR-0126 power concern returns in scoped form: a static screen with
  a 60 fps stream attached pays up to 60 forced composites per second.
  The cost is bounded by the stream's lifetime — an active recording is a
  deliberate, user-initiated state — and ends when the consumer
  disconnects. If idle-recording power becomes measurable, the refinement
  is a cheaper forced present (damage-free re-submit), not sparser
  delivery.
- Direct scanout is unavailable during capture, so recording a fullscreen
  game or video costs the composite path's GPU time. This is the same
  trade other compositors make while a capture consumer is attached.
- The portal backend needs no change: its negotiated-`max_fps` contract
  is honored by the compositor again.
- `docs/reference/ipc.md` describes the pacing contract and was updated
  with this decision.
