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
    pub fn present(
        &mut self,
        surface: &flux::Surface,
        frame: flux::SubmittedFrame<'_>,
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
        self.commit_scanout(scanout)?;
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

    pub(super) fn commit_scanout(&mut self, mut scanout: Scanout) -> Result<(), DrmError> {
        let mut request = atomic::AtomicModeReq::new();
        let acquire_fd = scanout
            .acquire_fence
            .as_ref()
            .map(AsRawFd::as_raw_fd)
            .unwrap_or(-1);
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
                self.release_scanout(scanout);
                return Err(commit_error(error));
            }
        }
        if let Err(error) = self.card().atomic_commit(flags, request) {
            self.release_scanout(scanout);
            return Err(commit_error(error));
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
