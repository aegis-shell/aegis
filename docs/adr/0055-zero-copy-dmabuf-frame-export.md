# ADR-0055: Zero-copy dmabuf export for ScreenCast and frame capture

- Status: Proposed
- Date: 2026-07-25

## Context

[ADR-0052](0052-scoped-output-frame-streaming.md) established continuous output frame streaming over scoped IPC (`StreamOutputStart`/`StreamOutputStop`), driving `org.freedesktop.impl.portal.ScreenCast` v2 via PipeWire shm buffers.

While CPU readback unblocked initial ScreenCast functionality, continuous CPU readback (GPU staging copy $\rightarrow$ CPU pixel conversion $\rightarrow$ memfd transfer $\rightarrow$ PipeWire shm) introduces memory bandwidth overhead at high frame rates and resolutions (e.g. 4K @ 60 FPS).

[flux](../../libs/flux) already supports direct GPU dmabuf export via `Surface::offscreen_dmabuf` and `export_dmabuf`. PipeWire and modern video consumers (Firefox, Chromium, OBS) natively support `SPA_VIDEO_FORMAT_DMA_DRM` zero-copy buffer passing.

## Decision

### 1. Dmabuf Export Metadata in IPC

Extend `StreamFrame` IPC metadata in `aegis-ipc` to support direct GPU dmabuf handles alongside CPU memfd buffers:

- `DmabufBufferSpec`: `width`, `height`, `drm_format` (DRM FOURCC), `modifier` (DRM format modifier), `stride`, `offset`.
- `damage_rects`: A list of compositor logical/physical damage rectangles (`Vec<Rect>`) per frame instead of forcing full-frame invalidation.

### 2. PipeWire `SPA_VIDEO_FORMAT_DMA_DRM` Negotiation

Update `aegis-portal` ScreenCast cast worker:
- Advertise both `SPA_VIDEO_FORMAT_DMA_DRM` and raw `BGRx` formats in `EnumFormat`.
- When the PipeWire consumer supports dmabuf import, transfer the exported dmabuf file descriptor directly into PipeWire SPA dmabuf buffers.
- If modifier negotiation fails or the consumer lacks dmabuf support, fall back seamlessly to CPU readback shm buffers.

### 3. Damage Rect Tracking

Expose compositor output damage bounding boxes (`damage_rects`) to `StreamFrame`, passing damage metadata to PipeWire consumers via `SPA_META_VideoDamage`.

## Consequences

- High-resolution screen sharing (4K / 60 FPS) achieves zero CPU copy overhead when consumed by hardware-accelerated clients.
- CPU readback remains fully functional as a universal fallback for non-dmabuf consumers.
- Damage tracking reduces compositing and encoding work for partially damaged screen regions.
