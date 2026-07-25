# aegis-wallpaper

`aegis-wallpaper` composes image, short-video, and glTF model wallpaper sources
as the bottom-most compositor layers.

## Responsibilities

- Decode supported still and animated image formats through `image`.
- Decode video sources through a managed `ffmpeg` child process.
- Pace multi-frame media and upload a new texture only when content changes.
- Scale the current source frame to the caller's output dimensions.
- Load binary glTF scenes through `flux-scene-graph` and auto-frame their
  world-space bounds.
- Animate an orbiting camera, directional light, and Phong highlights.
- Keep one depth target per frame-in-flight slot and recreate it on resize.
- Render models either directly to the output or into a sampleable offscreen
  color image for unified backdrop effects.

## Boundaries

This crate owns wallpaper decode, GPU scene resources, and animation state. It
does not choose a wallpaper, watch configuration, own the flux device or
surface, composite client surfaces, or draw shell chrome.

## Runtime Effect

`Wallpaper::from_path` decodes or opens 2D media. `Wallpaper::from_gltf` loads
a model-only scene; `set_model_from_gltf` overlays one on existing media.
`Wallpaper::draw` paints the 2D layer. `Wallpaper::draw_model` records a
depth-tested pass directly to the output; `Wallpaper::draw_model_to` records
the same layer into an offscreen image so the complete desktop can feed blur
or other image effects. Video support requires `ffmpeg` on the host.

## Use

```rust
let mut wallpaper =
    ass_wallpaper::Wallpaper::from_path("background.webp", 1920, 1080)?;
wallpaper.set_model_from_gltf(&device, &surface, "sculpture.glb")?;

wallpaper.draw(&device, &canvas, 1920.0, 1080.0);
canvas.end();
wallpaper.draw_model(&device, &mut frame);
canvas.begin(&frame, None)?;
```

The caller begins the first canvas pass before this snippet and ends the final
pass afterward. Construct the wallpaper once and retain it across frames so
decode state, scene resources, and per-slot depth targets remain effective.

## Related Documentation

- [Wallpaper decision](../../docs/adr/0018-wallpaper-crate.md)
- [Per-frame data flow](../../docs/explanation/architecture.md#per-frame-data-flow)
- [Workspace layout](../../docs/dev/project-layout.md)
