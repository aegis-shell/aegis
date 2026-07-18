# ADR-0039: Damage-driven shm snapshot and texture refresh (amends ADR-0015)

- Status: Accepted
- Date: 2026-07-18

## Context

[ADR-0015](0015-damage-tracking.md) introduced per-commit damage
tracking, but the pipeline around it still paid full-frame costs at
both ends. On commit, the server copied the entire shm buffer into the
compositor snapshot and bumped the surface generation; the renderer
treated a generation change as "re-upload the whole texture", so a
client repainting one line of text still cost a full
`width × height` memcpy, a fresh snapshot allocation, and a full
texture upload per commit. The incremental `update_region` path
existed but only ran when a commit carried damage *without* a new
buffer attach, which real (double-buffered) clients never produce.

flux uploads were synchronous at the time (optics ADR-0021), so every
full upload also stalled the render thread on a fence; per-rect
incremental uploads multiplied that stall. optics ADR-0022 removed the
stall; this ADR removes the redundant bytes.

## Decision

**The snapshot copy is damage-driven.** When the new buffer matches
the snapshot's size, the transform is `Normal`, the buffer scale is 1,
and the commit carries damage, the server copies only the damaged rows
from the client buffer onto the retained snapshot — correct because
the damage protocol guarantees the buffer differs from the previous
frame only inside the damaged region. The snapshot `Vec` is reused
while its size holds, ending the per-commit allocation. Anything else
(size change, transform, scale, empty damage = "no information")
forces a full copy. A dmabuf attach clears the snapshot so a later shm
commit can never blend into stale pixels.

**The texture refresh mirrors the same rule.** The renderer uploads
only when the surface committed new contents (generation changed) or
the cached texture's dimensions no longer match; damage alone never
triggers an upload, because damage rects persist until the next
commit and would otherwise re-upload a static surface every frame.
Within an upload, a same-size commit with usable damage refreshes the
union of the clamped damage rects in a single `update_region`;
everything else replaces the texture. The two guards are deliberately
identical on both sides — if the server copied incrementally, the
renderer must refresh incrementally, and vice versa, or the texture
tears.

**A failed partial refresh discards the cache entry.** The snapshot
always holds the full current frame; a failed `update_region` retires
the texture so the next frame re-uploads the whole snapshot instead
of drifting indefinitely.

## Alternatives

- **Keep full copy + full upload.** Rejected: it costs
  `O(buffer_area)` per commit at both ends for clients that repaint a
  few pixels, which is the common interactive case (terminals,
  editors).
- **Per-rect `update_region` calls.** Rejected: each rect costs a
  separate submission; the bounding box is one upload and is bounded
  above by the full surface, so pathological damage degenerates to the
  old behavior, not worse.
- **Key the refresh on damage alone, ignoring generation.** Rejected:
  damage persists until the next commit, so a static surface would
  re-upload every frame; generation is the only "new contents"
  signal.
- **Damage-only snapshot copy with no full-copy fallback.** Rejected:
  empty damage carries no information, and transform/scale break the
  1:1 surface-to-buffer mapping ADR-0015 relies on; the fallback keeps
  both cases correct.

## Consequences

- An shm commit now costs `O(damage_area)` at both the snapshot copy
  and the texture upload; terminals and editors stop paying
  full-frame bandwidth per keystroke.
- The per-commit whole-surface snapshot allocation is gone for
  stable-size surfaces.
- The generation counter changes meaning: from "the texture is stale"
  to "the surface committed new contents"; dimension matching owns
  staleness. Code touching the renderer cache must use the two-layer
  gate.
- Transformed and `buffer_scale > 1` surfaces keep the full path, as
  in ADR-0015; empty-damage commits keep the full path, matching the
  pre-existing conservative behavior.
- Update failure handling changes from "keep the stale texture until
  the next generation change" to "retire and re-upload next frame",
  closing an unbounded tear window.
