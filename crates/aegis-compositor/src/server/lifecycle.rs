use crate::*;

impl Server {
    /// Create the display, bind an auto-named socket, and advertise the core
    /// globals.
    pub fn new() -> Result<Server, ServerError> {
        Self::new_with_render_caps(true, true, Vec::new())
    }

    /// Create a server whose advertised buffer protocols match the actual
    /// Vulkan device. Clients must never discover dma-buf or explicit-sync
    /// globals that the renderer cannot honor.
    pub fn new_with_render_caps(
        dmabuf_supported: bool,
        explicit_sync_supported: bool,
        dmabuf_formats: Vec<aegis_model::dmabuf::DmabufFormat>,
    ) -> Result<Server, ServerError> {
        Self::new_with_render_caps_and_device(
            dmabuf_supported,
            explicit_sync_supported,
            dmabuf_formats,
            None,
        )
    }

    /// Create a server with renderer capabilities plus the renderer's Linux
    /// DRM device identity. A discoverable device enables linux-dmabuf v4
    /// feedback, which GPU clients use to select the same render device.
    pub fn new_with_render_caps_and_device(
        dmabuf_supported: bool,
        explicit_sync_supported: bool,
        dmabuf_formats: Vec<aegis_model::dmabuf::DmabufFormat>,
        dmabuf_main_device: Option<u64>,
    ) -> Result<Server, ServerError> {
        Self::new_with_dmabuf_feedback(
            dmabuf_supported,
            explicit_sync_supported,
            dmabuf_formats,
            dmabuf_main_device,
            Vec::new(),
            None,
        )
    }

    /// Create a server with separate renderer and KMS scanout capabilities.
    /// The scanout set is advertised as a preferred linux-dmabuf v4 tranche;
    /// the renderer set remains the mandatory fallback tranche.
    pub fn new_with_dmabuf_feedback(
        dmabuf_supported: bool,
        explicit_sync_supported: bool,
        dmabuf_formats: Vec<aegis_model::dmabuf::DmabufFormat>,
        dmabuf_main_device: Option<u64>,
        dmabuf_scanout_formats: Vec<aegis_model::dmabuf::DmabufFormat>,
        dmabuf_scanout_device: Option<u64>,
    ) -> Result<Server, ServerError> {
        unsafe {
            let display = ffi::wl_display_create();
            if display.is_null() {
                return Err(ServerError::DisplayCreate);
            }
            let sock = ffi::wl_display_add_socket_auto(display);
            if sock.is_null() {
                ffi::wl_display_destroy(display);
                return Err(ServerError::Socket);
            }
            let socket = CStr::from_ptr(sock).to_string_lossy().into_owned();
            if ffi::wl_display_init_shm(display) != 0 {
                ffi::wl_display_destroy(display);
                return Err(ServerError::Shm);
            }

            let mut state = Box::new(State::new(display));
            // Renderer-provided format/modifier table: the Vulkan device's real
            // sampleable+importable modifiers per fourcc, advertised verbatim
            // over zwp_linux_dmabuf_v1 so clients allocate GPU-optimal buffers
            // instead of falling back to LINEAR.
            state.dmabuf_formats = dmabuf_formats;
            state.dmabuf_main_device = dmabuf_main_device;
            state.dmabuf_scanout_formats = dmabuf_scanout_formats;
            state.dmabuf_scanout_device = dmabuf_scanout_device;
            // The keyboard is optional in the sense that its absence should
            // not crash the compositor — but a working keymap is needed for
            // interactive use, so a failure here is logged loudly. The seat
            // advertises keyboard capability only when this succeeded.
            match keyboard::Keyboard::new() {
                Ok(kb) => {
                    state.keyboard = Some(kb);
                }
                Err(e) => {
                    log::error!("[server] keyboard init failed: {e}; keyboard capability disabled");
                }
            }
            let data = &mut *state as *mut State as *mut c_void;

            ffi::wl_global_create(
                display,
                &ffi::wl_compositor_interface,
                4,
                data,
                compositor_bind,
            );
            let initial_output = state.output_infos[0].clone();
            create_output_global(state.as_mut(), initial_output, None);
            ffi::wl_global_create(
                display,
                &ffi::xdg_wm_base_interface,
                1,
                data,
                xdg_wm_base_bind,
            );
            ffi::wl_global_create(
                display,
                &ffi::wl_subcompositor_interface,
                1,
                data,
                subcompositor_bind,
            );
            if create_seat_global(state.as_mut(), HUMAN_SEAT).is_null() {
                ffi::wl_display_destroy(display);
                return Err(ServerError::SeatGlobal);
            }
            ffi::wl_global_create(
                display,
                &ffi::wl_data_device_manager_interface,
                3,
                data,
                ddm_bind,
            );
            if dmabuf_supported {
                let version = if state.dmabuf_main_device.is_some() {
                    4
                } else {
                    3
                };
                ffi::wl_global_create(
                    display,
                    &ffi::zwp_linux_dmabuf_v1_interface,
                    version,
                    data,
                    dmabuf_bind,
                );
            }
            if dmabuf_supported && explicit_sync_supported {
                ffi::wl_global_create(
                    display,
                    &ffi::zwp_linux_explicit_synchronization_v1_interface,
                    1,
                    data,
                    extensions::explicit_sync_bind,
                );
            }
            ffi::wl_global_create(
                display,
                &ffi::wp_viewporter_interface,
                1,
                data,
                viewporter_bind,
            );
            // Extension protocols. Each advertises its global so clients that
            // require it connect without a protocol error.
            ffi::wl_global_create(
                display,
                &ffi::zxdg_output_manager_v1_interface,
                3,
                data,
                extensions::xdg_output_manager_bind,
            );
            ffi::wl_global_create(
                display,
                &ffi::zxdg_decoration_manager_v1_interface,
                2,
                data,
                extensions::xdg_decoration_manager_bind,
            );
            // Cross-client transient parenting for out-of-process portal
            // dialogs. Both globals are visible to Interaction Domain clients: possession
            // of an unguessable export handle is the explicit authority, and
            // the portal prompter must import handles from sandboxed apps.
            ffi::wl_global_create(
                display,
                &ffi::zxdg_exporter_v2_interface,
                1,
                data,
                extensions::xdg_exporter_bind,
            );
            ffi::wl_global_create(
                display,
                &ffi::zxdg_importer_v2_interface,
                1,
                data,
                extensions::xdg_importer_bind,
            );
            // Do not advertise wp_presentation until feedback can be tied to
            // the corresponding commit and completed with a real presentation
            // timestamp. Clients fall back to wl_surface.frame, which this
            // server completes after the output presents.
            ffi::wl_global_create(
                display,
                &ffi::wp_fractional_scale_manager_v1_interface,
                1,
                data,
                extensions::fractional_scale_bind,
            );
            // wp_color_management_v1 (staging) at v1: parametric + ICC image
            // descriptions for client content tagging.
            ffi::wl_global_create(
                display,
                &ffi::wp_color_manager_v1_interface,
                1,
                data,
                extensions::color_manager_bind,
            );
            let idle_inhibit_global = ffi::wl_global_create(
                display,
                &ffi::zwp_idle_inhibit_manager_v1_interface,
                1,
                data,
                extensions::idle_inhibit_bind,
            );
            state
                .interaction_domain_hidden_globals
                .insert(idle_inhibit_global as usize);
            ffi::wl_global_create(
                display,
                &ffi::ext_idle_notifier_v1_interface,
                2,
                data,
                extensions::idle_notifier_bind,
            );
            let session_lock_global = ffi::wl_global_create(
                display,
                &ffi::ext_session_lock_manager_v1_interface,
                1,
                data,
                extensions::session_lock_bind,
            );
            state
                .interaction_domain_hidden_globals
                .insert(session_lock_global as usize);
            ffi::wl_global_create(
                display,
                &ffi::zwp_relative_pointer_manager_v1_interface,
                1,
                data,
                extensions::relative_pointer_bind,
            );
            ffi::wl_global_create(
                display,
                &ffi::zwp_pointer_constraints_v1_interface,
                1,
                data,
                extensions::pointer_constraints_bind,
            );
            ffi::wl_global_create(
                display,
                &ffi::zwp_pointer_gestures_v1_interface,
                3,
                data,
                extensions::pointer_gestures_bind,
            );
            ffi::wl_global_create(
                display,
                &ffi::zwp_tablet_manager_v2_interface,
                1,
                data,
                extensions::tablet_manager_bind,
            );
            ffi::wl_global_create(
                display,
                &ffi::zwp_keyboard_shortcuts_inhibit_manager_v1_interface,
                1,
                data,
                extensions::keyboard_shortcuts_inhibit_bind,
            );
            let foreign_toplevel_global = ffi::wl_global_create(
                display,
                &ffi::ext_foreign_toplevel_list_v1_interface,
                1,
                data,
                extensions::foreign_toplevel_bind,
            );
            state
                .interaction_domain_hidden_globals
                .insert(foreign_toplevel_global as usize);
            ffi::wl_global_create(
                display,
                &ffi::wp_cursor_shape_manager_v1_interface,
                2,
                data,
                extensions::cursor_shape_bind,
            );
            ffi::wl_global_create(
                display,
                &ffi::zwp_text_input_manager_v3_interface,
                1,
                data,
                extensions::text_input_bind,
            );
            let input_method_global = ffi::wl_global_create(
                display,
                &ffi::zwp_input_method_manager_v2_interface,
                1,
                data,
                extensions::input_method_manager_bind,
            );
            state
                .interaction_domain_hidden_globals
                .insert(input_method_global as usize);
            let virtual_keyboard_global = ffi::wl_global_create(
                display,
                &ffi::zwp_virtual_keyboard_manager_v1_interface,
                1,
                data,
                extensions::virtual_keyboard_manager_bind,
            );
            state
                .interaction_domain_hidden_globals
                .insert(virtual_keyboard_global as usize);
            ffi::wl_global_create(
                display,
                &ffi::xdg_activation_v1_interface,
                1,
                data,
                extensions::xdg_activation_bind,
            );
            ffi::wl_display_set_global_filter(
                display,
                Some(interaction_domain_global_filter),
                data,
            );

            Ok(Server {
                state,
                socket,
                interaction_domain_portals: Vec::new(),
                epoch: std::time::Instant::now(),
            })
        }
    }

    /// The socket name clients connect to (set `WAYLAND_DISPLAY` to this).
    pub fn socket(&self) -> &str {
        &self.socket
    }

    /// Raw fd of the server's Wayland event loop, for embedding in an outer
    /// poll set. It reads ready when client requests (surface commits, new
    /// connections) are pending, which is how an idle compositor gets woken
    /// by a committing client. Ownership stays with the display: never close
    /// it, and only ever dispatch it through [`Server::dispatch`].
    pub fn event_loop_fd(&self) -> std::os::fd::RawFd {
        unsafe {
            let loop_ = ffi::wl_display_get_event_loop(self.state.display);
            ffi::wl_event_loop_get_fd(loop_)
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        unsafe {
            // `wl_display_destroy` fires each live resource's destroy notify;
            // `surface_resource_destroy` frees each surface's box and nulls its
            // slot. This MUST run before the orphan-reclaim loop below — the
            // opposite order frees the boxes while the wl_resources still hold
            // dangling user_data pointers, so the notifys fired here would
            // dereference freed memory (use-after-free, observed as a flaky
            // shutdown segfault roughly one run in three).
            ffi::wl_display_destroy(self.state.display);
            // Reclaim any orphaned boxes whose destroy notify never fired
            // (slot still non-null). Boxes freed via their notify have a null
            // slot and are skipped, so there is no double-free.
            for &p in &self.state.surfaces {
                if !p.is_null() {
                    drop(Box::from_raw(p));
                }
            }
        }
    }
}
