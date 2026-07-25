# aegis-render

`aegis-render` converts committed client buffers into flux textures and
composites the surface tree into the current output frame.

## Responsibilities

- Upload `wl_shm` pixels and import supported dma-buf buffers.
- Apply buffer scale, transform, viewport, and subsurface geometry.
- Draw mapped surfaces in compositor z-order.
- Cache textures by surface generation and release unused GPU resources.
- Create the flux device configuration required by the compositor.

## Boundaries

This crate does not speak Wayland, own the presentation target, choose window
placement, draw shell chrome, or decode wallpaper media. Those concerns belong
to `aegis-compositor`, `aegis-backend`, `aegis-core`, `aegis-shell`, and `aegis-wallpaper`.

## Runtime Effect

For each frame, the renderer turns the server's surface snapshots into draw
calls on a caller-owned flux canvas. It retains only the texture cache needed
to avoid re-uploading unchanged buffers.

## Use

Construct one `Renderer` for the compositor lifetime. During each frame, pass
the mapped shm and dma-buf surface iterators to the matching draw methods, draw
subsurfaces, then call `gc` with the live surface identifiers. The executable
owns this sequencing because it also draws wallpaper and chrome around the
client scene.

## Related Documentation

- [Per-frame data flow](../../docs/explanation/architecture.md#per-frame-data-flow)
- [Client-buffer import decision](../../docs/adr/0004-client-buffers-via-flux-dmabuf-import.md)
- [Workspace layout](../../docs/dev/project-layout.md)

