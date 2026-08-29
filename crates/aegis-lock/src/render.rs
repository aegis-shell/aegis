//! Flux/Lens rendering for one or more lock-content surfaces.

use std::ffi::CStr;
use std::os::raw::c_void;
use std::path::{Path, PathBuf};
use std::time::Instant;

use aegis_config::{
    ColorScheme, LockScreenBackgroundConfig, LockScreenBackgroundMode, LockScreenStyle,
};
use aegis_design::{Design, themes};
use aegis_lock::LockState;
use ash::vk::{self, Handle};
use flux::Image;
use lens::{Color, Input, Ui};
use thiserror::Error;
use wayland_client::{Connection, Proxy, protocol::wl_surface};

use crate::profile::{Profile, clock_strings};
use crate::style::{FramePresentation, painter_for};

const INSTANCE_EXTENSIONS: [&CStr; 2] = [c"VK_KHR_surface", c"VK_KHR_wayland_surface"];
const DEVICE_EXTENSIONS: [&CStr; 1] = [c"VK_KHR_swapchain"];
const AVATAR_CAMERA: aegis_shell::persona::VrmCamera =
    aegis_shell::persona::VrmCamera::new(28.0, 0.25, 0.48, 0.0);

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
    /// Map a persona portrait error onto the lock's render error. Flux faults
    /// are preserved as-is; everything else becomes a descriptive `Avatar`.
    fn from_avatar(error: aegis_shell::persona::Error) -> Self {
        match error {
            aegis_shell::persona::Error::Flux(error) => RenderError::Flux(error),
            other => RenderError::Avatar(other.to_string()),
        }
    }
}

/// Whether the profile disc shows the user's picture, a 3D model, or the
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

impl From<aegis_shell::persona::PortraitKind> for AvatarStatus {
    fn from(kind: aegis_shell::persona::PortraitKind) -> Self {
        match kind {
            aegis_shell::persona::PortraitKind::Still => AvatarStatus::Image,
            aegis_shell::persona::PortraitKind::Vrm { animation } => AvatarStatus::Animated3d {
                animated: animation == aegis_shell::persona::AnimationSupport::Animated,
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
    portrait_config: aegis_shell::persona::PortraitConfig,
    avatar_watcher: Option<aegis_shell::persona::PortraitWatcher>,
    ash: AshBridge,
}

enum AvatarResource {
    Loaded(aegis_shell::persona::Portrait),
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

    fn advance(&mut self, delta_seconds: f32) -> Result<bool, aegis_shell::persona::Error> {
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

pub(crate) enum LockBackground {
    Wallpaper(Box<aegis_wallpaper::Wallpaper>),
    Solid([u8; 3]),
}

#[derive(Clone, Copy)]
pub(crate) struct LockPalette {
    pub(crate) foreground: Color,
    pub(crate) muted: Color,
    pub(crate) avatar_fill: [u8; 3],
    pub(crate) avatar_foreground: Color,
}

#[derive(Clone, Copy)]
pub(crate) struct LockVisual {
    pub(crate) style: LockScreenStyle,
    pub(crate) palette: LockPalette,
    /// The scheme-resolved design tokens behind every lock surface theme.
    pub(crate) design: Design,
    pub(crate) dim: f32,
    pub(crate) reduced_motion: bool,
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
        let portrait_config = aegis_shell::persona::PortraitConfig::current();
        let (avatar, avatar_status, avatar_watcher) = if resolved.style == LockScreenStyle::Centered
        {
            // Only the centered composition owns a persona portrait. Avoid
            // decoding, uploading, animating, or watching avatar resources for
            // the deliberately typographic cinematic and bsod compositions.
            let (avatar, status) = match aegis_shell::persona::Portrait::load_transactional(
                &device,
                &portrait_config,
                AVATAR_CAMERA,
            ) {
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
            let watcher = match aegis_shell::persona::PortraitWatcher::new(&portrait_config) {
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
                design: resolved.design,
                dim: resolved.background.dim,
                reduced_motion: resolved.reduced_motion,
            },
            avatar,
            avatar_status,
            portrait_config,
            avatar_watcher,
            ash,
        })
    }

    pub fn create_surface(
        &self,
        connection: &Connection,
        wl_surface: &wl_surface::WlSurface,
        logical_size: (u32, u32),
        scale: f32,
    ) -> Result<LockRenderSurface, RenderError> {
        // Buffer scale is owned by the caller: fractional-scale sessions keep
        // `wl_surface.buffer_scale` at 1 and carry the density through a
        // `wp_viewport` destination, so this code must not touch it.
        let physical_size = physical_size(logical_size, scale);
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
        LockRenderSurface::new(
            &self.device,
            surface,
            vk_surface,
            logical_size,
            scale,
            &self.visual.design,
        )
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
        profile: &Profile,
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
            profile,
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

    /// Whether the current composition animates continuously while engaged.
    ///
    /// The stop-screen percentage counter rides a slow loop, so that
    /// composition keeps requesting frames even when nothing else moves.
    #[must_use]
    pub fn composition_animates(&self, state: &LockState) -> bool {
        painter_for(self.visual).animates_while_engaged(state)
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
            .is_some_and(aegis_shell::persona::PortraitWatcher::needs_poll)
    }

    /// Build and publish an avatar replacement on the render thread. Failed
    /// or partial sources leave the last-known-good resource untouched.
    pub fn reload_avatar_if_ready(&mut self) -> bool {
        let ready = self
            .avatar_watcher
            .as_mut()
            .is_some_and(aegis_shell::persona::PortraitWatcher::poll);
        if !ready {
            return false;
        }
        if let Some(watcher) = &mut self.avatar_watcher
            && let Err(error) = watcher.refresh()
        {
            log::warn!("lock: could not refresh avatar watches: {error}");
        }
        let previous_motion = self.avatar.current_motion().map(str::to_owned);
        match aegis_shell::persona::Portrait::load_transactional(
            &self.device,
            &self.portrait_config,
            AVATAR_CAMERA,
        ) {
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

/// Buffer extent for one logical size at an arbitrary (possibly fractional)
/// output scale. Rounded to the nearest device pixel; the compositor maps the
/// buffer onto the surface through the `wp_viewport` destination, so a
/// fractional product does not resample beyond sub-pixel error.
fn physical_size(logical_size: (u32, u32), scale: f32) -> (u32, u32) {
    let scale = scale.max(0.01);
    (
        (logical_size.0.max(1) as f32 * scale).round().max(1.0) as u32,
        (logical_size.1.max(1) as f32 * scale).round().max(1.0) as u32,
    )
}

pub struct LockRenderSurface {
    surface: flux::Surface,
    canvas: flux::Canvas,
    ui: Ui,
    vk_surface: vk::SurfaceKHR,
    logical_size: (u32, u32),
    scale: f32,
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
        scale: f32,
        design: &Design,
    ) -> Result<Self, RenderError> {
        let canvas = flux::Canvas::new(&surface)?;
        let mut ui = unsafe { Ui::with_device(device.as_raw().cast::<lens::sys::flux_device>()) }?;
        ui.set_scale(scale);
        ui.set_theme(themes::application(design));
        Ok(Self {
            surface,
            canvas,
            ui,
            vk_surface,
            logical_size,
            scale,
        })
    }

    pub fn resize(&mut self, logical_size: (u32, u32), scale: f32) -> Result<(), RenderError> {
        let physical = physical_size(logical_size, scale);
        if self.surface.size() != physical {
            self.surface.resize(physical.0, physical.1)?;
        }
        self.logical_size = logical_size;
        self.scale = scale;
        self.ui.set_scale(scale);
        Ok(())
    }

    fn render(
        &mut self,
        assets: RenderAssets<'_>,
        state: &LockState,
        profile: &Profile,
        visual_progress: f32,
        now: Instant,
    ) -> Result<(), RenderError> {
        let frame = self.surface.begin_frame()?;
        let physical = self.surface.size();
        self.canvas
            .begin_frame(Some(&frame), Some(flux::rgba(8, 12, 24, 255)))?;
        let painter = painter_for(assets.visual);
        painter.paint_background(
            &self.canvas,
            assets.device,
            assets.background,
            physical,
            assets.visual.dim,
        );
        let feedback_offset = if assets.visual.reduced_motion {
            0.0
        } else {
            state
                .rejection_feedback_progress(now)
                .map_or(0.0, crate::style::common::rejection_shake_offset)
        };
        let presentation = FramePresentation {
            logical: self.logical_size,
            avatar: assets.avatar,
            avatar_status: assets.avatar_status,
            state,
            profile,
            progress: visual_progress.clamp(0.0, 1.0),
            feedback_offset,
            now,
            scale: self.scale,
        };
        painter.paint_materials(&self.canvas, &presentation);

        let mut input = Input::new(
            (self.logical_size.0 as f32, self.logical_size.1 as f32),
            1.0 / 60.0,
        );
        input.set_cursor(-10_000.0, -10_000.0);
        let (clock, date) = clock_strings();
        let engaged = aegis_lock::PresentationMode::Engaged == state.presentation()
            || presentation.progress > 0.02;
        self.ui.frame(&input, |ui| {
            painter.paint_clock(ui, &presentation, &clock, &date);
            if engaged {
                painter.paint_identity(ui, &presentation);
            }
        });
        unsafe {
            self.ui
                .render(self.canvas.as_raw().cast::<lens::sys::flux_canvas>())?;
        }
        self.canvas.end_frame_checked()?;
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
    design: Design,
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
        design: Design::for_scheme(preferences.color_scheme),
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
    // Resolve "no preference" through the same fallback the shell design
    // tokens use, so the lock screen never renders an undecided scheme.
    let scheme = scheme.or_dark();
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

#[cfg(test)]
mod tests {
    // Composition-specific behavior (counter math, stop-code copy, QR
    // matrices, layout geometry) lives with its module:
    // `style::bsod`, `style::common`, `style::qr`, and `aegis_lock::ui`.
}
