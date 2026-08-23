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

/// A region submits a prism group only when it has area and is visible; the
/// same predicate gates the submitted-id bookkeeping that aligns the
/// frame-lagged backdrop statistics with their bodies.
pub(super) fn glass_region_active(region: &aegis_shell::LiquidGlassRegion) -> bool {
    region.bounds.w > 0.0 && region.bounds.h > 0.0 && region.opacity > 0.0
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
    scheme: aegis_model::settings::ColorScheme,
) -> Vec<prism::LiquidGlassGroup> {
    let capture_scale = scale * capture_ratio;
    regions
        .iter()
        .filter(|region| glass_region_active(region))
        .map(|region| prism::LiquidGlassGroup {
            primary: prism::LiquidGlassShape {
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
            tint_color: glass_tint(scheme),
            // Reference-recipe bodies leave the descriptor defaults alone;
            // only legs carrying a role override or an adaptation writeback
            // pin per-group values.
            frost_strength: (region.frost_strength != 1.0).then_some(region.frost_strength),
            tint_strength: (region.tint_strength != 1.0).then_some(region.tint_strength),
            saturation: (region.saturation != 1.0).then_some(region.saturation),
            plate_polarity: (region.plate_polarity >= 0.0).then_some(region.plate_polarity),
            backdrop_energy: region
                .adaptation
                .map(|adaptation| adaptation.backdrop_energy),
            focus: region.focus.map(|focus| prism::LiquidGlassFocus {
                shape: prism::LiquidGlassShape {
                    x: focus.bounds.x * capture_scale - capture_origin.0 as f32 * capture_ratio,
                    y: focus.bounds.y * capture_scale - capture_origin.1 as f32 * capture_ratio,
                    width: focus.bounds.w * capture_scale,
                    height: focus.bounds.h * capture_scale,
                    corner_radius: focus.corner_radius * capture_scale,
                },
                strength: focus.strength.clamp(0.0, 1.0),
            }),
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

/// Exact backdrop capture configuration. Float values are stored as IEEE bit
/// patterns so equality is collision-free. A capture-side change invalidates
/// all per-slot effect caches and re-renders the scene into the capture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct BackdropCacheKey {
    capture_origin: (u32, u32),
    capture_extent: (u32, u32),
    physical_size: (u32, u32),
    sigma: u32,
    scale: u32,
    model_active: bool,
    capture_regions: Vec<BackdropCaptureRegion>,
    scene_overlays: Vec<u64>,
}

/// Effect-side backdrop configuration: every frost/liquid parameter that
/// changes only the composite built *from* the capture — region geometry,
/// opacity fades, shadows, focus fields, per-role material strengths and
/// the adaptation writeback. A material-only change re-runs blur + glass +
/// composite over the still-valid capture instead of re-rendering the scene,
/// so an animated tooltip fade or an adaptation step never churns clients.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct BackdropMaterialKey {
    frost_regions: Vec<[u32; 4]>,
    liquid_regions: Vec<[u32; 23]>,
    glass_tint: [u8; 3],
}

impl BackdropMaterialKey {
    pub(super) fn new(
        frost_regions: &[aegis_shell::BackdropRegion],
        liquid_regions: &[aegis_shell::LiquidGlassRegion],
        glass_tint: [u8; 3],
    ) -> Self {
        Self {
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
                    let focus = region.focus.unwrap_or_default();
                    let adaptation = region.adaptation.unwrap_or_default();
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
                        u32::from(region.focus.is_some()),
                        focus.bounds.x.to_bits(),
                        focus.bounds.y.to_bits(),
                        focus.bounds.w.to_bits(),
                        focus.bounds.h.to_bits(),
                        focus.corner_radius.to_bits(),
                        focus.strength.to_bits(),
                        region.frost_strength.to_bits(),
                        region.tint_strength.to_bits(),
                        region.saturation.to_bits(),
                        region.plate_polarity.to_bits(),
                        u32::from(region.adaptation.is_some()),
                        adaptation.plate_luminance.to_bits(),
                        adaptation.backdrop_energy.to_bits(),
                    ]
                })
                .collect(),
            glass_tint,
        }
    }
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
        window_switcher: Option<&aegis_shell::WindowSwitcherPresentation>,
    ) -> Self {
        fn push_rect(out: &mut Vec<u64>, rect: aegis_model::Rect) {
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
            scene_overlays.push(u64::from(switcher.selected.is_some()));
            scene_overlays.push(switcher.selected.map_or(0, |id| id.0));
            scene_overlays.push(switcher.inactive_content_brightness.to_bits() as u64);
            push_rect(&mut scene_overlays, switcher.panel);
            scene_overlays.push(switcher.cards.len() as u64);
            for card in &switcher.cards {
                scene_overlays.push(card.window.0);
                push_rect(&mut scene_overlays, card.geometry.preview);
                scene_overlays.push(card.corner_radius.to_bits() as u64);
            }
        } else {
            scene_overlays.push(0);
        }
        Self {
            capture_origin,
            capture_extent,
            physical_size,
            sigma: sigma.to_bits(),
            scale: scale.to_bits(),
            model_active,
            capture_regions: capture_regions.to_vec(),
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
    glass: prism::LiquidGlassFilter,
    captures: Vec<Option<BackdropCapture>>,
    /// Source damage missed by each effect-cache slot while another slot was
    /// presented. This mirrors swapchain damage history but is deliberately
    /// independent from output/chrome damage.
    source_slot_damage: Vec<FrameDamage>,
    config: Option<BackdropCacheKey>,
    /// Effect-side fingerprint **per frame slot**. The composite image the
    /// fingerprint describes is itself per slot (`BackdropCapture::composite`),
    /// so a material-only change must dirty each slot independently: a
    /// `Recompute` rewrites only the recording slot, and the other in-flight
    /// slots keep serving their previous composite until each in turn
    /// observes the new key. A single global fingerprint here would mark the
    /// change consumed after the first slot, leaving the rest presenting a
    /// stale shadow/glass composite on their next `Cached` frame — visible as
    /// the composite flickering between two versions every time the slots
    /// rotate.
    material: Vec<Option<BackdropMaterialKey>>,
    was_active: bool,
    failed_session: bool,
    unsupported: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum BackdropPlan {
    Direct,
    /// Capture the desktop footprint and rebuild this frame slot's effect.
    Refresh(Vec<BackdropCaptureRegion>),
    /// The capture is still valid; only the effect material changed. Rebuild
    /// this frame slot's composite from the existing capture image.
    Recompute,
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
            let input = aegis_model::Rect::new(
                region.origin.0 as i32,
                region.origin.1 as i32,
                region.extent.0 as i32,
                region.extent.1 as i32,
            );
            damage.iter().any(|dirty| dirty.intersect(input).is_some())
        })
        .collect()
}

/// Record `material` as the fingerprint this slot's composite will be built
/// with, returning whether the slot was previously serving a different one.
///
/// The fingerprint lifetime matches the composite image it describes — one
/// per frame slot — so a material change seen by one slot stays pending for
/// the other in-flight slots until each rebuilds its own composite. A single
/// shared fingerprint would mark the change consumed after the first slot
/// rebuilt, leaving the rest presenting a stale composite on their next
/// `Cached` frame — visible as the effect (notably the glass drop shadow)
/// flickering between two versions while the slots rotate.
pub(super) fn slot_material_changed(
    slots: &mut Vec<Option<BackdropMaterialKey>>,
    slot: usize,
    material: &BackdropMaterialKey,
) -> bool {
    if slots.len() <= slot {
        slots.resize_with(slot + 1, || None);
    }
    let changed = slots[slot].as_ref() != Some(material);
    slots[slot] = Some(material.clone());
    changed
}

/// A material change must rewrite the *entire* effect composite, not only the
/// source-damaged subset: undamaged regions would otherwise keep the previous
/// shadow/glass material indefinitely. The empty case passes through so the
/// planner can still emit `Recompute` (which already covers every capture
/// region).
pub(super) fn refresh_regions_covering_material_change(
    material_changed: bool,
    refresh_regions: Vec<BackdropCaptureRegion>,
    capture_regions: &[BackdropCaptureRegion],
) -> Vec<BackdropCaptureRegion> {
    if material_changed && !refresh_regions.is_empty() && refresh_regions != capture_regions {
        capture_regions.to_vec()
    } else {
        refresh_regions
    }
}

impl LauncherBackdrop {
    pub(super) fn new(device: &flux::Device) -> Result<Self, flux::Error> {
        Ok(Self {
            blur: flux::BlurFilter::new(device)?,
            glass: prism::LiquidGlassFilter::new(device)?,
            captures: Vec::new(),
            source_slot_damage: Vec::new(),
            config: None,
            material: Vec::new(),
            was_active: false,
            failed_session: false,
            unsupported: false,
        })
    }

    /// `extent` is the physical-pixel area the capture must cover (the blur
    /// regions' padded union, or the full surface for a live 3D wallpaper);
    /// the capture target is allocated at `extent / BACKDROP_DOWNSAMPLE`.
    /// With full-resolution captures the quotient is the extent itself.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn prepare(
        &mut self,
        active: bool,
        device: &flux::Device,
        surface: &flux::Surface,
        frame: &flux::Frame<'_>,
        config: BackdropCacheKey,
        material: BackdropMaterialKey,
        source_damage: &FrameDamage,
    ) -> BackdropPlan {
        if !active {
            self.was_active = false;
            self.failed_session = false;
            self.config = None;
            self.material.clear();
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

        let slot = frame.index() as usize;
        let material_changed = slot_material_changed(&mut self.material, slot, &material);
        if self.config.as_ref() != Some(&config) {
            self.config = Some(config.clone());
            for capture in self.captures.iter_mut().flatten() {
                capture.valid = false;
            }
            for pending in &mut self.source_slot_damage {
                *pending = FrameDamage::Full;
            }
        }
        self.material[slot] = Some(material);
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
        let refresh_regions = refresh_regions_covering_material_change(
            material_changed,
            refresh_regions,
            &config.capture_regions,
        );
        if refresh_regions.is_empty() {
            // The capture still holds the current scene; a material-only
            // change (fade, adaptation step, focus field) rebuilds just the
            // effect composite.
            if material_changed {
                BackdropPlan::Recompute
            } else {
                BackdropPlan::Cached
            }
        } else {
            // `finish_refresh` rebuilds every capture region's composite with
            // the current material, so the fingerprint written above is
            // already satisfied for this slot.
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
        glass_groups: &[prism::LiquidGlassGroup],
        glass_params: prism::LiquidGlassParams,
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
        // The capture pass sealed every refreshed region into this slot's
        // capture image, so the scene it holds is current: a full refresh
        // re-rendered all regions, and a partial refresh is only ever planned
        // over an already-valid capture. Mark it valid *before* rebuilding
        // the effects — `recompute_effects` refuses to run on a stale
        // capture, so a capture that just succeeded must establish validity
        // here or no capture ever could.
        if let Some(capture) = self.captures.get_mut(slot).and_then(Option::as_mut) {
            capture.valid = true;
        }
        self.recompute_effects(
            canvas,
            frame,
            sigma,
            blur_regions,
            frost_regions,
            all_backdrop_regions,
            glass_groups,
            glass_params,
        )
    }

    /// Rebuild this frame slot's effect composite from the still-valid
    /// capture image: blur, liquid glass, and the composite rewrite, with no
    /// preceding scene capture. This is the material-only counterpart of
    /// [`Self::finish_refresh`], entered without an open capture pass.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn recompute_effects(
        &mut self,
        canvas: &flux::Canvas,
        frame: &flux::Frame<'_>,
        sigma: f32,
        blur_regions: &[flux::BlurRegion],
        frost_regions: &[flux::BlurRegion],
        all_backdrop_regions: &[flux::BlurRegion],
        glass_groups: &[prism::LiquidGlassGroup],
        glass_params: prism::LiquidGlassParams,
    ) -> bool {
        let slot = frame.index() as usize;
        let Some(capture) = self.captures.get(slot).and_then(Option::as_ref) else {
            return false;
        };
        // A recompute only makes sense over a scene-capture that is still
        // current. The planner only emits `BackdropPlan::Recompute` for a
        // valid capture, and `finish_refresh` marks its just-sealed capture
        // valid before delegating here, so this guard can never fire on
        // either entry path; it exists only against future callers.
        if !capture.valid {
            return false;
        }
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

    /// Read the per-group backdrop statistics this frame slot last submitted
    /// (FLUX_MAX_FRAMES_IN_FLIGHT frames ago — the slot fence waited by
    /// `begin_frame` makes the mapped buffer stable while `frame` records).
    /// Group `i` of the returned stats aligns with group `i` of that
    /// submission; the caller keeps the id list to resolve identities.
    pub(super) fn glass_stats(
        &mut self,
        frame: &flux::Frame<'_>,
        out: &mut [prism::BackdropStats],
    ) -> Result<usize, flux::Error> {
        self.glass.stats(frame, out)
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
        self.material.clear();
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
    }

    /// Whether this frame must render the scene into the snapshot target.
    pub(super) fn needs_capture(&self) -> bool {
        self.armed && !self.captured && !self.failed
    }

    /// Whether the frozen snapshot replaces the live scene this frame.
    pub(super) fn active(&self) -> bool {
        self.armed && self.captured && !self.failed
    }

    pub(super) fn mark_captured(&mut self, frame: &flux::Frame<'_>) {
        self.active_slot = Some(frame.index() as usize);
        self.captured = true;
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
    retain_preview_sources: bool,
    soft_shadows: Option<&aegis_render::SoftShadowLayer<'_>>,
    shadow_style: aegis_model::window::WindowShadowStyle,
) {
    canvas.save();
    if scale != 1.0 {
        canvas.scale(scale, scale);
    }
    let (shm, dmabuf, surface_order) = if retain_preview_sources {
        (
            server.client_preview_surface_frames(),
            server.client_preview_surface_dmabuf_frames(),
            server.client_preview_surface_frame_order(),
        )
    } else {
        // One shared visibility/occlusion pass for both frame lists; the
        // damage assessment and scanout planner used to each recompute the
        // same O(windows × surfaces) walk per presented frame.
        let sets = server.desktop_frame_sets();
        let order = server.client_surface_frame_order_with(&sets.visible, &sets.occluded);
        (sets.shm, sets.dmabuf, order)
    };
    let windows = server.render_windows();
    renderer.gc(server.live_surface_ids());
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
            soft_shadows,
            shadow_style,
        );
    }
    // ADR-0029 close transitions: fading ghosts of just-closed windows paint
    // above the live desktop (they always unmap topmost relative to their
    // own tree; ghosts cannot occlude anything that is still interactive
    // because nothing below them changes while they fade).
    let ghosts = server.closing_frame_views();
    if !ghosts.is_empty() {
        renderer.draw_closing_frames(device, canvas, &ghosts);
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
    renderer.gc(server.live_surface_ids());
    renderer.draw_toplevels(device, canvas, &shm, (0.0, 0.0));
    renderer.draw_dmabuf_toplevels(device, canvas, &dmabuf, (0.0, 0.0));
    canvas.restore();
}

/// Overview scene (M9): the desktop dimmed, then every visible window drawn
/// as a live thumbnail on the shared `aegis_model::overview` grid — the exact
/// geometry the overview chrome uses for its frames, labels, and hit-testing.
/// Z-order is preserved bottom-to-top so overlapping thumbnails read like
/// the desktop stack. `progress` (0..1) is the chrome's reveal animation:
/// thumbnails interpolate from each window's real geometry into its grid
/// cell (and the scrim fades in) instead of popping onto the grid.
#[allow(clippy::too_many_arguments)]
pub(super) fn draw_overview_scene(
    canvas: &flux::Canvas,
    device: &flux::Device,
    renderer: &mut aegis_render::Renderer,
    server: &aegis_compositor::Server,
    logical_size: (u32, u32),
    scale: f32,
    scheme: aegis_model::settings::ColorScheme,
    progress: f32,
) {
    let t = progress.clamp(0.0, 1.0);
    let scrim_alpha = (200.0 * t).round() as u8;
    let (scrim_r, scrim_g, scrim_b) = overview_scrim(scheme);
    canvas.save();
    canvas.fill_rect(
        0.0,
        0.0,
        logical_size.0 as f32 * scale,
        logical_size.1 as f32 * scale,
        flux::rgba(scrim_r, scrim_g, scrim_b, scrim_alpha),
    );
    canvas.restore();

    let windows = server.windows();
    let snapshot = server.workspace_snapshot();
    let rail = snapshot
        .outputs
        .first()
        .map(|o| o.workspaces.len() > 1)
        .unwrap_or(false);
    let interaction_domain_shelf = server
        .interaction_domain_snapshot()
        .interaction_domains
        .iter()
        .any(|interaction_domain| {
            interaction_domain.kind == aegis_model::interaction_domain::InteractionDomainKind::Agent
                && interaction_domain.state
                    != aegis_model::interaction_domain::InteractionDomainState::Revoked
        });
    let display = aegis_model::Rect::new(0, 0, logical_size.0 as i32, logical_size.1 as i32);

    // Workspace rail miniatures along the top edge: every workspace on the
    // output drawn live into its tile, independent of the main grid — an
    // empty current workspace must not starve the rail.
    if rail {
        draw_workspace_rail_tiles(
            canvas, device, renderer, server, &snapshot, display, scale, t,
        );
    }

    if windows.is_empty() {
        return;
    }
    let area = aegis_model::overview::grid_area_with_interaction_domain_shelf(
        display,
        rail,
        interaction_domain_shelf,
    );
    let window_rects: Vec<(aegis_model::window::WindowId, aegis_model::Rect)> = windows
        .iter()
        .map(|w| {
            (
                w.id,
                aegis_model::Rect {
                    origin: w.position,
                    size: w.size,
                },
            )
        })
        .collect();
    // Closest-slot assignment pairs each window with the slot nearest its
    // real position, in input order; the chrome's hit-testing pairs the same
    // list the same way, so cells agree.
    let slots: Vec<aegis_model::Rect> = aegis_model::overview::assign_slots(area, &window_rects)
        .into_iter()
        .map(|(_, slot)| slot)
        .collect();
    let cells: std::collections::HashMap<
        aegis_model::window::WindowId,
        (aegis_model::Rect, aegis_model::Point, aegis_model::Size),
    > = windows
        .iter()
        .zip(slots.iter())
        .map(|(w, slot)| {
            // Fly-in: the cell starts at the window's real geometry and
            // lands on its aspect-fitted grid slot as `t` reaches 1.
            let cell = aegis_model::overview::animated_cell(
                *slot,
                aegis_model::Rect {
                    origin: w.position,
                    size: w.size,
                },
                t,
            );
            (w.id, (cell, w.position, w.size))
        })
        .collect();
    let map = move |window: Option<aegis_model::window::WindowId>, natural: aegis_model::Rect| {
        let Some((cell, base, win_size)) = window.and_then(|id| cells.get(&id)) else {
            return natural;
        };
        let k = cell.size.w as f32 / win_size.w.max(1) as f32;
        let remap = |v: i32, b: i32| (v - b) as f32 * k;
        aegis_model::Rect::new(
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
    let shm = server.client_preview_surface_frames();
    let dmabuf = server.client_preview_surface_dmabuf_frames();
    let surface_order = server.client_preview_surface_frame_order();
    renderer.draw_surfaces_ordered_mapped(device, canvas, &surface_order, &shm, &dmabuf, &map);
    canvas.restore();
}

/// Workspace rail miniatures: each workspace of the output drawn live into
/// its top-rail tile using the shared `aegis_model::overview` geometry, so
/// the chrome's tile frames, captions, and hit-testing line up. A tile is
/// clipped to its rounded rect so miniature surface trees never spill into
/// neighbouring tiles.
#[allow(clippy::too_many_arguments)]
fn draw_workspace_rail_tiles(
    canvas: &flux::Canvas,
    device: &flux::Device,
    renderer: &mut aegis_render::Renderer,
    server: &aegis_compositor::Server,
    snapshot: &aegis_model::workspace::WorkspaceSnapshot,
    display: aegis_model::Rect,
    scale: f32,
    progress: f32,
) {
    let Some(output) = snapshot.outputs.first() else {
        return;
    };
    let tiles = aegis_model::overview::rail(display, output.workspaces.len());
    let all = server.all_windows();
    canvas.save();
    if scale != 1.0 {
        canvas.scale(scale, scale);
    }
    for (entry, tile) in output.workspaces.iter().zip(tiles.iter()) {
        let tile_windows: Vec<&aegis_model::window::Window> = entry
            .toplevels
            .iter()
            .filter_map(|id| all.iter().find(|w| w.id == *id))
            .collect();
        if tile_windows.is_empty() {
            continue;
        }
        let content = aegis_model::overview::tile_content(*tile);
        let slots = aegis_model::overview::grid_with_spacing(
            content,
            tile_windows.len(),
            aegis_model::overview::TILE_GRID_MARGIN,
            aegis_model::overview::TILE_GRID_GAP,
        );
        let cells: std::collections::HashMap<
            aegis_model::window::WindowId,
            (aegis_model::Rect, aegis_model::Point, aegis_model::Size),
        > = tile_windows
            .iter()
            .zip(slots.iter())
            .map(|(window, slot)| {
                (
                    window.id,
                    (
                        aegis_model::overview::fit(*slot, window.size),
                        window.position,
                        window.size,
                    ),
                )
            })
            .collect();
        let map = move |wid: Option<aegis_model::window::WindowId>, natural: aegis_model::Rect| {
            let Some((cell, base, win_size)) = wid.and_then(|id| cells.get(&id)) else {
                return natural;
            };
            let k = cell.size.w as f32 / win_size.w.max(1) as f32;
            let remap = |v: i32, b: i32| (v - b) as f32 * k;
            aegis_model::Rect::new(
                cell.origin.x + remap(natural.origin.x, base.x).round() as i32,
                cell.origin.y + remap(natural.origin.y, base.y).round() as i32,
                (natural.size.w as f32 * k).round().max(1.0) as i32,
                (natural.size.h as f32 * k).round().max(1.0) as i32,
            )
        };
        let set: std::collections::HashSet<aegis_model::window::WindowId> =
            tile_windows.iter().map(|w| w.id).collect();
        let shm = server.client_preview_frames_for(&set);
        let dmabuf = server.client_preview_dmabuf_frames_for(&set);
        let order = server.client_preview_frame_order_for(&set);
        renderer.draw_surfaces_ordered_mapped_with_style(
            device,
            canvas,
            &order,
            &shm,
            &dmabuf,
            &map,
            aegis_render::MappedSurfaceStyle {
                opacity: progress.clamp(0.0, 1.0),
                brightness: 1.0,
                rounded_clip: *tile,
                corner_radius: 10.0,
            },
        );
    }
    canvas.restore();
}

/// Super+Tab scene: preserve the live desktop underneath a dim scrim, then
/// paint every visible window again into the shared horizontal preview strip.
/// Shell chrome draws labels and the selected-card border over these targets.
#[allow(clippy::too_many_arguments)]
pub(super) fn draw_window_switcher_scrim(
    canvas: &flux::Canvas,
    logical_size: (u32, u32),
    scale: f32,
    presentation: &aegis_shell::WindowSwitcherPresentation,
    scheme: aegis_model::settings::ColorScheme,
) {
    let scrim_alpha = (145.0 * presentation.visibility.clamp(0.0, 1.0)).round() as u8;
    let (scrim_r, scrim_g, scrim_b) = window_switcher_scrim(scheme);
    canvas.save();
    canvas.fill_rect(
        0.0,
        0.0,
        logical_size.0 as f32 * scale,
        logical_size.1 as f32 * scale,
        flux::rgba(scrim_r, scrim_g, scrim_b, scrim_alpha),
    );
    canvas.restore();
}

pub(super) fn draw_window_switcher_cards(
    canvas: &flux::Canvas,
    device: &flux::Device,
    renderer: &mut aegis_render::Renderer,
    server: &aegis_compositor::Server,
    scale: f32,
    presentation: &aegis_shell::WindowSwitcherPresentation,
) {
    let windows = server.windows();
    if windows.is_empty() || presentation.visibility <= 0.001 {
        return;
    }
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
    for card in &presentation.cards {
        let Some(window) = windows.iter().find(|window| window.id == card.window) else {
            continue;
        };
        let target_set: std::collections::HashSet<_> = [card.window].into_iter().collect();
        let shm = server.client_preview_frames_for(&target_set);
        let dmabuf = server.client_preview_dmabuf_frames_for(&target_set);
        let surface_order = server.client_preview_frame_order_for(&target_set);
        let brightness = aegis_shell::preview::content_brightness(
            presentation.selected,
            card.window,
            presentation.inactive_content_brightness,
        );
        draw_preview_card_scene(
            canvas,
            device,
            renderer,
            &surface_order,
            &shm,
            &dmabuf,
            window,
            card,
            presentation.visibility,
            brightness,
        );
    }
    canvas.restore();
}

#[allow(dead_code)]
pub(super) fn draw_window_switcher_scene(
    canvas: &flux::Canvas,
    device: &flux::Device,
    renderer: &mut aegis_render::Renderer,
    server: &aegis_compositor::Server,
    logical_size: (u32, u32),
    scale: f32,
    presentation: &aegis_shell::WindowSwitcherPresentation,
    scheme: aegis_model::settings::ColorScheme,
) {
    draw_window_switcher_scrim(canvas, logical_size, scale, presentation, scheme);
    draw_window_switcher_cards(canvas, device, renderer, server, scale, presentation);
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

    canvas.save();
    if scale != 1.0 {
        canvas.scale(scale, scale);
    }
    for presentation in presentations {
        if presentation.visibility <= 0.001 {
            continue;
        }
        for card in &presentation.cards {
            let Some(window) = windows.iter().find(|window| window.id == card.window) else {
                continue;
            };
            let target_set: std::collections::HashSet<_> = [card.window].into_iter().collect();
            let shm = server.client_preview_frames_for(&target_set);
            let dmabuf = server.client_preview_dmabuf_frames_for(&target_set);
            let surface_order = server.client_preview_frame_order_for(&target_set);
            let brightness = aegis_shell::preview::content_brightness(
                presentation.focused,
                card.window,
                presentation.inactive_content_brightness,
            );
            draw_preview_card_scene(
                canvas,
                device,
                renderer,
                &surface_order,
                &shm,
                &dmabuf,
                window,
                card,
                presentation.visibility,
                brightness,
            );
        }
    }
    canvas.restore();
}

#[allow(clippy::too_many_arguments)]
fn draw_preview_card_scene(
    canvas: &flux::Canvas,
    device: &flux::Device,
    renderer: &mut aegis_render::Renderer,
    surface_order: &[usize],
    shm: &[aegis_model::SurfacePixels<'_>],
    dmabuf: &[aegis_model::SurfaceDmabuf],
    window: &aegis_model::window::Window,
    card: &aegis_shell::PreviewCard,
    opacity: f32,
    brightness: f32,
) {
    let cell = aegis_model::overview::fit(card.geometry.preview, window.size);
    let base = window.position;
    let window_size = window.size;
    let target = window.id;
    let map = move |id: Option<aegis_model::window::WindowId>, natural: aegis_model::Rect| {
        if id != Some(target) {
            return aegis_model::Rect::new(-100_000, -100_000, 1, 1);
        }
        let factor = (cell.size.w as f32 / window_size.w.max(1) as f32)
            .min(cell.size.h as f32 / window_size.h.max(1) as f32);
        let remap = |value: i32, origin: i32| (value - origin) as f32 * factor;
        aegis_model::Rect::new(
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
    renderer.draw_surfaces_ordered_mapped_with_style(
        device,
        canvas,
        surface_order,
        shm,
        dmabuf,
        &map,
        aegis_render::MappedSurfaceStyle {
            opacity: opacity.clamp(0.0, 1.0),
            brightness: brightness.clamp(0.0, 1.0),
            rounded_clip: card.geometry.preview,
            corner_radius: card.corner_radius,
        },
    );
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
mod tests;

/// Compositor-side blurred drop shadows for floating windows (ADR-0139):
/// the Optics `flux_shadow_filter` renders a rounded-rect mask through a
/// Gaussian blur; this wrapper owns the filter, renders each window's mask
/// into a per-slot capture target, and records the shadow passes at a pass
/// boundary. `aegis-render` then composites the borrowed outputs beneath
/// each window tree.
///
/// Ownership mirrors `LauncherBackdrop`: one filter for the surface's frame
/// stream, per-slot mask images, no transient-pool leases, no device-wide
/// waits. The mask is re-rendered every frame a shadow is visible (the
/// windows move), so no cache invalidation tracking is needed beyond the
/// compositor's own damage (any geometry change already forces a full
/// repaint through the window signature).
pub(super) struct WindowShadowRenderer {
    filter: flux::ShadowFilter,
    /// One mask render target per frame-in-flight slot (ADR-0074 slot
    /// isolation; sizes vary per window so entries are rebuilt on extent
    /// change by the filter's own slot logic — the mask target here only
    /// needs a per-slot image because the canvas writes it inside this
    /// frame's recording).
    masks: Vec<Option<flux::Image>>,
}

/// A rendered shadow ready for compositing: the borrowed filter output for
/// one window this frame, plus its physical-pixel placement.
pub(super) struct RenderedShadow {
    /// The window this shadow belongs to.
    pub window: aegis_model::window::WindowId,
    /// Raw `flux_image` borrowed from the shadow filter's current frame
    /// slot. Lifetime: valid until the same slot is applied again, which
    /// happens on the next frame rotation — strictly after this frame's
    /// canvas passes and submit (ADR-0074 frame-slot lifetime). The
    /// composition root applies the filter exactly once per frame, before
    /// any output pass opens, so every composite of this pointer precedes
    /// the next apply on this slot.
    pub raw: *mut flux::sys::flux_image,
    /// Physical-pixel placement (already includes the blur margin).
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl WindowShadowRenderer {
    pub(super) fn new(device: &flux::Device) -> Result<Self, flux::Error> {
        Ok(Self {
            filter: flux::ShadowFilter::new(device)?,
            masks: Vec::new(),
        })
    }

    /// Render and record shadows for `windows` into this frame's slot.
    /// Must be called at a pass boundary (no active canvas pass). The
    /// returned placements carry raw image pointers borrowed from the
    /// filter — composite them within this frame.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn prepare(
        &mut self,
        device: &flux::Device,
        canvas: &flux::Canvas,
        frame: &flux::Frame<'_>,
        windows: &[aegis_model::window::Window],
        scale: f32,
        style: aegis_model::window::WindowShadowStyle,
    ) -> Vec<RenderedShadow> {
        if style != aegis_model::window::WindowShadowStyle::Soft {
            return Vec::new();
        }
        let slot = frame.index() as usize;
        let mut out = Vec::new();
        for window in windows
            .iter()
            .filter(|w| aegis_render::window_casts_shadow(w))
        {
            // Physical extent: the window rect inflated by the blur
            // footprint so the Gaussian skirt fits inside the image.
            let margin = (SHADOW_BLUR_SIGMA * 3.0 + SHADOW_OFFSET_Y.abs() + 2.0) * scale;
            let px_w = (window.size.w as f32 * scale + margin * 2.0).ceil().max(1.0) as u32;
            let px_h = (window.size.h as f32 * scale + margin * 2.0).ceil().max(1.0) as u32;
            if self.masks.len() <= slot {
                self.masks.resize_with(slot + 1, || None);
            }
            let rebuild = !matches!(&self.masks[slot],
                Some(existing) if existing.size().0 == px_w && existing.size().1 == px_h);
            if rebuild {
                let Ok(image) =
                    flux::Image::render_target(device, px_w, px_h, flux::Format::FLUX_FORMAT_RGBA8_UNORM)
                else {
                    continue;
                };
                self.masks[slot] = Some(image);
            }
            let Some(mask) = self.masks[slot].as_ref() else {
                continue;
            };

            // Draw the mask: opaque rounded rect at the window's position
            // within the padded extent.
            let clear = flux::rgba(0, 0, 0, 0);
            if canvas
                .begin_target_pass(
                    frame,
                    mask,
                    flux::CanvasPassOptions {
                        clear: Some(clear),
                        antialias: flux::CanvasAntialias::Auto,
                        render_area: None,
                        skip_stencil: true,
                    },
                )
                .is_err()
            {
                continue;
            }
            let inv = 1.0 / scale;
            canvas.save();
            canvas.scale(scale, scale);
            canvas.fill_rrect(
                inv * margin,
                inv * margin,
                window.size.w as f32,
                window.size.h as f32,
                SHADOW_CORNER_RADIUS,
                flux::rgba(255, 255, 255, 255),
            );
            canvas.restore();
            if canvas.end_target_checked().is_err() {
                continue;
            }

            let params = flux::ShadowParams {
                blur: SHADOW_BLUR_SIGMA * scale,
                offset_x: 0.0,
                offset_y: SHADOW_OFFSET_Y * scale,
                tint_red: 0.0,
                tint_green: 0.0,
                tint_blue: 0.0,
                alpha: if window.state.activated {
                    SHADOW_ALPHA_FOCUS
                } else {
                    SHADOW_ALPHA_IDLE
                },
            };
            match self.filter.apply(frame, mask, params) {
                Ok(shadow) => out.push(RenderedShadow {
                    window: window.id,
                    raw: shadow.as_raw(),
                    x: window.position.x as f32 * scale - margin,
                    y: window.position.y as f32 * scale - margin,
                    w: px_w as f32,
                    h: px_h as f32,
                }),
                Err(error) => {
                    log::debug!("window shadow: filter apply failed ({error}); skipping");
                }
            }
        }
        out
    }
}

/// Shadow design constants (policy values, ADR-0139: data, not mechanism).
pub(super) const SHADOW_BLUR_SIGMA: f32 = 10.0;
pub(super) const SHADOW_OFFSET_Y: f32 = 12.0;
pub(super) const SHADOW_CORNER_RADIUS: f32 = 12.0;
pub(super) const SHADOW_ALPHA_FOCUS: f32 = 0.45;
pub(super) const SHADOW_ALPHA_IDLE: f32 = 0.30;
