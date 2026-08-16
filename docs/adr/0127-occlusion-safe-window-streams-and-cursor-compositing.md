# ADR-0127: Occlusion-safe window streams and stream cursor compositing

- Status: Accepted
- Date: 2026-08-16

## Context

[ADR-0054](0054-interactive-target-picking.md) made a window stream a crop
of the shared desktop readback: the window's rectangle was cut out of
whatever the composited frame contained at presentation time. That is
fundamentally occlusion-unsafe — a window covered by another window, moved
to a different workspace, or minimized streamed the pixels of whatever lay
above it, which is both a correctness failure (the portal records the wrong
content) and an information-disclosure hazard (the consumer sees windows it
never consented to). Minimized windows effectively stopped streaming
altogether, exactly when a recording or a task-switcher thumbnail cares
most. Protocol 29 ([ADR-0126](0126-damage-driven-stream-pacing-and-geometry-renegotiation.md))
then negotiated three things the runtime had not implemented yet: window
targets opting into the dmabuf transport (accepted but silently started as
SHM), the `embedded` cursor mode (accepted but producing `hidden` frames),
and per-frame damage (conservatively reported as one full-frame rectangle).

The v26 one-shot `CaptureWindow`
([ADR-0117](0117-per-window-content-capture.md)) already solved the
rendering half of the problem: it draws a toplevel's complete surface tree
— subsurfaces and popups included — into a fresh offscreen target,
safe against occlusion, minimization, and foreign workspaces, at the
capture scale of the output the window sits on.

## Decision

Window streams are independently rendered, cursor-composited, and
damage-tracked; protocol version stays 29 because every wire shape was
already negotiated there.

1. **Each window stream renders its own offscreen target, cached per
   stream.** The one-shot window-capture mechanism is reused with one
   change: the offscreen surface is created once at stream start (and
   recreated on a client geometry restart) instead of per frame. A window
   stream produces a frame when its surface tree committed since the last
   capture *and* its `max_fps` interval elapsed — rendered offscreen
   immediately, independent of presentation — or when the one-second
   liveness tick forces a re-render of a clean tree, which keeps consumers
   fed and minimized windows' thumbnails honest. The dirty signal is a
   diff of the tree's per-surface content generations, the same machinery
   the output damage tracker diffs globally; the frame lists already
   enumerate exactly the window's tree. The one-shot `CaptureWindow` lane
   keeps its capture-worker priority; window-stream conversions are
   unreserved jobs on the same bounded worker, and a ready frame held
   behind one-shots logs instead of starving silently. A closed window
   ends the stream (`StreamEnded("window closed")`); a size change — the
   window's own, or a scale change of the output under it altering the
   physical extent — freezes it with `StreamGeometryChanged`; pure
   position moves are followed silently because the render maps the
   window's origin to (0, 0) every frame.
2. **Window targets honor the dmabuf transport.** The capture-surface
   machinery (3-slot ring, explicit sync_file fences, fence polling,
   consumer-ownership bookkeeping) is shared with the post-present blit:
   the ring/submit/export tail is one code path, driven by two capture
   sources — the presented-frame blit for output streams and the
   per-window tree render for window streams. Where an exportable capture
   surface is unavailable (nested backend) the start falls back to SHM
   with the existing warning.
3. **`embedded` composites the theme cursor into the stream.** dmabuf
   streams (output and window) draw the theme sprite into the capture
   target after the blit/render, using the software cursor's texture and
   hotspot machinery; SHM output streams read back the presented frame, so
   the capture worker blends the cursor bitmap into a second copy of the
   frame — per-stream modes make a shared pre-blend incorrect, and one
   extra frame copy on the worker keeps the blend off the frame thread.
   `embedded` for a window stream means "cursor when over this window";
   for a connector stream, "cursor inside the connector's rectangle"; the
   test is the cursor position (hotspot) inside the captured rect, with
   sprites clipped by the target. Only the compositor's theme cursor is
   embedded: a client-provided cursor surface is scene content — already
   present in output frames, never part of a window's surface tree. On the
   software-cursor fallback (nested or degraded direct display) the
   presented frame already contains the cursor, so `embedded` adds nothing
   and `hidden` cannot subtract it there; this is documented in the schema
   and the IPC reference. `hidden` remains the default and changes
   nothing. Cursor motion alone does not dirty a stream; an embedded
   cursor updates at the stream's damage-driven cadence, bounded by the
   one-second liveness tick — the same tradeoff the hardware cursor plane
   already makes.
4. **Stream frames carry real, translated damage when it is known.** Every
   presented frame's damage folds into a per-stream accumulator (direct
   scanout included — its damage is assessed even though streams cannot
   capture it). A capture samples the accumulator: whole-desktop as
   computed, connector and window targets translated by the crop origin
   and clipped to the target extent. The accumulator clears only on
   *delivery*, so a frame lost to backpressure or a security boundary
   folds its regions back and the next delivered frame still covers them;
   a moved crop origin invalidates the coordinate space and reports full
   damage. The wire contract stays conservative: a forced liveness frame,
   an empty intersection, or any unmappable state reports one full-frame
   rectangle, and an empty damage list is never sent.

## Alternatives

- **Keep cropping the desktop frame** is occlusion-unsafe and cannot be
  fixed within the model: the presented frame simply does not contain the
  window's pixels when something covers it. Independent rendering is the
  only correct shape, and reusing the v26 tree-render keeps it cheap.
- **One shared readback lane for window streams too** (the single global
  `pending_window_capture` slot extended to streams) would serialize every
  window stream through one surface and one lane, forcing round-robin
  scheduling and head-of-line blocking between streams. Per-stream cached
  targets with per-stream in-flight state let streams progress
  independently; the shared capture *worker* stays the single bounded
  conversion lane, where one-shots keep priority and holds are logged.
- **Pre-blending the cursor into the shared readback** would contaminate
  `hidden`-mode streams served by the same frame. Producing one
  cursor-composited twin on the worker next to the pristine frame serves
  both modes from one readback at the cost of a single extra copy, only
  when an embedded stream exists.
- **Empty damage lists for unchanged frames** would be the most precise
  contract, but consumers negotiated protocol 29 against "conservative
  full-frame damage" and an empty list is a new wire shape; over-reporting
  is always safe, so the fallback stays a full rectangle.
- **Per-window surface damage** (each changed surface's committed damage
  rects, translated) would be tighter than desktop-damage-clipped-to-the-
  window, but the server acknowledges committed damage at every present,
  so off-presentation window renders could not accumulate correctly
  without new cross-present bookkeeping per window. The desktop-space
  accumulator clipped to the window's rect reuses one mechanism for every
  target kind at the price of conservative over-reporting when an
  *occluder's* pixels change above the window.

## Consequences

- A window stream shows the window's real content wherever the window
  lives — occluded, minimized, or on another workspace — and can never
  leak a neighboring window's pixels into a consented capture.
- Window streams no longer wait for presentation: a committing window
  paces its stream at `max_fps` even on an otherwise static desktop, while
  an idle window costs one cheap re-render per second.
- The portal can open per-window zero-copy streams (dmabuf) and per-window
  or per-monitor streams with an embedded cursor, and can rely on damage
  rectangles for partial updates where they are reported.
- The capture worker's bounded lane now carries three unreserved stream
  job kinds (shared output readback, per-window readbacks) next to
  reserved one-shots; a one-shot delayed behind tiny window conversions is
  bounded by FIFO, and a window frame held behind one-shots logs after two
  seconds instead of starving silently.
- Follow-up work this decision creates: per-window surface-precise damage
  (needs cross-present damage bookkeeping per window), and an explicit
  wire note if empty damage lists ever become desirable.
