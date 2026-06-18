# ADR-0014: Buffer transform (CPU staging) and viewport crop

- Status: Accepted (buffer-scale portion superseded by [ADR-0020](0020-buffer-scale-applied-at-composite.md))
- Date: 2026-06-18

## Context

M2 left two compositor correctness gaps:

1. **Buffer transforms.** `wl_surface.set_buffer_transform` was accepted but
   ignored. Clients with pre-rotated buffers (tablets reporting the panel in
   portrait while the user holds the device in landscape, video clients
   pre-rotating for scanout) rendered with the buffer in its raw orientation.
2. **Viewport crop/scale.** `wp_viewport.set_source` / `set_destination`
   were also no-ops. Clients (mpv, browsers) that use viewport to crop video
   into a sub-rectangle of the buffer got the full buffer composited at
   buffer-native size.

Both need flux canvas API work the audit
([exploration done in this session](#)) flagged as blocked: transforms need
per-vertex UVs or a matrix push-constant in the image shader, and viewport
crop needs a thin `flux_canvas_draw_image_sub` entry point that exposes an
already-shader-ready code path.

## Decision

### 1. Viewport crop: small flux addition, full ass wiring

Add `flux_canvas_draw_image_sub(canvas, image, dst, src)` to flux — a ~5-line
C wrapper around the existing `draw_image_with_sampler_handle(c, img, sh, dst,
src, tint, kind=3u)` path that the plain `flux_canvas_draw_image` already
uses with `src = FLUX_SRC_WHOLE`. No shader changes; the fragment shader
already implements the full UV remap.

Add the Rust binding `flux::Canvas::draw_image_sub` mirroring the existing
`draw_image` signature with four extra `src_u, src_v, src_du, src_dv`
parameters in normalised texture coordinates.

In ass-server, `wp_viewport.set_source` / `set_destination` now store their
values on `SurfaceRec` as `viewport_src: Option<Rect>` (pixel coords) and
`viewport_dst: Option<Size>` (logical pixels). These thread through
`SurfaceGeometry` to the renderer.

The renderer computes the destination size and the source UV rect:

- Neither set: `dst = (buffer_w, buffer_h)`, full-image draw.
- Source only: `dst = source.size`, source UV rect from source/buffer ratio.
- Destination only: `dst = viewport_dst`, full-image draw.
- Both set: `dst = viewport_dst`, source UV rect from source/buffer ratio.

When a source rect is present, the renderer calls `draw_image_sub`; otherwise
it falls through to `draw_image`.

### 2. Buffer transforms: CPU staging in ass-render

The shader-side transform path in flux is more invasive (per-vertex UVs or
matrix push-constant), so M2 takes the CPU fallback: at upload time, if the
surface's `geometry.transform` is not `Normal`, the renderer transforms the
pixel buffer in software before uploading it as the cached texture. The
buffer dimensions reported to the renderer are also swapped for the 90° /
270° cases.

`transform_pixels(src, w, h, transform) -> Cow<[u8]>` implements all eight
transforms defined by `wl_surface` (`Normal`, `Rotate90/180/270`, and the
four `Flip*` variants). `Normal` returns the input as a borrowed `Cow` so
the common case pays nothing. Axis-swapping rotations return an owned
staging buffer.

Six unit tests cover the math: Normal is borrowed; Rotate90 of 2×2 and 3×1
(non-square) grids swap axes; Rotate180 reverses both axes; FlipHorizontal
mirrors within rows.

The server side replaces the noop `set_buffer_transform` handler with a real
one that stores the value as `pending_transform: Transform` on `SurfaceRec`,
applied at commit and surfaced to the renderer via
`SurfaceGeometry.transform`.

### 3. Buffer scale: stored, not applied

`set_buffer_scale` is now a real handler that stores the value as
`pending_scale: i32`, but the renderer does not yet divide destination
dimensions by the scale. HiDPI clients that commit at scale 2 will render
at 2× their intended size until either a GPU-side transform path lands in
flux or the renderer grows explicit scale handling.

## Alternatives

- **GPU-side transforms in flux.** Deferred: per-vertex UVs (the `_pad`
  channel that the glyph-run pipeline already uses) or a matrix
  push-constant in the image fragment shader. Both are localised changes
  to `canvas.c` + `canvas_image.frag`, but they are flux changes that
  deserve their own focused testing and review. The CPU fallback is
  correct and fast enough for static-image and most video clients in the
  meantime.
- **No viewport crop until the GPU path lands.** Rejected: viewport is a
  one-line flux entry point that exposes an already-working shader path.
  The 5-line addition is well within the project's co-development model
  (per [ADR-0005](0005-flux-core-binding-crate-in-flux-repo.md)).
- **Damage tracking as part of buffer transform.** Rejected: the two are
  orthogonal. Damage (Phase 4.5) avoids re-uploading unchanged regions
  and is independent of how a region is transformed. Tracking damage
  while also CPU-transforming would complicate both; keep them separate.

## Consequences

- Clients that pre-rotate their buffers and clients that crop via viewport
  now composite correctly. mpv-style video clients and tablet-panel
  rotations both work.
- One small flux entry point (`flux_canvas_draw_image_sub`) was added. It
  is a thin wrapper and changes no pipeline or shader code.
- CPU-side transforms cost one pixel copy per commit on transformed
  surfaces. The common case (Normal) pays zero. The cost is acceptable
  until flux grows the GPU path; the long-term plan is to migrate.
- HiDPI scale (`set_buffer_scale`) is accepted and stored but not yet
  applied at composite time. A HiDPI client renders larger than intended.
- Damage tracking remains deferred; every commit re-uploads the whole
  texture (or the transformed whole). Acceptable for static-image clients,
  a perf concern for video clients.
