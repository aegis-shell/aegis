use super::*;

/// One per-window pixel-capture request from an IPC connection thread,
/// answered by the main loop after it renders the window's surface tree into
/// a fresh offscreen target.
pub(super) struct WindowCaptureRequest {
    pub(super) window: tessera_model::window::WindowId,
    pub(super) reply: std::sync::mpsc::Sender<Result<tessera_ipc::CaptureWindowPayload, String>>,
}

/// Geometry and scale captured with the frame so the worker can assemble the
/// IPC payload without touching the compositor again.
pub(super) struct WindowCaptureContext {
    pub(super) window: tessera_model::window::WindowId,
    pub(super) scale_milli: u32,
    pub(super) rect: tessera_model::Rect,
}

/// A window capture whose frame was submitted and whose readback is still in
/// flight. The offscreen surface is deliberately not cached: it is created
/// per capture, owned here until the readback completes, and then dropped.
pub(super) struct PendingWindowCapture {
    pub(super) surface: flux::Surface,
    pub(super) readback: PendingReadback,
    pub(super) context: WindowCaptureContext,
    pub(super) reply: std::sync::mpsc::Sender<Result<tessera_ipc::CaptureWindowPayload, String>>,
}

pub(super) struct PreparedWindowCapture {
    pub(super) surface: flux::Surface,
    pub(super) readback: PendingReadback,
    pub(super) context: WindowCaptureContext,
}

/// One window's render geometry for an offscreen tree capture: the resolved
/// capture scale, the physical-pixel target extent it implies, and the
/// toplevel's logical placement at query time. Shared by the one-shot
/// `CaptureWindow` path and the per-window stream render targets
/// (ADR-0127).
pub(super) struct WindowTreeGeometry {
    pub(super) window: tessera_model::window::WindowId,
    pub(super) scale_milli: u32,
    pub(super) physical_width: u32,
    pub(super) physical_height: u32,
    /// Toplevel logical origin when the geometry was resolved; the render
    /// maps it to (0, 0), so a pure position move never changes the target.
    pub(super) origin: tessera_model::Point,
    /// Toplevel logical extent when the geometry was resolved.
    pub(super) logical_size: tessera_model::Size,
}

/// Resolve one window's offscreen-capture geometry from the live model. The
/// window is looked up across every workspace (`all_windows`): occluded,
/// minimized, and foreign-workspace windows stay capturable. The physical
/// extent follows the scale of the output the window is currently on, so a
/// move between differently scaled outputs changes it (a stream treats that
/// as a geometry change).
pub(super) fn window_tree_geometry(
    server: &tessera_compositor::Server,
    window: tessera_model::window::WindowId,
) -> Result<WindowTreeGeometry, String> {
    let model = server
        .all_windows()
        .into_iter()
        .find(|candidate| candidate.id == window)
        .ok_or_else(|| format!("unknown window {}", window.0))?;
    if model.size.w <= 0 || model.size.h <= 0 {
        return Err(format!("window {} has no measurable extent", window.0));
    }
    let scale_milli = window_capture_scale_milli(server, &model);
    let physical = |value: i32| {
        u32::try_from(
            u64::from(value as u32)
                .saturating_mul(u64::from(scale_milli))
                .div_ceil(1000),
        )
        .map_err(|_| "window capture extent overflows".to_owned())
    };
    Ok(WindowTreeGeometry {
        window,
        scale_milli,
        physical_width: physical(model.size.w)?.max(1),
        physical_height: physical(model.size.h)?.max(1),
        origin: model.position,
        logical_size: model.size,
    })
}

/// The capture scale for one window in milli-units: the output the window is
/// currently visible on (center-point hit test), the focused — here primary —
/// output when the window is occluded, minimized, or on another workspace,
/// and 1000 when no output exists at all.
fn window_capture_scale_milli(
    server: &tessera_compositor::Server,
    window: &tessera_model::window::Window,
) -> u32 {
    let outputs = server.output_infos();
    let visible = server
        .windows()
        .iter()
        .any(|candidate| candidate.id == window.id);
    let center = tessera_model::Point {
        x: window.position.x + window.size.w / 2,
        y: window.position.y + window.size.h / 2,
    };
    let output = if visible {
        outputs
            .iter()
            .find(|output| {
                let origin = output.geometry.logical_origin;
                let size = output.geometry.logical_size();
                center.x >= origin.x
                    && center.x < origin.x + size.w
                    && center.y >= origin.y
                    && center.y < origin.y + size.h
            })
            .or(outputs.first())
    } else {
        outputs.first()
    };
    output
        .map(|output| (output.geometry.scale.as_f32() * 1000.0).round() as u32)
        .filter(|scale| *scale > 0)
        .unwrap_or(1000)
}

/// Draw one window's complete surface tree into an open opaque pass on
/// `canvas`, mapped so the toplevel's logical origin lands at (0, 0). The
/// window keeps its real content whether it is visible, occluded, minimized,
/// or on another workspace; popups extending past the toplevel bounds are
/// clipped by the target extent (mirroring `StreamTarget::Window`
/// semantics). Shared by the one-shot capture and per-window streams.
pub(super) fn draw_window_tree(
    device: &flux::Device,
    renderer: &mut tessera_render::Renderer,
    server: &tessera_compositor::Server,
    canvas: &flux::Canvas,
    geometry: &WindowTreeGeometry,
) {
    let scale = geometry.scale_milli as f32 / 1000.0;
    canvas.save();
    if scale != 1.0 {
        canvas.scale(scale, scale);
    }
    let origin = geometry.origin;
    let map = move |_: Option<tessera_model::window::WindowId>, natural: tessera_model::Rect| {
        tessera_model::Rect::new(
            natural.origin.x - origin.x,
            natural.origin.y - origin.y,
            natural.size.w,
            natural.size.h,
        )
    };
    let shm = server.window_capture_frames(geometry.window);
    let dmabuf = server.window_capture_dmabuf_frames(geometry.window);
    let surface_order = server.window_capture_frame_order(geometry.window);
    renderer.draw_surfaces_ordered_mapped(device, canvas, &surface_order, &shm, &dmabuf, &map);
    canvas.restore();
}

/// Render one window's complete surface tree into a fresh offscreen readback
/// target and submit the frame. The window keeps its real content whether it
/// is visible, occluded, minimized, or on another workspace; the image's
/// origin is the toplevel's logical origin, so popups extending past the
/// toplevel bounds are clipped by the target extent (mirroring
/// `StreamTarget::Window` semantics).
pub(super) fn begin_window_capture(
    device: &flux::Device,
    renderer: &mut tessera_render::Renderer,
    server: &tessera_compositor::Server,
    window: tessera_model::window::WindowId,
    security_generation: u64,
    scheme: tessera_model::settings::ColorScheme,
) -> Result<PreparedWindowCapture, String> {
    let geometry = window_tree_geometry(server, window)?;
    let physical_width = geometry.physical_width;
    let physical_height = geometry.physical_height;
    let surface = flux::Surface::offscreen_readback(device, physical_width, physical_height)
        .map_err(|error| {
            format!(
                "allocate window {} render target: {error}{}",
                window.0,
                flux_last_error_detail()
            )
        })?;
    surface.prepare_readback().map_err(|error| {
        format!(
            "prepare window {} readback: {error}{}",
            window.0,
            flux_last_error_detail()
        )
    })?;
    let canvas = flux::Canvas::new(&surface).map_err(|error| {
        format!(
            "create window {} canvas: {error}{}",
            window.0,
            flux_last_error_detail()
        )
    })?;
    let mut frame = surface.begin_frame().map_err(|error| {
        format!(
            "begin window {} frame: {error}{}",
            window.0,
            flux_last_error_detail()
        )
    })?;
    begin_opaque_frame(&canvas, &frame, interaction_domain_clear(scheme)).map_err(|error| {
        format!(
            "begin window {} canvas: {error}{}",
            window.0,
            flux_last_error_detail()
        )
    })?;
    draw_window_tree(device, renderer, server, &canvas, &geometry);
    canvas.end_frame_checked().map_err(|error| {
        format!(
            "end window {} canvas: {error}{}",
            window.0,
            flux_last_error_detail()
        )
    })?;
    frame.request_readback().map_err(|error| {
        format!(
            "request window {} readback: {error}{}",
            window.0,
            flux_last_error_detail()
        )
    })?;
    frame
        .submit()
        .and_then(flux::SubmittedFrame::present)
        .map_err(|error| {
            format!(
                "submit window {} frame: {error}{}",
                window.0,
                flux_last_error_detail()
            )
        })?;
    Ok(PreparedWindowCapture {
        surface,
        readback: PendingReadback {
            width: physical_width,
            height: physical_height,
            crop: None,
            cursor: None,
            security_generation,
        },
        context: WindowCaptureContext {
            window,
            scale_milli: geometry.scale_milli,
            rect: tessera_model::Rect {
                origin: geometry.origin,
                size: geometry.logical_size,
            },
        },
    })
}
