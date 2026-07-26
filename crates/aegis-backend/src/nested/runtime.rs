use super::listeners::*;
use super::protocol::*;
use super::*;

impl NestedHost {
    /// Open a nested toplevel of the given initial size and title.
    pub fn open(title: &str, width: i32, height: i32) -> Result<NestedHost, NestedError> {
        unsafe {
            let display = ffi::wl_display_connect(ptr::null());
            if display.is_null() {
                return Err(NestedError::Connect);
            }

            let mut state = Box::new(State {
                compositor: ptr::null_mut(),
                wm_base: ptr::null_mut(),
                viewporter: ptr::null_mut(),
                fractional_scale_manager: ptr::null_mut(),
                cursor_shape_manager: ptr::null_mut(),
                cursor_shape_device: ptr::null_mut(),
                pointer_gestures_manager: ptr::null_mut(),
                gesture_swipe: ptr::null_mut(),
                gesture_pinch: ptr::null_mut(),
                gesture_hold: ptr::null_mut(),
                text_input_manager: ptr::null_mut(),
                text_input: ptr::null_mut(),
                seat: ptr::null_mut(),
                pointer: ptr::null_mut(),
                keyboard: ptr::null_mut(),
                last_pointer_serial: 0,
                configured: false,
                width,
                height,
                pending_width: 0,
                pending_height: 0,
                resized: false,
                should_close: false,
                outputs: Vec::new(),
                current_output: ptr::null_mut(),
                scale: 1,
                preferred_scale_120: 120,
                fractional_active: false,
                scale_changed: false,
                input_events: Vec::new(),
                pending_pointer_axis: PointerAxisFrame::default(),
                pointer_gesture_events: Vec::new(),
                text_input_events: Vec::new(),
                text_input_entered: false,
                text_input_state: TextInputState::default(),
            });
            let data = &mut *state as *mut State as *mut c_void;

            // Registry → bind globals.
            let registry = ffi::wl_proxy_marshal_flags(
                display as *mut ffi::wl_proxy,
                ffi::WL_DISPLAY_GET_REGISTRY,
                &ffi::wl_registry_interface,
                ffi::wl_proxy_get_version(display as *mut ffi::wl_proxy),
                0,
                ptr::null::<c_void>(),
            );
            ffi::wl_proxy_add_listener(
                registry,
                &REGISTRY_LISTENER as *const _ as *const c_void,
                data,
            );
            ffi::wl_display_roundtrip(display);

            if state.compositor.is_null() {
                return Err(NestedError::MissingGlobal("wl_compositor"));
            }
            if state.wm_base.is_null() {
                return Err(NestedError::MissingGlobal("xdg_wm_base"));
            }
            ffi::wl_proxy_add_listener(
                state.wm_base,
                &WM_BASE_LISTENER as *const _ as *const c_void,
                data,
            );
            // Install the host seat listener if the registry bound one. The
            // capabilities event arrives on the roundtrips below, at which
            // point we create the host pointer proxy.
            if !state.seat.is_null() {
                ffi::wl_proxy_add_listener(
                    state.seat,
                    &SEAT_LISTENER as *const _ as *const c_void,
                    data,
                );
                if !state.text_input_manager.is_null() {
                    let text_input = get_text_input(state.text_input_manager, state.seat);
                    if !text_input.is_null() {
                        ffi::wl_proxy_add_listener(
                            text_input,
                            &TEXT_INPUT_LISTENER as *const _ as *const c_void,
                            data,
                        );
                        state.text_input = text_input;
                    }
                }
            }

            // Surface + xdg roles. Listen for enter/leave so the buffer scale
            // tracks the output the window is shown on.
            let surface = create_surface(state.compositor);
            ffi::wl_proxy_add_listener(
                surface,
                &SURFACE_LISTENER as *const _ as *const c_void,
                data,
            );
            // Fractional scaling is useful only as a pair: the scale protocol
            // recommends a buffer size, and viewporter maps that buffer back
            // to the xdg-configured logical surface size. If either global is
            // absent, retain the core integer buffer-scale path.
            let (viewport, fractional_scale) =
                if !state.viewporter.is_null() && !state.fractional_scale_manager.is_null() {
                    let viewport = get_viewport(state.viewporter, surface);
                    let fractional_scale =
                        get_fractional_scale(state.fractional_scale_manager, surface);
                    if !viewport.is_null() && !fractional_scale.is_null() {
                        ffi::wl_proxy_add_listener(
                            fractional_scale,
                            &FRACTIONAL_SCALE_LISTENER as *const _ as *const c_void,
                            data,
                        );
                        state.fractional_active = true;
                        state.preferred_scale_120 = (state.scale.max(1) as u32) * 120;
                        (viewport, fractional_scale)
                    } else {
                        if !viewport.is_null() {
                            ffi::wl_proxy_destroy(viewport);
                        }
                        if !fractional_scale.is_null() {
                            ffi::wl_proxy_destroy(fractional_scale);
                        }
                        (ptr::null_mut(), ptr::null_mut())
                    }
                } else {
                    (ptr::null_mut(), ptr::null_mut())
                };
            let xdg_surface = get_xdg_surface(state.wm_base, surface);
            ffi::wl_proxy_add_listener(
                xdg_surface,
                &XDG_SURFACE_LISTENER as *const _ as *const c_void,
                data,
            );
            let toplevel = get_toplevel(xdg_surface);
            ffi::wl_proxy_add_listener(
                toplevel,
                &TOPLEVEL_LISTENER as *const _ as *const c_void,
                data,
            );
            set_string(toplevel, ffi::XDG_TOPLEVEL_SET_TITLE, title);
            set_string(toplevel, ffi::XDG_TOPLEVEL_SET_APP_ID, "ass");

            // Initial buffer-less commit to provoke the first configure, then
            // wait for it (Vulkan WSI provides the buffer on first present).
            // `state.configured` is flipped by the `on_xdg_surface_configure`
            // C callback during `wl_display_roundtrip`; clippy cannot see that
            // mutation across the FFI boundary, so the immutability check is a
            // false positive here.
            commit(surface);
            #[allow(clippy::while_immutable_condition)]
            while !state.configured {
                if ffi::wl_display_roundtrip(display) < 0 {
                    return Err(NestedError::Roundtrip);
                }
            }
            if state.resized {
                state.width = state.pending_width;
                state.height = state.pending_height;
                state.resized = false;
            }
            // Collect the initial preferred_scale before the swapchain is
            // created, avoiding a needless 1x first frame followed by resize.
            if state.fractional_active && ffi::wl_display_roundtrip(display) < 0 {
                return Err(NestedError::Roundtrip);
            }

            Ok(NestedHost {
                display,
                registry,
                surface,
                xdg_surface,
                toplevel,
                viewport,
                fractional_scale,
                state,
                ash: None,
                vk_surface: 0,
                touchpad_config: aegis_core::input::TouchpadConfig::default(),
                wakeup_fd: None,
            })
        }
    }

    /// Create a `VkSurfaceKHR` on `device`'s instance for this window. Returns
    /// the raw handle as a `*mut c_void` suitable for `flux::Surface::from_vk`.
    pub fn create_vk_surface(&mut self, device: &flux::Device) -> Result<*mut c_void, NestedError> {
        unsafe {
            let entry = ash::Entry::load().map_err(|_| NestedError::Vulkan)?;
            let raw_instance = device.vk_instance() as usize as u64;
            let instance =
                ash::Instance::load(entry.static_fn(), ash::vk::Instance::from_raw(raw_instance));

            let wl = ash::khr::wayland_surface::Instance::new(&entry, &instance);
            let info = ash::vk::WaylandSurfaceCreateInfoKHR::default()
                .display(self.display as *mut _)
                .surface(self.surface as *mut _);
            let surface = wl
                .create_wayland_surface(&info, None)
                .map_err(|_| NestedError::Vulkan)?;

            let raw = surface.as_raw();
            self.ash = Some((entry, instance));
            self.vk_surface = raw;
            Ok(raw as usize as *mut c_void)
        }
    }

    /// Logical window size as `u32` (the configured size, scale-independent).
    /// Use [`physical_size`](Self::physical_size) for swapchain extents.
    pub fn size_u32(&self) -> (u32, u32) {
        (
            self.state.width.max(1) as u32,
            self.state.height.max(1) as u32,
        )
    }

    /// Preferred render scale for the host surface. Fractional when the host
    /// supports fractional-scale + viewporter, otherwise the core integer
    /// `wl_output.scale` value.
    pub fn scale(&self) -> f32 {
        if self.state.fractional_active {
            self.state.preferred_scale_120.max(1) as f32 / 120.0
        } else {
            self.state.scale.max(1) as f32
        }
    }

    /// Physical (device-pixel) size = logical size × [`scale`](Self::scale).
    /// This is the swapchain extent; the buffer is divisible by `scale`, so
    /// `wl_surface.set_buffer_scale(scale)` is always valid.
    pub fn physical_size(&self) -> (u32, u32) {
        let s = self.scale();
        (
            (self.state.width.max(1) as f32 * s).round().max(1.0) as u32,
            (self.state.height.max(1) as f32 * s).round().max(1.0) as u32,
        )
    }

    /// Advertise the current buffer scale to the host. Applies on the next
    /// surface commit (the next present); call after sizing the swapchain to
    /// the matching physical size.
    pub fn set_buffer_scale(&self) {
        unsafe {
            if self.state.fractional_active {
                // fractional-scale-v1 requires buffer_scale=1. The Vulkan
                // buffer is rendered at logical*preferred_scale and the
                // viewport declares its logical surface-local destination.
                set_buffer_scale(self.surface, 1);
                viewport_set_destination(self.viewport, self.state.width, self.state.height);
            } else {
                set_buffer_scale(self.surface, self.state.scale);
            }
        };
    }

    /// Apply the focused inner client's committed text-input state to the
    /// host compositor. State is retained while the outer window is
    /// unfocused and replayed on the next host text-input `enter` event.
    pub fn set_text_input_state(&mut self, state: TextInputState) {
        self.state.text_input_state = state;
        unsafe { send_text_input_state(self.state.as_mut()) };
    }

    /// Drain IME events produced by the host compositor.
    pub fn take_text_input(&mut self) -> Vec<TextInputEvent> {
        std::mem::take(&mut self.state.text_input_events)
    }

    /// Drain touchpad gestures received from the host compositor.
    pub fn take_pointer_gestures(&mut self) -> Vec<PointerGestureEvent> {
        std::mem::take(&mut self.state.pointer_gesture_events)
    }

    /// Forward a client cursor-shape request to the host cursor. The most
    /// recent pointer enter/button serial authorizes the request.
    pub fn set_cursor_shape(&mut self, shape: u32) {
        if self.state.cursor_shape_device.is_null() || self.state.last_pointer_serial == 0 {
            return;
        }
        unsafe {
            cursor_shape_set_shape(
                self.state.cursor_shape_device,
                self.state.last_pointer_serial,
                shape.max(1),
            )
        };
    }

    /// Hide the host compositor cursor while an inner custom cursor surface
    /// is composited into the nested window (or the client explicitly asks
    /// for no cursor).
    pub fn hide_cursor(&mut self) {
        if self.state.pointer.is_null() || self.state.last_pointer_serial == 0 {
            return;
        }
        unsafe { pointer_hide_cursor(self.state.pointer, self.state.last_pointer_serial) };
    }

    pub fn should_close(&self) -> bool {
        self.state.should_close
    }
}

impl Backend for NestedHost {
    fn size(&self) -> Size {
        Size {
            w: self.state.width,
            h: self.state.height,
        }
    }

    fn physical_size(&self) -> (u32, u32) {
        NestedHost::physical_size(self)
    }

    fn scale(&self) -> f32 {
        NestedHost::scale(self)
    }

    fn size_u32(&self) -> (u32, u32) {
        NestedHost::size_u32(self)
    }

    fn output_infos(&self) -> Vec<aegis_core::output::OutputInfo> {
        let (width, height) = self.physical_size();
        vec![aegis_core::output::OutputInfo {
            connector: "nested".to_owned(),
            geometry: aegis_core::output::OutputGeometry {
                mode: aegis_core::output::OutputMode {
                    width: width as i32,
                    height: height as i32,
                    refresh_mhz: 0,
                },
                scale: aegis_core::output::Scale(self.scale()),
                transform: aegis_core::Transform::Normal,
                logical_origin: aegis_core::Point::default(),
            },
            // The outer compositor owns modesetting; there is nothing to
            // enumerate here.
            available_modes: Vec::new(),
        }]
    }

    fn set_touchpad_config(
        &mut self,
        config: aegis_core::input::TouchpadConfig,
    ) -> aegis_core::input::TouchpadStatus {
        self.touchpad_config = config;
        self.touchpad_status()
    }

    fn touchpad_status(&self) -> aegis_core::input::TouchpadStatus {
        aegis_core::input::TouchpadStatus {
            configurable: false,
            config: self.touchpad_config,
            ..aegis_core::input::TouchpadStatus::default()
        }
    }

    fn dispatch(&mut self) -> bool {
        unsafe {
            if ffi::wl_display_roundtrip(self.display) < 0 {
                self.state.should_close = true;
            }
        }
        !self.state.should_close
    }

    /// Non-blocking drain of already-buffered host events. Used while a chrome
    /// animation is in flight so the loop renders the next frame without
    /// sleeping on the host. Returns false only on a hard error; an idle queue
    /// still returns true (no events, but alive).
    fn dispatch_nonblocking(&mut self) -> bool {
        unsafe {
            // `wl_display_dispatch_pending` processes events already read into
            // the display's internal queue without blocking for new ones. A
            // negative return is a fatal connection error, not "no events".
            if ffi::wl_display_dispatch_pending(self.display) < 0 {
                self.state.should_close = true;
            }
        }
        !self.state.should_close
    }

    fn dispatch_timeout(&mut self, timeout: std::time::Duration) -> bool {
        unsafe {
            if ffi::wl_display_dispatch_pending(self.display) < 0 {
                self.state.should_close = true;
                return false;
            }
            // Flush requests before waiting. A would-block result is benign:
            // POLLOUT is unnecessary here because the regular frame loop will
            // retry and our small request stream fits the Wayland socket.
            let _ = ffi::wl_display_flush(self.display);
            // The second fd is the compositor's own Wayland server event
            // loop: a committing client must wake an idle compositor just
            // like outer-compositor input does. Its readability needs no
            // handling here — the main loop dispatches the server itself.
            let mut fds = [
                super::listeners::libc::pollfd {
                    fd: ffi::wl_display_get_fd(self.display),
                    events: super::listeners::libc::POLLIN,
                    revents: 0,
                },
                super::listeners::libc::pollfd {
                    fd: self.wakeup_fd.unwrap_or(-1),
                    events: super::listeners::libc::POLLIN,
                    revents: 0,
                },
            ];
            let timeout_ms = timeout.as_millis().min(i32::MAX as u128) as i32;
            let ready = super::listeners::libc::poll(fds.as_mut_ptr(), fds.len(), timeout_ms);
            let dispatch_failed =
                ready > 0 && fds[0].revents != 0 && ffi::wl_display_dispatch(self.display) < 0;
            if ready < 0 || dispatch_failed {
                self.state.should_close = true;
            }
        }
        !self.state.should_close
    }

    fn set_wakeup_fd(&mut self, fd: std::os::fd::RawFd) {
        self.wakeup_fd = Some(fd);
    }

    /// Drain input events buffered since the last call. Empty until the host
    /// seat advertises pointer capability and the host starts sending events.
    fn take_input(&mut self) -> Vec<aegis_core::input::InputEvent> {
        std::mem::take(&mut self.state.input_events)
    }

    /// Reports a new *logical* size when the host resized us or the output
    /// scale changed (the window moved to a differently-scaled monitor). The
    /// main loop derives the physical swapchain extent from
    /// [`physical_size`](NestedHost::physical_size); on a pure scale change the
    /// logical size is unchanged but the physical size is not.
    fn take_resize(&mut self) -> Option<Size> {
        if self.state.resized || self.state.scale_changed {
            if self.state.resized {
                self.state.width = self.state.pending_width;
                self.state.height = self.state.pending_height;
            }
            self.state.resized = false;
            self.state.scale_changed = false;
            Some(Size {
                w: self.state.width,
                h: self.state.height,
            })
        } else {
            None
        }
    }

    fn set_text_input_state(&mut self, state: TextInputState) {
        NestedHost::set_text_input_state(self, state);
    }

    fn take_text_input(&mut self) -> Vec<TextInputEvent> {
        NestedHost::take_text_input(self)
    }

    fn take_pointer_gestures(&mut self) -> Vec<PointerGestureEvent> {
        NestedHost::take_pointer_gestures(self)
    }

    fn set_cursor_shape(&mut self, shape: u32) {
        NestedHost::set_cursor_shape(self, shape);
    }

    fn hide_cursor(&mut self) {
        NestedHost::hide_cursor(self);
    }

    fn set_buffer_scale(&self) {
        NestedHost::set_buffer_scale(self);
    }
}

impl Drop for NestedHost {
    fn drop(&mut self) {
        unsafe {
            if let Some((entry, instance)) = &self.ash
                && self.vk_surface != 0
            {
                let surf = ash::khr::surface::Instance::new(entry, instance);
                surf.destroy_surface(ash::vk::SurfaceKHR::from_raw(self.vk_surface), None);
            }
            // Children before parents, display last. The pointer and keyboard
            // (if bound) are children of the seat; release them first. The
            // `wl_compositor` proxy is stored only on `state` and has no
            // destructor request, but destroying it explicitly keeps the
            // struct's teardown contract complete instead of relying on
            // disconnect to reap it.
            if !self.state.pointer.is_null() {
                destroy_pointer_gestures(self.state.as_mut());
                ffi::wl_proxy_destroy(self.state.pointer);
            }
            if !self.state.keyboard.is_null() {
                ffi::wl_proxy_destroy(self.state.keyboard);
            }
            // Bound outputs are independent globals; reap them before the
            // registry.
            for (output, _) in self.state.outputs.iter() {
                if !output.is_null() {
                    ffi::wl_proxy_destroy(*output);
                }
            }
            let compositor = self.state.compositor;
            let seat = self.state.seat;
            for p in [
                self.state.text_input,
                self.state.cursor_shape_device,
                self.fractional_scale,
                self.viewport,
                self.toplevel,
                self.xdg_surface,
                self.surface,
                self.wm_base_ptr(),
                self.state.fractional_scale_manager,
                self.state.viewporter,
                self.state.text_input_manager,
                self.state.cursor_shape_manager,
                self.state.pointer_gestures_manager,
                compositor,
                seat,
                self.registry,
            ] {
                if !p.is_null() {
                    ffi::wl_proxy_destroy(p);
                }
            }
            ffi::wl_display_disconnect(self.display);
        }
    }
}

impl NestedHost {
    fn wm_base_ptr(&self) -> *mut ffi::wl_proxy {
        self.state.wm_base
    }
}
