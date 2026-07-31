//! Flux/Lens rendering for one or more session-lock surfaces.

use std::ffi::CStr;
use std::os::raw::c_void;

use aegis_design::{Design, themes};
use aegis_lock::{LockState, PresentationMode, lock_layout};
use ash::vk::{self, Handle};
use flux::{GradientStop, Image};
use lens::{Align, Color, Input, LayoutOpts, OverlayOpts, Rect, Theme, Ui};
use thiserror::Error;
use wayland_client::{Connection, Proxy, protocol::wl_surface};

use crate::identity::{Identity, clock_strings};

const INSTANCE_EXTENSIONS: [&CStr; 2] = [c"VK_KHR_surface", c"VK_KHR_wayland_surface"];
const DEVICE_EXTENSIONS: [&CStr; 1] = [c"VK_KHR_swapchain"];

#[derive(Debug, Error)]
pub enum RenderError {
    #[error("Vulkan is unavailable")]
    Vulkan,
    #[error(transparent)]
    Flux(#[from] flux::Error),
    #[error(transparent)]
    Lens(#[from] lens::Error),
    #[error("built-in lock wallpaper could not be decoded: {0}")]
    Wallpaper(#[from] image::ImageError),
    #[error("avatar image could not be prepared: {0}")]
    Avatar(String),
}

impl RenderError {
    /// Map an `aegis-avatar` error onto the lock's render error. Flux faults
    /// are preserved as-is; everything else becomes a descriptive `Avatar`.
    fn from_avatar(error: aegis_avatar::Error) -> Self {
        match error {
            aegis_avatar::Error::Flux(error) => RenderError::Flux(error),
            other => RenderError::Avatar(other.to_string()),
        }
    }
}

/// Whether the identity orb shows the user's picture, a 3D model, or the
/// gradient fallback. All three render through the same `draw_image` path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AvatarStatus {
    /// A still user avatar was loaded and is composited as the orb.
    Image,
    /// A VRM 3D model was loaded; `animated` reports whether VRMA clips move.
    Animated3d { animated: bool },
    /// No avatar configured (or a decode failure): the gradient orb instead.
    Fallback,
}

impl From<aegis_avatar::AvatarKind> for AvatarStatus {
    fn from(kind: aegis_avatar::AvatarKind) -> Self {
        match kind {
            aegis_avatar::AvatarKind::Still => AvatarStatus::Image,
            aegis_avatar::AvatarKind::Animated3d { animation } => AvatarStatus::Animated3d {
                animated: animation == aegis_avatar::AnimationSupport::Animated,
            },
        }
    }
}

pub struct Graphics {
    pub device: flux::Device,
    background: Image,
    avatar: Image,
    avatar_status: AvatarStatus,
    ash: AshBridge,
}

impl Graphics {
    pub fn new(connection: &Connection) -> Result<Self, RenderError> {
        let device = flux::Device::new(true, &INSTANCE_EXTENSIONS, &DEVICE_EXTENSIONS, 2)?;
        let background = load_background(&device)?;
        // Avatar loading is delegated to aegis-avatar: it resolves the XDG
        // search path, decodes still images or loads VRM models, and returns a
        // single circle-masked texture. When nothing is configured, this lock
        // screen supplies its own procedural gradient orb.
        let (avatar, avatar_status) = match aegis_avatar::Avatar::load(&device) {
            Ok(Some(loaded)) => {
                let kind = loaded.kind;
                (loaded.texture, AvatarStatus::from(kind))
            }
            Ok(None) => (
                aegis_avatar::procedural_orb(&device).map_err(RenderError::from_avatar)?,
                AvatarStatus::Fallback,
            ),
            // Any avatar error (a corrupt file, a GPU upload fault) is a real
            // fault worth surfacing; fall back to the procedural orb only for
            // the benign "no candidate" case handled above.
            Err(error) => {
                log::warn!("lock: avatar load failed, using procedural orb: {error}");
                (
                    aegis_avatar::procedural_orb(&device).map_err(RenderError::from_avatar)?,
                    AvatarStatus::Fallback,
                )
            }
        };
        let ash = AshBridge::new(connection, &device)?;
        Ok(Self {
            device,
            background,
            avatar,
            avatar_status,
            ash,
        })
    }

    pub fn create_surface(
        &self,
        connection: &Connection,
        wl_surface: &wl_surface::WlSurface,
        logical_size: (u32, u32),
        scale: i32,
    ) -> Result<LockRenderSurface, RenderError> {
        let scale = scale.max(1);
        wl_surface.set_buffer_scale(scale);
        let physical_size = (
            logical_size.0.max(1).saturating_mul(scale as u32),
            logical_size.1.max(1).saturating_mul(scale as u32),
        );
        let vk_surface = self.ash.create_surface(connection, wl_surface)?;
        let surface = match unsafe {
            flux::Surface::from_vk(
                &self.device,
                vk_surface.as_raw() as usize as *mut c_void,
                physical_size.0,
                physical_size.1,
                true,
            )
        } {
            Ok(surface) => surface,
            Err(error) => {
                self.ash.destroy_surface(vk_surface);
                return Err(error.into());
            }
        };
        LockRenderSurface::new(&self.device, surface, vk_surface, logical_size, scale)
    }

    pub fn destroy_surface(&self, surface: LockRenderSurface) {
        self.device.wait_idle();
        let vk_surface = surface.vk_surface;
        drop(surface);
        self.ash.destroy_surface(vk_surface);
    }

    pub fn render(
        &self,
        surface: &mut LockRenderSurface,
        state: &LockState,
        identity: &Identity,
        visual_progress: f32,
    ) -> Result<(), RenderError> {
        surface.render(
            &self.background,
            &self.avatar,
            self.avatar_status,
            state,
            identity,
            visual_progress,
        )
    }
}

pub struct LockRenderSurface {
    surface: flux::Surface,
    canvas: flux::Canvas,
    ui: Ui,
    vk_surface: vk::SurfaceKHR,
    logical_size: (u32, u32),
    scale: i32,
}

impl LockRenderSurface {
    fn new(
        device: &flux::Device,
        surface: flux::Surface,
        vk_surface: vk::SurfaceKHR,
        logical_size: (u32, u32),
        scale: i32,
    ) -> Result<Self, RenderError> {
        let canvas = flux::Canvas::new(&surface)?;
        let mut ui = unsafe { Ui::with_device(device.as_raw().cast::<lens::sys::flux_device>()) }?;
        ui.set_scale(scale as f32);
        ui.set_theme(themes::application(&Design::dark()));
        Ok(Self {
            surface,
            canvas,
            ui,
            vk_surface,
            logical_size,
            scale,
        })
    }

    pub fn resize(&mut self, logical_size: (u32, u32), scale: i32) -> Result<(), RenderError> {
        let scale = scale.max(1);
        let physical = (
            logical_size.0.max(1).saturating_mul(scale as u32),
            logical_size.1.max(1).saturating_mul(scale as u32),
        );
        if self.surface.size() != physical {
            self.surface.resize(physical.0, physical.1)?;
        }
        self.logical_size = logical_size;
        self.scale = scale;
        self.ui.set_scale(scale as f32);
        Ok(())
    }

    fn render(
        &mut self,
        background: &Image,
        avatar: &Image,
        avatar_status: AvatarStatus,
        state: &LockState,
        identity: &Identity,
        visual_progress: f32,
    ) -> Result<(), RenderError> {
        let frame = self.surface.begin_frame()?;
        let physical = self.surface.size();
        self.canvas
            .begin(&frame, Some(flux::rgba(8, 12, 24, 255)))?;
        draw_background(&self.canvas, background, physical);
        draw_materials(
            &self.canvas,
            avatar,
            avatar_status,
            self.logical_size,
            self.scale as f32,
            state,
            visual_progress,
        );

        let mut input = Input::new(
            (self.logical_size.0 as f32, self.logical_size.1 as f32),
            1.0 / 60.0,
        );
        input.set_cursor(-10_000.0, -10_000.0);
        let design = Design::dark();
        let progress = visual_progress.clamp(0.0, 1.0);
        let (clock, date) = clock_strings();
        self.ui.frame(&input, |ui| {
            ui.set_theme(lock_theme(&design, 255));
            draw_clock(ui, self.logical_size, &clock, &date);
            if state.presentation() == PresentationMode::Engaged || progress > 0.02 {
                draw_identity(ui, self.logical_size, state, identity, progress);
            }
        });
        unsafe {
            self.ui
                .render(self.canvas.as_raw().cast::<lens::sys::flux_canvas>())?;
        }
        self.canvas.end();
        frame.submit()?.present()?;
        Ok(())
    }
}

struct AshBridge {
    _entry: ash::Entry,
    _instance: ash::Instance,
    wayland: ash::khr::wayland_surface::Instance,
    surface: ash::khr::surface::Instance,
}

impl AshBridge {
    fn new(connection: &Connection, device: &flux::Device) -> Result<Self, RenderError> {
        let entry = unsafe { ash::Entry::load() }.map_err(|_| RenderError::Vulkan)?;
        let instance = unsafe {
            ash::Instance::load(
                entry.static_fn(),
                vk::Instance::from_raw(device.vk_instance() as usize as u64),
            )
        };
        let wayland = ash::khr::wayland_surface::Instance::new(&entry, &instance);
        let surface = ash::khr::surface::Instance::new(&entry, &instance);
        // Keep a system-backed connection requirement visible at the boundary.
        if connection.backend().display_ptr().is_null() {
            return Err(RenderError::Vulkan);
        }
        Ok(Self {
            _entry: entry,
            _instance: instance,
            wayland,
            surface,
        })
    }

    fn create_surface(
        &self,
        connection: &Connection,
        surface: &wl_surface::WlSurface,
    ) -> Result<vk::SurfaceKHR, RenderError> {
        let display = connection.backend().display_ptr();
        let proxy = surface.id().as_ptr();
        if display.is_null() || proxy.is_null() {
            return Err(RenderError::Vulkan);
        }
        let info = vk::WaylandSurfaceCreateInfoKHR::default()
            .display(display.cast())
            .surface(proxy.cast());
        unsafe { self.wayland.create_wayland_surface(&info, None) }.map_err(|_| RenderError::Vulkan)
    }

    fn destroy_surface(&self, surface: vk::SurfaceKHR) {
        unsafe {
            self.surface.destroy_surface(surface, None);
        }
    }
}

fn load_background(device: &flux::Device) -> Result<Image, RenderError> {
    let source = image::load_from_memory(include_bytes!(
        "../../../assets/wallpapers/procedural-generation.png"
    ))?
    .to_rgba8();
    let (width, height) = source.dimensions();
    // The lock background needs to stay sharp on high-DPI panels, so cap the
    // atlas at 3840 px (covers 4K @ 1× and 1440p @ 2×) rather than 2048,
    // which left ultrawide and retina outputs visibly soft. A mild 6.0 sigma
    // keeps the "defocused wallpaper" aesthetic without the smeared look the
    // previous 14.0 produced.
    let target_width = width.min(3840);
    let target_height =
        ((height as f64 * target_width as f64 / width.max(1) as f64).round() as u32).max(1);
    let reduced = if target_width != width {
        image::imageops::resize(
            &source,
            target_width,
            target_height,
            image::imageops::FilterType::Lanczos3,
        )
    } else {
        source
    };
    let blurred = image::imageops::blur(&reduced, 6.0);
    Ok(Image::from_bytes(
        device,
        blurred.width(),
        blurred.height(),
        flux::Format::FLUX_FORMAT_RGBA8_UNORM,
        blurred.as_raw(),
    )?)
}

fn draw_background(canvas: &flux::Canvas, image: &Image, output: (u32, u32)) {
    let (iw, ih) = image.size();
    let cover = (output.0 as f32 / iw.max(1) as f32).max(output.1 as f32 / ih.max(1) as f32);
    let width = iw as f32 * cover;
    let height = ih as f32 * cover;
    canvas.draw_image(
        image,
        (output.0 as f32 - width) * 0.5,
        (output.1 as f32 - height) * 0.5,
        width,
        height,
    );
    canvas.fill_rect(
        0.0,
        0.0,
        output.0 as f32,
        output.1 as f32,
        flux::rgba(3, 7, 16, 76),
    );
    canvas.fill_rect_linear_gradient(
        (0.0, 0.0, output.0 as f32, output.1 as f32),
        (0.0, 0.0),
        (0.0, output.1 as f32),
        &[
            GradientStop::new(0.0, flux::rgba(4, 8, 18, 70)),
            GradientStop::new(0.55, flux::rgba(5, 9, 20, 18)),
            GradientStop::new(1.0, flux::rgba(3, 5, 12, 145)),
        ],
    );
}

fn draw_materials(
    canvas: &flux::Canvas,
    avatar: &Image,
    avatar_status: AvatarStatus,
    logical: (u32, u32),
    scale: f32,
    state: &LockState,
    visual_progress: f32,
) {
    let layout = lock_layout(logical.0 as f32, logical.1 as f32);
    let p = visual_progress.clamp(0.0, 1.0);
    if state.presentation() == PresentationMode::Ambient && p <= 0.02 {
        return;
    }
    let center = layout.width * 0.5;
    let avatar_x = (center - layout.avatar_size * 0.5) * scale;
    let avatar_y = (layout.avatar_y + (1.0 - p) * 18.0) * scale;
    let avatar_size = layout.avatar_size * scale;
    // Soft white halo behind the orb (the only fill that legitimately spans
    // the full square — it is a translucent disc built from a round rect with
    // a 50% radius, so it stays circular and never shows square corners).
    canvas.fill_rrect(
        avatar_x - 3.0 * scale,
        avatar_y - 3.0 * scale,
        avatar_size + 6.0 * scale,
        avatar_size + 6.0 * scale,
        avatar_size * 0.5 + 3.0 * scale,
        flux::rgba(255, 255, 255, (48.0 * p) as u8),
    );
    match avatar_status {
        AvatarStatus::Image | AvatarStatus::Animated3d { .. } | AvatarStatus::Fallback => {
            // Every avatar kind — a loaded photo, a rendered VRM model, or the
            // procedural fallback orb — is prepared upstream as a circle-masked,
            // premultiplied texture. A single draw_image composites a perfect
            // disc, so no square content can ever leak past the circular
            // keyline regardless of source aspect ratio or 3D framing.
            canvas.draw_image(avatar, avatar_x, avatar_y, avatar_size, avatar_size);
        }
    }
    // Crisp circular keyline drawn last so it frames whichever orb was used.
    canvas.fill_rrect(
        avatar_x,
        avatar_y,
        avatar_size,
        avatar_size,
        avatar_size * 0.5,
        flux::rgba(255, 255, 255, (28.0 * p) as u8),
    );

    let field_x = (center - layout.field_width * 0.5) * scale;
    let field_y = (layout.field_y + (1.0 - p) * 22.0) * scale;
    let field_w = layout.field_width * scale;
    let field_h = layout.field_height * scale;
    canvas.fill_rrect(
        field_x,
        field_y + 5.0 * scale,
        field_w,
        field_h,
        layout.field_height * 0.5 * scale,
        flux::rgba(0, 0, 0, (45.0 * p) as u8),
    );
    canvas.fill_rrect(
        field_x,
        field_y,
        field_w,
        field_h,
        layout.field_height * 0.5 * scale,
        flux::rgba(255, 255, 255, (48.0 * p) as u8),
    );
    canvas.stroke_rrect(
        field_x,
        field_y,
        field_w,
        field_h,
        layout.field_height * 0.5 * scale,
        flux::rgba(255, 255, 255, (92.0 * p) as u8),
        scale,
    );
}

fn draw_clock(ui: &mut lens::Frame, logical: (u32, u32), clock: &str, date: &str) {
    let layout = lock_layout(logical.0 as f32, logical.1 as f32);
    let width = layout.width.min(720.0);
    let x = (layout.width - width) * 0.5;
    ui.layer(
        "lock-clock",
        Rect {
            x,
            y: layout.clock_y,
            w: width,
            h: layout.clock_size + 12.0,
        },
        &centered_layer(),
        |ui| ui.label_compact_sized(clock, layout.clock_size),
    );
    ui.layer(
        "lock-date",
        Rect {
            x,
            y: layout.clock_y + layout.clock_size + 8.0,
            w: width,
            h: 28.0,
        },
        &centered_layer(),
        |ui| ui.label_compact_sized(date, if layout.height < 650.0 { 15.0 } else { 18.0 }),
    );
}

fn draw_identity(
    ui: &mut lens::Frame,
    logical: (u32, u32),
    state: &LockState,
    identity: &Identity,
    progress: f32,
) {
    let layout = lock_layout(logical.0 as f32, logical.1 as f32);
    let alpha = (255.0 * progress) as u8;
    let center = layout.width * 0.5;
    let shifted_avatar_y = layout.avatar_y + (1.0 - progress) * 18.0;
    ui.set_theme(lock_theme(&Design::dark(), alpha));
    ui.layer(
        "lock-avatar-label",
        Rect {
            x: center - layout.avatar_size * 0.5,
            y: shifted_avatar_y,
            w: layout.avatar_size,
            h: layout.avatar_size,
        },
        &centered_layer(),
        |ui| ui.label_compact_sized(&identity.initials, layout.avatar_size * 0.31),
    );
    ui.layer(
        "lock-display-name",
        Rect {
            x: center - 260.0,
            y: shifted_avatar_y + layout.avatar_size + 16.0,
            w: 520.0,
            h: 30.0,
        },
        &centered_layer(),
        |ui| ui.label_compact_sized(&identity.display_name, 19.0),
    );

    let field_y = layout.field_y + (1.0 - progress) * 22.0;
    let field_x = center - layout.field_width * 0.5;
    let dots = if state.password_len() == 0 {
        localized("Enter password", "输入密码")
    } else {
        let visible = state.password_len().min(18);
        format!(
            "{}{}",
            "•".repeat(visible),
            if state.password_len() > visible {
                "…"
            } else {
                ""
            }
        )
    };
    let muted = state.password_len() == 0;
    ui.set_theme(lock_theme(&Design::dark(), alpha).with_fg(if muted {
        Color::rgba(236, 240, 250, (145.0 * progress) as u8)
    } else {
        Color::rgba(255, 255, 255, alpha)
    }));
    ui.layer(
        "lock-password-content",
        Rect {
            x: field_x,
            y: field_y,
            w: layout.field_width,
            h: layout.field_height,
        },
        &OverlayOpts {
            pad: 0.0,
            cross: Align::Stretch,
            ..Default::default()
        },
        |ui| {
            ui.row_ex(
                &LayoutOpts {
                    width: layout.field_width,
                    height: layout.field_height,
                    pad: 15.0,
                    cross: Align::Center,
                    ..Default::default()
                },
                |ui| {
                    ui.label_compact_sized(&dots, 14.0);
                    ui.flex(1.0);
                    ui.label_compact_sized(if state.checking() { "···" } else { "→" }, 19.0);
                },
            );
        },
    );

    let status = if let Some(message) = state.message() {
        Some((message, true))
    } else if state.caps_lock() {
        Some((localized_ref("Caps Lock is on", "大写锁定已开启"), false))
    } else {
        state.keyboard_layout().map(|layout| (layout, false))
    };
    if let Some((message, error)) = status {
        let color = if error {
            Color::rgba(255, 174, 168, alpha)
        } else {
            Color::rgba(239, 242, 251, (190.0 * progress) as u8)
        };
        ui.set_theme(lock_theme(&Design::dark(), alpha).with_fg(color));
        ui.layer(
            "lock-status",
            Rect {
                x: center - 260.0,
                y: field_y + layout.field_height + 12.0,
                w: 520.0,
                h: 24.0,
            },
            &centered_layer(),
            |ui| ui.label_compact_sized(message, 12.0),
        );
    }
}

fn centered_layer() -> OverlayOpts {
    OverlayOpts {
        pad: 0.0,
        cross: Align::Center,
        ..Default::default()
    }
}

fn lock_theme(design: &Design, alpha: u8) -> Theme {
    themes::application(design)
        .with_bg(Color::TRANSPARENT)
        .with_fg(Color::rgba(248, 250, 255, alpha))
        .with_border(Color::rgba(255, 255, 255, alpha / 3))
}

fn localized(en: &str, zh: &str) -> String {
    localized_ref(en, zh).to_owned()
}

fn localized_ref<'a>(en: &'a str, zh: &'a str) -> &'a str {
    let locale = std::env::var("LC_ALL")
        .or_else(|_| std::env::var("LC_MESSAGES"))
        .or_else(|_| std::env::var("LANG"))
        .unwrap_or_default();
    if locale.starts_with("zh") { zh } else { en }
}
