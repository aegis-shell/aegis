//! Runtime backend selection and the common compositor-host facade.

use std::collections::HashMap;
use std::os::fd::OwnedFd;
use std::str::FromStr;
use std::time::Duration;

use ass_core::Size;
use ass_core::input::{
    InputEvent, PointerGestureEvent, TextInputEvent, TextInputState, TouchpadConfig, TouchpadStatus,
};
use ass_core::output::ModeSpec;

use crate::Backend;
use crate::drm::{DrmBackend, DrmError};
use crate::nested::{DEVICE_EXTENSIONS, INSTANCE_EXTENSIONS, NestedError, NestedHost};

/// Frame slots Flux runs concurrently. ADR-0038's frame pacing assumes three:
/// on DRM the offscreen ring must hold one image per slot so a frame being
/// rendered never aliases the image the CRTC is still scanning out (with the
/// flux default of two, rendering frame k reuses the buffer committed two
/// frames ago — exactly the one on screen until the pending flip lands).
const FRAMES_IN_FLIGHT: u32 = 3;

/// User-selected presentation backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    /// Use the outer Wayland compositor when one is present; otherwise use DRM.
    Auto,
    /// Drive atomic DRM/KMS directly.
    Drm,
    /// Present into an outer Wayland toplevel for development/testing.
    Nested,
}

impl FromStr for BackendKind {
    type Err = HostError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "auto" => Ok(Self::Auto),
            "drm" => Ok(Self::Drm),
            "nested" => Ok(Self::Nested),
            _ => Err(HostError::InvalidBackend(value.to_owned())),
        }
    }
}

/// Errors selecting, creating, or presenting through a host.
#[derive(Debug, thiserror::Error)]
pub enum HostError {
    #[error("unknown backend {0:?}; expected auto, drm, or nested")]
    InvalidBackend(String),
    #[error(transparent)]
    Nested(#[from] NestedError),
    #[error(transparent)]
    Drm(#[from] DrmError),
    #[error(transparent)]
    Flux(#[from] flux::Error),
}

/// One runtime-selected compositor host.
pub enum Host {
    Nested(NestedHost),
    Drm(DrmBackend),
}

impl Host {
    /// Open the selected backend. `configured_modes` carries the config's
    /// per-connector `mode` requests (ADR-0028); only the DRM path consumes
    /// them, so the very first modeset already honors the configured modes.
    pub fn open(
        kind: BackendKind,
        title: &str,
        width: i32,
        height: i32,
        configured_modes: HashMap<String, ModeSpec>,
    ) -> Result<Self, HostError> {
        match kind {
            BackendKind::Nested => Ok(Self::Nested(NestedHost::open(title, width, height)?)),
            BackendKind::Drm => Ok(Self::Drm(DrmBackend::open(configured_modes)?)),
            BackendKind::Auto => {
                let outer_wayland =
                    std::env::var_os("WAYLAND_DISPLAY").is_some_and(|value| !value.is_empty());
                if outer_wayland {
                    log::info!("backend: auto selected nested (WAYLAND_DISPLAY is set)");
                    Ok(Self::Nested(NestedHost::open(title, width, height)?))
                } else {
                    log::info!("backend: auto selected drm (no outer Wayland display)");
                    Ok(Self::Drm(DrmBackend::open(configured_modes)?))
                }
            }
        }
    }

    /// Create the Vulkan device with backend-mandatory extensions. dma-buf is
    /// optional for nested development, but mandatory for direct scanout.
    pub fn create_device(&self) -> Result<flux::Device, HostError> {
        match self {
            Self::Nested(_) => {
                let mut extensions = DEVICE_EXTENSIONS.to_vec();
                extensions.extend_from_slice(&flux::DMABUF_DEVICE_EXTENSIONS);
                extensions.extend_from_slice(&flux::DMABUF_SYNC_DEVICE_EXTENSIONS);
                match flux::Device::new(false, &INSTANCE_EXTENSIONS, &extensions, FRAMES_IN_FLIGHT) {
                    Ok(device) => Ok(device),
                    Err(error) => {
                        log::warn!(
                            "nested: explicit dma-buf sync unavailable ({error}); trying implicit-sync dma-buf"
                        );
                        let mut implicit_extensions = DEVICE_EXTENSIONS.to_vec();
                        implicit_extensions.extend_from_slice(&flux::DMABUF_DEVICE_EXTENSIONS);
                        match flux::Device::new(false, &INSTANCE_EXTENSIONS, &implicit_extensions, FRAMES_IN_FLIGHT) {
                            Ok(device) => Ok(device),
                            Err(error) => {
                                log::warn!(
                                    "nested: dma-buf Vulkan extensions unavailable ({error}); using swapchain-only device"
                                );
                                Ok(flux::Device::new(
                                    false,
                                    &INSTANCE_EXTENSIONS,
                                    &DEVICE_EXTENSIONS,
                                    FRAMES_IN_FLIGHT,
                                )?)
                            }
                        }
                    }
                }
            }
            Self::Drm(_) => {
                let mut extensions = flux::DMABUF_DEVICE_EXTENSIONS.to_vec();
                extensions.extend_from_slice(&flux::DMABUF_SYNC_DEVICE_EXTENSIONS);
                match flux::Device::new(true, &[], &extensions, FRAMES_IN_FLIGHT) {
                    Ok(device) => Ok(device),
                    Err(error) => {
                        log::warn!(
                            "drm: explicit-sync Vulkan extension unavailable ({error}); using fence-wait fallback"
                        );
                        Ok(flux::Device::new(
                            true,
                            &[],
                            &flux::DMABUF_DEVICE_EXTENSIONS,
                            FRAMES_IN_FLIGHT,
                        )?)
                    }
                }
            }
        }
    }

    pub fn create_surface(&mut self, device: &flux::Device) -> Result<flux::Surface, HostError> {
        match self {
            Self::Nested(host) => {
                let vk_surface = host.create_vk_surface(device)?;
                let (width, height) = host.physical_size();
                // SAFETY: NestedHost created this surface from device's Vulkan
                // instance and retains it for at least as long as Host.
                Ok(unsafe { flux::Surface::from_vk(device, vk_surface, width, height, true)? })
            }
            Self::Drm(host) => Ok(host.create_surface(device)?),
        }
    }

    pub fn present(
        &mut self,
        surface: &flux::Surface,
        frame: flux::SubmittedFrame<'_>,
    ) -> Result<Option<OwnedFd>, HostError> {
        match self {
            Self::Nested(_) => {
                frame.present()?;
                Ok(None)
            }
            Self::Drm(host) => Ok(host.present(surface, frame)?),
        }
    }

    /// Confirm completion of the most recently submitted secure frame.
    /// Direct DRM has an exact per-CRTC page-flip barrier. Nested mode is a
    /// development host and can only wait for completion of our Vulkan work;
    /// the outer compositor remains the physical presentation authority.
    pub fn wait_presented(&mut self, device: &flux::Device) -> Result<(), HostError> {
        match self {
            Self::Nested(_) => {
                device.wait_idle();
                Ok(())
            }
            Self::Drm(host) => Ok(host.wait_presented()?),
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Nested(_) => "nested",
            Self::Drm(_) => "drm",
        }
    }

    /// Direct KMS has no outer compositor cursor and therefore needs the
    /// composition root to paint cursor shapes into the scanout buffer.
    pub fn uses_software_cursor(&self) -> bool {
        matches!(self, Self::Drm(_))
    }
}

impl Backend for Host {
    fn size(&self) -> Size {
        match self {
            Self::Nested(host) => host.size(),
            Self::Drm(host) => host.size(),
        }
    }

    fn physical_size(&self) -> (u32, u32) {
        match self {
            Self::Nested(host) => host.physical_size(),
            Self::Drm(host) => host.physical_size(),
        }
    }

    fn scale(&self) -> f32 {
        match self {
            Self::Nested(host) => host.scale(),
            Self::Drm(host) => host.scale(),
        }
    }

    fn size_u32(&self) -> (u32, u32) {
        match self {
            Self::Nested(host) => host.size_u32(),
            Self::Drm(host) => host.size_u32(),
        }
    }

    fn output_infos(&self) -> Vec<ass_core::output::OutputInfo> {
        match self {
            Self::Nested(host) => host.output_infos(),
            Self::Drm(host) => host.output_infos(),
        }
    }

    fn set_configured_modes(&mut self, modes: HashMap<String, ModeSpec>) {
        match self {
            Self::Nested(host) => host.set_configured_modes(modes),
            Self::Drm(host) => host.set_configured_modes(modes),
        }
    }

    fn set_touchpad_config(&mut self, config: TouchpadConfig) -> TouchpadStatus {
        match self {
            Self::Nested(host) => host.set_touchpad_config(config),
            Self::Drm(host) => host.set_touchpad_config(config),
        }
    }

    fn touchpad_status(&self) -> TouchpadStatus {
        match self {
            Self::Nested(host) => host.touchpad_status(),
            Self::Drm(host) => host.touchpad_status(),
        }
    }

    fn dispatch(&mut self) -> bool {
        match self {
            Self::Nested(host) => host.dispatch(),
            Self::Drm(host) => host.dispatch(),
        }
    }

    fn dispatch_nonblocking(&mut self) -> bool {
        match self {
            Self::Nested(host) => host.dispatch_nonblocking(),
            Self::Drm(host) => host.dispatch_nonblocking(),
        }
    }

    fn dispatch_timeout(&mut self, timeout: Duration) -> bool {
        match self {
            Self::Nested(host) => host.dispatch_timeout(timeout),
            Self::Drm(host) => host.dispatch_timeout(timeout),
        }
    }

    fn take_input(&mut self) -> Vec<InputEvent> {
        match self {
            Self::Nested(host) => host.take_input(),
            Self::Drm(host) => host.take_input(),
        }
    }

    fn take_resize(&mut self) -> Option<Size> {
        match self {
            Self::Nested(host) => host.take_resize(),
            Self::Drm(host) => host.take_resize(),
        }
    }

    fn surface_needs_recreate(&self) -> bool {
        match self {
            Self::Nested(host) => host.surface_needs_recreate(),
            Self::Drm(host) => host.surface_needs_recreate(),
        }
    }

    fn set_text_input_state(&mut self, state: TextInputState) {
        match self {
            Self::Nested(host) => host.set_text_input_state(state),
            Self::Drm(host) => host.set_text_input_state(state),
        }
    }

    fn take_text_input(&mut self) -> Vec<TextInputEvent> {
        match self {
            Self::Nested(host) => host.take_text_input(),
            Self::Drm(host) => host.take_text_input(),
        }
    }

    fn take_pointer_gestures(&mut self) -> Vec<PointerGestureEvent> {
        match self {
            Self::Nested(host) => host.take_pointer_gestures(),
            Self::Drm(host) => host.take_pointer_gestures(),
        }
    }

    fn set_cursor_shape(&mut self, shape: u32) {
        match self {
            Self::Nested(host) => host.set_cursor_shape(shape),
            Self::Drm(host) => host.set_cursor_shape(shape),
        }
    }

    fn hide_cursor(&mut self) {
        match self {
            Self::Nested(host) => host.hide_cursor(),
            Self::Drm(host) => host.hide_cursor(),
        }
    }

    fn set_buffer_scale(&self) {
        match self {
            Self::Nested(host) => host.set_buffer_scale(),
            Self::Drm(host) => host.set_buffer_scale(),
        }
    }

    fn is_active(&self) -> bool {
        match self {
            Self::Nested(host) => host.is_active(),
            Self::Drm(host) => host.is_active(),
        }
    }

    fn switch_vt(&mut self, vt: i32) {
        match self {
            // The nested backend shares the host's session; VT switching is
            // the host compositor's business.
            Self::Nested(_) => {}
            Self::Drm(host) => host.switch_vt(vt),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_kind_parser_is_strict() {
        assert_eq!("auto".parse::<BackendKind>().unwrap(), BackendKind::Auto);
        assert_eq!("drm".parse::<BackendKind>().unwrap(), BackendKind::Drm);
        assert_eq!(
            "nested".parse::<BackendKind>().unwrap(),
            BackendKind::Nested
        );
        assert!("x11".parse::<BackendKind>().is_err());
    }

    #[test]
    fn drm_transient_errors_stay_matchable_through_host_error() {
        // main.rs treats exactly these as skip-the-frame conditions; pinning
        // the shape keeps a future enum edit from silently making them fatal.
        for error in [
            DrmError::FlipTimeout,
            DrmError::Inactive,
            DrmError::Reconfigured,
        ] {
            let host_error = HostError::from(error);
            assert!(matches!(
                host_error,
                HostError::Drm(DrmError::FlipTimeout | DrmError::Inactive | DrmError::Reconfigured)
            ));
        }
    }
}
