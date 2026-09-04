use super::*;

impl State {
    /// Allocate a fresh `wp_image_description_v1` identity (color-management
    /// v1): non-zero, never recycled within the session. The wraparound
    /// guard is theoretical — u32 space is unreachable in a real session.
    pub(crate) fn alloc_color_identity(&mut self) -> u32 {
        let id = self.color_identity_next.max(1);
        self.color_identity_next = id.wrapping_add(1);
        id
    }

    /// Record the window-open transition when a toplevel first maps
    /// (ADR-0029): the window fades in while growing from a slightly inset
    /// rect to its full mapped rect. No-op under reduced motion, for
    /// non-toplevel roles, or when the window is already mid-transition.
    pub(super) fn note_open_transition(&mut self, rec: *mut SurfaceRec) {
        if self.reduced_motion || rec.is_null() {
            return;
        }
        unsafe {
            if (*rec).xdg_toplevel.is_null() || (*rec).window.transition.is_some() {
                return;
            }
            let now = self.now_ms();
            let full = tessera_model::Rect {
                origin: (*rec).position,
                size: (*rec).window.size,
            };
            let from = inset_rect(full, full.size.w / 16, full.size.h / 16);
            if from.size.w < 2 || from.size.h < 2 || from == full {
                return;
            }
            (*rec).window.transition =
                Some(tessera_model::transition::WindowTransition::open(from, now));
        }
    }

    /// Snapshot a mapped toplevel's last frame into a ghost record and start
    /// its close transition (ADR-0029). Called from the unmap and
    /// toplevel-destroy paths *before* the buffer contents are cleared or the
    /// record reclaimed. No-op under reduced motion, for windows that were
    /// never mapped, or when the frame table is already at capacity — in the
    /// last case the oldest ghost is dropped, ending its fade early rather
    /// than growing compositor memory.
    ///
    /// # Safety
    /// `rec` must be a live, non-null surface record.
    pub(super) unsafe fn note_close_transition(&mut self, rec: *mut SurfaceRec) {
        if self.reduced_motion || rec.is_null() {
            return;
        }
        unsafe {
            let window = &(*rec).window;
            if (*rec).xdg_toplevel.is_null() || !(*rec).mapped {
                return;
            }
            // A close flight while the window is minimized is already
            // invisible; a ghost would paint it back onto the desktop.
            if window.minimized {
                return;
            }
            let rect = tessera_model::Rect {
                origin: window.position,
                size: window.size,
            };
            if rect.size.w < 2 || rect.size.h < 2 {
                return;
            }
            let (pixels, dmabuf) = if (*rec).content_is_dmabuf {
                (
                    Vec::new(),
                    (*rec).dmabuf.as_ref().and_then(|db| db.duplicate()),
                )
            } else {
                ((*rec).pixels.clone(), None)
            };
            if pixels.is_empty() && dmabuf.is_none() {
                // Nothing presentable to retain (a window that mapped without
                // ever committing a buffer): close is an instant removal.
                return;
            }
            let now = self.now_ms();
            let transition = tessera_model::transition::WindowTransition::close(rect, now);
            let frame = ClosingFrame {
                id: next_closing_frame_id(),
                rect,
                pixels,
                dmabuf,
                buffer_width: (*rec).width,
                buffer_height: (*rec).height,
                color: (*rec).image_description.clone(),
                transition,
            };
            if self.closing_frames.len() >= MAX_CLOSING_FRAMES {
                let dropped = self.closing_frames.remove(0);
                log::debug!(
                    "[server] close-transition table full; dropping ghost frame {} early",
                    dropped.id
                );
            }
            self.closing_frames.push(frame);
        }
    }

    pub(crate) fn now_ms(&self) -> u64 {
        self.epoch.elapsed().as_millis() as u64
    }

    pub(crate) fn persist_app_geometry(
        &mut self,
        app_id: &str,
        rect: tessera_model::Rect,
        workspace: Option<u32>,
        layout_role: Option<tessera_model::layout::LayoutRole>,
    ) {
        if app_id.is_empty() {
            return;
        }
        self.last_app_geometries.insert(app_id.to_owned(), rect);
        // The persisted store prunes itself to a fixed entry ceiling, but
        // `app_id` is client-supplied (`xdg_toplevel.set_app_id`): a client
        // churning ids would grow the in-memory map for the session while
        // the store stays bounded. Mirror the store's ceiling here.
        while self.last_app_geometries.len() > MAX_APP_GEOMETRY_ENTRIES {
            let Some(first) = self.last_app_geometries.keys().next().cloned() else {
                break;
            };
            self.last_app_geometries.remove(&first);
        }
        self.window_state_store.update(
            app_id.to_owned(),
            window_state::SavedWindowState {
                position: Some(rect.origin),
                size: Some(rect.size),
                workspace,
                layout_role,
                maximized: None,
            },
        );
        let _ = self
            .window_state_store
            .save_to_path(&self.window_state_path);
    }

    pub(crate) fn workspace_number_for_window(
        &self,
        window: tessera_model::window::WindowId,
    ) -> Option<u32> {
        let workspace = self.workspaces.workspace_of(window)?;
        let output = self.workspaces.workspace(workspace)?.output;
        let index = self
            .workspaces
            .output(output)?
            .workspaces
            .iter()
            .position(|candidate| *candidate == workspace)?;
        u32::try_from(index).ok()?.checked_add(1)
    }

    pub(super) fn new(display: *mut ffi::wl_display) -> State {
        let mut workspaces = tessera_model::workspace::WorkspaceModel::new();
        let output = workspaces.add_output("nested");
        let authority = InteractionDomainModel::new();
        let human_seat = Box::new(SeatRuntime::new(
            HUMAN_SEAT,
            HUMAN_INTERACTION_DOMAIN,
            HUMAN_PRINCIPAL,
            SeatCapabilities::ALL,
        ));
        let window_state_path = window_state::WindowStateStore::default_path();
        let window_state_store = window_state::WindowStateStore::load_from_path(&window_state_path);
        let mut last_app_geometries = std::collections::HashMap::new();
        for (app_id, entry) in &window_state_store.entries {
            if let (Some(pos), Some(sz)) = (entry.position, entry.size) {
                last_app_geometries.insert(
                    app_id.clone(),
                    tessera_model::Rect {
                        origin: pos,
                        size: sz,
                    },
                );
            }
        }
        State {
            display,
            authority,
            seats: std::collections::BTreeMap::from([(HUMAN_SEAT, human_seat)]),
            active_seat: HUMAN_SEAT,
            seat_globals: Vec::new(),
            interaction_domain_hidden_globals: std::collections::HashSet::new(),
            seat_resource_owners: std::collections::HashMap::new(),
            seat_resource_origins: std::collections::HashMap::new(),
            clients: std::collections::HashMap::new(),
            client_process_ids: std::collections::HashMap::new(),
            client_initial_interaction_domains: std::collections::HashMap::new(),
            client_bound_seats: std::collections::HashMap::new(),
            semantic_trees: tessera_semantic::SemanticTreeRegistry::default(),
            interaction_domain_placements: std::collections::BTreeMap::new(),
            pending_interaction_domain_layouts: std::collections::BTreeSet::new(),
            damaged_windows: std::collections::BTreeSet::new(),
            pending_interaction_domain_damage: std::collections::BTreeMap::new(),
            surfaces: Vec::new(),
            last_background_frame_callback_ms: 0,
            window_switcher: None,
            workspace_slide: None,
            attention_pulses: std::collections::HashMap::new(),
            closing_frames: Vec::new(),
            output_resources: Vec::new(),
            output_globals: Vec::new(),
            xdg_output_resources: Vec::new(),
            xdg_output_links: std::collections::HashMap::new(),
            dmabuf_feedback_resources: Vec::new(),
            idle_notifications: Vec::new(),
            idle_inhibitors: Vec::new(),
            ipc_idle_inhibit: false,
            known_tools: Vec::new(),
            retired_buffer_releases: Vec::new(),
            foreign_toplevel_lists: Vec::new(),
            foreign_handles: std::collections::HashMap::new(),
            xdg_foreign_exports: std::collections::HashMap::new(),
            xdg_foreign_imports: Vec::new(),
            activation_tokens: std::collections::HashMap::new(),
            pending_activation: None,
            pending_launch_placements: Vec::new(),
            pending_keyboard_focus: std::collections::BTreeMap::new(),
            pending_pointer_rehit: std::collections::BTreeSet::new(),
            session_lock: std::ptr::null_mut(),
            session_lock_phase: SessionLockPhase::Unlocked,
            allow_quit_while_locked: false,
            lock_focus_dirty: false,
            pending_lock_focus: std::ptr::null_mut(),
            pre_lock_keyboard_focus: std::ptr::null_mut(),
            pending_vt_switch: None,
            workspaces,
            output,
            layout_params: tessera_model::layout::LayoutParams::default(),
            reduced_motion: false,
            keyboard_repeat: tessera_model::input::KeyboardConfig::default(),
            minimize_animation: tessera_model::dock::MinimizeAnimationStyle::default(),
            minimize_targets: std::collections::HashMap::new(),
            decoration_policy: tessera_model::window::DecorationPolicy::default(),
            window_rules: Vec::new(),
            output_geometry: tessera_model::output::OutputGeometry::default(),
            output_infos: vec![tessera_model::output::OutputInfo {
                connector: "nested".to_owned(),
                geometry: tessera_model::output::OutputGeometry::default(),
                available_modes: Vec::new(),
                color_caps: tessera_model::edid::EdidColorCapabilities::default(),
            }],
            outputs_revision: 0,
            output_policies: std::collections::HashMap::new(),
            color_pipeline: tessera_model::output::ColorPipeline::default(),
            color_management_outputs: Vec::new(),
            color_identity_next: 2,
            color_pipeline_identity: 1,
            last_work_area: tessera_model::Rect::default(),
            epoch: std::time::Instant::now(),
            window_signature_memo: std::cell::Cell::new((0, 0, 0)),
            last_app_geometries,
            window_state_store,
            window_state_path,
            remember_window_positions: true,
            dmabuf_formats: Vec::new(),
            dmabuf_scanout_formats: Vec::new(),
            dmabuf_main_device: None,
            dmabuf_scanout_device: None,
            next_window_id: 1,
        }
    }

    pub(super) fn seat_runtime(&self, seat: SeatId) -> Option<&SeatRuntime> {
        self.seats.get(&seat).map(Box::as_ref)
    }

    pub(super) fn seat_runtime_mut(&mut self, seat: SeatId) -> Option<&mut SeatRuntime> {
        self.seats.get_mut(&seat).map(Box::as_mut)
    }

    pub(super) fn seat_for_resource(&self, resource: *mut ffi::wl_resource) -> Option<SeatId> {
        self.seat_resource_owners.get(&(resource as usize)).copied()
    }

    pub(super) fn seat_origin_for_resource(
        &self,
        resource: *mut ffi::wl_resource,
    ) -> Option<SeatId> {
        self.seat_resource_origins
            .get(&(resource as usize))
            .copied()
    }

    pub(super) fn track_seat_resource(&mut self, resource: *mut ffi::wl_resource, seat: SeatId) {
        self.seat_resource_owners.insert(resource as usize, seat);
        self.seat_resource_origins
            .entry(resource as usize)
            .or_insert(seat);
    }

    pub(super) fn track_routed_seat_resource(
        &mut self,
        resource: *mut ffi::wl_resource,
        advertised: SeatId,
        routed: SeatId,
    ) {
        self.seat_resource_owners.insert(resource as usize, routed);
        self.seat_resource_origins
            .insert(resource as usize, advertised);
    }

    pub(super) fn untrack_seat_resource(
        &mut self,
        resource: *mut ffi::wl_resource,
    ) -> Option<SeatId> {
        self.seat_resource_origins.remove(&(resource as usize));
        self.seat_resource_owners.remove(&(resource as usize))
    }

    pub(super) unsafe fn ensure_client(
        &mut self,
        client: *mut ffi::wl_client,
    ) -> tessera_model::interaction_domain::ClientId {
        unsafe { self.ensure_client_with_interaction_domain(client, None) }
    }

    pub(super) unsafe fn ensure_client_with_interaction_domain(
        &mut self,
        client: *mut ffi::wl_client,
        interaction_domain: Option<InteractionDomainId>,
    ) -> tessera_model::interaction_domain::ClientId {
        unsafe {
            if let Some(id) = self.clients.get(&(client as usize)).copied() {
                return id;
            }
            let security_context = interaction_domain.map(|interaction_domain| {
                format!("tessera.interaction_domain.{}", interaction_domain.0)
            });
            let id = self.authority.register_client(security_context);
            self.clients.insert(client as usize, id);
            let mut pid = 0;
            let mut uid = 0;
            let mut gid = 0;
            ffi::wl_client_get_credentials(client, &mut pid, &mut uid, &mut gid);
            if let Ok(pid) = u32::try_from(pid)
                && pid != 0
            {
                self.client_process_ids.insert(id, pid);
            }
            if let Some(interaction_domain) = interaction_domain {
                self.client_initial_interaction_domains
                    .insert(id, interaction_domain);
            }
            let record = Box::new(ClientDestroyRecord {
                listener: ffi::wl_listener {
                    link: ffi::wl_list {
                        prev: std::ptr::null_mut(),
                        next: std::ptr::null_mut(),
                    },
                    notify: Some(client_destroyed),
                },
                state: self as *mut State,
                client,
                id,
            });
            let record = Box::into_raw(record);
            ffi::wl_client_add_destroy_listener(client, &mut (*record).listener);
            id
        }
    }

    pub(super) fn client_view_interaction_domain(
        &self,
        client: *mut ffi::wl_client,
    ) -> InteractionDomainId {
        self.clients
            .get(&(client as usize))
            .and_then(|client| self.client_initial_interaction_domains.get(client))
            .copied()
            .unwrap_or(HUMAN_INTERACTION_DOMAIN)
    }

    pub(super) fn client_observes_window(
        &self,
        client: *mut ffi::wl_client,
        window: tessera_model::window::WindowId,
    ) -> bool {
        self.authority
            .interaction_domain_observes_window(self.client_view_interaction_domain(client), window)
    }

    pub(super) fn register_window(
        &mut self,
        client: tessera_model::interaction_domain::ClientId,
        window: tessera_model::window::WindowId,
    ) -> Result<(), InteractionDomainError> {
        let existing = self
            .authority
            .interaction_groups_for_client(client)
            .next()
            .map(|group| group.id);
        if let Some(group) = existing {
            self.authority.add_window_to_group(group, window)?;
        } else {
            let initial_interaction_domain = self
                .client_initial_interaction_domains
                .get(&client)
                .copied()
                .unwrap_or(HUMAN_INTERACTION_DOMAIN);
            let group = self.authority.create_interaction_group(
                client,
                &[window],
                initial_interaction_domain,
            )?;
            // A client launched through an Agent Interaction Domain owns its independent
            // seat from the first toplevel onward, but the physical desktop
            // still presents a read-only mirror. This is presentation policy,
            // not shared input authority: the human Interaction Domain is an observer and
            // therefore can never deliver events to this interaction group.
            if self
                .authority
                .interaction_domain(initial_interaction_domain)
                .is_some_and(|interaction_domain| {
                    interaction_domain.kind
                        == tessera_model::interaction_domain::InteractionDomainKind::Agent
                })
            {
                self.authority
                    .set_observer(group, HUMAN_INTERACTION_DOMAIN, true)?;
            }
        }
        self.queue_interaction_domain_layouts_for_window(window);
        Ok(())
    }

    pub(super) fn unregister_window(&mut self, window: tessera_model::window::WindowId) {
        self.semantic_trees.remove_window(window);
        if self
            .authority
            .interaction_group_for_window(window)
            .is_some()
        {
            let interaction_domains = self.interaction_domains_for_window(window);
            for interaction_domain in interaction_domains {
                self.queue_full_interaction_domain_damage(interaction_domain);
                self.pending_interaction_domain_layouts
                    .insert(interaction_domain);
            }
            self.damaged_windows.remove(&window);
            let _ = self.authority.remove_window(window);
            self.interaction_domain_placements
                .retain(|(_, placement_window), _| *placement_window != window);
        }
    }

    pub(super) fn interaction_domains_for_window(
        &self,
        window: tessera_model::window::WindowId,
    ) -> Vec<InteractionDomainId> {
        let Some(group) = self.authority.interaction_group_for_window(window) else {
            return Vec::new();
        };
        let mut interaction_domains =
            Vec::with_capacity(group.observer_interaction_domains.len() + 1);
        interaction_domains.push(group.control_interaction_domain);
        interaction_domains.extend(group.observer_interaction_domains.iter().copied());
        interaction_domains.sort_unstable();
        interaction_domains.dedup();
        interaction_domains
    }

    pub(super) fn queue_interaction_domain_layouts_for_window(
        &mut self,
        window: tessera_model::window::WindowId,
    ) {
        for interaction_domain in self.interaction_domains_for_window(window) {
            if interaction_domain != HUMAN_INTERACTION_DOMAIN {
                self.pending_interaction_domain_layouts
                    .insert(interaction_domain);
            }
        }
    }

    pub(super) fn queue_full_interaction_domain_damage(
        &mut self,
        interaction_domain: InteractionDomainId,
    ) {
        let Some(record) = self.authority.interaction_domain(interaction_domain) else {
            return;
        };
        let PresentationTarget::Virtual { output } = record.presentation else {
            return;
        };
        self.pending_interaction_domain_damage
            .entry(interaction_domain)
            .or_default()
            .push(tessera_model::Rect::new(
                0,
                0,
                output.width as i32,
                output.height as i32,
            ));
    }

    pub(super) unsafe fn note_client_used_seat(
        &mut self,
        client: *mut ffi::wl_client,
        seat: SeatId,
    ) {
        unsafe {
            let id = self.ensure_client(client);
            let bound = self.client_bound_seats.entry(client as usize).or_default();
            if self.client_initial_interaction_domains.contains_key(&id) {
                bound.insert(seat);
                return;
            }
            let was_multi_seat = bound.len() > 1;
            bound.insert(seat);
            if !was_multi_seat && bound.len() > 1 {
                let _ = self.authority.set_client_multi_seat(
                    id,
                    tessera_model::interaction_domain::MultiSeatSupport::Supported,
                );
                self.restore_native_multiseat_resources(client);
            }
        }
    }

    pub(super) fn client_routed_seat(
        &self,
        client: *mut ffi::wl_client,
        advertised: SeatId,
    ) -> SeatId {
        let Some(client_id) = self.clients.get(&(client as usize)).copied() else {
            return advertised;
        };
        if self.authority.client(client_id).is_some_and(|client| {
            client.multi_seat == tessera_model::interaction_domain::MultiSeatSupport::Supported
        }) {
            return advertised;
        }
        let interaction_domain = self
            .authority
            .interaction_groups_for_client(client_id)
            .next()
            .map(|group| group.control_interaction_domain)
            .or_else(|| {
                self.client_initial_interaction_domains
                    .get(&client_id)
                    .copied()
            });
        let Some(interaction_domain) = interaction_domain else {
            return advertised;
        };
        self.authority
            .snapshot()
            .seats
            .into_iter()
            .find(|seat| seat.interaction_domain == interaction_domain && seat.enabled)
            .map(|seat| seat.id)
            .unwrap_or(advertised)
    }

    pub(super) unsafe fn migrate_compatibility_resources(
        &mut self,
        client_id: tessera_model::interaction_domain::ClientId,
        target: SeatId,
    ) {
        unsafe {
            if self.authority.client(client_id).is_some_and(|client| {
                client.multi_seat == tessera_model::interaction_domain::MultiSeatSupport::Supported
            }) {
                return;
            }
            let Some(client) = self
                .clients
                .iter()
                .find_map(|(raw, id)| (*id == client_id).then_some(*raw as *mut ffi::wl_client))
            else {
                return;
            };
            if !self.seats.contains_key(&target) {
                return;
            }

            // Revoke offers that originated in the source interaction domain before moving the
            // destination devices. Already-issued offers are capabilities and
            // must not remain usable across an authority boundary.
            for (seat, runtime) in &mut self.seats {
                if *seat == target {
                    continue;
                }
                for device in runtime.data_devices.iter().copied().filter(|resource| {
                    !resource.is_null() && ffi::wl_resource_get_client(*resource) == client
                }) {
                    ffi::wl_resource_post_event(
                        device,
                        ffi::WL_DATA_DEVICE_SELECTION,
                        std::ptr::null_mut::<ffi::wl_resource>(),
                    );
                }
                for offer in runtime.data_offers.iter().copied().filter(|resource| {
                    !resource.is_null() && ffi::wl_resource_get_client(*resource) == client
                }) {
                    let record = ffi::wl_resource_get_user_data(offer) as *mut DataOfferRec;
                    if !record.is_null() {
                        (*record).source = std::ptr::null_mut();
                    }
                }
                // Same revocation for the data-control family: a migrated
                // client's pending offers must not remain usable across the
                // authority boundary, and a later `receive` must not reach a
                // source it may no longer address.
                for device in runtime
                    .data_control_devices
                    .iter()
                    .copied()
                    .filter(|resource| {
                        !resource.is_null() && ffi::wl_resource_get_client(*resource) == client
                    })
                {
                    ffi::wl_resource_post_event(
                        device,
                        ffi::EXT_DATA_CONTROL_DEVICE_V1_SELECTION,
                        std::ptr::null_mut::<ffi::wl_resource>(),
                    );
                }
                for offer in runtime
                    .data_control_offers
                    .iter()
                    .copied()
                    .filter(|resource| {
                        !resource.is_null() && ffi::wl_resource_get_client(*resource) == client
                    })
                {
                    let record = ffi::wl_resource_get_user_data(offer) as *mut DataControlOfferRec;
                    if !record.is_null() {
                        (*record).source = std::ptr::null_mut();
                    }
                }
            }

            macro_rules! migrate {
                ($field:ident) => {{
                    let mut moved = Vec::new();
                    for (seat, runtime) in &mut self.seats {
                        if *seat == target {
                            continue;
                        }
                        runtime.$field.retain(|resource| {
                            let belongs = !resource.is_null()
                                && ffi::wl_resource_get_client(*resource) == client;
                            if belongs {
                                moved.push(*resource);
                            }
                            !belongs
                        });
                    }
                    for resource in &moved {
                        self.seat_resource_owners.insert(*resource as usize, target);
                    }
                    self.seats
                        .get_mut(&target)
                        .expect("validated target seat disappeared")
                        .$field
                        .extend(moved);
                }};
            }

            migrate!(pointer_resources);
            migrate!(keyboard_resources);
            migrate!(touch_resources);
            migrate!(data_devices);
            migrate!(data_control_devices);
            migrate!(data_control_offers);
            migrate!(relative_pointers);
            migrate!(pointer_constraints);
            migrate!(pointer_gesture_swipes);
            migrate!(pointer_gesture_pinches);
            migrate!(pointer_gesture_holds);
            migrate!(cursor_shape_devices);
            migrate!(keyboard_shortcut_inhibitors);
            migrate!(tablet_seats);
            migrate!(tablet_devices);
            migrate!(tablet_tools);
            migrate!(text_inputs);

            // A text_input record keeps its seat authoritative for routing
            // and destruction; retarget the migrated records to the seat
            // list now holding them.
            for resource in self.seats[&target].text_inputs.iter().copied() {
                if !resource.is_null() && ffi::wl_resource_get_client(resource) == client {
                    crate::extensions::reseat_text_input(resource, target);
                }
            }
        }
    }

    pub(super) unsafe fn restore_native_multiseat_resources(
        &mut self,
        client: *mut ffi::wl_client,
    ) {
        unsafe {
            let live_seats = self
                .seats
                .keys()
                .copied()
                .collect::<std::collections::BTreeSet<_>>();
            macro_rules! restore {
                ($field:ident) => {{
                    let mut moved = Vec::<(SeatId, *mut ffi::wl_resource)>::new();
                    for (current, runtime) in &mut self.seats {
                        runtime.$field.retain(|resource| {
                            if resource.is_null()
                                || ffi::wl_resource_get_client(*resource) != client
                            {
                                return true;
                            }
                            let origin = self
                                .seat_resource_origins
                                .get(&(*resource as usize))
                                .copied()
                                .unwrap_or(*current);
                            if origin != *current && live_seats.contains(&origin) {
                                moved.push((origin, *resource));
                                false
                            } else {
                                true
                            }
                        });
                    }
                    for (origin, resource) in moved {
                        self.seat_resource_owners.insert(resource as usize, origin);
                        self.seats
                            .get_mut(&origin)
                            .expect("validated original seat disappeared")
                            .$field
                            .push(resource);
                    }
                }};
            }

            restore!(pointer_resources);
            restore!(keyboard_resources);
            restore!(touch_resources);
            restore!(data_devices);
            restore!(data_control_devices);
            restore!(data_control_offers);
            restore!(relative_pointers);
            restore!(pointer_constraints);
            restore!(pointer_gesture_swipes);
            restore!(pointer_gesture_pinches);
            restore!(pointer_gesture_holds);
            restore!(cursor_shape_devices);
            restore!(keyboard_shortcut_inhibitors);
            restore!(tablet_seats);
            restore!(tablet_devices);
            restore!(tablet_tools);
            restore!(text_inputs);

            // A text_input record keeps its seat authoritative for routing
            // and destruction; retarget the restored records to the seat
            // list now holding them.
            for (seat, runtime) in &self.seats {
                for resource in runtime.text_inputs.iter().copied() {
                    if !resource.is_null() && ffi::wl_resource_get_client(resource) == client {
                        crate::extensions::reseat_text_input(resource, *seat);
                    }
                }
            }
        }
    }

    /// Allocate a fresh, never-reused `WindowId` (ADR-0032). Called on the
    /// main loop when a toplevel role is acquired.
    pub(super) fn alloc_window_id(&mut self) -> tessera_model::window::WindowId {
        let id = self.next_window_id;
        self.next_window_id += 1;
        tessera_model::window::WindowId(id)
    }

    /// Iterate live surface records, skipping nulled slots. The returned
    /// iterator yields raw pointers; callers must validate liveness for any
    /// operation that holds the pointer across re-entry into libwayland.
    pub(super) fn live_surfaces(&self) -> impl Iterator<Item = *mut SurfaceRec> + '_ {
        self.surfaces.iter().copied().filter(|p| !p.is_null())
    }

    /// Crate-visible live-surface iterator for `extensions.rs`.
    pub(crate) fn live_surfaces_pub(&self) -> impl Iterator<Item = *mut SurfaceRec> + '_ {
        self.surfaces.iter().copied().filter(|p| !p.is_null())
    }
}

/// Ceiling on remembered per-`app_id` floating geometries. Matches the
/// persisted window-state store's entry ceiling so the in-memory view and
/// the on-disk view grow and prune together.
pub(crate) const MAX_APP_GEOMETRY_ENTRIES: usize = 500;
