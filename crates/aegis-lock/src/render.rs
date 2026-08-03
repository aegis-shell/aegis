//! Flux/Lens rendering for one or more lock-content surfaces.

use std::ffi::CStr;
use std::os::raw::c_void;
use std::path::{Path, PathBuf};
use std::time::Instant;

use aegis_config::{
    ColorScheme, LockScreenBackgroundConfig, LockScreenBackgroundMode, LockScreenStyle,
};
use aegis_design::{Design, themes};
use aegis_lock::{LockState, PresentationMode, lock_layout_for};
use ash::vk::{self, Handle};
use flux::{GradientStop, Image};
use lens::{Align, Color, Input, LayoutOpts, OverlayOpts, Rect, Theme, Ui};
use thiserror::Error;
use wayland_client::{Connection, Proxy, protocol::wl_surface};

use crate::identity::{Identity, clock_strings};

const INSTANCE_EXTENSIONS: [&CStr; 2] = [c"VK_KHR_surface", c"VK_KHR_wayland_surface"];
const DEVICE_EXTENSIONS: [&CStr; 1] = [c"VK_KHR_swapchain"];
const REJECTION_RGB: [u8; 3] = [255, 72, 84];
const VALIDATION_RGB: [u8; 3] = [190, 226, 255];

#[derive(Debug, Error)]
pub enum RenderError {
    #[error("Vulkan is unavailable")]
    Vulkan,
    #[error(transparent)]
    Flux(#[from] flux::Error),
    #[error(transparent)]
    Lens(#[from] lens::Error),
    #[error(transparent)]
    Wallpaper(#[from] aegis_wallpaper::Error),
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

/// Whether the identity disc shows the user's picture, a 3D model, or the
/// flat initial fallback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AvatarStatus {
    /// A still user avatar was loaded and is composited as a circular crop.
    Image,
    /// A VRM 3D model was loaded; `animated` reports whether VRMA clips move.
    Animated3d { animated: bool },
    /// No avatar configured (or a decode failure): a scheme-aware flat disc.
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
    background: LockBackground,
    visual: LockVisual,
    avatar: AvatarResource,
    avatar_status: AvatarStatus,
    avatar_watcher: Option<aegis_avatar::AvatarWatcher>,
    ash: AshBridge,
}

enum AvatarResource {
    Loaded(aegis_avatar::Avatar),
    Fallback,
}

impl AvatarResource {
    fn texture(&self) -> Option<&Image> {
        match self {
            Self::Loaded(avatar) => Some(avatar.texture()),
            Self::Fallback => None,
        }
    }

    fn is_animated(&self) -> bool {
        matches!(self, Self::Loaded(avatar) if avatar.is_animated())
    }

    fn advance(&mut self, delta_seconds: f32) -> Result<bool, aegis_avatar::Error> {
        match self {
            Self::Loaded(avatar) => avatar.advance(delta_seconds),
            Self::Fallback => Ok(false),
        }
    }

    fn current_motion(&self) -> Option<&str> {
        match self {
            Self::Loaded(avatar) => avatar.current_motion(),
            Self::Fallback => None,
        }
    }
}

enum LockBackground {
    Wallpaper(Box<aegis_wallpaper::Wallpaper>),
    Solid([u8; 3]),
}

#[derive(Clone, Copy)]
struct LockPalette {
    foreground: Color,
    muted: Color,
    avatar_fill: [u8; 3],
    avatar_foreground: Color,
}

#[derive(Clone, Copy)]
struct LockVisual {
    style: LockScreenStyle,
    palette: LockPalette,
    dim: f32,
    reduced_motion: bool,
}

#[derive(Debug, Default)]
pub struct GraphicsOptions {
    pub style: Option<LockScreenStyle>,
    pub background: Option<PathBuf>,
}

impl Graphics {
    pub fn new(connection: &Connection) -> Result<Self, RenderError> {
        Self::new_with_options(connection, GraphicsOptions::default())
    }

    pub fn new_with_options(
        connection: &Connection,
        options: GraphicsOptions,
    ) -> Result<Self, RenderError> {
        let device = flux::Device::new(true, &INSTANCE_EXTENSIONS, &DEVICE_EXTENSIONS, 2)?;
        let resolved = resolve_lock_appearance(options);
        let background = load_background(&resolved.background, resolved.config_path.as_deref())?;
        let (avatar, avatar_status, avatar_watcher) = if resolved.style == LockScreenStyle::Centered
        {
            // Only the centered composition owns an identity portrait. Avoid
            // decoding, uploading, animating, or watching avatar resources for
            // the deliberately typographic cinematic composition.
            let (avatar, status) = match aegis_avatar::Avatar::load_transactional(&device) {
                Ok(Some(mut loaded)) => {
                    if loaded.play_motion("greeting") {
                        log::debug!("lock: playing avatar action \"greeting\"");
                    } else if let Some(name) = loaded.play_random_action() {
                        log::debug!("lock: playing avatar action {name:?}");
                    }
                    let kind = loaded.kind();
                    (AvatarResource::Loaded(loaded), AvatarStatus::from(kind))
                }
                Ok(None) => (AvatarResource::Fallback, AvatarStatus::Fallback),
                // A bad avatar must not make the centered lock unusable.
                Err(error) => {
                    log::warn!("lock: avatar load failed, using initial fallback: {error}");
                    (AvatarResource::Fallback, AvatarStatus::Fallback)
                }
            };
            let watcher = match aegis_avatar::AvatarWatcher::new() {
                Ok(watcher) => Some(watcher),
                Err(error) => {
                    log::warn!("lock: avatar hot reload disabled: {error}");
                    None
                }
            };
            (avatar, status, watcher)
        } else {
            (AvatarResource::Fallback, AvatarStatus::Fallback, None)
        };
        let ash = AshBridge::new(connection, &device)?;
        Ok(Self {
            device,
            background,
            visual: LockVisual {
                style: resolved.style,
                palette: resolved.palette,
                dim: resolved.background.dim,
                reduced_motion: resolved.reduced_motion,
            },
            avatar,
            avatar_status,
            avatar_watcher,
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
        &mut self,
        surface: &mut LockRenderSurface,
        state: &LockState,
        identity: &Identity,
        visual_progress: f32,
        now: Instant,
    ) -> Result<(), RenderError> {
        surface.render(
            RenderAssets {
                device: &self.device,
                background: &mut self.background,
                avatar: self.avatar.texture(),
                avatar_status: self.avatar_status,
                visual: self.visual,
            },
            state,
            identity,
            visual_progress,
            now,
        )
    }

    #[must_use]
    pub fn feedback_animation_active(&self, state: &LockState, now: Instant) -> bool {
        !self.visual.reduced_motion
            && (state.rejection_feedback_progress(now).is_some()
                || state.validation_feedback_progress(now).is_some())
    }

    pub fn advance_avatar(&mut self, delta_seconds: f32) -> Result<bool, RenderError> {
        self.avatar
            .advance(delta_seconds)
            .map_err(RenderError::from_avatar)
    }

    #[must_use]
    pub fn avatar_is_animated(&self) -> bool {
        self.avatar.is_animated()
    }

    #[must_use]
    pub fn avatar_reload_pending(&self) -> bool {
        self.avatar_watcher
            .as_ref()
            .is_some_and(aegis_avatar::AvatarWatcher::needs_poll)
    }

    /// Build and publish an avatar replacement on the render thread. Failed
    /// or partial sources leave the last-known-good resource untouched.
    pub fn reload_avatar_if_ready(&mut self) -> bool {
        let ready = self
            .avatar_watcher
            .as_mut()
            .is_some_and(aegis_avatar::AvatarWatcher::poll);
        if !ready {
            return false;
        }
        if let Some(watcher) = &mut self.avatar_watcher
            && let Err(error) = watcher.refresh()
        {
            log::warn!("lock: could not refresh avatar watches: {error}");
        }
        let previous_motion = self.avatar.current_motion().map(str::to_owned);
        match aegis_avatar::Avatar::load_transactional(&self.device) {
            Ok(Some(mut loaded)) => {
                let restored = previous_motion
                    .as_deref()
                    .is_some_and(|name| loaded.play_motion(name));
                if !restored && loaded.play_motion("greeting") {
                    log::debug!("lock: playing avatar action \"greeting\" after reload");
                } else if !restored && let Some(name) = loaded.play_random_action() {
                    log::debug!("lock: playing avatar action {name:?} after reload");
                }
                self.avatar_status = AvatarStatus::from(loaded.kind());
                self.avatar = AvatarResource::Loaded(loaded);
                log::info!("lock: avatar hot reloaded");
                true
            }
            Ok(None) => {
                self.avatar = AvatarResource::Fallback;
                self.avatar_status = AvatarStatus::Fallback;
                log::info!("lock: avatar removed, using initial fallback");
                true
            }
            Err(error) => {
                log::warn!("lock: avatar hot reload failed, keeping current: {error}");
                if let Some(watcher) = &mut self.avatar_watcher {
                    watcher.retry();
                }
                false
            }
        }
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

struct RenderAssets<'a> {
    device: &'a flux::Device,
    background: &'a mut LockBackground,
    avatar: Option<&'a Image>,
    avatar_status: AvatarStatus,
    visual: LockVisual,
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
        assets: RenderAssets<'_>,
        state: &LockState,
        identity: &Identity,
        visual_progress: f32,
        now: Instant,
    ) -> Result<(), RenderError> {
        let frame = self.surface.begin_frame()?;
        let physical = self.surface.size();
        self.canvas
            .begin(&frame, Some(flux::rgba(8, 12, 24, 255)))?;
        draw_background(
            &self.canvas,
            assets.device,
            assets.background,
            physical,
            assets.visual.style,
            assets.visual.dim,
        );
        let feedback_offset = if assets.visual.reduced_motion {
            0.0
        } else {
            state
                .rejection_feedback_progress(now)
                .map_or(0.0, rejection_shake_offset)
        };
        draw_materials(
            &self.canvas,
            MaterialPresentation {
                avatar: assets.avatar,
                avatar_status: assets.avatar_status,
                visual: assets.visual,
                logical: self.logical_size,
                scale: self.scale as f32,
                state,
                progress: visual_progress,
                feedback_offset,
                now,
            },
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
            ui.set_theme(lock_theme(&design, assets.visual.palette.foreground, 255));
            draw_clock(ui, self.logical_size, assets.visual, &clock, &date);
            if state.presentation() == PresentationMode::Engaged || progress > 0.02 {
                draw_identity(
                    ui,
                    IdentityPresentation {
                        logical: self.logical_size,
                        visual: assets.visual,
                        avatar_status: assets.avatar_status,
                        state,
                        identity,
                        progress,
                        feedback_offset,
                    },
                );
            }
        });
        unsafe {
            self.ui
                .render(self.canvas.as_raw().cast::<lens::sys::flux_canvas>())?;
        }
        self.canvas.end_checked()?;
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

struct ResolvedLockAppearance {
    style: LockScreenStyle,
    background: LockScreenBackgroundConfig,
    config_path: Option<PathBuf>,
    palette: LockPalette,
    reduced_motion: bool,
}

fn resolve_lock_appearance(options: GraphicsOptions) -> ResolvedLockAppearance {
    let config_path = aegis_config::default_path();
    let config = config_path
        .as_deref()
        .and_then(|path| match aegis_config::load(path) {
            Ok(config) => config,
            Err(error) => {
                log::warn!("lock: ignoring invalid configuration: {error}");
                None
            }
        });
    let mut lock = config
        .as_ref()
        .map(|config| config.lock_screen.clone())
        .unwrap_or_default();
    if let Some(style) = options.style {
        lock.style = style;
    }
    if let Some(source) = options.background {
        let source = if source.is_absolute() {
            source
        } else {
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join(source)
        };
        lock.background.mode = LockScreenBackgroundMode::Image;
        lock.background.source = Some(source.to_string_lossy().into_owned());
        lock.background.color = None;
    }
    let preferences = config
        .as_ref()
        .map(aegis_config::Config::desktop_preferences)
        .unwrap_or_default();
    let palette = lock_palette(
        preferences.color_scheme,
        preferences
            .accent_color
            .map(|color| [color.red, color.green, color.blue]),
        lock.background.mode,
    );
    ResolvedLockAppearance {
        style: lock.style,
        background: lock.background,
        config_path,
        palette,
        reduced_motion: preferences.reduced_motion,
    }
}

fn load_background(
    config: &LockScreenBackgroundConfig,
    config_path: Option<&Path>,
) -> Result<LockBackground, RenderError> {
    match config.mode {
        LockScreenBackgroundMode::Builtin => Ok(LockBackground::Wallpaper(Box::new(
            aegis_wallpaper::Wallpaper::from_static_image_bytes(
                include_bytes!("../../../assets/wallpapers/procedural-generation.png"),
                "bundled lock background",
            )?,
        ))),
        LockScreenBackgroundMode::Solid => {
            let color = config
                .color
                .as_deref()
                .and_then(|value| aegis_config::AccentColor::parse_hex(value).ok())
                .map_or([8, 12, 20], |color| [color.red, color.green, color.blue]);
            Ok(LockBackground::Solid(color))
        }
        LockScreenBackgroundMode::Image => {
            let source = config
                .source
                .as_deref()
                .expect("validated lock image source");
            let path = configured_asset_path(config_path, source);
            match aegis_wallpaper::Wallpaper::from_image_path(&path) {
                Ok(wallpaper) => Ok(LockBackground::Wallpaper(Box::new(wallpaper))),
                Err(error) => {
                    log::warn!(
                        "lock: independent background {} could not be loaded; using bundled background: {error}",
                        path.display()
                    );
                    Ok(LockBackground::Wallpaper(Box::new(
                        aegis_wallpaper::Wallpaper::from_static_image_bytes(
                            include_bytes!("../../../assets/wallpapers/procedural-generation.png"),
                            "bundled lock background",
                        )?,
                    )))
                }
            }
        }
    }
}

fn configured_asset_path(config_path: Option<&Path>, source: &str) -> PathBuf {
    let path = Path::new(source);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        config_path
            .and_then(Path::parent)
            .unwrap_or_else(|| Path::new("."))
            .join(path)
    }
}

fn lock_palette(
    scheme: ColorScheme,
    accent: Option<[u8; 3]>,
    background: LockScreenBackgroundMode,
) -> LockPalette {
    let light_surface =
        scheme == ColorScheme::Light && background == LockScreenBackgroundMode::Solid;
    let avatar_fill = accent.unwrap_or(if scheme == ColorScheme::Light {
        [216, 222, 232]
    } else {
        [37, 49, 70]
    });
    let avatar_is_light = u32::from(avatar_fill[0]) * 299
        + u32::from(avatar_fill[1]) * 587
        + u32::from(avatar_fill[2]) * 114
        > 155_000;
    LockPalette {
        foreground: if light_surface {
            Color::rgba(25, 30, 42, 255)
        } else {
            Color::rgba(247, 248, 252, 255)
        },
        muted: if light_surface {
            Color::rgba(48, 56, 72, 174)
        } else {
            Color::rgba(229, 233, 242, 176)
        },
        avatar_fill,
        avatar_foreground: if avatar_is_light {
            Color::rgba(26, 31, 43, 255)
        } else {
            Color::rgba(250, 251, 254, 255)
        },
    }
}

fn draw_background(
    canvas: &flux::Canvas,
    device: &flux::Device,
    background: &mut LockBackground,
    output: (u32, u32),
    style: LockScreenStyle,
    dim: f32,
) {
    let artwork = matches!(&*background, LockBackground::Wallpaper(_));
    match background {
        LockBackground::Wallpaper(wallpaper) => {
            wallpaper.draw_cover(device, canvas, output.0 as f32, output.1 as f32);
        }
        LockBackground::Solid([red, green, blue]) => {
            canvas.fill_rect(
                0.0,
                0.0,
                output.0 as f32,
                output.1 as f32,
                flux::rgba(*red, *green, *blue, 255),
            );
        }
    }
    if !artwork {
        return;
    }
    let dim = (dim.clamp(0.0, 0.85) * 255.0).round() as u8;
    canvas.fill_rect(
        0.0,
        0.0,
        output.0 as f32,
        output.1 as f32,
        flux::rgba(3, 6, 12, dim),
    );
    let (start, end, stops) = match style {
        LockScreenStyle::Centered => (
            (0.0, 0.0),
            (0.0, output.1 as f32),
            [
                GradientStop::new(0.0, flux::rgba(2, 5, 12, 54)),
                GradientStop::new(0.55, flux::rgba(2, 5, 12, 10)),
                GradientStop::new(1.0, flux::rgba(2, 5, 12, 110)),
            ],
        ),
        LockScreenStyle::Cinematic => (
            (0.0, output.1 as f32 * 0.18),
            (output.0 as f32, output.1 as f32),
            [
                GradientStop::new(0.0, flux::rgba(2, 4, 9, 6)),
                GradientStop::new(0.58, flux::rgba(2, 4, 9, 34)),
                GradientStop::new(1.0, flux::rgba(2, 4, 9, 176)),
            ],
        ),
    };
    canvas.fill_rect_linear_gradient(
        (0.0, 0.0, output.0 as f32, output.1 as f32),
        start,
        end,
        &stops,
    );
}

struct MaterialPresentation<'a> {
    avatar: Option<&'a Image>,
    avatar_status: AvatarStatus,
    visual: LockVisual,
    logical: (u32, u32),
    scale: f32,
    state: &'a LockState,
    progress: f32,
    feedback_offset: f32,
    now: Instant,
}

fn draw_materials(canvas: &flux::Canvas, presentation: MaterialPresentation<'_>) {
    let MaterialPresentation {
        avatar,
        avatar_status,
        visual,
        logical,
        scale,
        state,
        progress: visual_progress,
        feedback_offset,
        now,
    } = presentation;
    let LockVisual {
        style,
        palette,
        reduced_motion,
        ..
    } = visual;
    let layout = lock_layout_for(style, logical.0 as f32, logical.1 as f32);
    let p = visual_progress.clamp(0.0, 1.0);
    if state.presentation() == PresentationMode::Ambient && p <= 0.02 {
        return;
    }
    if style == LockScreenStyle::Centered {
        let avatar_x = layout.avatar_x * scale;
        let avatar_y = (layout.avatar_y + (1.0 - p) * 18.0) * scale;
        let avatar_size = layout.avatar_size * scale;
        match avatar_status {
            AvatarStatus::Image | AvatarStatus::Animated3d { .. } => {
                // GPU-rendered VRM frames stay square internally; the analytic
                // rounded-image clip keeps every source a perfect disc without
                // a readback/re-upload on each animation frame.
                if let Some(avatar) = avatar {
                    canvas.draw_image_rrect(
                        avatar,
                        avatar_x,
                        avatar_y,
                        avatar_size,
                        avatar_size,
                        avatar_size * 0.5,
                    );
                }
            }
            AvatarStatus::Fallback => {
                let [red, green, blue] = palette.avatar_fill;
                canvas.fill_rrect(
                    avatar_x,
                    avatar_y,
                    avatar_size,
                    avatar_size,
                    avatar_size * 0.5,
                    flux::rgba(red, green, blue, (255.0 * p) as u8),
                );
            }
        }
        // A hairline frames both real avatars and the flat initial fallback.
        // It must remain a stroke: filling this shape is what washed the old
        // blue fallback toward white.
        canvas.stroke_rrect(
            avatar_x,
            avatar_y,
            avatar_size,
            avatar_size,
            avatar_size * 0.5,
            flux::rgba(255, 255, 255, (62.0 * p) as u8),
            scale,
        );
    }

    let field_x = (layout.field_x + feedback_offset) * scale;
    let field_y = (layout.field_y + (1.0 - p) * 22.0) * scale;
    let field_w = layout.field_width * scale;
    let field_h = layout.field_height * scale;
    match style {
        LockScreenStyle::Centered => {
            let [error_red, error_green, error_blue] = REJECTION_RGB;
            canvas.fill_rrect(
                field_x,
                field_y,
                field_w,
                field_h,
                10.0 * scale,
                if state.rejected() {
                    flux::rgba(38, 8, 14, (174.0 * p) as u8)
                } else {
                    flux::rgba(4, 8, 16, (142.0 * p) as u8)
                },
            );
            canvas.stroke_rrect(
                field_x,
                field_y,
                field_w,
                field_h,
                10.0 * scale,
                if state.rejected() {
                    flux::rgba(error_red, error_green, error_blue, (238.0 * p) as u8)
                } else if state.validation_pending() {
                    let [red, green, blue] = VALIDATION_RGB;
                    flux::rgba(red, green, blue, (168.0 * p) as u8)
                } else {
                    flux::rgba(255, 255, 255, (62.0 * p) as u8)
                },
                scale,
            );
        }
        LockScreenStyle::Cinematic => {
            let ([red, green, blue], rail_alpha) = if state.rejected() {
                (REJECTION_RGB, 218.0)
            } else if state.validation_pending() {
                (VALIDATION_RGB, 132.0)
            } else if state.password_len() > 0 {
                ([245, 247, 252], 132.0)
            } else {
                ([245, 247, 252], 82.0)
            };
            canvas.fill_rect(
                field_x,
                field_y + field_h - 1.5 * scale,
                field_w,
                1.5 * scale,
                flux::rgba(red, green, blue, (rail_alpha * p) as u8),
            );
            if state.validation_pending()
                && !reduced_motion
                && let Some(progress) = state.validation_feedback_progress(now)
            {
                draw_cinematic_validation_sweep(
                    canvas, field_x, field_y, field_w, field_h, scale, progress, p,
                );
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_cinematic_validation_sweep(
    canvas: &flux::Canvas,
    field_x: f32,
    field_y: f32,
    field_w: f32,
    field_h: f32,
    scale: f32,
    progress: f32,
    alpha: f32,
) {
    let sweep_w = (field_w * 0.28).clamp(72.0 * scale, 144.0 * scale);
    let sweep_x = field_x - sweep_w + (field_w + sweep_w) * progress.clamp(0.0, 1.0);
    let rail_y = field_y + field_h - 1.5 * scale;
    canvas.save();
    canvas.clip_rect(field_x, rail_y - 5.0 * scale, field_w, 10.0 * scale);
    canvas.fill_rect_linear_gradient(
        (sweep_x, rail_y - 4.0 * scale, sweep_w, 8.0 * scale),
        (sweep_x, rail_y),
        (sweep_x + sweep_w, rail_y),
        &[
            GradientStop::new(0.0, flux::rgba(150, 210, 255, 0)),
            GradientStop::new(0.5, flux::rgba(190, 226, 255, (112.0 * alpha) as u8)),
            GradientStop::new(1.0, flux::rgba(150, 210, 255, 0)),
        ],
    );
    canvas.fill_rect_linear_gradient(
        (sweep_x, rail_y, sweep_w, 1.5 * scale),
        (sweep_x, rail_y),
        (sweep_x + sweep_w, rail_y),
        &[
            GradientStop::new(0.0, flux::rgba(210, 238, 255, 0)),
            GradientStop::new(0.5, flux::rgba(226, 245, 255, (250.0 * alpha) as u8)),
            GradientStop::new(1.0, flux::rgba(210, 238, 255, 0)),
        ],
    );
    canvas.restore();
}

fn draw_clock(
    ui: &mut lens::Frame,
    logical: (u32, u32),
    visual: LockVisual,
    clock: &str,
    date: &str,
) {
    let LockVisual { style, palette, .. } = visual;
    let layout = lock_layout_for(style, logical.0 as f32, logical.1 as f32);
    let alignment = match style {
        LockScreenStyle::Centered => Align::Center,
        LockScreenStyle::Cinematic => Align::End,
    };
    ui.layer(
        "lock-clock",
        Rect {
            x: layout.clock_x,
            y: layout.clock_y,
            w: layout.clock_width,
            h: layout.clock_size + 12.0,
        },
        &aligned_layer(alignment),
        |ui| ui.label_compact_sized(clock, layout.clock_size),
    );
    ui.set_theme(lock_theme(&Design::dark(), palette.muted, 255));
    ui.layer(
        "lock-date",
        Rect {
            x: layout.clock_x,
            y: layout.clock_y + layout.clock_size + 8.0,
            w: layout.clock_width,
            h: 28.0,
        },
        &aligned_layer(alignment),
        |ui| {
            ui.label_compact_sized(
                date,
                if style == LockScreenStyle::Cinematic {
                    13.0
                } else if layout.height < 650.0 {
                    15.0
                } else {
                    18.0
                },
            );
        },
    );
}

struct IdentityPresentation<'a> {
    logical: (u32, u32),
    visual: LockVisual,
    avatar_status: AvatarStatus,
    state: &'a LockState,
    identity: &'a Identity,
    progress: f32,
    feedback_offset: f32,
}

fn draw_identity(ui: &mut lens::Frame, presentation: IdentityPresentation<'_>) {
    let IdentityPresentation {
        logical,
        visual,
        avatar_status,
        state,
        identity,
        progress,
        feedback_offset,
    } = presentation;
    let LockVisual { style, palette, .. } = visual;
    let layout = lock_layout_for(style, logical.0 as f32, logical.1 as f32);
    let alpha = (255.0 * progress) as u8;
    let shifted_avatar_y = layout.avatar_y + (1.0 - progress) * 18.0;
    if style == LockScreenStyle::Centered && avatar_status == AvatarStatus::Fallback {
        ui.set_theme(lock_theme(
            &Design::dark(),
            palette.avatar_foreground,
            alpha,
        ));
        ui.layer(
            "lock-avatar-label",
            Rect {
                x: layout.avatar_x,
                y: shifted_avatar_y,
                w: layout.avatar_size,
                h: layout.avatar_size,
            },
            &centered_layer(),
            |ui| {
                ui.row_ex(
                    &LayoutOpts {
                        width: layout.avatar_size,
                        height: layout.avatar_size,
                        pad: 0.0,
                        cross: Align::Center,
                        ..Default::default()
                    },
                    |ui| {
                        ui.flex(1.0);
                        ui.spacer(0.0);
                        ui.label_compact_sized(&identity.initials, layout.avatar_size * 0.36);
                        ui.flex(1.0);
                        ui.spacer(0.0);
                    },
                );
            },
        );
    }
    ui.set_theme(lock_theme(&Design::dark(), palette.foreground, alpha));
    let (name_x, name_y, name_width, name_height, name_alignment, name_size) = match style {
        LockScreenStyle::Centered => (
            (layout.width - 520.0) * 0.5,
            shifted_avatar_y + layout.avatar_size + 16.0,
            520.0,
            30.0,
            Align::Center,
            19.0,
        ),
        LockScreenStyle::Cinematic => (
            layout.field_x,
            layout.field_y - 50.0,
            layout.field_width * 0.64,
            32.0,
            Align::Start,
            24.0,
        ),
    };
    let display_name = if style == LockScreenStyle::Cinematic {
        identity.display_name.to_uppercase()
    } else {
        identity.display_name.clone()
    };
    ui.layer(
        "lock-display-name",
        Rect {
            x: name_x,
            y: name_y,
            w: name_width,
            h: name_height,
        },
        &aligned_layer(name_alignment),
        |ui| {
            if style == LockScreenStyle::Cinematic {
                // Keep the cinematic identity quiet and precise. Lens titles
                // are deliberately bold; the regular compact run gives this
                // line the lighter stroke requested by the composition.
                ui.label_compact_sized(&display_name, name_size);
            } else {
                ui.label_compact_sized(&display_name, name_size);
            }
        },
    );

    if style == LockScreenStyle::Cinematic
        && let Some(keyboard) = keyboard_status(state)
    {
        ui.set_theme(lock_theme(&Design::dark(), palette.muted, alpha));
        let indicator_width = layout.field_width * 0.32;
        ui.layer(
            "lock-keyboard-status",
            Rect {
                x: layout.field_x + layout.field_width - indicator_width,
                // Both boxes finish on the same line even though their
                // type sizes differ.
                y: name_y + name_height - 17.0,
                w: indicator_width,
                h: 17.0,
            },
            &aligned_layer(Align::End),
            |ui| ui.label_compact_sized(&keyboard, 11.0),
        );
    }

    let field_y = layout.field_y + (1.0 - progress) * 22.0;
    let field_x = layout.field_x + feedback_offset;
    let dots = if style == LockScreenStyle::Cinematic {
        cinematic_password_marks(state.password_len())
    } else if state.password_len() == 0 {
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
    let credential_label = credential_label(&dots, state.credential_revision());
    let muted = state.password_len() == 0 && !state.rejected() && !state.validation_pending();
    let credential_color = if state.rejected() {
        let [red, green, blue] = REJECTION_RGB;
        Color::rgba(red, green, blue, 255)
    } else if state.validation_pending() {
        let [red, green, blue] = VALIDATION_RGB;
        Color::rgba(red, green, blue, 224)
    } else if muted {
        palette.muted
    } else {
        palette.foreground
    };
    ui.set_theme(lock_theme(&Design::dark(), credential_color, alpha));
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
                    pad: if style == LockScreenStyle::Cinematic {
                        0.0
                    } else {
                        15.0
                    },
                    cross: Align::Center,
                    ..Default::default()
                },
                |ui| {
                    ui.label_compact_sized(
                        &credential_label,
                        if style == LockScreenStyle::Cinematic {
                            12.0
                        } else {
                            14.0
                        },
                    );
                },
            );
        },
    );

    let status = if state.rejected() {
        None
    } else if let Some(message) = state.message() {
        Some((message.to_owned(), true))
    } else if style == LockScreenStyle::Centered && state.validation_pending() {
        Some((localized("Checking…", "正在验证…"), false))
    } else if style == LockScreenStyle::Centered {
        keyboard_status(state).map(|status| (status, false))
    } else {
        None
    };
    if let Some((message, error)) = status {
        let color = if error {
            Color::rgba(255, 174, 168, 255)
        } else {
            palette.muted
        };
        ui.set_theme(lock_theme(&Design::dark(), color, alpha));
        let (status_y, alignment) = match style {
            LockScreenStyle::Centered => (field_y + layout.field_height + 12.0, Align::Center),
            LockScreenStyle::Cinematic => (field_y - 48.0, Align::End),
        };
        ui.layer(
            "lock-status",
            Rect {
                x: if style == LockScreenStyle::Centered {
                    (layout.width - 520.0) * 0.5
                } else {
                    field_x
                },
                y: status_y,
                w: if style == LockScreenStyle::Centered {
                    520.0
                } else {
                    layout.field_width
                },
                h: 24.0,
            },
            &aligned_layer(alignment),
            |ui| ui.label_compact_sized(&message, 12.0),
        );
    }
}

fn keyboard_status(state: &LockState) -> Option<String> {
    match (state.caps_lock(), state.keyboard_layout()) {
        (true, Some(layout)) => Some(format!("CAPS · {layout}")),
        (true, None) => Some("CAPS".to_owned()),
        (false, Some(layout)) => Some(layout.to_owned()),
        (false, None) => None,
    }
}

fn centered_layer() -> OverlayOpts {
    aligned_layer(Align::Center)
}

fn aligned_layer(alignment: Align) -> OverlayOpts {
    OverlayOpts {
        pad: 0.0,
        cross: alignment,
        ..Default::default()
    }
}

fn lock_theme(design: &Design, foreground: Color, alpha: u8) -> Theme {
    let (red, green, blue, source_alpha) = foreground.components();
    let alpha = ((u16::from(source_alpha) * u16::from(alpha)) / 255) as u8;
    themes::application(design)
        .with_bg(Color::TRANSPARENT)
        .with_fg(Color::rgba(red, green, blue, alpha))
        .with_border(Color::rgba(255, 255, 255, alpha / 3))
}

fn rejection_shake_offset(progress: f32) -> f32 {
    let progress = progress.clamp(0.0, 1.0);
    let envelope = 1.0 - progress;
    (progress * std::f32::consts::TAU * 3.0).sin() * 10.0 * envelope
}

fn cinematic_password_marks(password_len: usize) -> String {
    // `Secret` already bounds the input. Do not cap the visible sequence:
    // doing so makes both typing and deletion appear stuck above the cap.
    "◆  ".repeat(password_len).trim_end().to_owned()
}

fn credential_label(visible: &str, revision: u64) -> String {
    // Lens hides the `##` suffix while hashing the complete label as widget
    // identity. A monotonically changing suffix prevents deletion from
    // reviving an older retained node/record for the same shorter text.
    format!("{visible}##lock-credential-{revision}")
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

#[cfg(test)]
mod tests {
    use super::{cinematic_password_marks, credential_label, rejection_shake_offset};

    #[test]
    fn rejection_shake_crosses_both_sides_and_settles_at_origin() {
        assert!(rejection_shake_offset(1.0 / 12.0) > 0.0);
        assert!(rejection_shake_offset(3.0 / 12.0) < 0.0);
        assert_eq!(rejection_shake_offset(1.0), 0.0);
    }

    #[test]
    fn cinematic_password_marks_never_render_empty_placeholders() {
        assert_eq!(cinematic_password_marks(0), "");
        assert_eq!(cinematic_password_marks(2), "◆  ◆");
        assert_eq!(cinematic_password_marks(8), "◆  ◆  ◆  ◆  ◆  ◆  ◆  ◆");
        assert_ne!(cinematic_password_marks(8), cinematic_password_marks(7));
    }

    #[test]
    fn credential_edits_receive_unique_hidden_widget_identity() {
        let before = credential_label("◆  ◆", 7);
        let after_delete = credential_label("◆", 8);
        assert_eq!(before.split("##").next(), Some("◆  ◆"));
        assert_eq!(after_delete.split("##").next(), Some("◆"));
        assert_ne!(before, after_delete);
    }
}
