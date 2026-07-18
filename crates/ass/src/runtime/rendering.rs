use super::*;

/// Backdrop effects are evaluated at quarter resolution, then upsampled behind
/// the launcher. Dual-Kawase removes the lost high-frequency detail, while the
/// 16x pixel reduction bounds the cost of live 2D + 3D wallpaper capture.
pub(super) const BACKDROP_DOWNSAMPLE: u32 = 4;

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

    pub(super) fn prepare(
        &mut self,
        active: bool,
        device: &flux::Device,
        surface: &flux::Surface,
        frame: &flux::Frame<'_>,
        surface_size: (u32, u32),
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
        if self.unsupported || self.failed_session || surface_size.0 == 0 || surface_size.1 == 0 {
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
            surface_size.0.div_ceil(BACKDROP_DOWNSAMPLE).max(1),
            surface_size.1.div_ceil(BACKDROP_DOWNSAMPLE).max(1),
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
        if let Err(error) = canvas.begin_target(frame, target, Some(clear)) {
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

pub(super) fn draw_wallpaper_background(
    canvas: &flux::Canvas,
    device: &flux::Device,
    wallpaper: &mut Option<ass_wallpaper::Wallpaper>,
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
    renderer: &mut ass_render::Renderer,
    server: &ass_server::Server,
    scale: f32,
) {
    canvas.save();
    if scale != 1.0 {
        canvas.scale(scale, scale);
    }
    let shm = server.toplevel_frames();
    let dmabuf = server.toplevel_dmabuf_frames();
    let sub_shm_below = server.subsurface_frames_below();
    let sub_shm_above = server.subsurface_frames_above();
    let sub_dmabuf_below = server.subsurface_dmabuf_frames_below();
    let sub_dmabuf_above = server.subsurface_dmabuf_frames_above();
    let overlay_shm = server.overlay_frames();
    let overlay_dmabuf = server.overlay_dmabuf_frames();
    renderer.gc(shm
        .iter()
        .map(|frame| frame.id)
        .chain(dmabuf.iter().map(|frame| frame.id))
        .chain(sub_shm_below.iter().map(|frame| frame.id))
        .chain(sub_shm_above.iter().map(|frame| frame.id))
        .chain(sub_dmabuf_below.iter().map(|frame| frame.id))
        .chain(sub_dmabuf_above.iter().map(|frame| frame.id))
        .chain(overlay_shm.iter().map(|frame| frame.id))
        .chain(overlay_dmabuf.iter().map(|frame| frame.id)));
    renderer.draw_subsurfaces(device, canvas, &sub_shm_below);
    renderer.draw_dmabuf_subsurfaces(device, canvas, &sub_dmabuf_below);
    renderer.draw_toplevels(device, canvas, &shm, (0.0, 0.0));
    renderer.draw_dmabuf_toplevels(device, canvas, &dmabuf, (0.0, 0.0));
    renderer.draw_subsurfaces(device, canvas, &sub_shm_above);
    renderer.draw_dmabuf_subsurfaces(device, canvas, &sub_dmabuf_above);
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
    renderer: &mut ass_render::Renderer,
    server: &ass_server::Server,
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
/// as a live thumbnail on the shared `ass_core::overview` grid — the exact
/// geometry the overview chrome uses for its frames, labels, and hit-testing.
/// Z-order is preserved bottom-to-top so overlapping thumbnails read like
/// the desktop stack.
pub(super) fn draw_overview_scene(
    canvas: &flux::Canvas,
    device: &flux::Device,
    renderer: &mut ass_render::Renderer,
    server: &ass_server::Server,
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
        realm.kind == ass_core::realm::RealmKind::Agent
            && realm.state != ass_core::realm::RealmState::Revoked
    });
    let display = ass_core::Rect::new(0, 0, logical_size.0 as i32, logical_size.1 as i32);
    let area = ass_core::overview::grid_area_with_realm_shelf(display, rail, realm_shelf);
    let slots = ass_core::overview::grid(area, windows.len());
    let cells: std::collections::HashMap<
        ass_core::window::WindowId,
        (ass_core::Rect, ass_core::Point, ass_core::Size),
    > = windows
        .iter()
        .zip(slots.iter())
        .map(|(w, slot)| {
            (
                w.id,
                (ass_core::overview::fit(*slot, w.size), w.position, w.size),
            )
        })
        .collect();
    let map = move |window: Option<ass_core::window::WindowId>, natural: ass_core::Rect| {
        let Some((cell, base, win_size)) = window.and_then(|id| cells.get(&id)) else {
            return natural;
        };
        let k = cell.size.w as f32 / win_size.w.max(1) as f32;
        let remap = |v: i32, b: i32| (v - b) as f32 * k;
        ass_core::Rect::new(
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
    let shm = server.toplevel_frames();
    let dmabuf = server.toplevel_dmabuf_frames();
    let sub_shm_below = server.subsurface_frames_below();
    let sub_shm_above = server.subsurface_frames_above();
    let sub_dmabuf_below = server.subsurface_dmabuf_frames_below();
    let sub_dmabuf_above = server.subsurface_dmabuf_frames_above();
    renderer.gc(shm
        .iter()
        .map(|frame| frame.id)
        .chain(dmabuf.iter().map(|frame| frame.id))
        .chain(sub_shm_below.iter().map(|frame| frame.id))
        .chain(sub_shm_above.iter().map(|frame| frame.id))
        .chain(sub_dmabuf_below.iter().map(|frame| frame.id))
        .chain(sub_dmabuf_above.iter().map(|frame| frame.id)));
    renderer.draw_subsurfaces_mapped(device, canvas, &sub_shm_below, &map);
    renderer.draw_dmabuf_subsurfaces_mapped(device, canvas, &sub_dmabuf_below, &map);
    renderer.draw_toplevels_mapped(device, canvas, &shm, &map);
    renderer.draw_dmabuf_toplevels_mapped(device, canvas, &dmabuf, &map);
    renderer.draw_subsurfaces_mapped(device, canvas, &sub_shm_above, &map);
    renderer.draw_dmabuf_subsurfaces_mapped(device, canvas, &sub_dmabuf_above, &map);
    canvas.restore();
}

/// Software cursor for direct KMS. First the XDG cursor theme
/// (`$XCURSOR_THEME`/`$XCURSOR_SIZE`, inheritance included) via
/// [`cursor::CursorCache`]; the hand-drawn glyph set below remains as the
/// fallback when the theme ships no image for the requested shape.
/// Client-provided cursor surfaces are already composited by `ass-server`;
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
        return;
    }
    draw_glyph_cursor(canvas, position, shape, scale);
}

/// Hand-drawn glyph fallback: used only when the XDG theme has no image for
/// the shape (or no theme is installed at all).
pub(super) fn draw_glyph_cursor(
    canvas: &flux::Canvas,
    position: (f32, f32),
    shape: u32,
    scale: f32,
) {
    let black = flux::rgba(12, 12, 16, 255);
    let white = flux::rgba(245, 245, 248, 255);
    canvas.save();
    canvas.scale(scale, scale);
    canvas.translate(position.0, position.1);

    let bar = |canvas: &flux::Canvas, x: f32, y: f32, w: f32, h: f32| {
        canvas.fill_rrect(x - 1.0, y - 1.0, w + 2.0, h + 2.0, 1.5, black);
        canvas.fill_rrect(x, y, w, h, 1.0, white);
    };
    match shape {
        7 | 8 | 32 | 36 => {
            bar(canvas, 8.0, 0.0, 3.0, 19.0);
            bar(canvas, 0.0, 8.0, 19.0, 3.0);
        }
        9 => {
            bar(canvas, 8.0, 0.0, 3.0, 20.0);
            bar(canvas, 4.0, 0.0, 11.0, 3.0);
            bar(canvas, 4.0, 17.0, 11.0, 3.0);
        }
        10 => {
            bar(canvas, 0.0, 8.0, 20.0, 3.0);
            bar(canvas, 0.0, 4.0, 3.0, 11.0);
            bar(canvas, 17.0, 4.0, 3.0, 11.0);
        }
        18 | 25 | 26 | 30 => {
            bar(canvas, 0.0, 8.0, 20.0, 3.0);
            bar(canvas, 0.0, 4.0, 3.0, 11.0);
            bar(canvas, 17.0, 4.0, 3.0, 11.0);
        }
        19 | 22 | 27 | 31 => {
            bar(canvas, 8.0, 0.0, 3.0, 20.0);
            bar(canvas, 4.0, 0.0, 11.0, 3.0);
            bar(canvas, 4.0, 17.0, 11.0, 3.0);
        }
        20 | 24 | 28 => {
            canvas.translate(2.0, 15.0);
            canvas.rotate(-std::f32::consts::FRAC_PI_4);
            bar(canvas, 0.0, 0.0, 22.0, 3.0);
            bar(canvas, 0.0, -3.0, 3.0, 9.0);
            bar(canvas, 19.0, -3.0, 3.0, 9.0);
        }
        21 | 23 | 29 => {
            canvas.translate(4.0, 0.0);
            canvas.rotate(std::f32::consts::FRAC_PI_4);
            bar(canvas, 0.0, 0.0, 22.0, 3.0);
            bar(canvas, 0.0, -3.0, 3.0, 9.0);
            bar(canvas, 19.0, -3.0, 3.0, 9.0);
        }
        _ => {
            // Arrow-like diagonal with a short tail; hotspot is (0, 0).
            canvas.rotate(-std::f32::consts::FRAC_PI_4);
            bar(canvas, 0.0, 0.0, 18.0, 4.0);
            bar(canvas, 10.0, 2.0, 4.0, 10.0);
        }
    }
    canvas.restore();
}
