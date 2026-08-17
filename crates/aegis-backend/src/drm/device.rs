use super::*;

pub(super) fn scanout_formats_support(
    formats: &HashMap<u32, Vec<u64>>,
    fourcc: u32,
    modifier: u64,
) -> bool {
    formats
        .get(&fourcc)
        .is_some_and(|modifiers| modifiers.contains(&modifier))
}

#[repr(C)]
struct SyncMergeData {
    name: [u8; 32],
    fd2: i32,
    fence: i32,
    flags: u32,
    pad: u32,
}

/// `SYNC_IOC_MERGE` from `<linux/sync_file.h>`. The UAPI encoding and
/// `sync_merge_data` layout are stable across Linux architectures supported by
/// Aegis.
const SYNC_IOC_MERGE: libc::c_ulong = 0xC030_3E03;

fn merge_sync_fences(mut fences: Vec<OwnedFd>) -> Option<OwnedFd> {
    let mut merged = fences.pop()?;
    for fence in fences {
        let mut data = SyncMergeData {
            name: [0; 32],
            fd2: fence.as_raw_fd(),
            fence: -1,
            flags: 0,
            pad: 0,
        };
        data.name[..9].copy_from_slice(b"aegis-kms");
        let result = unsafe {
            libc::ioctl(
                merged.as_raw_fd(),
                SYNC_IOC_MERGE,
                &mut data as *mut SyncMergeData,
            )
        };
        if result < 0 || data.fence < 0 {
            log::warn!(
                "drm: failed to merge multi-output completion fences: {}",
                std::io::Error::last_os_error()
            );
            return None;
        }
        // SAFETY: a successful SYNC_IOC_MERGE returns a new owned sync_file.
        merged = unsafe { OwnedFd::from_raw_fd(data.fence) };
    }
    Some(merged)
}

fn close_raw_fences(fences: &mut [i32]) {
    for fence in fences {
        if *fence >= 0 {
            unsafe { libc::close(*fence) };
            *fence = -1;
        }
    }
}

/// Whether a scanout carrying a producer acquire fence may be committed to
/// the selected primary planes without violating explicit-sync semantics.
///
/// An absent fence uses the dma-buf's implicit synchronization. A present
/// sync_file, however, must be passed to every primary plane through
/// \`IN_FENCE_FD\`; silently dropping it can expose a buffer while Firefox is
/// still rendering into it and produce a partially updated/"split" frame.
fn scanout_acquire_fence_supported(has_acquire_fence: bool, all_planes_support: bool) -> bool {
    !has_acquire_fence || all_planes_support
}

fn dmabuf_identity(fd: BorrowedFd<'_>) -> std::io::Result<DmabufIdentity> {
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: `fd` is live for the call and fstat initializes the complete
    // `libc::stat` object on success.
    if unsafe { libc::fstat(fd.as_raw_fd(), stat.as_mut_ptr()) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let stat = unsafe { stat.assume_init() };
    Ok(DmabufIdentity {
        device: stat.st_dev,
        inode: stat.st_ino,
    })
}

fn composite_fb_reapable(reusable: bool, users: u32) -> bool {
    !reusable && users == 0
}

fn composite_fb_slot_conflicts(existing: CompositeFbKey, candidate: CompositeFbKey) -> bool {
    existing.epoch == candidate.epoch && existing.slot == candidate.slot && existing != candidate
}

/// The retained dma-buf descriptor of the currently scanned-out composited
/// frame (see [`DrmBackend::presented_dmabuf`]).
pub(super) struct PresentedComposite {
    fd: OwnedFd,
    width: u32,
    height: u32,
    stride: u32,
    modifier: u64,
}

impl DrmBackend {
    /// Duplicate the dma-buf descriptor of the composited frame most recently
    /// presented to scanout, for the zero-copy stream fan-out (IPC protocol
    /// 25). `None` when no composited frame is on screen (startup, direct
    /// client scanout) or the descriptor cannot be duplicated.
    pub fn presented_dmabuf(&self) -> Option<crate::host::PresentedDmabuf> {
        let presented = self.presented_composite.as_ref()?;
        let fd = presented.fd.try_clone().ok()?;
        Some(crate::host::PresentedDmabuf {
            fd,
            width: presented.width,
            height: presented.height,
            stride: presented.stride,
            modifier: presented.modifier,
        })
    }

    /// Start a new Flux surface/storage epoch. Existing cache entries stay
    /// alive while `current` or `retiring` references them, then are reaped.
    pub(super) fn invalidate_composite_fb_cache(&mut self) {
        self.composite_fb_epoch = self.composite_fb_epoch.wrapping_add(1);
        for entry in self.composite_fb_cache.values_mut() {
            entry.reusable = false;
        }
        self.reap_composite_fb_cache();
    }

    /// Forget cache records after libseat revoked and closed the old DRM fd.
    /// The kernel already destroyed those framebuffer/GEM handles, so issuing
    /// cleanup ioctls against the replacement fd would be both wrong and
    /// potentially destructive if handle numbers were reused.
    pub(super) fn forget_composite_fb_cache(&mut self) {
        self.composite_fb_cache.clear();
        self.composite_fb_reap.clear();
        self.composite_fb_epoch = self.composite_fb_epoch.wrapping_add(1);
    }

    /// Destroy all cache-owned resources while the originating DRM fd is live.
    pub(super) fn destroy_composite_fb_cache(&mut self) {
        self.composite_fb_reap.clear();
        self.composite_fb_reap
            .extend(self.composite_fb_cache.keys().copied());
        let keys = std::mem::take(&mut self.composite_fb_reap);
        for key in &keys {
            if let Some(entry) = self.composite_fb_cache.remove(key) {
                debug_assert_eq!(entry.users, 0, "destroying a referenced cache entry");
                let card = self.card();
                if let Err(error) = card.destroy_framebuffer(entry.framebuffer) {
                    log::warn!("DRM: failed to destroy cached framebuffer: {error}");
                }
                if let Err(error) = card.close_buffer(entry.gem) {
                    log::warn!("DRM: failed to close cached GEM handle: {error}");
                }
            }
        }
        self.composite_fb_reap = keys;
    }

    pub(super) fn reap_composite_fb_cache(&mut self) {
        self.composite_fb_reap.clear();
        self.composite_fb_reap
            .extend(self.composite_fb_cache.iter().filter_map(|(key, entry)| {
                composite_fb_reapable(entry.reusable, entry.users).then_some(*key)
            }));
        let keys = std::mem::take(&mut self.composite_fb_reap);
        for key in &keys {
            if let Some(entry) = self.composite_fb_cache.remove(key) {
                let card = self.card();
                if let Err(error) = card.destroy_framebuffer(entry.framebuffer) {
                    log::warn!("DRM: failed to destroy stale cached framebuffer: {error}");
                }
                if let Err(error) = card.close_buffer(entry.gem) {
                    log::warn!("DRM: failed to close stale cached GEM handle: {error}");
                }
            }
        }
        self.composite_fb_reap = keys;
    }

    fn import_composite_framebuffer(
        &self,
        fd: BorrowedFd<'_>,
        width: u32,
        height: u32,
        stride: u32,
        modifier: DrmModifier,
        format: DrmFourcc,
    ) -> Result<(control::framebuffer::Handle, BufferHandle), DrmError> {
        let card = self.card();
        let gem = card.prime_fd_to_buffer(fd)?;
        let buffer = ImportedBuffer {
            size: (width, height),
            stride,
            modifier,
            format,
            offset: 0,
            gem,
        };
        let flags = if modifier == DrmModifier::Invalid {
            FbCmd2Flags::empty()
        } else {
            FbCmd2Flags::MODIFIERS
        };
        match card.add_planar_framebuffer(&buffer, flags) {
            Ok(framebuffer) => Ok((framebuffer, gem)),
            Err(error) => {
                let _ = card.close_buffer(gem);
                Err(error.into())
            }
        }
    }

    /// Acquire the seat, choose a connected DRM card/output, enable atomic
    /// modesetting, and attach libinput to the same seat. `configured_modes`
    /// carries the config's per-connector `mode` requests (ADR-0028) so the
    /// very first modeset already honors them; `configured_color` carries the
    /// per-connector `hdr` / `deep_color` policy and `configured_icc` the
    /// per-connector ICC profile paths, likewise.
    pub fn open(
        configured_modes: HashMap<String, ModeSpec>,
        configured_color: HashMap<String, ColorPolicy>,
        configured_icc: HashMap<String, String>,
    ) -> Result<Self, DrmError> {
        let pending = Rc::new(Cell::new(None));
        let callback_pending = Rc::clone(&pending);
        let seat = libseat::Seat::open(move |_seat, event| {
            callback_pending.set(Some(match event {
                libseat::SeatEvent::Enable => PendingSeatEvent::Enable,
                libseat::SeatEvent::Disable => PendingSeatEvent::Disable,
            }));
        })
        .map_err(|error| DrmError::Seat(format!("open failed: {error:?}")))?;
        let seat = Rc::new(RefCell::new(seat));

        // libseat delivers the initial enable asynchronously on some backends.
        // Do not touch device nodes until it has granted the session.
        log::info!("libseat: waiting for seat to become active; switch VT if this hangs");
        while !matches!(pending.take(), Some(PendingSeatEvent::Enable)) {
            seat.borrow_mut()
                .dispatch(-1)
                .map_err(|error| DrmError::Seat(format!("initial dispatch: {error:?}")))?;
        }

        let (card, displays) =
            open_card_and_outputs(&seat, &configured_modes, &configured_color, &configured_icc)?;
        let size = displays.size;
        let cursor_extent = (
            card.get_driver_capability(DriverCapability::CursorWidth)
                .unwrap_or(64)
                .clamp(1, u64::from(u32::MAX)) as u32,
            card.get_driver_capability(DriverCapability::CursorHeight)
                .unwrap_or(64)
                .clamp(1, u64::from(u32::MAX)) as u32,
        );

        let input_devices = Rc::new(RefCell::new(HashMap::new()));
        let interface = SeatInputInterface {
            seat: Rc::clone(&seat),
            devices: Rc::clone(&input_devices),
        };
        let mut input = Libinput::new_with_udev(interface);
        let seat_name = seat.borrow_mut().name().to_owned();
        input
            .udev_assign_seat(&seat_name)
            .map_err(|()| DrmError::InputSeat(seat_name.clone()))?;
        input.dispatch()?;
        let hotplug = match udev::MonitorBuilder::new()
            .and_then(|monitor| monitor.match_subsystem("drm"))
            .and_then(udev::MonitorBuilder::listen)
        {
            Ok(monitor) => Some(monitor),
            Err(error) => {
                log::warn!("drm: hotplug monitoring unavailable: {error}");
                None
            }
        };

        log::info!(
            "drm: {} output(s) on {} using {} (desktop {}x{}, {:?}, {} shared modifier(s))",
            displays.outputs.len(),
            card.path.display(),
            seat_name,
            size.0,
            size.1,
            displays.format,
            displays.modifiers.len()
        );
        for output in &displays.outputs {
            let mode = output.mode.size();
            log::info!(
                "drm: {} at {},{}: {}x{} @ {} Hz, {:?}, auto scale {:.2}",
                output.name,
                output.x,
                output.y,
                mode.0,
                mode.1,
                output.mode.vrefresh(),
                output.kind,
                output.scale.as_f32()
            );
            match (output.physical_size_mm, output.ppi) {
                (Some((width, height)), Some(ppi)) => log::info!(
                    "drm: {} physical size {}x{} mm, {:.1} PPI",
                    output.name,
                    width,
                    height,
                    ppi
                ),
                (Some((width, height)), None) => log::warn!(
                    "drm: {} reported implausible physical size {}x{} mm; using scale 1.00",
                    output.name,
                    width,
                    height
                ),
                (None, None) => log::info!(
                    "drm: {} has no physical-size metadata; using scale 1.00",
                    output.name
                ),
                (None, Some(_)) => unreachable!("PPI requires physical dimensions"),
            }
        }
        let cursor_planes = displays
            .outputs
            .iter()
            .filter(|output| output.cursor.is_some())
            .count();
        log::info!(
            "drm: plane allocation: {} primary assigned, {} cursor assigned, {} overlay available (overlay offload disabled)",
            displays.outputs.len(),
            cursor_planes,
            displays.overlay.available,
        );
        if cursor_planes == displays.outputs.len() {
            log::info!(
                "drm: hardware cursor enabled on every output (maximum {}x{})",
                cursor_extent.0,
                cursor_extent.1
            );
        } else {
            log::warn!(
                "drm: hardware cursor unavailable on {}/{} output(s); using composited cursor",
                displays.outputs.len().saturating_sub(cursor_planes),
                displays.outputs.len()
            );
        }

        let mut backend = Self {
            seat,
            seat_event: pending,
            input_devices,
            input: Some(input),
            hotplug,
            card: Some(card),
            displays,
            active: true,
            failed: false,
            modeset_done: false,
            pending_flips: HashSet::new(),
            current: None,
            retiring: None,
            composite_fb_cache: HashMap::new(),
            composite_fb_epoch: 0,
            composite_fb_reap: Vec::new(),
            cursor_buffers: Vec::new(),
            cursor_state: None,
            cursor_plane_active: false,
            cursor_extent,
            hardware_cursor_failed: false,
            hardware_cursor_retry_at: None,
            hardware_cursor_failures: 0,
            input_events: Vec::new(),
            gesture_events: Vec::new(),
            pointer: (size.0 as f32 * 0.5, size.1 as f32 * 0.5),
            explicit_sync: false,
            sync_capable: false,
            render_ready: true,
            outputs_powered: true,
            presented_composite: None,
            hotplug_pending: false,
            pending_resize: None,
            surface_modifiers: Vec::new(),
            surface_color_mode: DisplayColorMode::Sdr,
            surface_icc: None,
            gamma_blob: None,
            hdr_metadata_blob: None,
            surface_stale: false,
            configured_modes,
            configured_color,
            configured_icc,
            touchpads: HashMap::new(),
            touchpad_config: TouchpadConfig::default(),
            wakeup_fd: None,
        };
        // udev_assign_seat + dispatch queues the initial DeviceAdded events
        // even before the fd becomes pollable. Drain them now so Settings can
        // report devices without waiting for the user's first touch.
        backend.drain_input_queue();
        Ok(backend)
    }

    /// Create Flux's exportable offscreen target at the selected display mode.
    /// Load the configured ICC profile's parametric color space (ADR-0069
    /// two-tier extraction covers matrix+TRC profiles). `None` — the sRGB
    /// default — when unset, unreadable, or LUT-only.
    fn configured_icc_space(&self) -> Option<flux::ColorSpace> {
        let path = self.displays.icc_profile.as_deref()?;
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) => {
                log::warn!("drm: cannot read ICC profile {path}: {error}");
                return None;
            }
        };
        match flux::IccProfile::new(&bytes) {
            Ok(profile) => match profile.color_space() {
                Some(space) => {
                    log::info!("drm: framebuffer color space from ICC profile {path}");
                    Some(space)
                }
                None => {
                    log::warn!("drm: ICC profile {path} is not matrix+TRC; framebuffer stays sRGB");
                    None
                }
            },
            Err(error) => {
                log::warn!("drm: cannot parse ICC profile {path}: {error}");
                None
            }
        }
    }

    pub fn create_surface(&mut self, device: &flux::Device) -> Result<flux::Surface, DrmError> {
        // A recreated Flux surface reuses slot numbers for different VkImages.
        // Advance the epoch before allocating it so an equal-size recreation
        // can never hit a framebuffer imported from the previous surface.
        self.invalidate_composite_fb_cache();
        // The retained presented descriptor belongs to the previous surface's
        // images; a recreated surface never reuses them.
        self.presented_composite = None;
        let (width, height) = self.physical_size();
        let color_mode = self.displays.color_mode;
        let icc_space = match color_mode {
            DisplayColorMode::Sdr | DisplayColorMode::SdrDeepColor => self.configured_icc_space(),
            DisplayColorMode::Hdr => None,
        };
        let surface = match (color_mode, icc_space) {
            (DisplayColorMode::Sdr, None) => {
                flux::Surface::offscreen_dmabuf(device, width, height, &self.displays.modifiers)?
            }
            (DisplayColorMode::Sdr, Some(space)) => {
                flux::Surface::offscreen_dmabuf_with_color_options(
                    device,
                    width,
                    height,
                    &self.displays.modifiers,
                    flux::SurfaceColorOptions {
                        color_spaces: &[space],
                        ..Default::default()
                    },
                )?
            }
            (DisplayColorMode::SdrDeepColor, icc) => {
                // An ICC profile takes precedence over the sRGB default.
                let spaces = match icc {
                    Some(space) => vec![space],
                    None => vec![flux::ColorSpace::SRGB],
                };
                flux::Surface::offscreen_dmabuf_with_color_options(
                    device,
                    width,
                    height,
                    &self.displays.modifiers,
                    flux::SurfaceColorOptions {
                        color_spaces: &spaces,
                        offscreen_formats: &[flux::Format::FLUX_FORMAT_RGB10A2_UNORM],
                        ..Default::default()
                    },
                )?
            }
            (DisplayColorMode::Hdr, _) => flux::Surface::offscreen_dmabuf_with_color_options(
                device,
                width,
                height,
                &self.displays.modifiers,
                flux::SurfaceColorOptions {
                    color_spaces: &[flux::ColorSpace::BT2020_PQ],
                    offscreen_formats: &[flux::Format::FLUX_FORMAT_RGB10A2_UNORM],
                    ..Default::default()
                },
            )?,
        };
        if !surface.is_exportable() {
            return Err(DrmError::DmabufUnsupported);
        }
        let info = surface.info();
        log::info!(
            "drm: compositor output {color_mode:?}: {}x{} {:?} content {:?}",
            info.width,
            info.height,
            info.format,
            info.content_space,
        );
        if let Some(modifier) = surface.dmabuf_modifier() {
            log::info!(
                "drm: compositor output modifier {modifier:#018x}{}",
                if modifier == u64::from(DrmModifier::Linear) {
                    " (LINEAR fallback)"
                } else {
                    " (device-native tiled)"
                }
            );
        }
        // Remember which intersection the surface was built with; a hotplug
        // that changes it flags the surface stale until it is recreated here.
        self.surface_modifiers = self.displays.modifiers.clone();
        self.surface_color_mode = color_mode;
        self.surface_icc = self.displays.icc_profile.clone();
        // The HDR static-metadata blob rides every connector commit in HDR
        // mode; build it with the surface and retire it when leaving HDR.
        match color_mode {
            DisplayColorMode::Hdr if self.hdr_metadata_blob.is_none() => {
                match self
                    .card()
                    .create_property_blob(&Self::hdr_output_metadata_bytes()[..])
                {
                    Ok(value @ property::Value::Blob(id)) => {
                        self.hdr_metadata_blob = Some((value, id));
                    }
                    other => {
                        log::warn!("drm: HDR_OUTPUT_METADATA blob allocation failed: {other:?}");
                    }
                }
            }
            DisplayColorMode::Sdr | DisplayColorMode::SdrDeepColor => {
                if let Some((_, id)) = self.hdr_metadata_blob.take() {
                    let _ = self.card().destroy_property_blob(id);
                }
            }
            DisplayColorMode::Hdr => {}
        }
        self.surface_stale = false;
        self.sync_capable = flux::dmabuf_sync_supported(device);
        self.explicit_sync = self
            .displays
            .outputs
            .iter()
            .all(|output| output.primary.props.in_fence_fd.is_some())
            && self.sync_capable;
        log::info!(
            "drm: explicit synchronization {}",
            if self.explicit_sync {
                "enabled"
            } else {
                "unavailable; using GPU fence wait"
            }
        );
        Ok(surface)
    }

    /// Complete a Flux frame, export it, and queue it on the primary plane.
    /// The next backend dispatch waits for its page-flip event before another
    /// frame may be rendered.
    ///
    /// `damage` is the conservative bounding box of this frame's changes in
    /// physical desktop (framebuffer) pixels, forwarded to KMS as
    /// `FB_DAMAGE_CLIPS` where the plane supports it. `None` means "unknown",
    /// which commits a full-output clip — always safe for the driver.
    /// Whether a client dma-buf with this `(fourcc, modifier)` could be scanned
    /// out directly on the selected primary planes. Direct scanout is only
    /// taken when every active output's primary plane accepts the exact pair;
    /// otherwise the compositor falls back to rendering. The check reuses the
    /// selected primary planes' real per-format modifier intersection. The
    /// client format need not equal the compositor render-target format (for
    /// example, an ARGB game can scan out while the desktop uses XRGB).
    pub fn supports_scanout(&self, fourcc: u32, modifier: u64) -> bool {
        scanout_formats_support(&self.displays.scanout_formats, fourcc, modifier)
    }

    /// First re-probe delay after a cursor-plane commit rejection. Each
    /// consecutive failure doubles it, capped at
    /// `HARDWARE_CURSOR_RETRY_MAX`.
    const HARDWARE_CURSOR_RETRY_BASE: Duration = Duration::from_secs(5);
    const HARDWARE_CURSOR_RETRY_MAX: Duration = Duration::from_secs(300);

    pub fn hardware_cursor_supported(&self) -> bool {
        !self.hardware_cursor_failed
            && !self.displays.outputs.is_empty()
            && self
                .displays
                .outputs
                .iter()
                .all(|output| output.cursor.is_some())
    }

    pub fn disable_hardware_cursor(&mut self) {
        self.cursor_state = None;
        self.hardware_cursor_failed = true;
        let backoff = Self::HARDWARE_CURSOR_RETRY_BASE
            .saturating_mul(1 << self.hardware_cursor_failures.min(10))
            .min(Self::HARDWARE_CURSOR_RETRY_MAX);
        self.hardware_cursor_failures = self.hardware_cursor_failures.saturating_add(1);
        self.hardware_cursor_retry_at = Some(std::time::Instant::now() + backoff);
        log::info!("drm: hardware cursor disabled; probing again in {backoff:?}");
    }

    /// Re-arm the cursor plane once its failure backoff has elapsed. Returns
    /// true on the re-arm edge only. The probe is the next ordinary cursor
    /// commit: a renewed rejection disables the plane again with a longer
    /// backoff, and a successful cursor-active commit resets the count.
    pub fn poll_hardware_cursor_retry(&mut self) -> bool {
        if !self.hardware_cursor_failed {
            return false;
        }
        let due = self
            .hardware_cursor_retry_at
            .is_some_and(|at| std::time::Instant::now() >= at);
        if !due {
            return false;
        }
        self.hardware_cursor_failed = false;
        self.hardware_cursor_retry_at = None;
        log::info!("drm: probing hardware cursor again after backoff");
        true
    }

    /// Reclaim scanout ownership after the runtime's watchdog declared the
    /// page-flip completion event lost. `retiring` is deliberately NOT
    /// released: with the event gone, KMS may still be scanning out that
    /// buffer, and freeing it would corrupt the displayed frame. The next
    /// commit replaces `retiring` wholesale, so at most one framebuffer
    /// leaks per lost event — the safe trade against freeing live scanout
    /// memory.
    pub fn recover_lost_presentation(&mut self) {
        if self.pending_flips.is_empty() {
            return;
        }
        log::error!(
            "drm: reclaiming scanout ownership for {} CRTC(s) after a lost page-flip event",
            self.pending_flips.len()
        );
        self.pending_flips.clear();
    }

    pub fn set_hardware_cursor(
        &mut self,
        cursor: Option<crate::host::HardwareCursor<'_>>,
    ) -> Result<(), DrmError> {
        if !self.hardware_cursor_supported() {
            return Err(DrmError::CursorUnsupported(
                "not every output has a compatible ARGB8888 cursor plane",
            ));
        }
        let Some(cursor) = cursor else {
            self.cursor_state = None;
            return Ok(());
        };
        let (width, height) = cursor.size;
        let expected = width
            .checked_mul(height)
            .and_then(|pixels| pixels.checked_mul(4))
            .map(|bytes| bytes as usize)
            .ok_or(DrmError::CursorUnsupported("sprite size overflows"))?;
        if width == 0 || height == 0 || cursor.pixels.len() != expected {
            return Err(DrmError::CursorUnsupported(
                "sprite must be non-empty tightly packed BGRA8",
            ));
        }
        if width > self.cursor_extent.0 || height > self.cursor_extent.1 {
            return Err(DrmError::CursorUnsupported(
                "sprite exceeds the DRM cursor-size capability",
            ));
        }
        if cursor.hotspot.0 >= width || cursor.hotspot.1 >= height {
            return Err(DrmError::CursorUnsupported(
                "hotspot lies outside the sprite",
            ));
        }

        let buffer = if let Some(index) = self.cursor_buffers.iter().position(|buffer| {
            buffer.content_size == cursor.size && buffer.pixels.as_slice() == cursor.pixels
        }) {
            index
        } else {
            let card = self.card();
            let mut dumb = card.create_dumb_buffer(self.cursor_extent, DrmFourcc::Argb8888, 32)?;
            let pitch = dumb.pitch() as usize;
            {
                let mut mapping = card.map_dumb_buffer(&mut dumb)?;
                mapping.fill(0);
                let row_bytes = width as usize * 4;
                for row in 0..height as usize {
                    let source = &cursor.pixels[row * row_bytes..(row + 1) * row_bytes];
                    let destination = &mut mapping[row * pitch..row * pitch + row_bytes];
                    destination.copy_from_slice(source);
                }
            }
            let imported = ImportedBuffer {
                size: self.cursor_extent,
                stride: dumb.pitch(),
                modifier: DrmModifier::Invalid,
                format: DrmFourcc::Argb8888,
                offset: 0,
                gem: dumb.handle(),
            };
            let framebuffer = match card.add_planar_framebuffer(&imported, FbCmd2Flags::empty()) {
                Ok(framebuffer) => framebuffer,
                Err(error) => {
                    let _ = card.destroy_dumb_buffer(dumb);
                    return Err(error.into());
                }
            };
            self.cursor_buffers.push(CursorBuffer {
                framebuffer,
                dumb,
                pixels: cursor.pixels.to_vec(),
                content_size: cursor.size,
            });
            self.cursor_buffers.len() - 1
        };
        self.cursor_state = Some(CursorState {
            buffer,
            position: cursor.position,
            hotspot: cursor.hotspot,
        });
        Ok(())
    }

    /// Import a client dma-buf as a DRM framebuffer for direct scanout, reusing
    /// the same prime-import + add_framebuffer path as the composited export but
    /// honoring the client's real fourcc, modifier, and (possibly non-zero)
    /// plane offset. `fd` is a caller-duplicated descriptor (the client keeps
    /// the original); `acquire_fence` is an optional duplicate sync_file.
    fn import_scanout_client(
        &self,
        fd: std::os::fd::BorrowedFd,
        desc: ClientScanoutDesc,
        acquire_fence: Option<OwnedFd>,
    ) -> Result<Scanout, DrmError> {
        let card = self.card();
        let gem = card.prime_fd_to_buffer(fd)?;
        let buffer = ImportedBuffer {
            size: (desc.width, desc.height),
            stride: desc.stride,
            modifier: desc.modifier,
            format: desc.format,
            offset: desc.offset,
            gem,
        };
        let flags = if desc.modifier == DrmModifier::Invalid {
            FbCmd2Flags::empty()
        } else {
            FbCmd2Flags::MODIFIERS
        };
        match card.add_planar_framebuffer(&buffer, flags) {
            Ok(framebuffer) => Ok(Scanout {
                framebuffer,
                gem,
                slot: 0,
                acquire_fence,
                ownership: ScanoutOwnership::TransientClient,
            }),
            Err(error) => {
                let _ = card.close_buffer(gem);
                Err(error.into())
            }
        }
    }

    pub fn present(
        &mut self,
        surface: &flux::Surface,
        frame: flux::SubmittedFrame<'_>,
        damage: Option<&[aegis_model::Rect]>,
    ) -> Result<Option<OwnedFd>, DrmError> {
        if !self.active || !self.render_ready || !self.outputs_powered {
            return Err(DrmError::Inactive);
        }
        if !self.pending_flips.is_empty() {
            return Err(DrmError::Busy);
        }
        if self.pending_resize.is_some() {
            return Err(DrmError::Reconfigured);
        }
        frame.present()?;
        let dmabuf = if self.explicit_sync {
            surface.export_dmabuf_explicit()?
        } else {
            surface.export_dmabuf()?
        };
        // Retain an independent descriptor of the frame going to scanout so
        // zero-copy stream consumers can import exactly what is on screen.
        self.presented_composite = match dmabuf.fd.try_clone() {
            Ok(fd) => Some(PresentedComposite {
                fd,
                width: dmabuf.width,
                height: dmabuf.height,
                stride: dmabuf.stride,
                modifier: dmabuf.modifier,
            }),
            Err(error) => {
                log::warn!("drm: cannot retain presented dma-buf for streaming: {error}");
                None
            }
        };
        let completion_fence = dmabuf
            .acquire_fence
            .as_ref()
            .map(OwnedFd::try_clone)
            .transpose()?;
        let scanout = self.import_scanout(dmabuf)?;
        self.commit_scanout(scanout, damage, PrimaryPlaneFrame::Composited)?;
        Ok(completion_fence)
    }

    /// Page-flip a single client dma-buf directly onto the primary plane,
    /// bypassing the Vulkan composite entirely. This is the full-output client
    /// fast path: an unoccluded, opaque surface whose actual buffer geometry
    /// covers the output and whose `(fourcc, modifier)` the plane accepts is
    /// imported and committed as-is. XDG fullscreen/maximized state is not a
    /// KMS eligibility criterion.
    /// No flux frame is involved, so this is called instead of (not alongside)
    /// [`Self::present`]. Any kernel import/commit failure is reported as an error
    /// so the runtime falls back to compositing the next frame.
    pub fn present_scanout(
        &mut self,
        client: &aegis_model::SurfaceDmabuf,
        damage: Option<&[aegis_model::Rect]>,
    ) -> Result<Option<OwnedFd>, DrmError> {
        if !self.active || !self.render_ready || !self.outputs_powered {
            return Err(DrmError::Inactive);
        }
        if !self.pending_flips.is_empty() {
            return Err(DrmError::Busy);
        }
        if self.pending_resize.is_some() {
            return Err(DrmError::Reconfigured);
        }
        let format =
            DrmFourcc::try_from(client.drm_format).map_err(|_| DrmError::ScanoutUnsupported)?;
        let modifier = DrmModifier::from(client.modifier);
        let all_planes_support_in_fence = self
            .displays
            .outputs
            .iter()
            .all(|output| output.primary.props.in_fence_fd.is_some());
        if !scanout_acquire_fence_supported(client.acquire_fence >= 0, all_planes_support_in_fence)
        {
            // Never guess that implicit synchronization also covers an
            // explicitly supplied producer fence. The runtime will composite
            // this frame instead; Flux imports and waits on the same fence.
            return Err(DrmError::ScanoutUnsupported);
        }
        // Duplicate the client fd and optional acquire fence: the server owns
        // the originals, which stay live for future commits.
        let dup_fd = unsafe { libc::dup(client.fd) };
        if dup_fd < 0 {
            return Err(DrmError::ScanoutUnsupported);
        }
        let owned = unsafe { OwnedFd::from_raw_fd(dup_fd) };
        let dup_fence = if client.acquire_fence >= 0 {
            // The duplicate is consumed by KMS through IN_FENCE_FD. Failure to
            // duplicate is a direct-scanout rejection, never permission to
            // drop the client's synchronization contract.
            unsafe { BorrowedFd::borrow_raw(client.acquire_fence) }
                .try_clone_to_owned()
                .map(Some)
                .map_err(|_| DrmError::ScanoutUnsupported)?
        } else {
            None
        };
        let scanout = self.import_scanout_client(
            owned.as_fd(),
            ClientScanoutDesc {
                width: client.width as u32,
                height: client.height as u32,
                stride: client.stride,
                offset: client.offset,
                format,
                modifier,
            },
            dup_fence,
        )?;
        let completion_fence =
            self.commit_scanout(scanout, damage, PrimaryPlaneFrame::DirectClient)?;
        // A client buffer now owns the primary plane; the retained composited
        // descriptor no longer describes what is on screen.
        self.presented_composite = None;
        if completion_fence.is_none() {
            // Older KMS drivers lack CRTC OUT_FENCE_PTR. Preserve correct
            // wl_buffer release semantics by waiting for the page flip only
            // on that compatibility path; modern drivers remain fully
            // pipelined through the returned sync_file.
            self.wait_for_flip(Duration::from_secs(1))?;
        }
        Ok(completion_fence)
    }

    /// Commit only KMS cursor-plane state while retaining the currently
    /// scanned-out primary framebuffer. This is the pointer-motion fast path:
    /// no Vulkan frame, dma-buf export, GEM import, or primary-plane flip.
    pub fn present_cursor(&mut self) -> Result<(), DrmError> {
        if !self.active || !self.render_ready || !self.outputs_powered {
            return Err(DrmError::Inactive);
        }
        if !self.pending_flips.is_empty() {
            return Err(DrmError::Busy);
        }
        if self.pending_resize.is_some() {
            return Err(DrmError::Reconfigured);
        }
        let framebuffer = self
            .current
            .as_ref()
            .map(|scanout| scanout.framebuffer)
            .ok_or(DrmError::ScanoutUnsupported)?;
        let mut request = atomic::AtomicModeReq::new();
        for output in &self.displays.outputs {
            // Re-state the unchanged primary plane so PAGE_FLIP_EVENT has an
            // unambiguous CRTC target on drivers that do not emit events for
            // a cursor-only property delta.
            request.add_property(
                output.primary.handle,
                output.primary.props.fb_id,
                property::Value::Framebuffer(Some(framebuffer)),
            );
            request.add_property(
                output.primary.handle,
                output.primary.props.crtc_id,
                property::Value::CRTC(Some(output.crtc)),
            );
            add_cursor_plane_to_commit(
                &mut request,
                output,
                self.cursor_state,
                &self.cursor_buffers,
            );
        }
        if let Err(error) = self.card().atomic_commit(
            AtomicCommitFlags::NONBLOCK | AtomicCommitFlags::PAGE_FLIP_EVENT,
            request,
        ) {
            let cursor_rejected = !commit_error_is_transient(&error)
                && (self.cursor_state.is_some() || self.cursor_plane_active);
            if cursor_rejected {
                log::warn!(
                    "drm: cursor-only atomic commit rejected ({error}); disabling hardware cursor"
                );
                self.disable_hardware_cursor();
                return Err(DrmError::CursorFallback);
            }
            return Err(commit_error(error));
        }
        self.cursor_plane_active = self.cursor_state.is_some();
        if self.cursor_plane_active {
            // A cursor-active commit landed: the plane is healthy again.
            self.hardware_cursor_failures = 0;
        }
        self.pending_flips = self
            .displays
            .outputs
            .iter()
            .map(|output| output.crtc)
            .collect();
        Ok(())
    }

    pub fn is_active(&self) -> bool {
        self.active && self.render_ready
    }

    /// Wait until the most recently queued atomic commit has generated a
    /// page-flip event for every active CRTC. Session-lock uses this barrier
    /// before telling the locker that the secure frame is visible.
    pub fn wait_presented(&mut self) -> Result<(), DrmError> {
        if self.pending_flips.is_empty() {
            return Ok(());
        }
        self.wait_for_flip(Duration::from_secs(1))
    }

    pub(super) fn card(&self) -> &Card {
        self.card.as_ref().expect("DRM card exists until drop")
    }

    pub(super) fn import_scanout(
        &mut self,
        dmabuf: flux::SurfaceDmabuf,
    ) -> Result<Scanout, DrmError> {
        let modifier = DrmModifier::from(dmabuf.modifier);
        let format = self.displays.format;
        let identity = match dmabuf_identity(dmabuf.fd.as_fd()) {
            Ok(identity) => Some(identity),
            Err(error) => {
                // Identity is an optimization prerequisite, not a presentation
                // prerequisite. Fall back to one-shot ownership instead of
                // rejecting an otherwise valid frame.
                log::warn!(
                    "drm: cannot identify compositor dma-buf; disabling framebuffer reuse for this frame: {error}"
                );
                None
            }
        };

        let key = identity.map(|identity| CompositeFbKey {
            epoch: self.composite_fb_epoch,
            slot: dmabuf.slot,
            identity,
            width: dmabuf.width,
            height: dmabuf.height,
            stride: dmabuf.stride,
            fourcc: format as u32,
            modifier: dmabuf.modifier,
        });

        if let Some(key) = key {
            // A slot is expected to keep one backing object for the life of a
            // Flux surface. If it changes, retire the former entry rather than
            // trusting slot alone; current/retiring references defer cleanup.
            for (cached_key, entry) in &mut self.composite_fb_cache {
                if composite_fb_slot_conflicts(*cached_key, key) {
                    entry.reusable = false;
                }
            }
            self.reap_composite_fb_cache();

            if let Some(entry) = self.composite_fb_cache.get_mut(&key) {
                entry.reusable = true;
                entry.users = entry
                    .users
                    .checked_add(1)
                    .expect("scanout cache user overflow");
                return Ok(Scanout {
                    framebuffer: entry.framebuffer,
                    gem: entry.gem,
                    slot: dmabuf.slot,
                    acquire_fence: dmabuf.acquire_fence,
                    ownership: ScanoutOwnership::Cached(key),
                });
            }

            let (framebuffer, gem) = self.import_composite_framebuffer(
                dmabuf.fd.as_fd(),
                dmabuf.width,
                dmabuf.height,
                dmabuf.stride,
                modifier,
                format,
            )?;
            self.composite_fb_cache.insert(
                key,
                CachedCompositeFb {
                    framebuffer,
                    gem,
                    users: 1,
                    reusable: true,
                },
            );
            Ok(Scanout {
                framebuffer,
                gem,
                slot: dmabuf.slot,
                acquire_fence: dmabuf.acquire_fence,
                ownership: ScanoutOwnership::Cached(key),
            })
        } else {
            let (framebuffer, gem) = self.import_composite_framebuffer(
                dmabuf.fd.as_fd(),
                dmabuf.width,
                dmabuf.height,
                dmabuf.stride,
                modifier,
                format,
            )?;
            Ok(Scanout {
                framebuffer,
                gem,
                slot: dmabuf.slot,
                acquire_fence: dmabuf.acquire_fence,
                ownership: ScanoutOwnership::TransientCompositor,
            })
        }
    }

    /// CTA-861.3 static metadata type 1 for the BT.2020 PQ framebuffer, as
    /// `struct hdr_output_metadata` (linux/hdmi.h): metadata_type 1, EOTF
    /// ST 2084, BT.2020 primaries, D65 white, 1000 cd/m² mastering. Padded to
    /// the kernel struct's 32-byte size.
    fn hdr_output_metadata_bytes() -> [u8; 32] {
        let mut blob = [0u8; 32];
        let mut at = 0usize;
        let put_u32 = |blob: &mut [u8; 32], at: &mut usize, value: u32| {
            blob[*at..*at + 4].copy_from_slice(&value.to_ne_bytes());
            *at += 4;
        };
        let put_u16 = |blob: &mut [u8; 32], at: &mut usize, value: u16| {
            blob[*at..*at + 2].copy_from_slice(&value.to_ne_bytes());
            *at += 2;
        };
        put_u32(&mut blob, &mut at, 1); // HDMI static metadata type 1
        blob[at] = 2; // EOTF: ST 2084 (PQ)
        blob[at + 1] = 0; // static metadata type 1
        at += 2;
        let xy = |v: f32| (f64::from(v) / 0.00002).round() as u16; // 0.00002 units
        for (x, y) in [
            (0.708f32, 0.292f32), // BT.2020 R
            (0.170, 0.797),       // BT.2020 G
            (0.131, 0.046),       // BT.2020 B
            (0.3127, 0.3290),     // D65 white point
        ] {
            put_u16(&mut blob, &mut at, xy(x));
            put_u16(&mut blob, &mut at, xy(y));
        }
        put_u16(&mut blob, &mut at, 1000); // max display mastering luminance (cd/m²)
        put_u16(&mut blob, &mut at, 50); // min: 50 × 0.0001 = 0.005 cd/m²
        put_u16(&mut blob, &mut at, 1000); // MaxCLL
        put_u16(&mut blob, &mut at, 400); // MaxFALL
        blob
    }

    pub(super) fn commit_scanout(
        &mut self,
        mut scanout: Scanout,
        damage: Option<&[aegis_model::Rect]>,
        frame: PrimaryPlaneFrame,
    ) -> Result<Option<OwnedFd>, DrmError> {
        if !scanout.ownership.matches_frame(frame) {
            // Source and lifetime are one contract. Reject a mismatched future
            // caller before it can request client release fences for a
            // compositor image or omit them for a client buffer.
            self.release_scanout(scanout);
            return Err(DrmError::ScanoutUnsupported);
        }
        let all_planes_support_in_fence = self
            .displays
            .outputs
            .iter()
            .all(|output| output.primary.props.in_fence_fd.is_some());
        if !scanout_acquire_fence_supported(
            scanout.acquire_fence.is_some(),
            all_planes_support_in_fence,
        ) {
            // Keep the invariant at the shared commit boundary as well as the
            // direct-scanout preflight, so future callers cannot accidentally
            // submit an unfinished producer buffer to a plane that cannot wait
            // for it.
            self.release_scanout(scanout);
            return Err(DrmError::ScanoutUnsupported);
        }
        let mut request = atomic::AtomicModeReq::new();
        let acquire_fd = scanout
            .acquire_fence
            .as_ref()
            .map(AsRawFd::as_raw_fd)
            .unwrap_or(-1);
        // Per-commit FB_DAMAGE_CLIPS blobs, destroyed once the commit ioctl
        // has run (the kernel references the blob, it does not borrow ours).
        let mut damage_blobs: Vec<u64> = Vec::new();
        let use_out_fences = frame.needs_kms_completion_fence()
            && self
                .displays
                .outputs
                .iter()
                .all(|output| output.props.crtc_out_fence_ptr.is_some());
        let mut out_fence_fds = vec![-1_i32; self.displays.outputs.len()];
        for output in &self.displays.outputs {
            let props = output.props;
            let primary = &output.primary;
            let plane_props = primary.props;
            let (width, height) = output.mode.size();
            add_cursor_plane_to_commit(
                &mut request,
                output,
                self.cursor_state,
                &self.cursor_buffers,
            );
            request.add_property(
                output.connector,
                props.connector_crtc_id,
                property::Value::CRTC(Some(output.crtc)),
            );
            request.add_property(output.crtc, props.crtc_mode_id, output.mode_blob);
            request.add_property(
                output.crtc,
                props.crtc_active,
                property::Value::Boolean(true),
            );
            // Color pipeline signaling. These connector properties are
            // sticky, so every commit sets them explicitly: an SDR commit
            // after HDR must reset Colorspace and clear the HDR metadata.
            if let Some((colorspace, default_value, bt2020_value)) =
                output.props.connector_colorspace
            {
                let value = match self.displays.color_mode {
                    DisplayColorMode::Hdr => bt2020_value,
                    DisplayColorMode::Sdr | DisplayColorMode::SdrDeepColor => default_value,
                };
                // The harvested enum value travels as a raw u64; the kernel
                // compares values, not the drm-rs variant.
                request.add_property(
                    output.connector,
                    colorspace,
                    property::Value::UnsignedRange(value),
                );
            }
            if let Some(hdr_metadata) = output.props.connector_hdr_metadata {
                let value = match self.displays.color_mode {
                    DisplayColorMode::Hdr => self
                        .hdr_metadata_blob
                        .as_ref()
                        .map(|(value, _)| *value)
                        .unwrap_or(property::Value::Blob(0)),
                    DisplayColorMode::Sdr | DisplayColorMode::SdrDeepColor => {
                        property::Value::Blob(0)
                    }
                };
                request.add_property(output.connector, hdr_metadata, value);
            }
            if let Some(max_bpc) = output.props.connector_max_bpc {
                let bpc = match self.displays.color_mode {
                    DisplayColorMode::Sdr => 8,
                    DisplayColorMode::SdrDeepColor | DisplayColorMode::Hdr => 10,
                };
                request.add_property(
                    output.connector,
                    max_bpc,
                    property::Value::UnsignedRange(bpc),
                );
            }
            request.add_property(
                primary.handle,
                plane_props.fb_id,
                property::Value::Framebuffer(Some(scanout.framebuffer)),
            );
            request.add_property(
                primary.handle,
                plane_props.crtc_id,
                property::Value::CRTC(Some(output.crtc)),
            );
            request.add_property(
                primary.handle,
                plane_props.src_x,
                property::Value::UnsignedRange((output.x as u64) << 16),
            );
            request.add_property(
                primary.handle,
                plane_props.src_y,
                property::Value::UnsignedRange((output.y as u64) << 16),
            );
            request.add_property(
                primary.handle,
                plane_props.src_w,
                property::Value::UnsignedRange((width as u64) << 16),
            );
            request.add_property(
                primary.handle,
                plane_props.src_h,
                property::Value::UnsignedRange((height as u64) << 16),
            );
            request.add_property(
                primary.handle,
                plane_props.crtc_x,
                property::Value::SignedRange(0),
            );
            request.add_property(
                primary.handle,
                plane_props.crtc_y,
                property::Value::SignedRange(0),
            );
            request.add_property(
                primary.handle,
                plane_props.crtc_w,
                property::Value::UnsignedRange(width as u64),
            );
            request.add_property(
                primary.handle,
                plane_props.crtc_h,
                property::Value::UnsignedRange(height as u64),
            );
            if let Some(in_fence) = plane_props.in_fence_fd {
                request.add_property(
                    primary.handle,
                    in_fence,
                    property::Value::SignedRange(acquire_fd as i64),
                );
            }
            // Damage hint for PSR-style scanout: every commit sets the
            // property explicitly so no stale clip can linger regardless of
            // kernel stickiness. An untouched output gets blob 0 (NULL: "no
            // damage information", which drivers must read as full damage),
            // never an empty clip list.
            if let Some(clips_prop) = plane_props.fb_damage_clips {
                let (w32, h32) = (u32::from(width), u32::from(height));
                let clips = damage_clip_for_output(damage, output.x, output.y, w32, h32);
                let full = [
                    output.x as i32,
                    output.y as i32,
                    (output.x + w32) as i32,
                    (output.y + h32) as i32,
                ];
                let full_blob = primary.full_damage_blob.as_ref().map(|(value, _)| *value);
                // An empty clip list means the output is untouched: a NULL
                // (zero) hint, which KMS reads as "no damage information".
                // A single clip equal to the whole output reuses the
                // pre-allocated full-damage blob. Otherwise build a blob from
                // every clip so disjoint regions stay separate for PSR2.
                let value = if clips.is_empty() {
                    property::Value::Blob(0)
                } else if clips.len() == 1
                    && clips[0] == full
                    && let Some(value) = full_blob
                {
                    value
                } else {
                    match self.card().create_property_blob(&clips[..]) {
                        Ok(value @ property::Value::Blob(id)) => {
                            damage_blobs.push(id);
                            value
                        }
                        // Fall back to "no damage information"
                        // (conservative full) if the blob cannot be
                        // allocated: a missing or NULL hint is always
                        // safe, a wrong one is not.
                        _ => full_blob.unwrap_or(property::Value::Blob(0)),
                    }
                };
                request.add_property(primary.handle, clips_prop, value);
            }
        }
        let mut flags = AtomicCommitFlags::NONBLOCK | AtomicCommitFlags::PAGE_FLIP_EVENT;
        if !self.modeset_done {
            flags |= AtomicCommitFlags::ALLOW_MODESET;
            // The kernel rejects TEST_ONLY | PAGE_FLIP_EVENT with EINVAL, so
            // this preflight cannot be merged with the real commit below.
            if let Err(error) = self.card().atomic_commit(
                AtomicCommitFlags::ALLOW_MODESET | AtomicCommitFlags::TEST_ONLY,
                request.clone(),
            ) {
                close_raw_fences(&mut out_fence_fds);
                for blob in damage_blobs {
                    let _ = self.card().destroy_property_blob(blob);
                }
                let cursor_rejected = !commit_error_is_transient(&error)
                    && (self.cursor_state.is_some() || self.cursor_plane_active);
                if cursor_rejected {
                    log::warn!(
                        "drm: cursor-plane TEST_ONLY commit rejected ({error}); disabling hardware cursor"
                    );
                    self.disable_hardware_cursor();
                    self.release_scanout(scanout);
                    return Err(DrmError::CursorFallback);
                }
                self.release_scanout(scanout);
                return Err(commit_error(error));
            }
        }
        // OUT_FENCE_PTR is an execution result, not part of mode validation.
        // Add it only after TEST_ONLY so the preflight never allocates or
        // mutates sync-file storage.
        if use_out_fences {
            for (output_index, output) in self.displays.outputs.iter().enumerate() {
                let pointer = (&mut out_fence_fds[output_index] as *mut i32) as usize as u64;
                request.add_property(
                    output.crtc,
                    output.props.crtc_out_fence_ptr.expect("checked above"),
                    property::Value::UnsignedRange(pointer),
                );
            }
        }
        if let Err(error) = self.card().atomic_commit(flags, request) {
            close_raw_fences(&mut out_fence_fds);
            for blob in damage_blobs {
                let _ = self.card().destroy_property_blob(blob);
            }
            let cursor_rejected = !commit_error_is_transient(&error)
                && (self.cursor_state.is_some() || self.cursor_plane_active);
            if cursor_rejected {
                log::warn!(
                    "drm: cursor-plane atomic commit rejected ({error}); disabling hardware cursor"
                );
                self.disable_hardware_cursor();
                self.release_scanout(scanout);
                return Err(DrmError::CursorFallback);
            }
            self.release_scanout(scanout);
            return Err(commit_error(error));
        }
        for blob in damage_blobs {
            let _ = self.card().destroy_property_blob(blob);
        }
        self.cursor_plane_active = self.cursor_state.is_some();
        if self.cursor_plane_active {
            // A cursor-active commit landed: the plane is healthy again.
            self.hardware_cursor_failures = 0;
        }

        // The ioctl imported the sync_file; userspace retains and closes its
        // descriptor immediately after atomic_commit returns.
        scanout.acquire_fence = None;

        if let Some(unretired) = self.retiring.take() {
            // Only reachable after recover_lost_presentation: the buffer may
            // still be scanned out, so its handles are deliberately leaked
            // rather than freed under live scanout.
            log::warn!(
                "drm: leaking scanout slot {} framebuffer; its page-flip event was lost",
                unretired.slot
            );
        }
        self.retiring = self.current.take();
        self.current = Some(scanout);
        self.pending_flips = self
            .displays
            .outputs
            .iter()
            .map(|output| output.crtc)
            .collect();
        self.modeset_done = true;
        let fences = out_fence_fds
            .into_iter()
            .filter(|fd| *fd >= 0)
            // SAFETY: OUT_FENCE_PTR writes fresh owned sync_file descriptors.
            .map(|fd| unsafe { OwnedFd::from_raw_fd(fd) })
            .collect::<Vec<_>>();
        if use_out_fences && fences.len() == self.displays.outputs.len() {
            Ok(merge_sync_fences(fences))
        } else {
            drop(fences);
            Ok(None)
        }
    }

    pub(super) fn wait_for_flip(&mut self, timeout: Duration) -> Result<(), DrmError> {
        let alive = self.pump(Some(timeout), true);
        if !alive {
            return Err(DrmError::FlipTimeout);
        }
        if !self.pending_flips.is_empty() {
            return Err(DrmError::FlipTimeout);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(epoch: u64, slot: u32, inode: u64) -> CompositeFbKey {
        CompositeFbKey {
            epoch,
            slot,
            identity: DmabufIdentity { device: 7, inode },
            width: 3072,
            height: 1920,
            stride: 12_288,
            fourcc: DrmFourcc::Xrgb8888 as u32,
            modifier: u64::from(DrmModifier::Linear),
        }
    }

    #[test]
    fn dmabuf_identity_is_stable_across_duplicated_fds() {
        let first = std::fs::File::open("/dev/null").unwrap();
        let duplicate = first.try_clone().unwrap();
        assert_eq!(
            dmabuf_identity(first.as_fd()).unwrap(),
            dmabuf_identity(duplicate.as_fd()).unwrap()
        );
    }

    #[test]
    fn cache_key_rejects_same_slot_with_new_storage() {
        let original = key(4, 1, 100);
        assert!(!composite_fb_slot_conflicts(original, original));
        assert!(composite_fb_slot_conflicts(original, key(4, 1, 101)));
        assert!(!composite_fb_slot_conflicts(original, key(4, 2, 101)));
        // Epoch invalidation handles old-surface entries globally; a key from
        // another epoch must not evict the candidate slot in the new epoch.
        assert!(!composite_fb_slot_conflicts(original, key(5, 1, 101)));
    }

    #[test]
    fn invalidated_cache_entry_waits_for_last_scanout_reference() {
        assert!(!composite_fb_reapable(true, 0));
        assert!(!composite_fb_reapable(false, 1));
        assert!(composite_fb_reapable(false, 0));
    }

    #[test]
    fn scanout_requires_in_fence_support_only_for_explicit_acquire_fences() {
        // An implicit-sync buffer does not need the optional plane property.
        assert!(scanout_acquire_fence_supported(false, false));
        // An explicit producer fence is safe only when every selected primary
        // plane can import it into the same atomic commit.
        assert!(scanout_acquire_fence_supported(true, true));
        assert!(!scanout_acquire_fence_supported(true, false));
    }
}
