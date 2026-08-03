use super::*;

/// Backdrop effects are evaluated at full physical resolution. Liquid-glass
/// lensing samples the sharp capture directly, so a downsampled capture would
/// read as a low-resolution smear behind every glass body. The capture is
/// still clamped to the union of the declared regions (plus blur footprint),
/// and the blur itself stays cheap through the fixed-cost dual-Kawase
/// pyramid, so the full-resolution target only costs the region render.
pub(super) const BACKDROP_DOWNSAMPLE: u32 = 1;

/// Convert output damage to the single physical-pixel rectangle accepted by
/// Vulkan dynamic rendering. `None` deliberately means a full-destination
/// pass: both `FrameDamage::Full` and the conservative empty-area fallback
/// must leave scissoring disabled.
pub(super) fn frame_damage_render_area(repaint: &FrameDamage) -> Option<flux::CanvasRenderArea> {
    repaint.area_union().and_then(|rect| {
        (!rect.is_empty()).then_some(flux::CanvasRenderArea {
            x: rect.origin.x,
            y: rect.origin.y,
            width: rect.size.w as u32,
            height: rect.size.h as u32,
        })
    })
}

/// Resume the output after the no-stencil client/image pass for arbitrary
/// compositor UI. Lens may emit path fills whose correct winding semantics
/// require stencil, so shell drawing must never share Aegis's optimized
/// no-stencil base pass. `render_area` is copied from that base pass so the
/// split preserves partial-repaint bounds while `clear: None` preserves every
/// pixel already composited below the UI.
pub(super) fn begin_stencil_frame_overlay(
    canvas: &flux::Canvas,
    frame: &flux::Frame<'_>,
    render_area: Option<flux::CanvasRenderArea>,
) -> Result<(), flux::Error> {
    canvas.begin_pass(
        frame,
        flux::CanvasPassOptions {
            clear: None,
            antialias: flux::CanvasAntialias::None,
            render_area,
            skip_stencil: false,
        },
    )
}

/// Target-pass counterpart to [`begin_stencil_frame_overlay`]. Screenshot
/// freeze capture initially records only opaque wallpaper/client image work
/// without stencil, then resumes through this helper before baking Lens chrome
/// into the target.
pub(super) fn begin_stencil_target_overlay(
    canvas: &flux::Canvas,
    frame: &flux::Frame<'_>,
    target: &flux::Image,
    render_area: Option<flux::CanvasRenderArea>,
) -> Result<(), flux::Error> {
    canvas.begin_target_pass(
        frame,
        target,
        flux::CanvasPassOptions {
            clear: None,
            antialias: flux::CanvasAntialias::None,
            render_area,
            skip_stencil: false,
        },
    )
}

/// Begin a compositor canvas pass without Flux's clear-triggered 4x MSAA.
///
/// Legacy Flux selects its multisample render-and-resolve path whenever a
/// clear colour is supplied. That is useful for arbitrary vector artwork, but
/// disproportionately expensive for a compositor pass whose dominant work is
/// opaque image quads: at 3072x1920 it turns one output pixel into four colour
/// samples plus a full-frame resolve. Aegis chrome already uses analytic
/// rounded-rectangle coverage and coverage-texture glyphs, so request the
/// explicit one-sample attachment-clear path.
///
/// `clear` must be opaque because the compositor output does not preserve
/// destination alpha between layers.
pub(super) fn begin_opaque_frame(
    canvas: &flux::Canvas,
    frame: &flux::Frame<'_>,
    clear: u32,
) -> Result<(), flux::Error> {
    debug_assert_eq!(clear >> 24, 0xff, "compositor pass clear must be opaque");
    canvas.begin_pass(
        frame,
        flux::CanvasPassOptions {
            clear: Some(clear),
            antialias: flux::CanvasAntialias::None,
            render_area: None,
            skip_stencil: true,
        },
    )
}

/// Begin an opaque output pass clipped to the accumulated damage for this
/// ring slot. The full scene may still be submitted, but rasterization,
/// texture sampling, blending and framebuffer writes stay inside the scissor.
pub(super) fn begin_opaque_frame_repaint(
    canvas: &flux::Canvas,
    frame: &flux::Frame<'_>,
    _size: (u32, u32),
    clear: u32,
    repaint: &FrameDamage,
) -> Result<(), flux::Error> {
    debug_assert_eq!(clear >> 24, 0xff, "compositor pass clear must be opaque");
    // Vulkan's renderArea is a single rectangle, so the pass is bounded by the
    // UNION of the dirty-rect list even though the KMS hint carries every rect.
    // (A list smaller than its union means pixels inside the union but outside
    // every rect are re-cleared to the opaque background, which is harmless for
    // an opaque compositor pass: they were already that colour.)
    match frame_damage_render_area(repaint) {
        Some(render_area) => canvas.begin_pass(
            frame,
            flux::CanvasPassOptions {
                clear: Some(clear),
                antialias: flux::CanvasAntialias::None,
                render_area: Some(render_area),
                skip_stencil: true,
            },
        )?,
        None => {
            canvas.begin_pass(
                frame,
                flux::CanvasPassOptions {
                    clear: Some(clear),
                    antialias: flux::CanvasAntialias::None,
                    render_area: None,
                    skip_stencil: true,
                },
            )?;
        }
    }
    Ok(())
}

/// Offscreen-target counterpart to [`begin_opaque_frame`].
pub(super) fn begin_opaque_target(
    canvas: &flux::Canvas,
    frame: &flux::Frame<'_>,
    target: &flux::Image,
    clear: u32,
) -> Result<(), flux::Error> {
    debug_assert_eq!(clear >> 24, 0xff, "compositor pass clear must be opaque");
    canvas.begin_target_pass(
        frame,
        target,
        flux::CanvasPassOptions {
            clear: Some(clear),
            antialias: flux::CanvasAntialias::None,
            render_area: None,
            skip_stencil: true,
        },
    )
}

/// One connected backdrop capture/compute region in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct BackdropCaptureRegion {
    pub(super) origin: (u32, u32),
    pub(super) extent: (u32, u32),
}

/// Connected groups of declared backdrop regions in physical pixels,
/// expanded by the blur footprint and aligned to the capture grid.
///
/// Keeping disjoint groups separate is critical: a top HUD plus a bottom Dock
/// must not turn into one full-output compute dispatch merely because their
/// bounding box spans the screen. Overlapping padded regions are merged
/// transitively so no blur pass writes the same pixels twice.
pub(super) fn blur_capture_regions(
    regions: &[aegis_shell::BackdropRegion],
    logical_size: (u32, u32),
    physical_size: (u32, u32),
    scale: f32,
    sigma: f32,
) -> Vec<BackdropCaptureRegion> {
    let pad = 3.0 * sigma * scale;
    let align = BACKDROP_DOWNSAMPLE as f32;
    let mut merged: Vec<(u32, u32, u32, u32)> = Vec::new();
    for region in regions {
        let x = region.x.max(0.0);
        let y = region.y.max(0.0);
        let w = region.w.max(0.0).min(logical_size.0 as f32 - x) * scale;
        let h = region.h.max(0.0).min(logical_size.1 as f32 - y) * scale;
        if w <= 0.0 || h <= 0.0 {
            continue;
        }
        let mut rect = (
            ((((x * scale) - pad) / align).floor() * align).max(0.0) as u32,
            ((((y * scale) - pad) / align).floor() * align).max(0.0) as u32,
            ((((x * scale + w) + pad) / align).ceil() * align).min(physical_size.0 as f32) as u32,
            ((((y * scale + h) + pad) / align).ceil() * align).min(physical_size.1 as f32) as u32,
        );
        if rect.2 <= rect.0 || rect.3 <= rect.1 {
            continue;
        }

        // Merge every transitively overlapping/touching padded rectangle.
        // `swap_remove` keeps the loop allocation-free after the small Vec
        // grows to the number of active chrome bodies.
        let mut index = 0;
        while index < merged.len() {
            let other = merged[index];
            if rect.0 <= other.2 && other.0 <= rect.2 && rect.1 <= other.3 && other.1 <= rect.3 {
                rect = (
                    rect.0.min(other.0),
                    rect.1.min(other.1),
                    rect.2.max(other.2),
                    rect.3.max(other.3),
                );
                merged.swap_remove(index);
                index = 0;
            } else {
                index += 1;
            }
        }
        merged.push(rect);
    }
    merged.sort_unstable_by_key(|rect| (rect.1, rect.0));
    merged
        .into_iter()
        .map(|(x0, y0, x1, y1)| BackdropCaptureRegion {
            origin: (x0, y0),
            extent: (x1 - x0, y1 - y0),
        })
        .collect()
}

/// Union of the declared backdrop regions in physical pixels, expanded by
/// the blur footprint and aligned to the capture grid.
///
/// The backdrop capture only needs to cover what the blur can sample: the
/// regions themselves plus a 3σ margin on every side (dual-Kawase gathers
/// within roughly that radius), so the offscreen pass renders a fraction of
/// the desktop instead of the full screen. Origin and size are aligned to
/// [`BACKDROP_DOWNSAMPLE`] so the capture maps the scene at exactly
/// 1/BACKDROP_DOWNSAMPLE on both axes and the origin stays an integer
/// capture-pixel offset. Everything is clamped to the physical extent, so
/// blur sampling at the screen edge keeps the capture's clamp-to-edge
/// behaviour rather than sampling undefined padding.
pub(super) fn blur_capture_bounds(
    regions: &[aegis_shell::BackdropRegion],
    logical_size: (u32, u32),
    physical_size: (u32, u32),
    scale: f32,
    sigma: f32,
) -> ((u32, u32), (u32, u32)) {
    let connected = blur_capture_regions(regions, logical_size, physical_size, scale, sigma);
    if connected.is_empty() {
        return ((0, 0), physical_size);
    }
    let x0 = connected
        .iter()
        .map(|region| region.origin.0)
        .min()
        .unwrap();
    let y0 = connected
        .iter()
        .map(|region| region.origin.1)
        .min()
        .unwrap();
    let x1 = connected
        .iter()
        .map(|region| region.origin.0 + region.extent.0)
        .max()
        .unwrap();
    let y1 = connected
        .iter()
        .map(|region| region.origin.1 + region.extent.1)
        .max()
        .unwrap();
    ((x0, y0), (x1 - x0, y1 - y0))
}

/// Map physical-output capture regions into the reusable capture image.
/// Origins round down and far edges round up so downsampling never drops a
/// source pixel required by the blur footprint.
pub(super) fn blur_regions_in_capture(
    regions: &[BackdropCaptureRegion],
    capture_origin: (u32, u32),
    capture_extent: (u32, u32),
    capture_size: (u32, u32),
) -> Vec<flux::BlurRegion> {
    let scale_x = capture_size.0 as f32 / capture_extent.0.max(1) as f32;
    let scale_y = capture_size.1 as f32 / capture_extent.1.max(1) as f32;
    regions
        .iter()
        .filter_map(|region| {
            let rel_x0 = region.origin.0.saturating_sub(capture_origin.0);
            let rel_y0 = region.origin.1.saturating_sub(capture_origin.1);
            let rel_x1 = region
                .origin
                .0
                .saturating_add(region.extent.0)
                .saturating_sub(capture_origin.0);
            let rel_y1 = region
                .origin
                .1
                .saturating_add(region.extent.1)
                .saturating_sub(capture_origin.1);
            let x0 = ((rel_x0 as f32 * scale_x).floor() as u32).min(capture_size.0);
            let y0 = ((rel_y0 as f32 * scale_y).floor() as u32).min(capture_size.1);
            let x1 = ((rel_x1 as f32 * scale_x).ceil() as u32).min(capture_size.0);
            let y1 = ((rel_y1 as f32 * scale_y).ceil() as u32).min(capture_size.1);
            (x1 > x0 && y1 > y0).then_some(flux::BlurRegion {
                x: x0,
                y: y0,
                width: x1 - x0,
                height: y1 - y0,
            })
        })
        .collect()
}

/// Map logical backdrop bodies into capture-target pixels. These rectangles
/// are used only as Canvas clips while material output is persisted in the
/// per-slot transparent composite cache.
pub(super) fn backdrop_regions_in_capture(
    regions: &[aegis_shell::BackdropRegion],
    capture_origin: (u32, u32),
    capture_extent: (u32, u32),
    capture_size: (u32, u32),
    output_scale: f32,
) -> Vec<flux::BlurRegion> {
    let ratio_x = capture_size.0 as f32 / capture_extent.0.max(1) as f32;
    let ratio_y = capture_size.1 as f32 / capture_extent.1.max(1) as f32;
    regions
        .iter()
        .filter_map(|region| {
            let x0 = (region.x.max(0.0) * output_scale - capture_origin.0 as f32) * ratio_x;
            let y0 = (region.y.max(0.0) * output_scale - capture_origin.1 as f32) * ratio_y;
            let x1 =
                ((region.x + region.w).max(0.0) * output_scale - capture_origin.0 as f32) * ratio_x;
            let y1 =
                ((region.y + region.h).max(0.0) * output_scale - capture_origin.1 as f32) * ratio_y;
            let x0 = x0.floor().max(0.0).min(capture_size.0 as f32) as u32;
            let y0 = y0.floor().max(0.0).min(capture_size.1 as f32) as u32;
            let x1 = x1.ceil().max(0.0).min(capture_size.0 as f32) as u32;
            let y1 = y1.ceil().max(0.0).min(capture_size.1 as f32) as u32;
            (x1 > x0 && y1 > y0).then_some(flux::BlurRegion {
                x: x0,
                y: y0,
                width: x1 - x0,
                height: y1 - y0,
            })
        })
        .collect()
}

/// Convert output-logical liquid-glass declarations into coordinates of the
/// capture image consumed by Optics. This is the single mapping
/// used for shape, corner radius, refraction and the final image draw.
/// Shadow distances scale like the shape; the alpha passes through.
pub(super) fn liquid_glass_groups(
    regions: &[aegis_shell::LiquidGlassRegion],
    capture_origin: (u32, u32),
    scale: f32,
    capture_ratio: f32,
) -> Vec<flux::LiquidGlassGroup> {
    let capture_scale = scale * capture_ratio;
    regions
        .iter()
        .filter(|region| region.bounds.w > 0.0 && region.bounds.h > 0.0 && region.opacity > 0.0)
        .map(|region| flux::LiquidGlassGroup {
            primary: flux::LiquidGlassShape {
                x: region.bounds.x * capture_scale - capture_origin.0 as f32 * capture_ratio,
                y: region.bounds.y * capture_scale - capture_origin.1 as f32 * capture_ratio,
                width: region.bounds.w * capture_scale,
                height: region.bounds.h * capture_scale,
                corner_radius: region.corner_radius * capture_scale,
            },
            merged: None,
            blend_radius: 0.0,
            opacity: region.opacity.clamp(0.0, 1.0),
            shadow_alpha: region.shadow_alpha.clamp(0.0, 1.0),
            shadow_blur: region.shadow_blur.max(0.0) * capture_scale,
            shadow_offset_y: region.shadow_offset_y * capture_scale,
            tint_color: [255, 255, 255],
        })
        .collect()
}

pub(super) struct BackdropCapture {
    image: flux::Image,
    /// Transparent, already-composited frost/liquid result sampled by the
    /// output pass. Keeping this separate from the captured desktop is what
    /// lets a slot skip both capture and compute when its source footprint did
    /// not change.
    composite: flux::Image,
    size: (u32, u32),
    format: flux::Format,
    valid: bool,
}

struct ScreenshotCapture {
    image: flux::Image,
    size: (u32, u32),
    format: flux::Format,
}

/// Exact backdrop configuration. Float values are stored as IEEE bit patterns
/// so equality is collision-free and every geometry/material change
/// invalidates all per-slot effect caches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct BackdropCacheKey {
    capture_origin: (u32, u32),
    capture_extent: (u32, u32),
    physical_size: (u32, u32),
    sigma: u32,
    scale: u32,
    model_active: bool,
    capture_regions: Vec<BackdropCaptureRegion>,
    frost_regions: Vec<[u32; 4]>,
    liquid_regions: Vec<[u32; 10]>,
    scene_overlays: Vec<u64>,
}

impl BackdropCacheKey {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        capture_origin: (u32, u32),
        capture_extent: (u32, u32),
        physical_size: (u32, u32),
        sigma: f32,
        scale: f32,
        model_active: bool,
        capture_regions: &[BackdropCaptureRegion],
        frost_regions: &[aegis_shell::BackdropRegion],
        liquid_regions: &[aegis_shell::LiquidGlassRegion],
        window_switcher: Option<&aegis_shell::WindowSwitcherPresentation>,
        live_previews: &[aegis_shell::LivePreviewPresentation],
    ) -> Self {
        fn push_rect(out: &mut Vec<u64>, rect: aegis_core::Rect) {
            out.extend([
                rect.origin.x as u64,
                rect.origin.y as u64,
                rect.size.w as u64,
                rect.size.h as u64,
            ]);
        }
        let mut scene_overlays = Vec::new();
        if let Some(switcher) = window_switcher {
            scene_overlays.push(1);
            scene_overlays.push(switcher.visibility.to_bits() as u64);
            push_rect(&mut scene_overlays, switcher.panel);
            scene_overlays.push(switcher.cards.len() as u64);
            for card in &switcher.cards {
                scene_overlays.push(card.window.0);
                push_rect(&mut scene_overlays, card.geometry.preview);
            }
        } else {
            scene_overlays.push(0);
        }
        scene_overlays.push(live_previews.len() as u64);
        for preview in live_previews {
            scene_overlays.push(preview.cards.len() as u64);
            for card in &preview.cards {
                scene_overlays.push(card.window.0);
                push_rect(&mut scene_overlays, card.geometry.preview);
            }
        }
        Self {
            capture_origin,
            capture_extent,
            physical_size,
            sigma: sigma.to_bits(),
            scale: scale.to_bits(),
            model_active,
            capture_regions: capture_regions.to_vec(),
            frost_regions: frost_regions
                .iter()
                .map(|region| {
                    [
                        region.x.to_bits(),
                        region.y.to_bits(),
                        region.w.to_bits(),
                        region.h.to_bits(),
                    ]
                })
                .collect(),
            liquid_regions: liquid_regions
                .iter()
                .map(|region| {
                    [
                        region.bounds.x.to_bits(),
                        region.bounds.y.to_bits(),
                        region.bounds.w.to_bits(),
                        region.bounds.h.to_bits(),
                        region.corner_radius.to_bits(),
                        region.opacity.to_bits(),
                        region.shadow_alpha.to_bits(),
                        region.shadow_blur.to_bits(),
                        region.shadow_offset_y.to_bits(),
                        0,
                    ]
                })
                .collect(),
            scene_overlays,
        }
    }
}

/// Live desktop capture used behind the full-screen application launcher.
///
/// Capture images and blur intermediates are both indexed by frame slot. A
/// slot is rewritten only after `begin_frame` has waited its fence, avoiding
/// device-wide stalls while a 3D wallpaper continues animating.
pub(super) struct LauncherBackdrop {
    blur: flux::BlurFilter,
    glass: flux::LiquidGlassFilter,
    captures: Vec<Option<BackdropCapture>>,
    /// Source damage missed by each effect-cache slot while another slot was
    /// presented. This mirrors swapchain damage history but is deliberately
    /// independent from output/chrome damage.
    source_slot_damage: Vec<FrameDamage>,
    config: Option<BackdropCacheKey>,
    was_active: bool,
    failed_session: bool,
    unsupported: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum BackdropPlan {
    Direct,
    /// Capture the desktop footprint and rebuild this frame slot's effect.
    Refresh(Vec<BackdropCaptureRegion>),
    /// Reuse the already-composited effect image for this frame slot.
    Cached,
}

fn backdrop_refresh_regions(
    valid: bool,
    model_active: bool,
    source_damage: &FrameDamage,
    input_regions: &[BackdropCaptureRegion],
) -> Vec<BackdropCaptureRegion> {
    if model_active || !valid || matches!(source_damage, FrameDamage::Full) {
        return input_regions.to_vec();
    }
    let FrameDamage::Area(damage) = source_damage else {
        return Vec::new();
    };
    input_regions
        .iter()
        .copied()
        .filter(|region| {
            let input = aegis_core::Rect::new(
                region.origin.0 as i32,
                region.origin.1 as i32,
                region.extent.0 as i32,
                region.extent.1 as i32,
            );
            damage.iter().any(|dirty| dirty.intersect(input).is_some())
        })
        .collect()
}

impl LauncherBackdrop {
    pub(super) fn new(device: &flux::Device) -> Result<Self, flux::Error> {
        Ok(Self {
            blur: flux::BlurFilter::new(device)?,
            glass: flux::LiquidGlassFilter::new(device)?,
            captures: Vec::new(),
            source_slot_damage: Vec::new(),
            config: None,
            was_active: false,
            failed_session: false,
            unsupported: false,
        })
    }

    /// `extent` is the physical-pixel area the capture must cover (the blur
    /// regions' padded union, or the full surface for a live 3D wallpaper);
    /// the capture target is allocated at `extent / BACKDROP_DOWNSAMPLE`.
    /// With full-resolution captures the quotient is the extent itself.
    pub(super) fn prepare(
        &mut self,
        active: bool,
        device: &flux::Device,
        surface: &flux::Surface,
        frame: &flux::Frame<'_>,
        config: BackdropCacheKey,
        source_damage: &FrameDamage,
    ) -> BackdropPlan {
        if !active {
            self.was_active = false;
            self.failed_session = false;
            self.config = None;
            for capture in self.captures.iter_mut().flatten() {
                capture.valid = false;
            }
            return BackdropPlan::Direct;
        }

        let opening = !self.was_active;
        self.was_active = true;
        if opening {
            self.failed_session = false;
        }
        let extent = config.capture_extent;
        if self.unsupported || self.failed_session || extent.0 == 0 || extent.1 == 0 {
            return BackdropPlan::Direct;
        }

        if self.config.as_ref() != Some(&config) {
            self.config = Some(config.clone());
            for capture in self.captures.iter_mut().flatten() {
                capture.valid = false;
            }
            for pending in &mut self.source_slot_damage {
                *pending = FrameDamage::Full;
            }
        }
        let format = match surface.format() {
            flux::Format::FLUX_FORMAT_RGBA8_UNORM | flux::Format::FLUX_FORMAT_BGRA8_UNORM => {
                flux::Format::FLUX_FORMAT_RGBA8_UNORM
            }
            other => {
                log::warn!(
                    "launcher: realtime backdrop unavailable for surface format {other:?}; using translucent fallback"
                );
                self.unsupported = true;
                return BackdropPlan::Direct;
            }
        };

        let size = (
            extent.0.div_ceil(BACKDROP_DOWNSAMPLE).max(1),
            extent.1.div_ceil(BACKDROP_DOWNSAMPLE).max(1),
        );
        let slot = frame.index() as usize;
        if self.captures.len() <= slot {
            self.captures.resize_with(slot + 1, || None);
        }
        if self.source_slot_damage.len() <= slot {
            self.source_slot_damage.resize(slot + 1, FrameDamage::Full);
        }
        let target_stale = self.captures[slot]
            .as_ref()
            .is_none_or(|capture| capture.size != size || capture.format != format);
        if target_stale {
            match (
                flux::Image::render_target(device, size.0, size.1, format),
                flux::Image::render_target(device, size.0, size.1, format),
            ) {
                (Ok(image), Ok(composite)) => {
                    self.captures[slot] = Some(BackdropCapture {
                        image,
                        composite,
                        size,
                        format,
                        valid: false,
                    });
                }
                (Err(error), _) | (_, Err(error)) => {
                    log::warn!(
                        "launcher: failed to allocate realtime backdrop target ({error}); using translucent fallback"
                    );
                    self.failed_session = true;
                    return BackdropPlan::Direct;
                }
            }
        }
        let missed =
            union_frame_damage(self.source_slot_damage[slot].clone(), source_damage.clone());
        let capture = self.captures[slot]
            .as_ref()
            .expect("backdrop target allocation succeeded");
        let refresh_regions = backdrop_refresh_regions(
            capture.valid,
            config.model_active,
            &missed,
            &config.capture_regions,
        );
        if refresh_regions.is_empty() {
            BackdropPlan::Cached
        } else {
            BackdropPlan::Refresh(refresh_regions)
        }
    }

    pub(super) fn begin_capture(
        &mut self,
        canvas: &flux::Canvas,
        frame: &flux::Frame<'_>,
        clear: u32,
        render_area: flux::CanvasRenderArea,
    ) -> bool {
        let Some(target) = self.target(frame) else {
            return false;
        };
        let result = canvas.begin_target_pass(
            frame,
            target,
            flux::CanvasPassOptions {
                clear: Some(clear),
                antialias: flux::CanvasAntialias::None,
                render_area: Some(render_area),
                skip_stencil: true,
            },
        );
        if let Err(error) = result {
            log::warn!(
                "launcher: failed to begin backdrop capture ({error}); using translucent fallback"
            );
            self.failed_session = true;
            return false;
        }
        true
    }

    pub(super) fn target(&self, frame: &flux::Frame<'_>) -> Option<&flux::Image> {
        self.captures
            .get(frame.index() as usize)
            .and_then(Option::as_ref)
            .map(|capture| &capture.image)
    }

    pub(super) fn capture_size(&self, frame: &flux::Frame<'_>) -> Option<(u32, u32)> {
        self.captures
            .get(frame.index() as usize)
            .and_then(Option::as_ref)
            .map(|capture| capture.size)
    }

    /// Finish a capture, rebuild the realtime effects, and persist the result
    /// in this frame slot's transparent composite image. Later uses of the
    /// same slot can sample that image without executing capture or compute.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn finish_refresh(
        &mut self,
        canvas: &flux::Canvas,
        frame: &flux::Frame<'_>,
        sigma: f32,
        blur_regions: &[flux::BlurRegion],
        frost_regions: &[flux::BlurRegion],
        all_backdrop_regions: &[flux::BlurRegion],
        glass_groups: &[flux::LiquidGlassGroup],
        glass_params: flux::LiquidGlassParams,
    ) -> bool {
        let slot = frame.index() as usize;
        if let Err(error) = canvas.end_target_checked() {
            log::warn!(
                "launcher: backdrop capture pass failed ({error}); using translucent fallback"
            );
            self.failed_session = true;
            if let Some(capture) = self.captures.get_mut(slot).and_then(Option::as_mut) {
                capture.valid = false;
            }
            return false;
        }
        let Some(capture) = self.captures.get(slot).and_then(Option::as_ref) else {
            return false;
        };
        let refreshed = (|| -> Result<(), flux::Error> {
            let blurred = self
                .blur
                .apply_regions(frame, &capture.image, sigma, blur_regions)?;
            let liquid = if glass_groups.is_empty() {
                None
            } else {
                match self
                    .glass
                    .apply(frame, &capture.image, &blurred, glass_groups, glass_params)
                {
                    Ok(image) => Some(image),
                    Err(error) => {
                        log::warn!(
                            "launcher: liquid-glass dispatch failed ({error}); using frost fallback"
                        );
                        None
                    }
                }
            };
            let drawn_frost_regions = if liquid.is_some() {
                frost_regions
            } else {
                // Liquid dispatch failure intentionally degrades each analytic
                // body to the same cached frost material instead of leaving a
                // transparent hole.
                all_backdrop_regions
            };

            // Clear and rewrite only the disconnected padded input regions.
            // Geometry/material changes allocate or invalidate the target, so
            // every pixel that could contain an old effect is covered here.
            for region in blur_regions {
                canvas.begin_target_pass(
                    frame,
                    &capture.composite,
                    flux::CanvasPassOptions {
                        clear: Some(flux::rgba(0, 0, 0, 0)),
                        antialias: flux::CanvasAntialias::None,
                        render_area: Some(flux::CanvasRenderArea {
                            x: region.x as i32,
                            y: region.y as i32,
                            width: region.width,
                            height: region.height,
                        }),
                        skip_stencil: true,
                    },
                )?;
                for frost in drawn_frost_regions {
                    canvas.save();
                    canvas.clip_rect(
                        frost.x as f32,
                        frost.y as f32,
                        frost.width as f32,
                        frost.height as f32,
                    );
                    blurred.draw(
                        canvas,
                        0.0,
                        0.0,
                        capture.size.0 as f32,
                        capture.size.1 as f32,
                    );
                    canvas.restore();
                }
                if let Some(image) = liquid.as_ref() {
                    image.draw(
                        canvas,
                        0.0,
                        0.0,
                        capture.size.0 as f32,
                        capture.size.1 as f32,
                    );
                }
                canvas.end_target_checked()?;
            }
            Ok(())
        })();
        match refreshed {
            Ok(()) => {
                if let Some(capture) = self.captures.get_mut(slot).and_then(Option::as_mut) {
                    capture.valid = true;
                }
                true
            }
            Err(error) => {
                log::warn!(
                    "launcher: realtime backdrop dispatch failed ({error}); using translucent fallback"
                );
                self.failed_session = true;
                if let Some(capture) = self.captures.get_mut(slot).and_then(Option::as_mut) {
                    capture.valid = false;
                }
                false
            }
        }
    }

    /// Draw this frame slot's persistent transparent effect image.
    pub(super) fn draw_cached(
        &self,
        canvas: &flux::Canvas,
        frame: &flux::Frame<'_>,
        origin: (u32, u32),
        extent: (u32, u32),
    ) -> bool {
        let Some(capture) = self
            .captures
            .get(frame.index() as usize)
            .and_then(Option::as_ref)
            .filter(|capture| capture.valid)
        else {
            return false;
        };
        // The transparent cache is initialized only inside the disconnected
        // padded effect regions. Never sample undefined pixels between/outside
        // those regions (notably the large gap between a HUD and Dock).
        let Some(config) = self.config.as_ref() else {
            return false;
        };
        for region in &config.capture_regions {
            canvas.save();
            canvas.clip_rect(
                region.origin.0 as f32,
                region.origin.1 as f32,
                region.extent.0 as f32,
                region.extent.1 as f32,
            );
            canvas.draw_image(
                &capture.composite,
                origin.0 as f32,
                origin.1 as f32,
                extent.0 as f32,
                extent.1 as f32,
            );
            canvas.restore();
        }
        true
    }

    /// Draw the unfiltered capture as an opaque output base. This is valid
    /// only on a refresh frame whose capture covers the complete output.
    pub(super) fn draw_capture_opaque(
        &self,
        canvas: &flux::Canvas,
        frame: &flux::Frame<'_>,
        extent: (u32, u32),
    ) -> bool {
        let Some(capture) = self
            .captures
            .get(frame.index() as usize)
            .and_then(Option::as_ref)
        else {
            return false;
        };
        canvas.draw_image_opaque(&capture.image, 0.0, 0.0, extent.0 as f32, extent.1 as f32);
        true
    }

    /// Force every slot to rebuild on the next active backdrop frame. Used by
    /// overview and screenshot-freeze modes, which replace the sampled scene.
    pub(super) fn invalidate(&mut self) {
        for capture in self.captures.iter_mut().flatten() {
            capture.valid = false;
        }
        for pending in &mut self.source_slot_damage {
            *pending = FrameDamage::Full;
        }
    }

    /// Advance source-damage history only after the output was successfully
    /// presented, matching the lifetime of the corresponding frame slot.
    pub(super) fn record_present(&mut self, slot: usize, source_damage: FrameDamage) {
        record_composite_present(&mut self.source_slot_damage, slot, source_damage);
    }
}

/// Frozen desktop snapshot shown while the screenshot selector is open.
///
/// On the trigger frame the whole frame — desktop scene *and* chrome — is
/// rendered into a full-resolution offscreen image; every later frame
/// samples that image and renders only the selector on top, so the screen
/// keeps showing exactly the trigger frame until the user confirms or
/// cancels. Images are indexed by frame slot for the same in-flight reason
/// as [`LauncherBackdrop`]: a slot is rewritten only after `begin_frame`
/// has waited its fence.
///
/// Session flow: `ScreenshotFreeze::request_open` arms a session and
/// defers the selector opening; the capture frame renders the normal frame
/// into the target (the selector is not open yet, so its scrim stays out
/// of the snapshot), then the compositor opens the selector over the
/// frozen image. `ScreenshotFreeze::should_disarm` ends the session
/// once the selector closes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct CaptureCursorState {
    /// Logical output coordinates sampled when the screenshot was triggered.
    pub(super) position: (f32, f32),
    /// Effective compositor/theme cursor shape at the trigger instant.
    pub(super) shape: u32,
    /// Whether the compositor-owned theme cursor was hidden at the trigger
    /// instant. A client surface may still be the visible cursor.
    pub(super) hidden: bool,
    /// Whether the visible cursor came from a client-provided cursor surface.
    pub(super) client_surface: bool,
}

pub(super) struct ScreenshotFreeze {
    captures: Vec<Option<ScreenshotCapture>>,
    active_slot: Option<usize>,
    /// Logical cursor state sampled synchronously with the trigger, before a
    /// later input batch or capture frame can move it.
    trigger_cursor: Option<CaptureCursorState>,
    /// The compositor-owned cursor as it appeared on the trigger frame.
    /// Client cursor surfaces are already part of `captures`; this is the
    /// themed cursor used by nested backends and direct KMS.
    cursor: Option<CaptureCursor>,
    /// A freeze session is in progress (requested, capturing, or frozen).
    pub(super) armed: bool,
    /// The snapshot holds the trigger frame.
    pub(super) captured: bool,
    /// Capture failed; the session degrades to the live scene.
    pub(super) failed: bool,
    /// The selector opens once the capture frame has been rendered.
    pub(super) pending_open: bool,
    /// The selector was opened under this session.
    pub(super) opened: bool,
}

impl ScreenshotFreeze {
    pub(super) fn new() -> Self {
        Self {
            captures: Vec::new(),
            active_slot: None,
            trigger_cursor: None,
            cursor: None,
            armed: false,
            captured: false,
            failed: false,
            pending_open: false,
            opened: false,
        }
    }

    /// Arm a freeze session; the selector opens after the capture frame.
    pub(super) fn request_open(&mut self, cursor: Option<CaptureCursorState>) {
        if self.armed {
            return;
        }
        self.armed = true;
        self.captured = false;
        self.failed = false;
        self.pending_open = true;
        self.opened = false;
        self.active_slot = None;
        self.trigger_cursor = cursor;
        self.cursor = None;
    }

    /// Whether this frame must render the scene into the snapshot target.
    pub(super) fn needs_capture(&self) -> bool {
        self.armed && !self.captured && !self.failed
    }

    /// Whether the frozen snapshot replaces the live scene this frame.
    pub(super) fn active(&self) -> bool {
        self.armed && self.captured && !self.failed
    }

    pub(super) fn mark_captured(&mut self, frame: &flux::Frame<'_>, cursor: Option<CaptureCursor>) {
        self.active_slot = Some(frame.index() as usize);
        self.cursor = cursor;
        self.captured = true;
    }

    pub(super) fn cursor(&self) -> Option<&CaptureCursor> {
        self.cursor.as_ref()
    }

    pub(super) fn trigger_cursor(&self) -> Option<CaptureCursorState> {
        self.trigger_cursor
    }

    pub(super) fn mark_opened(&mut self) {
        self.pending_open = false;
        self.opened = true;
    }

    /// The selector closed (confirmed or cancelled); the frame that closed
    /// it still presents the snapshot, so live rendering resumes next frame.
    pub(super) fn should_disarm(&self, selector_active: bool) -> bool {
        self.armed && self.opened && !selector_active
    }

    pub(super) fn disarm(&mut self) {
        self.armed = false;
        self.captured = false;
        self.failed = false;
        self.pending_open = false;
        self.opened = false;
        self.active_slot = None;
        self.trigger_cursor = None;
        self.cursor = None;
    }

    /// The snapshot image, once the trigger frame has been captured.
    pub(super) fn image(&self) -> Option<&flux::Image> {
        if !self.captured {
            return None;
        }
        self.captures
            .get(self.active_slot?)?
            .as_ref()
            .map(|capture| &capture.image)
    }

    /// This frame's snapshot target, after [`ensure_target`](Self::ensure_target)
    /// succeeded.
    pub(super) fn target(&self, frame: &flux::Frame<'_>) -> Option<&flux::Image> {
        self.captures
            .get(frame.index() as usize)?
            .as_ref()
            .map(|capture| &capture.image)
    }

    /// Allocate (or reuse) this frame slot's full-resolution snapshot target.
    pub(super) fn ensure_target(
        &mut self,
        device: &flux::Device,
        surface: &flux::Surface,
        frame: &flux::Frame<'_>,
        surface_size: (u32, u32),
    ) -> bool {
        let format = match surface.format() {
            flux::Format::FLUX_FORMAT_RGBA8_UNORM | flux::Format::FLUX_FORMAT_BGRA8_UNORM => {
                flux::Format::FLUX_FORMAT_RGBA8_UNORM
            }
            other => {
                log::warn!(
                    "screenshot: frame freeze unavailable for surface format {other:?}; falling back to live scene"
                );
                return false;
            }
        };
        if surface_size.0 == 0 || surface_size.1 == 0 {
            return false;
        }

        let slot = frame.index() as usize;
        if self.captures.len() <= slot {
            self.captures.resize_with(slot + 1, || None);
        }
        let stale = self.captures[slot]
            .as_ref()
            .is_none_or(|capture| capture.size != surface_size || capture.format != format);
        if stale {
            match flux::Image::render_target(device, surface_size.0, surface_size.1, format) {
                Ok(image) => {
                    self.captures[slot] = Some(ScreenshotCapture {
                        image,
                        size: surface_size,
                        format,
                    });
                }
                Err(error) => {
                    log::warn!(
                        "screenshot: failed to allocate freeze target ({error}); falling back to live scene"
                    );
                    return false;
                }
            }
        }
        true
    }
}

pub(super) fn draw_wallpaper_background(
    canvas: &flux::Canvas,
    device: &flux::Device,
    wallpaper: &mut Option<aegis_wallpaper::Wallpaper>,
    logical_size: (u32, u32),
    scale: f32,
) {
    canvas.save();
    if scale != 1.0 {
        canvas.scale(scale, scale);
    }
    if let Some(wallpaper) = wallpaper.as_mut() {
        wallpaper.draw(device, canvas, logical_size.0 as f32, logical_size.1 as f32);
    }
    canvas.restore();
}

pub(super) fn draw_client_scene(
    canvas: &flux::Canvas,
    device: &flux::Device,
    renderer: &mut aegis_render::Renderer,
    server: &aegis_compositor::Server,
    scale: f32,
) {
    canvas.save();
    if scale != 1.0 {
        canvas.scale(scale, scale);
    }
    let shm = server.client_surface_frames();
    let dmabuf = server.client_surface_dmabuf_frames();
    let surface_order = server.client_surface_frame_order();
    let overlay_shm = server.overlay_frames();
    let overlay_dmabuf = server.overlay_dmabuf_frames();
    let windows = server.render_windows();
    renderer.gc(shm
        .iter()
        .map(|frame| frame.id)
        .chain(dmabuf.iter().map(|frame| frame.id))
        .chain(overlay_shm.iter().map(|frame| frame.id))
        .chain(overlay_dmabuf.iter().map(|frame| frame.id)));
    if let Some(slide) = server.workspace_slide_presentation() {
        let output = slide.output;
        let animated_windows = slide
            .layers
            .iter()
            .flat_map(|layer| layer.windows.iter().copied())
            .collect::<std::collections::HashSet<_>>();
        let static_windows = shm
            .iter()
            .filter_map(|frame| frame.window)
            .chain(dmabuf.iter().filter_map(|frame| frame.window))
            .filter(|window| !animated_windows.contains(window))
            .collect::<std::collections::HashSet<_>>();
        if !static_windows.is_empty() {
            renderer.draw_workspace_surface_layer(
                device,
                canvas,
                &surface_order,
                &shm,
                &dmabuf,
                aegis_render::WorkspaceSurfaceLayer::new(&windows, &static_windows),
            );
        }
        for layer in slide.layers {
            let window_filter = layer
                .windows
                .into_iter()
                .collect::<std::collections::HashSet<_>>();
            canvas.save();
            // The page clip moves with the page, while the output framebuffer
            // supplies the final viewport clip. Adjacent pages therefore meet
            // at one exact edge and can never paint over one another.
            canvas.clip_rect(
                output.origin.x as f32 + layer.offset_x,
                output.origin.y as f32,
                output.size.w as f32,
                output.size.h as f32,
            );
            canvas.translate(layer.offset_x, 0.0);
            renderer.draw_workspace_surface_layer(
                device,
                canvas,
                &surface_order,
                &shm,
                &dmabuf,
                aegis_render::WorkspaceSurfaceLayer::new(&windows, &window_filter),
            );
            canvas.restore();
        }
    } else {
        renderer.draw_surfaces_ordered_with_window_shadows(
            device,
            canvas,
            &surface_order,
            &shm,
            &dmabuf,
            &windows,
        );
    }
    canvas.restore();
}

/// Input-method popups, drag icons, and client cursor surfaces are protocol
/// overlays, not ordinary client content. Draw them after compositor chrome
/// so a Dock or status bar cannot cover the candidate panel or cursor.
pub(super) fn draw_client_overlays(
    canvas: &flux::Canvas,
    device: &flux::Device,
    renderer: &mut aegis_render::Renderer,
    server: &aegis_compositor::Server,
    scale: f32,
    include_cursor: bool,
    cursor_position: Option<(f32, f32)>,
) {
    canvas.save();
    if scale != 1.0 {
        canvas.scale(scale, scale);
    }
    let overlay_shm = server.overlay_frames_with_cursor_at(include_cursor, cursor_position);
    let overlay_dmabuf =
        server.overlay_dmabuf_frames_with_cursor_at(include_cursor, cursor_position);
    renderer.draw_toplevels(device, canvas, &overlay_shm, (0.0, 0.0));
    renderer.draw_dmabuf_toplevels(device, canvas, &overlay_dmabuf, (0.0, 0.0));
    canvas.restore();
}

/// Fail-closed session-lock composition. The opaque physical-pixel fill is
/// emitted after all normal desktop/chrome work, then only surfaces owned by
/// the active lock client (plus its cursor) are allowed above it.
pub(super) fn draw_lock_scene(
    canvas: &flux::Canvas,
    device: &flux::Device,
    renderer: &mut aegis_render::Renderer,
    server: &aegis_compositor::Server,
    physical_size: (u32, u32),
    scale: f32,
) {
    canvas.save();
    canvas.fill_rect(
        0.0,
        0.0,
        physical_size.0 as f32,
        physical_size.1 as f32,
        flux::rgba(0, 0, 0, 255),
    );
    canvas.restore();

    canvas.save();
    if scale != 1.0 {
        canvas.scale(scale, scale);
    }
    let shm = server.lock_frames();
    let dmabuf = server.lock_dmabuf_frames();
    renderer.gc(shm
        .iter()
        .map(|frame| frame.id)
        .chain(dmabuf.iter().map(|frame| frame.id)));
    renderer.draw_toplevels(device, canvas, &shm, (0.0, 0.0));
    renderer.draw_dmabuf_toplevels(device, canvas, &dmabuf, (0.0, 0.0));
    canvas.restore();
}

/// Overview scene (M9): the desktop dimmed, then every visible window drawn
/// as a live thumbnail on the shared `aegis_core::overview` grid — the exact
/// geometry the overview chrome uses for its frames, labels, and hit-testing.
/// Z-order is preserved bottom-to-top so overlapping thumbnails read like
/// the desktop stack.
pub(super) fn draw_overview_scene(
    canvas: &flux::Canvas,
    device: &flux::Device,
    renderer: &mut aegis_render::Renderer,
    server: &aegis_compositor::Server,
    logical_size: (u32, u32),
    scale: f32,
) {
    canvas.save();
    canvas.fill_rect(
        0.0,
        0.0,
        logical_size.0 as f32 * scale,
        logical_size.1 as f32 * scale,
        flux::rgba(8, 10, 20, 200),
    );
    canvas.restore();

    let windows = server.windows();
    if windows.is_empty() {
        return;
    }
    let rail = server
        .workspace_snapshot()
        .outputs
        .first()
        .map(|o| o.workspaces.len() > 1)
        .unwrap_or(false);
    let interaction_domain_shelf = server
        .interaction_domain_snapshot()
        .interaction_domains
        .iter()
        .any(|interaction_domain| {
            interaction_domain.kind == aegis_core::interaction_domain::InteractionDomainKind::Agent
                && interaction_domain.state
                    != aegis_core::interaction_domain::InteractionDomainState::Revoked
        });
    let display = aegis_core::Rect::new(0, 0, logical_size.0 as i32, logical_size.1 as i32);
    let area = aegis_core::overview::grid_area_with_interaction_domain_shelf(
        display,
        rail,
        interaction_domain_shelf,
    );
    let slots = aegis_core::overview::grid(area, windows.len());
    let cells: std::collections::HashMap<
        aegis_core::window::WindowId,
        (aegis_core::Rect, aegis_core::Point, aegis_core::Size),
    > = windows
        .iter()
        .zip(slots.iter())
        .map(|(w, slot)| {
            (
                w.id,
                (aegis_core::overview::fit(*slot, w.size), w.position, w.size),
            )
        })
        .collect();
    let map = move |window: Option<aegis_core::window::WindowId>, natural: aegis_core::Rect| {
        let Some((cell, base, win_size)) = window.and_then(|id| cells.get(&id)) else {
            return natural;
        };
        let k = cell.size.w as f32 / win_size.w.max(1) as f32;
        let remap = |v: i32, b: i32| (v - b) as f32 * k;
        aegis_core::Rect::new(
            cell.origin.x + remap(natural.origin.x, base.x).round() as i32,
            cell.origin.y + remap(natural.origin.y, base.y).round() as i32,
            (natural.size.w as f32 * k).round().max(1.0) as i32,
            (natural.size.h as f32 * k).round().max(1.0) as i32,
        )
    };

    canvas.save();
    if scale != 1.0 {
        canvas.scale(scale, scale);
    }
    let shm = server.client_surface_frames();
    let dmabuf = server.client_surface_dmabuf_frames();
    let surface_order = server.client_surface_frame_order();
    renderer.gc(shm
        .iter()
        .map(|frame| frame.id)
        .chain(dmabuf.iter().map(|frame| frame.id)));
    renderer.draw_surfaces_ordered_mapped(device, canvas, &surface_order, &shm, &dmabuf, &map);
    canvas.restore();
}

/// Super+Tab scene: preserve the live desktop underneath a dim scrim, then
/// paint every visible window again into the shared horizontal preview strip.
/// Shell chrome draws labels and the selected-card border over these targets.
pub(super) fn draw_window_switcher_scene(
    canvas: &flux::Canvas,
    device: &flux::Device,
    renderer: &mut aegis_render::Renderer,
    server: &aegis_compositor::Server,
    logical_size: (u32, u32),
    scale: f32,
    presentation: &aegis_shell::WindowSwitcherPresentation,
) {
    let scrim_alpha = (145.0 * presentation.visibility.clamp(0.0, 1.0)).round() as u8;
    canvas.save();
    canvas.fill_rect(
        0.0,
        0.0,
        logical_size.0 as f32 * scale,
        logical_size.1 as f32 * scale,
        flux::rgba(5, 7, 12, scrim_alpha),
    );
    canvas.restore();

    let windows = server.windows();
    if windows.is_empty() {
        return;
    }
    let cells: std::collections::HashMap<
        aegis_core::window::WindowId,
        (aegis_core::Rect, aegis_core::Point, aegis_core::Size),
    > = presentation
        .cards
        .iter()
        .filter_map(|card| {
            let window = windows.iter().find(|window| window.id == card.window)?;
            Some((
                card.window,
                (
                    aegis_core::overview::fit(card.geometry.preview, window.size),
                    window.position,
                    window.size,
                ),
            ))
        })
        .collect();
    let map = move |window: Option<aegis_core::window::WindowId>, natural: aegis_core::Rect| {
        let Some((cell, base, window_size)) = window.and_then(|id| cells.get(&id)) else {
            return natural;
        };
        let scale = (cell.size.w as f32 / window_size.w.max(1) as f32)
            .min(cell.size.h as f32 / window_size.h.max(1) as f32);
        let remap = |value: i32, origin: i32| (value - origin) as f32 * scale;
        aegis_core::Rect::new(
            cell.origin.x + remap(natural.origin.x, base.x).round() as i32,
            cell.origin.y + remap(natural.origin.y, base.y).round() as i32,
            (natural.size.w as f32 * scale).round().max(1.0) as i32,
            (natural.size.h as f32 * scale).round().max(1.0) as i32,
        )
    };

    canvas.save();
    if scale != 1.0 {
        canvas.scale(scale, scale);
    }
    canvas.clip_rect(
        presentation.panel.origin.x as f32,
        presentation.panel.origin.y as f32,
        presentation.panel.size.w as f32,
        presentation.panel.size.h as f32,
    );
    let shm = server.client_surface_frames();
    let dmabuf = server.client_surface_dmabuf_frames();
    let surface_order = server.client_surface_frame_order();
    renderer.gc(shm
        .iter()
        .map(|frame| frame.id)
        .chain(dmabuf.iter().map(|frame| frame.id)));
    renderer.draw_surfaces_ordered_mapped_with_opacity(
        device,
        canvas,
        &surface_order,
        &shm,
        &dmabuf,
        &map,
        presentation.visibility,
    );
    canvas.restore();
}

/// Draw compositor-owned live-preview popovers contributed by ordinary shell
/// chrome. Each card gets its own clip and mapping pass so a window's
/// subsurfaces cannot bleed into a neighbouring card and unrelated windows
/// are never redrawn over the popover.
pub(super) fn draw_live_preview_scenes(
    canvas: &flux::Canvas,
    device: &flux::Device,
    renderer: &mut aegis_render::Renderer,
    server: &aegis_compositor::Server,
    scale: f32,
    presentations: &[aegis_shell::LivePreviewPresentation],
) {
    if presentations.is_empty() {
        return;
    }
    let windows = server.windows();
    let shm = server.client_surface_frames();
    let dmabuf = server.client_surface_dmabuf_frames();
    let surface_order = server.client_surface_frame_order();
    renderer.gc(shm
        .iter()
        .map(|frame| frame.id)
        .chain(dmabuf.iter().map(|frame| frame.id)));

    canvas.save();
    if scale != 1.0 {
        canvas.scale(scale, scale);
    }
    for presentation in presentations {
        for card in &presentation.cards {
            let Some(window) = windows.iter().find(|window| window.id == card.window) else {
                continue;
            };
            let cell = aegis_core::overview::fit(card.geometry.preview, window.size);
            let base = window.position;
            let window_size = window.size;
            let target = window.id;
            let map = move |id: Option<aegis_core::window::WindowId>, natural: aegis_core::Rect| {
                if id != Some(target) {
                    return aegis_core::Rect::new(-100_000, -100_000, 1, 1);
                }
                let factor = (cell.size.w as f32 / window_size.w.max(1) as f32)
                    .min(cell.size.h as f32 / window_size.h.max(1) as f32);
                let remap = |value: i32, origin: i32| (value - origin) as f32 * factor;
                aegis_core::Rect::new(
                    cell.origin.x + remap(natural.origin.x, base.x).round() as i32,
                    cell.origin.y + remap(natural.origin.y, base.y).round() as i32,
                    (natural.size.w as f32 * factor).round().max(1.0) as i32,
                    (natural.size.h as f32 * factor).round().max(1.0) as i32,
                )
            };
            canvas.save();
            canvas.clip_rect(
                card.geometry.preview.origin.x as f32,
                card.geometry.preview.origin.y as f32,
                card.geometry.preview.size.w as f32,
                card.geometry.preview.size.h as f32,
            );
            renderer.draw_surfaces_ordered_mapped(
                device,
                canvas,
                &surface_order,
                &shm,
                &dmabuf,
                &map,
            );
            canvas.restore();
        }
    }
    canvas.restore();
}

/// Software cursor for direct KMS, sourced exclusively from the XDG cursor
/// theme (`$XCURSOR_THEME`/`$XCURSOR_SIZE`, inheritance included) via
/// [`cursor::CursorCache`].
/// Client-provided cursor surfaces are already composited by `aegis-server`;
/// this covers the cursor-shape protocol and compositor-owned cursors.
pub(super) fn draw_software_cursor(
    canvas: &flux::Canvas,
    device: &flux::Device,
    cache: &mut cursor::CursorCache,
    position: (f32, f32),
    shape: u32,
    scale: f32,
) {
    if let Some(cursor) = cache.get(device, shape, scale) {
        canvas.save();
        canvas.scale(scale, scale);
        let inv = 1.0 / scale.max(0.25);
        canvas.draw_image(
            &cursor.image,
            position.0 - cursor.xhot * inv,
            position.1 - cursor.yhot * inv,
            cursor.width * inv,
            cursor.height * inv,
        );
        canvas.restore();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn region(x: f32, y: f32, w: f32, h: f32) -> aegis_shell::BackdropRegion {
        aegis_shell::BackdropRegion { x, y, w, h }
    }

    #[test]
    fn backdrop_refresh_is_driven_by_source_footprint() {
        let input = [
            BackdropCaptureRegion {
                origin: (0, 0),
                extent: (1920, 80),
            },
            BackdropCaptureRegion {
                origin: (0, 1000),
                extent: (1920, 80),
            },
        ];
        let video_above_dock = FrameDamage::Area(vec![aegis_core::Rect::new(100, 100, 800, 450)]);
        assert!(backdrop_refresh_regions(true, false, &video_above_dock, &input,).is_empty());
        let video_under_dock = FrameDamage::Area(vec![aegis_core::Rect::new(100, 1020, 800, 60)]);
        assert_eq!(
            backdrop_refresh_regions(true, false, &video_under_dock, &input),
            vec![input[1]]
        );
        assert_eq!(
            backdrop_refresh_regions(false, false, &FrameDamage::None, &input),
            input
        );
        assert_eq!(
            backdrop_refresh_regions(true, true, &FrameDamage::None, &input),
            input
        );
    }

    #[test]
    fn backdrop_cache_key_tracks_geometry_and_material_exactly() {
        let capture = [BackdropCaptureRegion {
            origin: (0, 1000),
            extent: (1920, 80),
        }];
        let frost = [region(0.0, 1040.0, 1920.0, 40.0)];
        let base = BackdropCacheKey::new(
            (0, 1000),
            (1920, 80),
            (1920, 1080),
            12.0,
            1.0,
            false,
            &capture,
            &frost,
            &[],
            None,
            &[],
        );
        let sigma_changed = BackdropCacheKey::new(
            (0, 1000),
            (1920, 80),
            (1920, 1080),
            12.5,
            1.0,
            false,
            &capture,
            &frost,
            &[],
            None,
            &[],
        );
        assert_ne!(base, sigma_changed);
        assert_eq!(base, base.clone());
    }

    #[test]
    fn screenshot_freeze_keeps_the_trigger_cursor_snapshot() {
        let trigger = CaptureCursorState {
            position: (42.25, 73.5),
            shape: 7,
            hidden: true,
            client_surface: true,
        };
        let later = CaptureCursorState {
            position: (900.0, 500.0),
            shape: 1,
            hidden: true,
            client_surface: false,
        };
        let mut freeze = ScreenshotFreeze::new();

        freeze.request_open(Some(trigger));
        freeze.request_open(Some(later));
        assert_eq!(freeze.trigger_cursor(), Some(trigger));

        freeze.disarm();
        assert_eq!(freeze.trigger_cursor(), None);
    }

    #[test]
    fn capture_bounds_cover_top_bar_with_blur_margin() {
        // 32px status bar at the top of a 1920x1080 output, sigma 12: the
        // capture spans the full width but only the bar plus the 3σ margin.
        let (origin, size) = blur_capture_bounds(
            &[region(0.0, 0.0, 1920.0, 32.0)],
            (1920, 1080),
            (1920, 1080),
            1.0,
            12.0,
        );
        assert_eq!(origin, (0, 0));
        assert_eq!(size, (1920, 68));
    }

    #[test]
    fn capture_bounds_union_disjoint_regions() {
        // Top bar + bottom dock: the union covers both, including margins.
        let (origin, size) = blur_capture_bounds(
            &[
                region(0.0, 0.0, 1920.0, 32.0),
                region(400.0, 1040.0, 1120.0, 40.0),
            ],
            (1920, 1080),
            (1920, 1080),
            1.0,
            12.0,
        );
        assert_eq!(origin, (0, 0));
        assert_eq!(size, (1920, 1080));
    }

    #[test]
    fn capture_regions_keep_top_bar_and_bottom_dock_disjoint() {
        let regions = blur_capture_regions(
            &[
                region(0.0, 0.0, 1920.0, 32.0),
                region(400.0, 1040.0, 1120.0, 40.0),
            ],
            (1920, 1080),
            (1920, 1080),
            1.0,
            12.0,
        );
        assert_eq!(
            regions,
            vec![
                BackdropCaptureRegion {
                    origin: (0, 0),
                    extent: (1920, 68),
                },
                BackdropCaptureRegion {
                    origin: (364, 1004),
                    extent: (1192, 76),
                },
            ]
        );
    }

    #[test]
    fn capture_regions_merge_overlapping_blur_footprints_transitively() {
        let regions = blur_capture_regions(
            &[
                region(100.0, 100.0, 40.0, 40.0),
                region(170.0, 100.0, 40.0, 40.0),
                region(240.0, 100.0, 40.0, 40.0),
            ],
            (400, 300),
            (400, 300),
            1.0,
            12.0,
        );
        assert_eq!(
            regions,
            vec![BackdropCaptureRegion {
                origin: (64, 64),
                extent: (252, 112),
            }]
        );
    }

    #[test]
    fn capture_regions_map_outward_into_downsampled_target() {
        let mapped = blur_regions_in_capture(
            &[BackdropCaptureRegion {
                origin: (101, 121),
                extent: (81, 41),
            }],
            (50, 100),
            (400, 200),
            (200, 100),
        );
        assert_eq!(
            mapped,
            vec![flux::BlurRegion {
                x: 25,
                y: 10,
                width: 41,
                height: 21,
            }]
        );
    }

    #[test]
    #[allow(clippy::modulo_one)]
    fn capture_bounds_align_to_downsample() {
        // A floating region: origin/size land on BACKDROP_DOWNSAMPLE
        // multiples so the capture grid stays exact. With a full-resolution
        // capture the bounds are the exact padded region union.
        let (origin, size) = blur_capture_bounds(
            &[region(100.0, 100.0, 200.0, 50.0)],
            (1920, 1080),
            (1920, 1080),
            1.0,
            12.0,
        );
        assert_eq!(origin, (64, 64));
        assert_eq!(size, (272, 122));
        assert_eq!(origin.0 % BACKDROP_DOWNSAMPLE, 0);
        assert_eq!(origin.1 % BACKDROP_DOWNSAMPLE, 0);
        assert_eq!(size.0 % BACKDROP_DOWNSAMPLE, 0);
        assert_eq!(size.1 % BACKDROP_DOWNSAMPLE, 0);
    }

    #[test]
    fn capture_bounds_respect_output_scale() {
        // scale=2: regions are logical, bounds physical (16 logical px bar
        // = 32 physical px; margin is 3σ in physical pixels).
        let (origin, size) = blur_capture_bounds(
            &[region(0.0, 0.0, 960.0, 16.0)],
            (960, 540),
            (1920, 1080),
            2.0,
            12.0,
        );
        assert_eq!(origin, (0, 0));
        assert_eq!(size, (1920, 104));
    }

    #[test]
    fn liquid_glass_geometry_maps_to_the_downsampled_capture_once() {
        let source = aegis_shell::LiquidGlassRegion {
            bounds: region(400.0, 100.0, 320.0, 74.0),
            corner_radius: 18.0,
            opacity: 0.4,
            ..Default::default()
        };
        // Output scale 2, capture downsample 1/2, physical capture origin
        // (600, 120): capture coordinates are logical*1 - origin*0.5.
        let groups = liquid_glass_groups(&[source], (600, 120), 2.0, 0.5);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].primary.x, 100.0);
        assert_eq!(groups[0].primary.y, 40.0);
        assert_eq!(groups[0].primary.width, 320.0);
        assert_eq!(groups[0].primary.height, 74.0);
        assert_eq!(groups[0].primary.corner_radius, 18.0);
        assert_eq!(groups[0].opacity, 0.4);
        assert!(groups[0].merged.is_none());
    }

    #[test]
    fn capture_bounds_clamp_to_physical_extent() {
        // Bottom-edge dock: the margin past the screen edge is clamped away.
        let (origin, size) = blur_capture_bounds(
            &[region(400.0, 1048.0, 1120.0, 32.0)],
            (1920, 1080),
            (1920, 1080),
            1.0,
            12.0,
        );
        assert_eq!(origin, (364, 1012));
        assert_eq!(size, (1192, 68));
    }

    #[test]
    fn capture_bounds_fall_back_to_full_frame_without_regions() {
        let (origin, size) = blur_capture_bounds(
            &[region(10.0, 10.0, 0.0, 0.0)],
            (1920, 1080),
            (1920, 1080),
            1.0,
            12.0,
        );
        assert_eq!(origin, (0, 0));
        assert_eq!(size, (1920, 1080));
    }

    #[test]
    fn opaque_frame_fill_replaces_undefined_and_previous_contents() {
        let Ok(device) = flux::Device::new(true, &[], &[], 0) else {
            return;
        };
        let size = (32, 24);
        let surface = flux::Surface::offscreen(&device, size.0, size.1).unwrap();
        let canvas = flux::Canvas::new(&surface).unwrap();

        for expected in [[13, 77, 191, 255], [211, 43, 29, 255]] {
            let frame = surface.begin_frame().unwrap();
            begin_opaque_frame(
                &canvas,
                &frame,
                flux::rgba(expected[0], expected[1], expected[2], expected[3]),
            )
            .unwrap();
            canvas.end_checked().unwrap();
            frame.submit().unwrap().present().unwrap();

            let mut pixels = vec![0; size.0 as usize * size.1 as usize * 4];
            surface.read_pixels(&mut pixels).unwrap();
            assert!(
                pixels
                    .chunks_exact(4)
                    .all(|pixel| pixel == expected.as_slice()),
                "opaque fill did not replace every output pixel"
            );
        }
    }

    #[test]
    fn damaged_base_and_stencil_overlay_preserve_pixels_outside_the_scissor() {
        let Ok(device) = flux::Device::new(true, &[], &[], 1) else {
            return;
        };
        let size = (32, 24);
        let surface = flux::Surface::offscreen(&device, size.0, size.1).unwrap();
        let canvas = flux::Canvas::new(&surface).unwrap();

        let frame = surface.begin_frame().unwrap();
        begin_opaque_frame(&canvas, &frame, flux::rgba(200, 30, 20, 255)).unwrap();
        canvas.end_checked().unwrap();
        frame.submit().unwrap().present().unwrap();

        let frame = surface.begin_frame().unwrap();
        let repaint = FrameDamage::Area(vec![aegis_core::Rect::new(8, 6, 10, 9)]);
        begin_opaque_frame_repaint(
            &canvas,
            &frame,
            size,
            flux::rgba(10, 80, 220, 255),
            &repaint,
        )
        .unwrap();
        // End the optimized image/base pass, then exercise the exact pass
        // boundary used before Lens. A self-intersecting fill is intentional:
        // Flux rejects it from a no-stencil pass, so successful checked end
        // proves the overlay really has stencil rather than merely being a
        // second no-stencil LOAD pass.
        canvas.end_checked().unwrap();
        begin_stencil_frame_overlay(&canvas, &frame, frame_damage_render_area(&repaint)).unwrap();
        let arena = flux::Arena::with_capacity(4096).unwrap();
        let path = flux::Path::new(&arena).unwrap();
        path.move_to(11.0, 8.0)
            .line_to(15.0, 12.0)
            .line_to(11.0, 12.0)
            .line_to(15.0, 8.0)
            .close();
        canvas.fill_path(&path, &flux::Paint::solid(flux::rgba(20, 220, 70, 255)));
        canvas.end_checked().unwrap();
        frame.submit().unwrap().present().unwrap();

        let mut pixels = vec![0; size.0 as usize * size.1 as usize * 4];
        surface.read_pixels(&mut pixels).unwrap();
        let pixel = |x: usize, y: usize| &pixels[(y * size.0 as usize + x) * 4..][..4];
        assert_eq!(pixel(0, 0), [200, 30, 20, 255]);
        assert_eq!(pixel(9, 7), [10, 80, 220, 255]);
        assert_eq!(pixel(17, 14), [10, 80, 220, 255]);
        assert_eq!(pixel(18, 15), [200, 30, 20, 255]);
    }

    #[test]
    fn frame_damage_render_area_uses_the_exact_union() {
        let damage = FrameDamage::Area(vec![
            aegis_core::Rect::new(11, 7, 5, 9),
            aegis_core::Rect::new(29, 3, 4, 8),
        ]);
        assert_eq!(
            frame_damage_render_area(&damage),
            Some(flux::CanvasRenderArea {
                x: 11,
                y: 3,
                width: 22,
                height: 13,
            })
        );
        assert_eq!(frame_damage_render_area(&FrameDamage::Full), None);
        assert_eq!(frame_damage_render_area(&FrameDamage::None), None);
    }
}
