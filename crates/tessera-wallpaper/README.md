# tessera-wallpaper

`tessera-wallpaper` composes image, short-video, glTF, and multi-plane parallax
wallpaper sources as the bottom-most compositor layers.

## Responsibilities

- Decode supported still and animated image formats through `image`.
- Decode video sources through a managed `ffmpeg` child process.
- Pace multi-frame media and upload a new texture only when content changes.
- Cover-scale image planes with displacement-safe overscan.
- Interpolate exposed-wallpaper pointer targets across missing observations.
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

`Wallpaper::from_path` preserves automatic image/video detection for callers
such as the Wallpaper portal. `Wallpaper::from_image_path`,
`Wallpaper::from_video_path`, and `Wallpaper::from_gltf` construct explicit
single-source modes. `Wallpaper::from_parallax_layers` constructs an ordered
image stack whose normalized depths control relative pointer displacement.

`Wallpaper::draw` paints image, video, or parallax content.
`Wallpaper::draw_model` records a depth-tested pass directly to the output;
`Wallpaper::draw_model_to` records the same layer into an offscreen image so
the complete desktop can feed backdrop effects. Video support requires
`ffmpeg` on the host.

## Use

```rust
let mut wallpaper =
    tessera_wallpaper::Wallpaper::from_path("background.webp", 1920, 1080)?;
wallpaper.set_model_from_gltf(&device, &surface, "sculpture.glb")?;

wallpaper.draw(&device, &canvas, 1920.0, 1080.0);
canvas.end();
wallpaper.draw_model(&device, &mut frame);
canvas.begin(&frame, None)?;
```

Parallax planes are supplied back-to-front. The caller withholds a pointer
sample whenever a client or shell surface covers the desktop point:

```rust
use std::time::Duration;

let layers = [
    tessera_wallpaper::ParallaxLayerSpec::new("far.png", 0.0),
    tessera_wallpaper::ParallaxLayerSpec::new("mid.png", 0.45),
    tessera_wallpaper::ParallaxLayerSpec::new("near.png", 1.0),
];
let mut wallpaper = tessera_wallpaper::Wallpaper::from_parallax_layers(
    &layers,
    tessera_wallpaper::ParallaxOptions {
        max_shift: 36.0,
        transition: Duration::from_millis(260),
    },
)?;
wallpaper.set_pointer_position(Some((640.0, 360.0)), (1280.0, 720.0));
```

For the first snippet, the caller begins the first canvas pass before the code
and ends the final pass afterward. Construct every wallpaper once and retain
it across frames so decode state, scene resources, and per-slot depth targets
remain effective.

## Related Documentation

- [Wallpaper decision](../../docs/adr/0018-wallpaper-crate.md)
- [Wallpaper modes and parallax](../../docs/adr/0092-explicit-wallpaper-modes-and-continuous-parallax.md)
- [Wallpaper configuration](../../docs/reference/config.md#wallpaper)
- [Per-frame data flow](../../docs/explanation/architecture.md#per-frame-data-flow)
- [Workspace layout](../../docs/dev/project-layout.md)
