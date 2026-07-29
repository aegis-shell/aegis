use super::*;

/// Backdrop effects are evaluated at quarter resolution, then upsampled behind
/// the launcher. Dual-Kawase removes the lost high-frequency detail, while the
/// 16x pixel reduction bounds the cost of live 2D + 3D wallpaper capture.
pub(super) const BACKDROP_DOWNSAMPLE: u32 = 4;

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
        },
    )
}

/// Begin an opaque output pass clipped to the accumulated damage for this
/// ring slot. The full scene may still be submitted, but rasterization,
/// texture sampling, blending and framebuffer writes stay inside the scissor.
pub(super) fn begin_opaque_frame_repaint(
    canvas: &flux::Canvas,
    frame: &flux::Frame<'_>,
    size: (u32, u32),
    clear: u32,
    repaint: FrameDamage,
) -> Result<(), flux::Error> {
    debug_assert_eq!(clear >> 24, 0xff, "compositor pass clear must be opaque");
    match repaint {
        FrameDamage::Area(rect) => {
            canvas.begin(frame, None)?;
            canvas.clip_rect(
                rect.origin.x as f32,
                rect.origin.y as f32,
                rect.size.w as f32,
                rect.size.h as f32,
            );
            canvas.fill_rect(0.0, 0.0, size.0 as f32, size.1 as f32, clear);
        }
        FrameDamage::Full | FrameDamage::None => {
            canvas.begin_pass(
                frame,
                flux::CanvasPassOptions {
                    clear: Some(clear),
                    antialias: flux::CanvasAntialias::None,
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
        },
    )
}

/// Union of the declared backdrop regions in physical pixels, expanded by
/// the blur footprint and aligned to the downsample factor.
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
    let mut x0 = f32::INFINITY;
    let mut y0 = f32::INFINITY;
    let mut x1 = f32::NEG_INFINITY;
    let mut y1 = f32::NEG_INFINITY;
    for region in regions {
        // Same clamping as the composition pass: regions are logical and
        // may extend past the output edge.
        let x = region.x.max(0.0);
        let y = region.y.max(0.0);
        let w = region.w.max(0.0).min(logical_size.0 as f32 - x) * scale;
        let h = region.h.max(0.0).min(logical_size.1 as f32 - y) * scale;
        if w <= 0.0 || h <= 0.0 {
            continue;
        }
        x0 = x0.min(x * scale);
        y0 = y0.min(y * scale);
        x1 = x1.max(x * scale + w);
        y1 = y1.max(y * scale + h);
    }
    if !x0.is_finite() {
        return ((0, 0), physical_size);
    }
    let pad = 3.0 * sigma * scale;
    let align = BACKDROP_DOWNSAMPLE as f32;
    let ox = (((x0 - pad) / align).floor() * align).max(0.0);
    let oy = (((y0 - pad) / align).floor() * align).max(0.0);
    let ex = ((x1 + pad) / align).ceil() * align;
    let ey = ((y1 + pad) / align).ceil() * align;
    let ex = ex.max(ox + align).min(physical_size.0 as f32);
    let ey = ey.max(oy + align).min(physical_size.1 as f32);
    ((ox as u32, oy as u32), ((ex - ox) as u32, (ey - oy) as u32))
}

pub(super) struct BackdropCapture {
    image: flux::Image,
    size: (u32, u32),
    format: flux::Format,
}

/// Live desktop capture used behind the full-screen application launcher.
///
/// Capture images and blur intermediates are both indexed by frame slot. A
/// slot is rewritten only after `begin_frame` has waited its fence, avoiding
/// device-wide stalls while a 3D wallpaper continues animating.
pub(super) struct LauncherBackdrop {
    blur: flux::BlurFilter,
    captures: Vec<Option<BackdropCapture>>,
    was_active: bool,
    failed_session: bool,
    unsupported: bool,
}

#[derive(Clone, Copy)]
pub(super) enum BackdropPlan {
    Direct,
    Capture,
}

impl LauncherBackdrop {
    pub(super) fn new(device: &flux::Device) -> Result<Self, flux::Error> {
        Ok(Self {
            blur: flux::BlurFilter::new(device)?,
            captures: Vec::new(),
            was_active: false,
            failed_session: false,
            unsupported: false,
        })
    }

    /// `extent` is the physical-pixel area the capture must cover (the blur
    /// regions' padded union, or the full surface for a live 3D wallpaper);
    /// the capture target is allocated at `extent / BACKDROP_DOWNSAMPLE`.
    pub(super) fn prepare(
        &mut self,
        active: bool,
        device: &flux::Device,
        surface: &flux::Surface,
        frame: &flux::Frame<'_>,
        extent: (u32, u32),
    ) -> BackdropPlan {
        if !active {
            self.was_active = false;
            self.failed_session = false;
            return BackdropPlan::Direct;
        }

        let opening = !self.was_active;
        self.was_active = true;
        if opening {
            self.failed_session = false;
        }
        if self.unsupported || self.failed_session || extent.0 == 0 || extent.1 == 0 {
            return BackdropPlan::Direct;
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
        let target_stale = self.captures[slot]
            .as_ref()
            .is_none_or(|capture| capture.size != size || capture.format != format);
        if target_stale {
            match flux::Image::render_target(device, size.0, size.1, format) {
                Ok(image) => {
                    self.captures[slot] = Some(BackdropCapture {
                        image,
                        size,
                        format,
                    });
                }
                Err(error) => {
                    log::warn!(
                        "launcher: failed to allocate realtime backdrop target ({error}); using translucent fallback"
                    );
                    self.failed_session = true;
                    return BackdropPlan::Direct;
                }
            }
        }
        BackdropPlan::Capture
    }

    pub(super) fn begin_capture(
        &mut self,
        canvas: &flux::Canvas,
        frame: &flux::Frame<'_>,
        clear: u32,
    ) -> bool {
        let Some(target) = self.target(frame) else {
            return false;
        };
        if let Err(error) = begin_opaque_target(canvas, frame, target, clear) {
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

    pub(super) fn end_capture_and_blur<'backdrop>(
        &'backdrop mut self,
        canvas: &flux::Canvas,
        frame: &flux::Frame<'_>,
        sigma: f32,
    ) -> Option<flux::BlurredImage<'backdrop>> {
        canvas.end_target();
        let slot = frame.index() as usize;
        let capture = self.captures.get(slot)?.as_ref()?;
        match self.blur.apply(frame, &capture.image, sigma) {
            Ok(image) => Some(image),
            Err(error) => {
                log::warn!(
                    "launcher: realtime backdrop dispatch failed ({error}); using translucent fallback"
                );
                self.failed_session = true;
                None
            }
        }
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
/// Session flow: [`request_open`](Self::request_open) arms a session and
/// defers the selector opening; the capture frame renders the normal frame
/// into the target (the selector is not open yet, so its scrim stays out
/// of the snapshot), then the compositor opens the selector over the
/// frozen image. [`should_disarm`](Self::should_disarm) ends the session
/// once the selector closes.
pub(super) struct ScreenshotFreeze {
    captures: Vec<Option<BackdropCapture>>,
    active_slot: Option<usize>,
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
            cursor: None,
            armed: false,
            captured: false,
            failed: false,
            pending_open: false,
            opened: false,
        }
    }

    /// Arm a freeze session; the selector opens after the capture frame.
    pub(super) fn request_open(&mut self) {
        if self.armed {
            return;
        }
        self.armed = true;
        self.captured = false;
        self.failed = false;
        self.pending_open = true;
        self.opened = false;
        self.active_slot = None;
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
                    self.captures[slot] = Some(BackdropCapture {
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
    let windows = server.windows();
    renderer.gc(shm
        .iter()
        .map(|frame| frame.id)
        .chain(dmabuf.iter().map(|frame| frame.id))
        .chain(overlay_shm.iter().map(|frame| frame.id))
        .chain(overlay_dmabuf.iter().map(|frame| frame.id)));
    renderer.draw_surfaces_ordered_with_window_shadows(
        device,
        canvas,
        &surface_order,
        &shm,
        &dmabuf,
        &windows,
    );
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
) {
    canvas.save();
    if scale != 1.0 {
        canvas.scale(scale, scale);
    }
    let overlay_shm = server.overlay_frames_with_cursor(include_cursor);
    let overlay_dmabuf = server.overlay_dmabuf_frames_with_cursor(include_cursor);
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
    let realm_shelf = server.realm_snapshot().realms.iter().any(|realm| {
        realm.kind == aegis_core::realm::RealmKind::Agent
            && realm.state != aegis_core::realm::RealmState::Revoked
    });
    let display = aegis_core::Rect::new(0, 0, logical_size.0 as i32, logical_size.1 as i32);
    let area = aegis_core::overview::grid_area_with_realm_shelf(display, rail, realm_shelf);
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
) {
    canvas.save();
    canvas.fill_rect(
        0.0,
        0.0,
        logical_size.0 as f32 * scale,
        logical_size.1 as f32 * scale,
        flux::rgba(5, 7, 12, 145),
    );
    canvas.restore();

    let windows = server.windows();
    if windows.is_empty() {
        return;
    }
    let display = aegis_core::Rect::new(0, 0, logical_size.0 as i32, logical_size.1 as i32);
    let layout = aegis_core::window_switcher::layout(display, windows.len());
    let cells: std::collections::HashMap<
        aegis_core::window::WindowId,
        (aegis_core::Rect, aegis_core::Point, aegis_core::Size),
    > = windows
        .iter()
        .zip(layout.cards.iter())
        .map(|(window, card)| {
            (
                window.id,
                (
                    aegis_core::overview::fit(card.preview, window.size),
                    window.position,
                    window.size,
                ),
            )
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

impl CompositorRuntime {
    /// Pick a client dma-buf to page-flip directly onto the primary plane,
    /// bypassing the Vulkan composite. This is the fullscreen-game fast path.
    ///
    /// The bar is intentionally high and fully conservative: a miss here
    /// merely composites (always correct), while a false hit would scan out a
    /// wrong/tearing buffer or drop the cursor. The candidate must:
    ///
    ///   - be the *only* visible dmabuf toplevel;
    ///   - cover the whole output at (0,0) with a `Normal` transform and no
    ///     viewport source/destination clipping;
    ///   - have a post-transform buffer size equal to the output's;
    ///   - be acceptable to the active primary plane
    ///     ([`Host::supports_scanout`]); and
    ///   - have nothing else to composite: a visible cursor needs either a
    ///     KMS cursor plane or the composite, and no client overlay, shell
    ///     chrome, blur, transition, capture, or lock may be active.
    ///
    /// `physical_size` is the output's pixel dimensions; `cursor_hidden`
    /// reflects the current cursor visibility state.
    pub(super) fn pick_scanout_candidate(
        &self,
        physical_size: (u32, u32),
        cursor_hidden: bool,
    ) -> Option<aegis_core::SurfaceDmabuf> {
        // Nothing should be composited on top of the client: shell animations,
        // overview/switcher, backdrop blur, or a software cursor all need the
        // framebuffer and disqualify direct scanout.
        if self.shell.anim_pending()
            || self.shell.overview_active()
            || self.shell.window_switcher_active()
            || self.shell.backdrop_blur_sigma() > 0.0
            || self.server.session_locked()
            || self.server.transitions_pending()
            || self.capture_worker.is_busy()
            || self.pending_capture.is_some()
            || self.pending_realm_capture.is_some()
            || self.screenshot_freeze.armed
            || self.shell.requires_composition()
        {
            return None;
        }
        // Client cursor surfaces, drag icons, and other protocol overlays are
        // separate scene elements. They cannot ride the compositor-owned KMS
        // cursor plane, so never drop them merely because the base toplevel is
        // scanout-compatible.
        if !self.server.overlay_frames().is_empty()
            || !self.server.overlay_dmabuf_frames().is_empty()
        {
            return None;
        }
        // A software cursor is painted into the composite. When it is visible
        // the direct-scanout path would drop it, so only take scanout when the
        // cursor is hidden. Nested mode owns no scanout and is excluded by the
        // supports_scanout check below.
        if self.host.uses_software_cursor() && !cursor_hidden {
            return None;
        }
        // A scanout buffer must be the only visible client surface, not just
        // the only dma-buf toplevel. Otherwise an shm popup or either kind of
        // subsurface would silently disappear.
        if !self.server.client_surface_frames().is_empty() {
            return None;
        }
        let all_dmabufs = self.server.client_surface_dmabuf_frames();
        if all_dmabufs.len() != 1 {
            return None;
        }
        let mut frames = self.server.toplevel_dmabuf_frames();
        if frames.len() != 1 || frames[0].id != all_dmabufs[0].id {
            return None;
        }
        let f = frames.pop()?;
        // Direct scanout only honors the trivial placement: identity transform,
        // origin at (0,0), and no viewport source/destination crop. A rotated
        // or sub-rect client must be composited.
        if f.geometry.transform != aegis_core::Transform::Normal
            || f.geometry.position != (aegis_core::Point { x: 0, y: 0 })
            || f.geometry.viewport_src.is_some()
            || f.geometry.viewport_dst.is_some()
        {
            return None;
        }
        // The buffer must exactly fill the output; a size mismatch would need
        // hardware scaling (not configured here) or letterboxing.
        if (f.width as u32, f.height as u32) != physical_size {
            return None;
        }
        // The primary plane must accept this fourcc/modifier pair.
        if !self.host.supports_scanout(f.drm_format, f.modifier) {
            return None;
        }
        Some(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn region(x: f32, y: f32, w: f32, h: f32) -> aegis_shell::BackdropRegion {
        aegis_shell::BackdropRegion { x, y, w, h }
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
    fn capture_bounds_align_to_downsample() {
        // A floating region: origin/size land on BACKDROP_DOWNSAMPLE
        // multiples so the capture scale stays exactly 1/4.
        let (origin, size) = blur_capture_bounds(
            &[region(100.0, 100.0, 200.0, 50.0)],
            (1920, 1080),
            (1920, 1080),
            1.0,
            12.0,
        );
        assert_eq!(origin, (64, 64));
        assert_eq!(size, (272, 124));
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
            canvas.end();
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
    fn damaged_opaque_frame_preserves_pixels_outside_the_scissor() {
        let Ok(device) = flux::Device::new(true, &[], &[], 1) else {
            return;
        };
        let size = (32, 24);
        let surface = flux::Surface::offscreen(&device, size.0, size.1).unwrap();
        let canvas = flux::Canvas::new(&surface).unwrap();

        let frame = surface.begin_frame().unwrap();
        begin_opaque_frame(&canvas, &frame, flux::rgba(200, 30, 20, 255)).unwrap();
        canvas.end();
        frame.submit().unwrap().present().unwrap();

        let frame = surface.begin_frame().unwrap();
        begin_opaque_frame_repaint(
            &canvas,
            &frame,
            size,
            flux::rgba(10, 80, 220, 255),
            FrameDamage::Area(aegis_core::Rect::new(8, 6, 10, 9)),
        )
        .unwrap();
        canvas.end();
        frame.submit().unwrap().present().unwrap();

        let mut pixels = vec![0; size.0 as usize * size.1 as usize * 4];
        surface.read_pixels(&mut pixels).unwrap();
        let pixel = |x: usize, y: usize| &pixels[(y * size.0 as usize + x) * 4..][..4];
        assert_eq!(pixel(0, 0), [200, 30, 20, 255]);
        assert_eq!(pixel(9, 7), [10, 80, 220, 255]);
        assert_eq!(pixel(17, 14), [10, 80, 220, 255]);
        assert_eq!(pixel(18, 15), [200, 30, 20, 255]);
    }
}
