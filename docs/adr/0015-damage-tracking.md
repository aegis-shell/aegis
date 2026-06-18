# ADR-0015: Per-commit damage tracking

- Status: Accepted
- Date: 2026-06-18

## Context

Before this ADR, every commit re-uploaded the entire surface texture. For
clients that update small regions per frame (terminal cursor blink, editor
keystrokes, partial video frame updates), the cost was `O(width * height)`
per commit even when only a few pixels changed. flux already exposes
`flux_image_update_region(image, x, y, w, h, data, bytes)` for partial
uploads — ass just was not using it.

## Decision

### 1. Damage tracking on `SurfaceRec`

Two new fields:

- `pending_damage: Vec<Rect>` accumulates rects from `wl_surface.damage`
  and `damage_buffer` between commits.
- `committed_damage: Vec<Rect>` is rotated in at commit time and read by
  the renderer this frame, then cleared at the next commit.

Damage handlers accept both `damage` (v1, surface coords) and `damage_buffer`
(v4, buffer coords). With buffer_scale=1 (the only scale M2 currently
applies at composite) they are equivalent, so both accumulate into the same
`Vec`. When buffer_scale becomes load-bearing, damage_buffer will need
pre-multiplication.

### 2. Surface damage through `SurfacePixels`

`ass_core::SurfacePixels` gains `damage: &[Rect]`. The server lends the
rec's `committed_damage` for the frame. Empty slice means "client reported
no damage; fall back to the generation-based cache check."

### 3. Renderer uses `update_region` for incremental updates

The toplevel draw path now has three branches:

1. **Cache miss or generation changed**: full upload via `from_bytes`
   (existing path, including CPU-side transform staging).
2. **Cache hit, non-empty damage, transform is Normal**: incremental
   upload via `flux::Image::update_region` per rect. Each rect is clamped
   to surface bounds; the sub-rect's bytes are extracted from the source
   buffer and uploaded.
3. **Cache hit, no damage, or transform non-Normal**: skip the upload
   this frame (the cached texture holds last frame's contents).

After a successful incremental pass, the cache's stored generation is
bumped to the current one so the next frame's stale check does not
re-upload the whole texture.

### 4. Damage is skipped under transform

When `geometry.transform != Normal`, the incremental path is bypassed.
Damage rects are in surface-local pixel coords; mapping them to the
transformed staging buffer's coordinate space is possible but fiddly
(axis-swap for 90°/270°, row/column reversal for the flips) and the
CPU-transform path already pays a full copy on commit. The full-upload
path runs on the next generation change anyway. Adding transformed-damage
support is a future optimization.

## Alternatives

- **Always full-upload.** Rejected: terminal and editor clients blink
   their cursors by damaging a 16×32 rect and committing. Re-uploading
   a 1920×1080 texture for that is wasteful at 60 Hz.
- **Damage in surface coords only; ignore damage_buffer.** Rejected: v4
   damage_buffer is what most modern clients use; the spec recommends
   it over `damage` because it is unaffected by future scale handling.
- **Damage under transform.** Deferred: it is a correctness-preserving
   optimization (the full upload still produces correct output), and
   the math interacts with the CPU transform staging in non-obvious
   ways. Land the simple path first.
- **Coalesce overlapping damage rects.** Deferred: clients usually send
   non-overlapping damage. The renderer's per-rect `update_region` call
   is cheap relative to the upload itself.

## Consequences

- Terminal and editor clients no longer pay full-texture upload cost on
  every keystroke or cursor blink. The path is `O(damage_area)` per
  commit instead of `O(buffer_area)`.
- Clients that do not send damage see no behavior change — the cache
  hit + empty-damage branch skips the upload entirely (the cached
  texture holds last frame's contents, which match the client's
  committed state).
- HiDPI scaling and damage interaction is documented as not handled:
  when buffer_scale becomes load-bearing, damage_buffer's rects will
  need division by scale to land in surface coords.
- Transformed surfaces still pay full uploads; the optimization is
  skipped for them.
- `flux::Image::update_region` is a new public method on the Rust
  binding, mirroring the existing C entry point. No flux pipeline
  or shader changes.
