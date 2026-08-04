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
to `aegis-compositor`, `aegis-backend`, `aegis-model`, `aegis-shell`, and `aegis-wallpaper`.

## Runtime Effect

For each frame, the renderer turns the server's surface snapshots into draw
calls on a caller-owned flux canvas. It retains only the texture cache needed
to avoid re-uploading unchanged buffers.

## Use

Construct one `Renderer` for the compositor lifetime. During each frame, pass
the complete mapped shm and dma-buf frame sets plus the compositor's
authoritative surface order to `draw_surfaces_ordered`, then call `gc` with
the live surface identifiers. Do not draw backing types or subsurface classes
in separate global passes: each toplevel's complete surface tree is one
stacking unit. The executable owns this sequencing because it also draws
wallpaper and chrome around the client scene.

## Related Documentation

- [Per-frame data flow](../../docs/explanation/architecture.md#per-frame-data-flow)
- [Client-buffer import decision](../../docs/adr/0004-client-buffers-via-flux-dmabuf-import.md)
- [Client surface compositing decision](../../docs/adr/0061-window-tree-atomic-client-surface-compositing.md)
- [Workspace layout](../../docs/dev/project-layout.md)
