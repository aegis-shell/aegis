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

impl DrmBackend {
    /// Acquire the seat, choose a connected DRM card/output, enable atomic
    /// modesetting, and attach libinput to the same seat. `configured_modes`
    /// carries the config's per-connector `mode` requests (ADR-0028) so the
    /// very first modeset already honors them.
    pub fn open(configured_modes: HashMap<String, ModeSpec>) -> Result<Self, DrmError> {
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

        let (card, displays) = open_card_and_outputs(&seat, &configured_modes)?;
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
            cursor_buffers: Vec::new(),
            cursor_state: None,
            cursor_plane_active: false,
            cursor_extent,
            hardware_cursor_failed: false,
            input_events: Vec::new(),
            gesture_events: Vec::new(),
            pointer: (size.0 as f32 * 0.5, size.1 as f32 * 0.5),
            explicit_sync: false,
            sync_capable: false,
            render_ready: true,
            outputs_powered: true,
            hotplug_pending: false,
            pending_resize: None,
            surface_modifiers: Vec::new(),
            surface_stale: false,
            configured_modes,
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
    pub fn create_surface(&mut self, device: &flux::Device) -> Result<flux::Surface, DrmError> {
        let (width, height) = self.physical_size();
        let surface =
            flux::Surface::offscreen_dmabuf(device, width, height, &self.displays.modifiers)?;
        if !surface.is_exportable() {
            return Err(DrmError::DmabufUnsupported);
        }
        // Remember which intersection the surface was built with; a hotplug
        // that changes it flags the surface stale until it is recreated here.
        self.surface_modifiers = self.displays.modifiers.clone();
        self.surface_stale = false;
        self.sync_capable = flux::dmabuf_sync_supported(device);
        self.explicit_sync = self
            .displays
            .outputs
            .iter()
            .all(|output| output.props.plane_in_fence_fd.is_some())
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
        damage: Option<aegis_core::Rect>,
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
        let completion_fence = dmabuf
            .acquire_fence
            .as_ref()
            .map(OwnedFd::try_clone)
            .transpose()?;
        let scanout = self.import_scanout(dmabuf)?;
        self.commit_scanout(scanout, damage, false)?;
        Ok(completion_fence)
    }

    /// Page-flip a single client dma-buf directly onto the primary plane,
    /// bypassing the Vulkan composite entirely. This is the fullscreen-game
    /// fast path: a fullscreen, unoccluded, opaque client surface whose
    /// `(fourcc, modifier)` the plane accepts is imported and committed as-is.
    /// No flux frame is involved, so this is called instead of (not alongside)
    /// [`Self::present`]. Any kernel import/commit failure is reported as an error
    /// so the runtime falls back to compositing the next frame.
    pub fn present_scanout(
        &mut self,
        client: &aegis_core::SurfaceDmabuf,
        damage: Option<aegis_core::Rect>,
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
        // Duplicate the client fd and optional acquire fence: the server owns
        // the originals, which stay live for future commits.
        let dup_fd = unsafe { libc::dup(client.fd) };
        if dup_fd < 0 {
            return Err(DrmError::ScanoutUnsupported);
        }
        let owned = unsafe { OwnedFd::from_raw_fd(dup_fd) };
        let dup_fence = if client.acquire_fence >= 0 {
            // Two independent dups: one consumed by the kernel at commit
            // (import_scanout_client stores it on the Scanout, which
            // commit_scanout nulls after the IN_FENCE_FD is imported), and a
            // second returned as the per-frame completion signal so the
            // renderer's present-ack path stays unchanged.
            unsafe { BorrowedFd::borrow_raw(client.acquire_fence) }
                .try_clone_to_owned()
                .ok()
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
        let completion_fence = self.commit_scanout(scanout, damage, true)?;
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
                output.plane,
                output.props.plane_fb_id,
                property::Value::Framebuffer(Some(framebuffer)),
            );
            request.add_property(
                output.plane,
                output.props.plane_crtc_id,
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

    pub(super) fn import_scanout(&self, dmabuf: flux::SurfaceDmabuf) -> Result<Scanout, DrmError> {
        let card = self.card();
        let gem = card.prime_fd_to_buffer(dmabuf.fd.as_fd())?;
        let modifier = DrmModifier::from(dmabuf.modifier);
        let buffer = ImportedBuffer {
            size: (dmabuf.width, dmabuf.height),
            stride: dmabuf.stride,
            modifier,
            format: self.displays.format,
            offset: 0,
            gem,
        };
        let flags = if modifier == DrmModifier::Invalid {
            FbCmd2Flags::empty()
        } else {
            FbCmd2Flags::MODIFIERS
        };
        match card.add_planar_framebuffer(&buffer, flags) {
            Ok(framebuffer) => Ok(Scanout {
                framebuffer,
                gem,
                slot: dmabuf.slot,
                acquire_fence: dmabuf.acquire_fence,
            }),
            Err(error) => {
                let _ = card.close_buffer(gem);
                Err(error.into())
            }
        }
    }

    pub(super) fn commit_scanout(
        &mut self,
        mut scanout: Scanout,
        damage: Option<aegis_core::Rect>,
        request_completion_fence: bool,
    ) -> Result<Option<OwnedFd>, DrmError> {
        let mut request = atomic::AtomicModeReq::new();
        let acquire_fd = scanout
            .acquire_fence
            .as_ref()
            .map(AsRawFd::as_raw_fd)
            .unwrap_or(-1);
        // Per-commit FB_DAMAGE_CLIPS blobs, destroyed once the commit ioctl
        // has run (the kernel references the blob, it does not borrow ours).
        let mut damage_blobs: Vec<u64> = Vec::new();
        let use_out_fences = request_completion_fence
            && self
                .displays
                .outputs
                .iter()
                .all(|output| output.props.crtc_out_fence_ptr.is_some());
        let mut out_fence_fds = vec![-1_i32; self.displays.outputs.len()];
        for output in &self.displays.outputs {
            let props = output.props;
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
            request.add_property(
                output.plane,
                props.plane_fb_id,
                property::Value::Framebuffer(Some(scanout.framebuffer)),
            );
            request.add_property(
                output.plane,
                props.plane_crtc_id,
                property::Value::CRTC(Some(output.crtc)),
            );
            request.add_property(
                output.plane,
                props.plane_src_x,
                property::Value::UnsignedRange((output.x as u64) << 16),
            );
            request.add_property(
                output.plane,
                props.plane_src_y,
                property::Value::UnsignedRange((output.y as u64) << 16),
            );
            request.add_property(
                output.plane,
                props.plane_src_w,
                property::Value::UnsignedRange((width as u64) << 16),
            );
            request.add_property(
                output.plane,
                props.plane_src_h,
                property::Value::UnsignedRange((height as u64) << 16),
            );
            request.add_property(
                output.plane,
                props.plane_crtc_x,
                property::Value::SignedRange(0),
            );
            request.add_property(
                output.plane,
                props.plane_crtc_y,
                property::Value::SignedRange(0),
            );
            request.add_property(
                output.plane,
                props.plane_crtc_w,
                property::Value::UnsignedRange(width as u64),
            );
            request.add_property(
                output.plane,
                props.plane_crtc_h,
                property::Value::UnsignedRange(height as u64),
            );
            if let Some(in_fence) = props.plane_in_fence_fd {
                request.add_property(
                    output.plane,
                    in_fence,
                    property::Value::SignedRange(acquire_fd as i64),
                );
            }
            // Damage hint for PSR-style scanout: every commit sets the
            // property explicitly so no stale clip can linger regardless of
            // kernel stickiness. An untouched output gets blob 0 (NULL: "no
            // damage information", which drivers must read as full damage),
            // never an empty clip list.
            if let Some(clips_prop) = props.plane_fb_damage_clips {
                let (w32, h32) = (u32::from(width), u32::from(height));
                let value = match damage_clip_for_output(damage, output.x, output.y, w32, h32) {
                    Some(rect) => {
                        let full = [
                            output.x as i32,
                            output.y as i32,
                            (output.x + w32) as i32,
                            (output.y + h32) as i32,
                        ];
                        let full_blob = output.full_damage_blob.as_ref().map(|(value, _)| *value);
                        if rect == full
                            && let Some(value) = full_blob
                        {
                            value
                        } else {
                            match self.card().create_property_blob(&[rect][..]) {
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
                        }
                    }
                    None => property::Value::Blob(0),
                };
                request.add_property(output.plane, clips_prop, value);
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

        // The ioctl imported the sync_file; userspace retains and closes its
        // descriptor immediately after atomic_commit returns.
        scanout.acquire_fence = None;

        debug_assert!(self.retiring.is_none());
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
