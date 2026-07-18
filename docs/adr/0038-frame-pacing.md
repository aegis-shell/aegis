# ADR-0038: Frame pacing — event-driven loop with presentation throttling

- Status: Accepted
- Date: 2026-07-18

## Context

The main loop renders on demand: it blocks on the host event queue and
draws one frame per wakeup. Animations (chrome springs, window
transitions, the 3D wallpaper — see
[ADR-0029](0029-animation-and-effect-policy.md)) need a continuous
cadence, and the loop originally produced it by sleeping to a ~60 fps
budget (`thread::sleep` for the remainder of a 16.667 ms interval) and
then dispatching host events non-blocking. Three problems followed.
The sleep is not aligned to the presentation engine, so the frame
cadence drifts against vblank and periodically lands a frame just past
the deadline; input arriving during the sleep waits out the remainder;
and because the default wallpaper carries a 3D model, the loop is in
the animating branch essentially always.

Two smaller defects sat in the same path. Any `begin_frame` error —
including a fence/acquire timeout, which is transient — rebuilt the
swapchain. And on the nested backend, releasing retired client buffers
called `vkDeviceWaitIdle` on the compositor thread, stalling the whole
device whenever a window closed or resized.

## Decision

**Presentation is the only metronome.** The loop never paces itself
with a blind sleep. While animating it waits on the host event queue
with a deadline (~60 fps budget); input arriving mid-budget wakes the
loop and is processed immediately. The cap is only an upper bound —
the real cadence comes from the backend's presentation throttle: FIFO
swapchain acquire nested, the KMS page-flip wait on DRM. While idle
the loop blocks on the event queue with a one-second timeout, as
before.

**Frame-begin errors are classified.** A timeout from
`surface.begin_frame` (the previous frame's GPU work did not retire,
or the presentation engine released no image) is transient: the loop
skips the iteration and retries at the next wakeup. Only structural
errors (out-of-date/lost) rebuild the swapchain.

**Buffer release never waits on the device.** On backends with an
exportable completion fence (DRM), retired buffers release against
that fence. On nested, where no such fence exists, release is deferred
by more presented frames than flux's in-flight slots (3) instead of
stalling on `vkDeviceWaitIdle`.

## Alternatives

- **Keep sleep-based pacing.** Rejected: the sleep adds phase drift
  on top of the backend's own throttle and queues input behind a
  timer.
- **Pace on the host's `wl_surface.frame` callback (nested).**
  Rejected as redundant: the FIFO swapchain already throttles to the
  host's refresh; a second callback clock can only disagree with it.
- **Render at the native refresh rate with no cap.** Rejected for
  now: the 3D wallpaper would cost twice the GPU on a 120 Hz panel.
  The deadline mechanism does not preclude lifting the cap later.

## Consequences

- Animation cadence is vblank-locked on both backends; input latency
  during animation drops from up-to-one-frame to immediate dispatch.
- A transient GPU hiccup no longer tears down and recreates the
  swapchain.
- Closing or resizing a window on the nested backend no longer stalls
  the compositor thread on a device-wide wait; release trails by at
  most four frames.
- The 60 fps cap remains a deliberate product choice for animated
  content; the deadline mechanism makes it cheap to revisit.
