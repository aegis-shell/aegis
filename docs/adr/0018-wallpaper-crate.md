# ADR-0018: Wallpaper as an independent crate

- Status: Accepted
- Date: 2026-06-18

## Context

The shell chrome drawn by `aegis-shell` (window list, Quit button, SSD
title bars from [ADR-0017](0017-server-side-decorations-via-overlays.md))
sits on top of the per-frame clear colour. There was no background
layer beneath client surfaces, and no path to display user-chosen
artwork. Users expect to set a desktop wallpaper, commonly as a still
image and increasingly as a short looping video.

The wallpaper has very different dependency and lifecycle
characteristics from the chrome:

- Decoding PNG/JPEG/GIF/WebP/etc. pulls in the `image` crate (and its
  format crates), which the rest of the compositor does not need.
- Short-video wallpaper implies either an `ffmpeg` child process or a
  `gstreamer`/`ffmpeg-next` dependency, neither of which belongs in
  `aegis-render` or `aegis-compositor`.
- The wallpaper is a leaf feature: it consumes
  `flux::Image::from_bytes` and `Canvas::draw_image`, the same public
  seam `aegis-render` already uses, and produces no data the rest of the
  compositor reads back.

[ADR-0001](0001-scope-and-responsibility-boundary.md) places compositor
chrome in `aegis-shell` and compositing in `aegis-render`, but is silent on
"the layer beneath everything." Putting wallpaper in either crate would
either pull image-decode deps into the renderer or pull render concerns
into the shell.

## Decision

### 1. New crate `aegis-wallpaper`

A leaf workspace crate at `crates/aegis-wallpaper`, depending on `flux`
(for `Image::from_bytes` and `Canvas::draw_image`) and `image` (for
decode). It owns no flux device or canvas; the main loop passes them
per frame, mirroring the `ass_render::Renderer` seam.

### 2. Multi-format image decode via the `image` crate

Static images (PNG, JPEG, WebP still, BMP, TIFF, TGA, QOI, ICO, PNM)
decode to a single full-canvas BGRA8 buffer. Animated GIF and WebP
produce a frame sequence with per-frame durations; sub-rect frames are
composited onto the source's full canvas during decode so every emitted
frame has identical dimensions.

The crate advertises this through `Wallpaper::from_path`, which tries
image decode first; on failure it falls through to the video path.

### 3. Short-video decode via an external `ffmpeg` child process

For video files, the crate spawns `ffmpeg` with
`-vf fps=...,scale=W:H` and `-pix_fmt bgra -f rawvideo -`, reading
raw tightly-packed BGRA8 frames from stdout. A background reader thread
always keeps the most recently decoded frame in a slot; the
compositor's main thread reads it non-blocking on each `draw`. When
`ffmpeg` reaches EOF (short video), the reader respawns it so playback
loops.

The output resolution is fixed at construction time (passed by the
main loop as the host's current size). Resizes after that point
GPU-scale the existing texture via `Canvas::draw_image`'s destination
size; the decode resolution is not re-negotiated.

`Drop` flips a shutdown flag that the reader checks between full
frames; the next `ffmpeg` exit ends the thread. The reader cannot be
interrupted mid-`read()`, so a video source dropped while `ffmpeg` is
still producing a frame keeps the thread alive until that frame lands
or `ffmpeg` exits.

### 4. `Wallpaper::draw` is the only per-frame entry point

The main loop calls `draw(device, canvas, dst_w, dst_h)` once per
frame before any client surfaces. Internally:

1. The source advances — animated stills by wall-clock time, video by
   pulling the latest slot frame.
2. A generation counter tags the current frame; if it changed, the
   pixels are re-uploaded to a fresh `flux::Image`.
3. The cached image is drawn to the canvas at the requested
   destination size.

### 5. The main loop opts in via `$ASS_WALLPAPER`

The binary reads `ASS_WALLPAPER` at startup. If set, it loads the
path; if load fails or the variable is unset, the frame's clear colour
shows through and the rest of the compositor is unaffected. There is
no in-process configuration file or live reload path yet.

## Alternatives

- **Wallpaper inside `aegis-shell`.** Rejected: would pull `image` and
   the ffmpeg child management into the chrome crate, blurring the
   chrome-vs-content seam. The shell consumes input and draws widgets;
   it should not own decode pipelines.
- **Wallpaper inside `aegis-render`.** Rejected: `aegis-render` composites
   client buffers into the scene; it should not own a user-content
   loader. Mixing the two would also force every consumer of
   `aegis-render` to link the image-decode dependency graph.
- **Live video decode in-process via `ffmpeg-next` or `gstreamer`.**
   Deferred: both add heavy native dependencies (`libavcodec-dev` or
   `libgstreamer-1.0-dev` plus plugins) and complicate cross-distro
   builds. The external `ffmpeg` child covers short looping video for
   the cost of one extra process and is trivially swappable for
   `ffmpeg-next` later if a live, long-form video wallpaper becomes a
   goal — the `Wallpaper::draw` API does not change.
- **GStreamer with dmabuf zero-copy into flux.** Deferred: would be
   the highest-performance path (ass already imports dmabuf via flux,
   see [ADR-0004](0004-client-buffers-via-flux-dmabuf-import.md)) but
   requires the heaviest dependency set and the most plumbing. Tracked
   as a possible follow-up if video wallpaper turns into a primary use
   case.
- **Decode video on the main thread, blocking.** Rejected: stalling
   the compositor's frame loop on `ffmpeg`'s decode cadence would
   drop frames and stall input routing. The background reader thread
   keeps the main loop non-blocking.

## Consequences

- A new leaf crate `aegis-wallpaper` joins the workspace. The `ass`
  binary depends on it; no other crate does.
- The main loop learns one new optional step: load the wallpaper from
  `$ASS_WALLPAPER` at startup (if set), draw it before client
  surfaces. When unset, behaviour is unchanged.
- Adding `image` to the workspace dependency graph increases cold
  build time of `aegis-wallpaper` and its dependents, but the seam
  keeps that cost off `aegis-compositor`, `aegis-render`, and `aegis-shell`.
- The video path requires `ffmpeg` installed on the host. If absent,
  `Wallpaper::from_path` returns an error for video files; image
  wallpapers still work. The main loop logs and continues without a
  wallpaper.
- Resizing the host window after load does not re-decode video; the
  frame is GPU-scaled on draw. Visually correct, mildly wasteful if
  the user drastically resizes.
- Replacing the wallpaper at runtime is not yet supported: the video
  reader thread is shut down only when the `Wallpaper` is dropped. A
  follow-up can add a `Wallpaper::replace` path.
- Animated GIF/WebP frames are composited in-memory at the source's
  full canvas size. A 1920×1080 animated WebP at 30 frames holds
  ~250 MB of BGRA8 in memory. Acceptable for wallpaper-class assets;
  pathological sources would need frame-on-demand decode in a
  follow-up.
