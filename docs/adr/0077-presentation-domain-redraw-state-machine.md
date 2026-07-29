# ADR-0077: Presentation-domain redraw state machine

- Status: Accepted
- Date: 2026-07-29

## Context

[ADR-0038](0038-frame-pacing.md) made presentation the compositor's
metronome, but the runtime still represented the redraw lifecycle through
separate animation, damage, and pending-page-flip conditions. The direct DRM
backend also waited for an in-flight page flip inside ordinary dispatch and
presentation calls. That protected scanout ownership, but it coupled input
latency to vblank and left the legal transitions implicit.

A client frame callback could prevent the no-damage fast path and force an
otherwise empty composite and atomic commit. Visual timers could also expire
while KMS still owned a batch, turning a wait that could not make rendering
progress into non-blocking polling.

Niri demonstrates a stronger design kernel: each presentation target has an
explicit redraw lifecycle, real and estimated vblank are distinct, and new
work coalesces while a submitted frame is in flight. Aegis cannot copy its
per-output state directly. The current Aegis renderer produces one desktop
framebuffer and submits one atomic batch spanning every active CRTC, so the
ownership unit is the whole batch rather than one connector.

## Decision

Aegis owns one explicit redraw state machine per **presentation domain**. A
presentation domain is the smallest set of outputs submitted and retired as
one ownership unit. The current direct backend has one host-wide domain
because one atomic commit spans every active CRTC.

The lifecycle distinguishes idle, queued, synchronous rendering, waiting for
a real vblank, waiting for an estimated vblank, and unavailable presentation.
Starting a render consumes the queued edge. Submission, no-damage completion,
and retry are legal only as outcomes of that render transaction.

Backend event dispatch remains live while a DRM batch is in flight. Input,
Wayland commits, session events, and hotplug continue to run and accumulate
one redraw request, but a second render or atomic commit cannot begin until
every owned CRTC reports its page flip. An unrelated page-flip event cannot
retire the batch. A one-second watchdog reports a stalled batch without
violating its ownership boundary.

Input snapshots that arrive while KMS owns a frame are coalesced. Current
level state replaces older level state; press, release, key, text, scroll, and
IME deletion edges accumulate until the next render. VT loss discards
presentation-epoch edges and capture requests rather than replaying them
after device authority returns. Resume and output recreation force a full
redraw.

A successful submission completes pending `wl_surface.frame` callbacks and
then waits for the backend boundary before accepting another frame. A
no-damage assessment submits nothing. It completes callbacks at most once per
estimated refresh cycle, while real newly damaged content may render before
that estimate expires. Aegis does not advertise `wp_presentation` until a
feedback request can be associated with the corresponding surface commit and
completed from real page-flip metadata.

The estimated interval follows the presentation domain. With a multi-CRTC
atomic batch, the slowest active CRTC bounds retirement. Nested mode uses a
60 Hz estimate because the outer compositor owns the physical clock. Its
estimated deadline is anchored at render start, so time already spent waiting
inside an outer FIFO presentation counts toward that cycle instead of charging
a second refresh interval. Wallpaper media keeps its own absolute source
deadline, and a continuously animated 3D wallpaper retains an independent
60 Hz budget. Visual deadlines never shorten a real-vblank or
unavailable-presentation wait.

This decision supersedes [ADR-0038](0038-frame-pacing.md).

## Alternatives

- **Keep distributed booleans and backend-local waits.** Rejected: ownership
  remains correct, but event latency and legal transitions stay coupled to
  incidental call order.
- **Copy Niri's per-output state machine.** Rejected for the current renderer:
  independently queueing outputs that share one framebuffer and atomic batch
  would claim an ownership boundary the backend does not provide.
- **Render once for every frame callback.** Rejected: a callback is a pacing
  request, not output damage, and must not require an empty GPU submission.
- **Release a batch when the watchdog expires.** Rejected: elapsed wall time
  does not prove that KMS stopped reading a scanout buffer.

## Consequences

- Input and Wayland traffic stay responsive during an in-flight page flip.
- Redraws, callback-only commits, and visual timers cannot create a busy
  submit loop at vblank.
- Multi-output page-flip ownership and VT/hotplug cancellation have explicit,
  testable transitions.
- Mixed-refresh outputs remain paced as one atomic domain. Independent
  per-output pacing requires a later renderer and backend change that splits
  the presentation domains; it cannot be introduced only in the scheduler.
- Real `wp_presentation` feedback remains follow-up work and must use
  page-flip sequence and timestamp data, not an estimated callback clock.
- The runtime has more states, but illegal render outcomes fail at their
  transition boundary instead of producing a later buffer-ownership error.
