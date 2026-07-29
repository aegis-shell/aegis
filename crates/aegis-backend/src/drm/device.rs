use super::*;

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
                "drm: {} at {},{}: {}x{} @ {} Hz",
                output.name,
                output.x,
                output.y,
                mode.0,
                mode.1,
                output.mode.vrefresh()
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
            input_events: Vec::new(),
            gesture_events: Vec::new(),
            pointer: (size.0 as f32 * 0.5, size.1 as f32 * 0.5),
            explicit_sync: false,
            sync_capable: false,
            render_ready: true,
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
    /// negotiated display-set modifier intersection plus a format match against
    /// the fourcc the desktop itself was configured for.
    pub fn supports_scanout(&self, fourcc: u32, modifier: u64) -> bool {
        if self.displays.format as u32 != fourcc {
            return false;
        }
        self.displays.modifiers.contains(&modifier)
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
        if !self.active || !self.render_ready {
            return Err(DrmError::Inactive);
        }
        if !self.pending_flips.is_empty() {
            self.wait_for_flip(Duration::from_secs(1))?;
        }
        // The wait above pumps session and hotplug events, either of which may
        // have changed the world under us. A queued resize means the display
        // set was swapped and this frame was rendered for the old topology:
        // skip it instead of committing a stale buffer against the new set.
        if !self.active || !self.render_ready {
            return Err(DrmError::Inactive);
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
        self.commit_scanout(scanout, damage)?;
        Ok(completion_fence)
    }

    /// Page-flip a single client dma-buf directly onto the primary plane,
    /// bypassing the Vulkan composite entirely. This is the fullscreen-game
    /// fast path: a fullscreen, unoccluded, opaque client surface whose
    /// `(fourcc, modifier)` the plane accepts is imported and committed as-is.
    /// No flux frame is involved, so this is called instead of (not alongside)
    /// [`present`]. Any kernel import/commit failure is reported as an error
    /// so the runtime falls back to compositing the next frame.
    pub fn present_scanout(
        &mut self,
        client: &aegis_core::SurfaceDmabuf,
        damage: Option<aegis_core::Rect>,
    ) -> Result<Option<OwnedFd>, DrmError> {
        if !self.active || !self.render_ready {
            return Err(DrmError::Inactive);
        }
        if !self.pending_flips.is_empty() {
            self.wait_for_flip(Duration::from_secs(1))?;
        }
        if !self.active || !self.render_ready {
            return Err(DrmError::Inactive);
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
        let completion_fence = if client.acquire_fence >= 0 {
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
        self.commit_scanout(scanout, damage)?;
        Ok(completion_fence)
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
    ) -> Result<(), DrmError> {
        let mut request = atomic::AtomicModeReq::new();
        let acquire_fd = scanout
            .acquire_fence
            .as_ref()
            .map(AsRawFd::as_raw_fd)
            .unwrap_or(-1);
        // Per-commit FB_DAMAGE_CLIPS blobs, destroyed once the commit ioctl
        // has run (the kernel references the blob, it does not borrow ours).
        let mut damage_blobs: Vec<u64> = Vec::new();
        for output in &self.displays.outputs {
            let props = output.props;
            let (width, height) = output.mode.size();
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
                for blob in damage_blobs {
                    let _ = self.card().destroy_property_blob(blob);
                }
                self.release_scanout(scanout);
                return Err(commit_error(error));
            }
        }
        if let Err(error) = self.card().atomic_commit(flags, request) {
            for blob in damage_blobs {
                let _ = self.card().destroy_property_blob(blob);
            }
            self.release_scanout(scanout);
            return Err(commit_error(error));
        }
        for blob in damage_blobs {
            let _ = self.card().destroy_property_blob(blob);
        }

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
        Ok(())
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
