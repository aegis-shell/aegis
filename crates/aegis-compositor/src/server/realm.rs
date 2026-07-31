use crate::*;

impl Server {
    /// Point-in-time authority state for the shell and IPC layers.
    pub fn realm_snapshot(&self) -> RealmSnapshot {
        self.state.authority.snapshot()
    }

    /// Current authority model revision. Cheap alternative to
    /// [`Self::realm_snapshot`] for per-frame change detection: the frame
    /// loop rebuilds the owned snapshot only when this moves.
    pub fn realm_revision(&self) -> u64 {
        self.state.authority.revision()
    }

    /// Prepare a capability listener for one compositor-mediated Realm launch.
    ///
    /// The randomized host pathname exists only while bubblewrap installs a
    /// bind mount of the socket inode. [`Self::activate_realm_portal`] refuses
    /// the portal until the launcher has removed that ambient pathname.
    pub fn prepare_realm_portal(&self, realm: RealmId) -> Result<RealmPortal, RealmRuntimeError> {
        let record = self
            .state
            .authority
            .realm(realm)
            .ok_or(RealmError::UnknownRealm(realm))?;
        if record.state != aegis_core::realm::RealmState::Active {
            return Err(RealmError::RealmNotActive(realm).into());
        }

        let runtime = std::env::var_os("XDG_RUNTIME_DIR")
            .ok_or_else(|| RealmRuntimeError::Portal("XDG_RUNTIME_DIR is not available".into()))?;
        let base = PathBuf::from(runtime).join("aegis-portals");
        std::fs::DirBuilder::new()
            .mode(0o700)
            .recursive(true)
            .create(&base)
            .map_err(|error| RealmRuntimeError::Portal(error.to_string()))?;
        std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o700))
            .map_err(|error| RealmRuntimeError::Portal(error.to_string()))?;

        let mut last_error = None;
        for _ in 0..8 {
            let token = random_portal_token()
                .map_err(|error| RealmRuntimeError::Portal(error.to_string()))?;
            let directory = base.join(format!("realm-{}-{token}", realm.0));
            match std::fs::DirBuilder::new().mode(0o700).create(&directory) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    last_error = Some(error);
                    continue;
                }
                Err(error) => return Err(RealmRuntimeError::Portal(error.to_string())),
            }
            let path = directory.join("wayland");
            let listener = match UnixListener::bind(&path) {
                Ok(listener) => listener,
                Err(error) => {
                    let _ = std::fs::remove_dir(&directory);
                    return Err(RealmRuntimeError::Portal(error.to_string()));
                }
            };
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
                .map_err(|error| RealmRuntimeError::Portal(error.to_string()))?;
            listener
                .set_nonblocking(true)
                .map_err(|error| RealmRuntimeError::Portal(error.to_string()))?;
            return Ok(RealmPortal {
                realm,
                path,
                listener,
            });
        }
        Err(RealmRuntimeError::Portal(format!(
            "could not allocate a unique portal path: {}",
            last_error
                .map(|error| error.to_string())
                .unwrap_or_else(|| "random-name collision".into())
        )))
    }

    /// Activate a portal after bubblewrap has made its socket inode private to
    /// the sandbox mount namespace.
    pub fn activate_realm_portal(&mut self, portal: RealmPortal) -> Result<(), RealmRuntimeError> {
        let record = self
            .state
            .authority
            .realm(portal.realm)
            .ok_or(RealmError::UnknownRealm(portal.realm))?;
        if record.state != aegis_core::realm::RealmState::Active {
            return Err(RealmError::RealmNotActive(portal.realm).into());
        }
        if portal.path.exists() {
            return Err(RealmRuntimeError::Portal(
                "sandbox launch gate did not remove the ambient portal path".into(),
            ));
        }
        self.realm_portals.push(portal);
        Ok(())
    }

    pub(crate) fn accept_realm_portal_clients(&mut self) {
        for portal in &self.realm_portals {
            loop {
                let (stream, _) = match portal.listener.accept() {
                    Ok(connection) => connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(error) => {
                        log::warn!("Realm {} portal accept failed: {error}", portal.realm.0);
                        break;
                    }
                };
                if let Err(error) = stream.set_nonblocking(true) {
                    log::warn!(
                        "Realm {} portal client could not become non-blocking: {error}",
                        portal.realm.0
                    );
                    continue;
                }
                let fd = stream.into_raw_fd();
                let wl_client = unsafe { ffi::wl_client_create(self.state.display, fd) };
                if wl_client.is_null() {
                    unsafe { libc_close(fd) };
                    log::warn!("Realm {} portal client was rejected", portal.realm.0);
                    continue;
                }
                unsafe {
                    self.state
                        .ensure_client_with_realm(wl_client, Some(portal.realm));
                }
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn realm_portal_count(&self) -> usize {
        self.realm_portals.len()
    }

    pub fn configure_realm_output(
        &mut self,
        realm: RealmId,
        output: VirtualOutput,
    ) -> Result<(), RealmRuntimeError> {
        self.state
            .authority
            .configure_virtual_output(realm, output)?;
        unsafe { update_realm_output_global(self.state.as_mut(), realm, output) };
        self.layout_virtual_realm(realm)?;
        Ok(())
    }

    pub fn realm_output(&self, realm: RealmId) -> Option<VirtualOutput> {
        match self.state.authority.realm(realm)?.presentation {
            PresentationTarget::Virtual { output } => Some(output),
            _ => None,
        }
    }

    /// Window layout metadata corresponding to the directed Realm render.
    pub fn realm_window_placements(
        &self,
        realm: RealmId,
    ) -> Vec<aegis_core::realm::RealmWindowPlacement> {
        let mut placements = self
            .state
            .realm_placements
            .iter()
            .filter_map(|((placement_realm, window), output_rect)| {
                if *placement_realm != realm
                    || !self.state.authority.realm_observes_window(realm, *window)
                {
                    return None;
                }
                let rec = self.find_surface_by_window_id(*window);
                if rec.is_null() || unsafe { !(*rec).mapped || (*rec).window.minimized } {
                    return None;
                }
                let surface_size = unsafe {
                    if (*rec).window.size.w > 0 && (*rec).window.size.h > 0 {
                        (*rec).window.size
                    } else {
                        surface_logical_size(&*rec)
                    }
                };
                (surface_size.w > 0 && surface_size.h > 0).then_some(
                    aegis_core::realm::RealmWindowPlacement {
                        window: *window,
                        output_rect: *output_rect,
                        surface_size,
                    },
                )
            })
            .collect::<Vec<_>>();
        placements.sort_by_key(|placement| placement.window);
        placements
    }

    /// Create an independently advertised agent seat and its authority realm.
    ///
    /// The XKB state is prepared before the authority mutation. If advertising
    /// the Wayland global fails, the new realm is immediately revoked so no
    /// active-but-unreachable authority survives the failed operation.
    pub fn create_agent_realm(
        &mut self,
        label: impl Into<String>,
        capabilities: SeatCapabilities,
    ) -> Result<RealmBundle, RealmRuntimeError> {
        let keyboard = if capabilities.keyboard {
            Some(
                keyboard::Keyboard::new()
                    .map_err(|error| RealmRuntimeError::Keyboard(error.to_string()))?,
            )
        } else {
            None
        };
        let bundle = self.state.authority.create_agent_realm(label, capabilities);
        let mut runtime = Box::new(SeatRuntime::new(
            bundle.seat,
            bundle.realm,
            bundle.principal,
            capabilities,
        ));
        runtime.keyboard = keyboard;
        self.state.seats.insert(bundle.seat, runtime);

        let global = unsafe { create_seat_global(self.state.as_mut(), bundle.seat) };
        if global.is_null() {
            self.state
                .authority
                .revoke_realm(bundle.realm, HUMAN_REALM)
                .expect("new agent realm must be revocable");
            self.state.seats.remove(&bundle.seat);
            return Err(RealmRuntimeError::SeatGlobal);
        }
        let output = self
            .realm_output(bundle.realm)
            .expect("new agent realm must have a virtual output");
        if !unsafe { create_realm_output_global(self.state.as_mut(), bundle.realm, output) } {
            let _ = self.revoke_realm(bundle.realm, HUMAN_REALM);
            return Err(RealmRuntimeError::OutputGlobal);
        }
        debug_assert!(self.state.authority.validate().is_ok());
        Ok(bundle)
    }

    /// Stop all input delivery from a realm while preserving its identity and
    /// transferable window authority for a later resume.
    pub fn pause_realm(&mut self, realm: RealmId) -> Result<(), RealmRuntimeError> {
        let seat_ids = self.realm_seat_ids(realm)?;
        self.state.authority.pause_realm(realm)?;
        for seat in seat_ids {
            self.quiesce_seat(seat);
            self.publish_seat_capabilities(seat);
        }
        Ok(())
    }

    /// Resume a paused realm with clean keyboard/modifier/grab state.
    pub fn resume_realm(&mut self, realm: RealmId) -> Result<(), RealmRuntimeError> {
        let seat_ids = self.realm_seat_ids(realm)?;
        for seat in &seat_ids {
            if let Some(runtime) = self.state.seat_runtime_mut(*seat)
                && runtime.capabilities.keyboard
                && runtime.keyboard.is_none()
            {
                runtime.keyboard = Some(
                    keyboard::Keyboard::new()
                        .map_err(|error| RealmRuntimeError::Keyboard(error.to_string()))?,
                );
            }
        }
        self.state.authority.resume_realm(realm)?;
        for seat in seat_ids {
            self.publish_seat_capabilities(seat);
        }
        self.state.queue_full_realm_damage(realm);
        Ok(())
    }

    /// Permanently revoke an agent realm, remove its registry globals, quiesce
    /// every bound resource, and atomically return controlled groups to the
    /// fallback realm in the core authority model.
    pub fn revoke_realm(
        &mut self,
        realm: RealmId,
        fallback: RealmId,
    ) -> Result<RealmRevocation, RealmRuntimeError> {
        let seat_ids = self.realm_seat_ids(realm)?;
        let fallback_seat = self
            .realm_seat_ids(fallback)?
            .into_iter()
            .next()
            .ok_or(RealmRuntimeError::RealmHasNoSeat(fallback))?;
        let groups = self.state.authority.snapshot().interaction_groups;
        let output_membership_before = groups
            .iter()
            .filter(|group| group.control_realm == realm || group.observer_realms.contains(&realm))
            .map(|group| {
                (
                    group.id,
                    (
                        group.windows.iter().copied().collect::<Vec<_>>(),
                        std::iter::once(group.control_realm)
                            .chain(group.observer_realms.iter().copied())
                            .collect::<std::collections::BTreeSet<_>>(),
                    ),
                )
            })
            .collect::<std::collections::BTreeMap<_, _>>();
        let transfers = groups
            .into_iter()
            .filter(|group| group.control_realm == realm)
            .map(|group| (group.client, group.windows.into_iter().collect::<Vec<_>>()))
            .collect::<Vec<_>>();
        let all_windows = transfers
            .iter()
            .flat_map(|(_, windows)| windows.iter().copied())
            .collect::<Vec<_>>();
        // Close every sandbox-only listener before changing authority so no
        // connection can enter while revocation is in progress.
        self.realm_portals.retain(|portal| portal.realm != realm);
        self.clear_transferred_focus(realm, &all_windows);
        let receipt = self.state.authority.revoke_realm(realm, fallback)?;
        for (client, _) in &transfers {
            unsafe {
                self.state
                    .migrate_compatibility_resources(*client, fallback_seat);
            }
        }
        for (group, (windows, before)) in output_membership_before {
            let after = self.interaction_group_output_realms(group);
            unsafe {
                update_windows_output_membership(self.state.as_ref(), &windows, &before, &after);
            }
        }
        // Clients launched through this Realm's private portals are part of
        // the revoked sandbox, not transferable host applications. Disconnect
        // them synchronously so they cannot create new surfaces in the gap
        // before the process supervisor delivers SIGKILL.
        let launched_clients = self
            .state
            .clients
            .iter()
            .filter_map(|(raw, client)| {
                (self.state.client_initial_realms.get(client) == Some(&realm))
                    .then_some(*raw as *mut ffi::wl_client)
            })
            .collect::<Vec<_>>();
        for client in launched_clients {
            unsafe { ffi::wl_client_destroy(client) };
        }
        self.refresh_foreign_toplevel_visibility(&all_windows);
        for seat in seat_ids {
            self.quiesce_seat(seat);
            self.publish_seat_capabilities(seat);
            for global in &mut self.state.seat_globals {
                if global.seat == seat && global.active {
                    unsafe { ffi::wl_global_destroy(global.global) };
                    global.active = false;
                }
            }
        }
        for output in &mut self.state.output_globals {
            if output.realm == Some(realm) && output.active {
                unsafe { ffi::wl_global_destroy(output.global) };
                output.global = std::ptr::null_mut();
                output.active = false;
            }
        }
        let _ = self.layout_virtual_realm(realm);
        let _ = self.layout_virtual_realm(fallback);
        debug_assert!(self.state.authority.validate().is_ok());
        Ok(receipt)
    }

    /// Atomically move control of the target window's complete client
    /// interaction group to another realm. aegis intentionally groups every
    /// toplevel on one Wayland client connection; a single-instance
    /// application therefore needs no app-side changes, while split authority
    /// can never create an ambiguous seat stream. Native multi-seat detection
    /// affects resource routing, not the transfer unit.
    pub fn transfer_window_control(
        &mut self,
        window: aegis_core::window::WindowId,
        target: RealmId,
        retain_source_as_observer: bool,
    ) -> Result<AuthorityTransfer, RealmRuntimeError> {
        let (group, client) = self
            .state
            .authority
            .interaction_group_for_window(window)
            .map(|group| (group.id, group.client))
            .ok_or(RealmError::UnknownWindow(window))?;
        let output_realms_before = self.interaction_group_output_realms(group);
        let target_seat = self
            .realm_seat_ids(target)?
            .into_iter()
            .next()
            .ok_or(RealmRuntimeError::RealmHasNoSeat(target))?;
        let receipt = self.state.authority.transfer_control(
            group,
            target,
            TransferOptions {
                retain_source_as_observer,
            },
        )?;
        let output_realms_after = self.interaction_group_output_realms(group);
        unsafe {
            update_windows_output_membership(
                self.state.as_ref(),
                &receipt.windows,
                &output_realms_before,
                &output_realms_after,
            );
        }
        self.clear_transferred_focus(receipt.from, &receipt.windows);
        unsafe {
            self.state
                .migrate_compatibility_resources(client, target_seat);
        }
        self.layout_virtual_realm(target)?;
        if receipt.from != HUMAN_REALM {
            self.layout_virtual_realm(receipt.from)?;
        }
        self.refresh_foreign_toplevel_visibility(&receipt.windows);
        debug_assert!(self.state.authority.validate().is_ok());
        Ok(receipt)
    }

    pub(crate) fn interaction_group_output_realms(
        &self,
        group: aegis_core::realm::InteractionGroupId,
    ) -> std::collections::BTreeSet<RealmId> {
        self.state
            .authority
            .interaction_group(group)
            .map(|group| {
                std::iter::once(group.control_realm)
                    .chain(group.observer_realms.iter().copied())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Commit a bounded, optimistic Realm transaction and then apply its
    /// infallible protocol/runtime consequences. XKB objects needed by resume
    /// operations and target-seat existence are prepared before the authority
    /// model commits, so an error cannot leave model and protocol state split.
    pub fn transact_realms(
        &mut self,
        expected_revision: Option<u64>,
        mutations: &[RealmMutation],
    ) -> Result<RealmTransactionReceipt, RealmRuntimeError> {
        let mut prepared_keyboards = std::collections::BTreeMap::new();
        let mut output_membership_before = std::collections::BTreeMap::new();
        for mutation in mutations {
            match *mutation {
                RealmMutation::TransferWindow { window, target, .. } => {
                    if self.realm_seat_ids(target)?.is_empty() {
                        return Err(RealmRuntimeError::RealmHasNoSeat(target));
                    }
                    if let Some(group) = self
                        .state
                        .authority
                        .interaction_group_for_window(window)
                        .map(|group| group.id)
                    {
                        output_membership_before.entry(group).or_insert_with(|| {
                            (
                                self.state
                                    .authority
                                    .interaction_group(group)
                                    .map(|group| group.windows.iter().copied().collect::<Vec<_>>())
                                    .unwrap_or_default(),
                                self.interaction_group_output_realms(group),
                            )
                        });
                    }
                }
                RealmMutation::SetObserver { group, .. } => {
                    output_membership_before.entry(group).or_insert_with(|| {
                        (
                            self.state
                                .authority
                                .interaction_group(group)
                                .map(|group| group.windows.iter().copied().collect::<Vec<_>>())
                                .unwrap_or_default(),
                            self.interaction_group_output_realms(group),
                        )
                    });
                }
                RealmMutation::SetState {
                    realm,
                    state: aegis_core::realm::RealmState::Active,
                } => {
                    for seat in self.realm_seat_ids(realm)? {
                        let needs_keyboard = self.state.seat_runtime(seat).is_some_and(|runtime| {
                            runtime.capabilities.keyboard && runtime.keyboard.is_none()
                        });
                        if needs_keyboard && !prepared_keyboards.contains_key(&seat) {
                            prepared_keyboards.insert(
                                seat,
                                keyboard::Keyboard::new().map_err(|error| {
                                    RealmRuntimeError::Keyboard(error.to_string())
                                })?,
                            );
                        }
                    }
                }
                _ => {}
            }
        }

        let receipt = self
            .state
            .authority
            .transact(expected_revision, mutations)?;

        for (mutation, result) in mutations.iter().zip(&receipt.results) {
            match result {
                RealmMutationResult::Transferred { receipt: transfer } => {
                    self.clear_transferred_focus(transfer.from, &transfer.windows);
                    let (client, target_seat) = self
                        .state
                        .authority
                        .interaction_group(transfer.group)
                        .and_then(|group| {
                            self.realm_seat_ids(transfer.to)
                                .ok()
                                .and_then(|seats| seats.into_iter().next())
                                .map(|seat| (group.client, seat))
                        })
                        .expect("transaction preflight guaranteed a target seat");
                    unsafe {
                        self.state
                            .migrate_compatibility_resources(client, target_seat);
                    }
                    let _ = self.layout_virtual_realm(transfer.to);
                    if transfer.from != HUMAN_REALM {
                        let _ = self.layout_virtual_realm(transfer.from);
                    }
                    self.refresh_foreign_toplevel_visibility(&transfer.windows);
                }
                RealmMutationResult::ObserverChanged { group, realm, .. } => {
                    let _ = self.layout_virtual_realm(*realm);
                    let windows = self
                        .state
                        .authority
                        .interaction_group(*group)
                        .map(|group| group.windows.iter().copied().collect::<Vec<_>>())
                        .unwrap_or_default();
                    self.refresh_foreign_toplevel_visibility(&windows);
                }
                RealmMutationResult::OutputConfigured { realm, output, .. } => {
                    unsafe {
                        update_realm_output_global(self.state.as_mut(), *realm, *output);
                    }
                    let _ = self.layout_virtual_realm(*realm);
                }
                RealmMutationResult::StateChanged { realm, state, .. } => {
                    let seats = self
                        .realm_seat_ids(*realm)
                        .expect("committed state mutation references a known realm");
                    match state {
                        aegis_core::realm::RealmState::Active => {
                            for seat in &seats {
                                if let Some(keyboard) = prepared_keyboards.remove(seat) {
                                    self.state
                                        .seat_runtime_mut(*seat)
                                        .expect("authority seat must have runtime")
                                        .keyboard = Some(keyboard);
                                }
                            }
                            self.state.queue_full_realm_damage(*realm);
                        }
                        aegis_core::realm::RealmState::Paused => {
                            for seat in &seats {
                                self.quiesce_seat(*seat);
                            }
                        }
                        aegis_core::realm::RealmState::Revoked => {
                            unreachable!("revocation is not a transactional state")
                        }
                    }
                    for seat in seats {
                        self.publish_seat_capabilities(seat);
                    }
                }
            }
            debug_assert!(matches!(
                (mutation, result),
                (
                    RealmMutation::TransferWindow { .. },
                    RealmMutationResult::Transferred { .. }
                ) | (
                    RealmMutation::SetObserver { .. },
                    RealmMutationResult::ObserverChanged { .. },
                ) | (
                    RealmMutation::ConfigureOutput { .. },
                    RealmMutationResult::OutputConfigured { .. },
                ) | (
                    RealmMutation::SetState { .. },
                    RealmMutationResult::StateChanged { .. }
                )
            ));
        }
        for (group, (windows, before)) in output_membership_before {
            let after = self.interaction_group_output_realms(group);
            unsafe {
                update_windows_output_membership(self.state.as_ref(), &windows, &before, &after);
            }
        }
        debug_assert!(self.state.authority.validate().is_ok());
        Ok(receipt)
    }

    pub(crate) fn refresh_foreign_toplevel_visibility(
        &mut self,
        windows: &[aegis_core::window::WindowId],
    ) {
        for window in windows {
            let surface = self.find_surface_by_window_id(*window);
            if !surface.is_null() {
                unsafe {
                    extensions::foreign_toplevel_authority_changed(surface, self.state.as_mut())
                };
            }
        }
    }

    pub(crate) fn realm_seat_ids(&self, realm: RealmId) -> Result<Vec<SeatId>, RealmError> {
        if self.state.authority.realm(realm).is_none() {
            return Err(RealmError::UnknownRealm(realm));
        }
        Ok(self
            .state
            .authority
            .snapshot()
            .seats
            .into_iter()
            .filter(|seat| seat.realm == realm)
            .map(|seat| seat.id)
            .collect())
    }

    pub(crate) fn clear_transferred_focus(
        &mut self,
        source_realm: RealmId,
        windows: &[aegis_core::window::WindowId],
    ) {
        let seats = self.realm_seat_ids(source_realm).unwrap_or_default();
        for seat in seats {
            let pointer = self
                .state
                .seat_runtime(seat)
                .map(|runtime| runtime.pointer_focus)
                .unwrap_or(std::ptr::null_mut());
            let keyboard = self
                .state
                .seat_runtime(seat)
                .map(|runtime| runtime.keyboard_focus)
                .unwrap_or(std::ptr::null_mut());
            let pointer_moved = self.resource_belongs_to_windows(pointer, windows);
            let keyboard_moved = self.resource_belongs_to_windows(keyboard, windows);
            if !pointer_moved && !keyboard_moved {
                continue;
            }
            let Some(_guard) = ActiveSeatGuard::enter(self.state.as_mut(), seat) else {
                continue;
            };
            if pointer_moved {
                self.change_pointer_focus(std::ptr::null_mut());
            }
            if keyboard_moved {
                self.change_keyboard_focus(std::ptr::null_mut());
            }
            if self
                .state
                .interactive
                .is_some_and(|grab| windows.contains(&grab.window_id()))
            {
                self.finish_interactive();
            }
            if self.state.drag.is_some() {
                unsafe { cancel_drag(self.state.as_mut(), true) };
            }
            self.state.implicit_grab_active = false;
        }
    }

    pub(crate) fn resource_belongs_to_windows(
        &self,
        resource: *mut ffi::wl_resource,
        windows: &[aegis_core::window::WindowId],
    ) -> bool {
        if resource.is_null() {
            return false;
        }
        let rec = unsafe { ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec };
        let root = unsafe { surface_root_toplevel(rec) };
        !root.is_null() && windows.contains(unsafe { &(*root).window.id })
    }

    pub(crate) fn layout_virtual_realm(&mut self, realm: RealmId) -> Result<(), RealmRuntimeError> {
        let Some(output) = self.realm_output(realm) else {
            if self.state.authority.realm(realm).is_some() {
                return Ok(());
            }
            return Err(RealmError::UnknownRealm(realm).into());
        };
        let mut windows = self
            .state
            .authority
            .snapshot()
            .interaction_groups
            .into_iter()
            .filter(|group| group.control_realm == realm || group.observer_realms.contains(&realm))
            .flat_map(|group| group.windows)
            .collect::<Vec<_>>();
        windows.sort_unstable();
        windows.dedup();
        self.state
            .realm_placements
            .retain(|(placement_realm, window), _| {
                *placement_realm != realm || windows.contains(window)
            });
        let area = aegis_core::Rect::new(0, 0, output.width as i32, output.height as i32);
        let slots = aegis_core::overview::grid(area, windows.len());
        for (window, slot) in windows.into_iter().zip(slots) {
            let rec = self.find_surface_by_window_id(window);
            let size = if rec.is_null() {
                slot.size
            } else {
                unsafe {
                    if (*rec).window.size.w > 0 && (*rec).window.size.h > 0 {
                        (*rec).window.size
                    } else {
                        surface_logical_size(&*rec)
                    }
                }
            };
            self.state
                .realm_placements
                .insert((realm, window), aegis_core::overview::fit(slot, size));
        }
        // Placements may have shifted for every window, so a full-output
        // invalidation is the only conservative damage until the new layout is
        // presented. Surface commits use window-local damage thereafter.
        self.state.queue_full_realm_damage(realm);
        Ok(())
    }

    pub(crate) fn publish_seat_capabilities(&mut self, seat: SeatId) {
        let capabilities = seat_wire_capabilities(&self.state, seat);
        let resources = self
            .state
            .seat_runtime(seat)
            .map(|runtime| runtime.seat_resources.clone())
            .unwrap_or_default();
        for resource in resources.into_iter().filter(|resource| !resource.is_null()) {
            unsafe {
                ffi::wl_resource_post_event(resource, ffi::WL_SEAT_CAPABILITIES, capabilities)
            };
        }
    }

    pub(crate) fn quiesce_seat(&mut self, seat: SeatId) {
        let Some(_guard) = ActiveSeatGuard::enter_existing(self.state.as_mut(), seat) else {
            return;
        };
        self.change_pointer_focus(std::ptr::null_mut());
        self.change_keyboard_focus(std::ptr::null_mut());
        let Some(runtime) = self.state.seat_runtime_mut(seat) else {
            return;
        };
        unsafe {
            for touch in runtime
                .touch_resources
                .iter()
                .copied()
                .filter(|resource| !resource.is_null())
            {
                ffi::wl_resource_post_event(touch, ffi::WL_TOUCH_CANCEL);
            }
        }
        runtime.pointer_focus = std::ptr::null_mut();
        runtime.keyboard_focus = std::ptr::null_mut();
        runtime.tablet_focus = std::ptr::null_mut();
        runtime.implicit_grab_active = false;
        runtime.interactive = None;
        runtime.compositor_pointer_grab = false;
        runtime.drag = None;
        runtime.swipe_gesture_client = std::ptr::null_mut();
        runtime.pinch_gesture_client = std::ptr::null_mut();
        runtime.hold_gesture_client = std::ptr::null_mut();
        runtime.depressed_mods = aegis_core::input::Mods::NONE;
        runtime.client_pressed_keys.clear();
        runtime.keyboard = None;
        runtime.cursor_surface = std::ptr::null_mut();
        runtime.cursor_hidden = false;
        runtime.cursor_shape = 1;
    }

    /// Process pending client events and flush queued events. Non-blocking.
    pub fn dispatch(&mut self) {
        self.accept_realm_portal_clients();
        unsafe {
            let loop_ = ffi::wl_display_get_event_loop(self.state.display);
            ffi::wl_event_loop_dispatch(loop_, 0);
            ffi::wl_display_flush_clients(self.state.display);
        }
        let pending_layouts = std::mem::take(&mut self.state.pending_realm_layouts);
        for realm in pending_layouts {
            if let Err(error) = self.layout_virtual_realm(realm) {
                log::warn!("could not update Realm {} layout: {error}", realm.0);
            }
        }
        if let Some((seat, surface)) = self.state.pending_activation.take()
            && let Some(_guard) = ActiveSeatGuard::enter(self.state.as_mut(), seat)
        {
            self.change_keyboard_focus(surface);
        }
        // A toplevel that maps without taking focus (hidden workspace,
        // observation mirror, minimized) lands at the Vec tail from its
        // wl_surface creation; keep it below the always-on-top band.
        self.restack_always_on_top_band();
        let pending_popup_focus = std::mem::take(&mut self.state.pending_popup_focus);
        for (seat, surface) in pending_popup_focus {
            if let Some(_guard) = ActiveSeatGuard::enter(self.state.as_mut(), seat) {
                self.change_keyboard_focus(surface);
            }
        }
        if self.state.lock_focus_dirty {
            self.state.lock_focus_dirty = false;
            let focus = self.state.pending_lock_focus;
            self.change_pointer_focus(focus);
            self.change_keyboard_focus(focus);
        }
        if self.state.session_lock_phase.is_active() {
            // The compositor-rendered opaque fallback is sufficient after the
            // bounded grace period even if the locker has not mapped every
            // output yet. The event is deferred until presentation_complete.
            self.state
                .session_lock_phase
                .expire_surface_grace(std::time::Instant::now(), std::time::Duration::from_secs(1));
        }
        unsafe { extensions::update_idle_notifications(self.state.as_mut()) };
    }

    /// Drain scene damage in virtual-output logical coordinates.
    ///
    /// A surface commit invalidates the complete Realm-local placement of its
    /// root toplevel. This is deliberately conservative: transform, viewport,
    /// subsurface and layout changes cannot under-report pixels, while agents
    /// can still avoid polling or recapturing unrelated outputs. Topology
    /// changes enqueue full-output damage. The queue is bounded to at most 64
    /// rectangles per Realm, collapsing excess entries to one bounding box.
    pub fn take_realm_damage(
        &mut self,
    ) -> std::collections::BTreeMap<RealmId, Vec<aegis_core::Rect>> {
        let changed_windows = std::mem::take(&mut self.state.damaged_windows);
        let mut damage = std::mem::take(&mut self.state.pending_realm_damage);

        if self.state.session_lock_phase.is_active() {
            return std::collections::BTreeMap::new();
        }

        for window in changed_windows {
            for ((realm, placement_window), placement) in &self.state.realm_placements {
                if *placement_window != window
                    || !self.state.authority.realm_observes_window(*realm, window)
                {
                    continue;
                }
                let Some(record) = self.state.authority.realm(*realm) else {
                    continue;
                };
                if record.state != aegis_core::realm::RealmState::Active
                    || !matches!(record.presentation, PresentationTarget::Virtual { .. })
                {
                    continue;
                }
                damage.entry(*realm).or_default().push(*placement);
            }
        }

        damage.retain(|realm, rects| {
            let Some(record) = self.state.authority.realm(*realm) else {
                return false;
            };
            let PresentationTarget::Virtual { output } = record.presentation else {
                return false;
            };
            if record.state != aegis_core::realm::RealmState::Active {
                return false;
            }
            normalize_realm_damage(
                rects,
                aegis_core::Rect::new(0, 0, output.width as i32, output.height as i32),
            );
            !rects.is_empty()
        });
        damage
    }

    /// Set or clear the effective surfaceless idle inhibitor held by scoped
    /// IPC connections. While set, idle notifications stay resumed as if a
    /// visible per-surface inhibitor were active.
    pub fn set_ipc_idle_inhibit(&mut self, inhibited: bool) {
        if self.state.ipc_idle_inhibit == inhibited {
            return;
        }
        log::info!("idle: IPC idle inhibit {inhibited}");
        self.state.ipc_idle_inhibit = inhibited;
        unsafe { extensions::update_idle_notifications(self.state.as_mut()) };
    }

    /// Development-only escape hatch (`[dev] allow_quit_while_locked`): while
    /// set, the global Quit binding still matches during an active session
    /// lock. Will be removed before release; do not rely on it.
    pub fn set_allow_quit_while_locked(&mut self, allow: bool) {
        if self.state.allow_quit_while_locked == allow {
            return;
        }
        log::info!("input: allow quit while locked {allow}");
        self.state.allow_quit_while_locked = allow;
    }

    /// Whether normal client content and compositor chrome must be hidden.
    /// This becomes true as soon as a lock request is accepted, before the
    /// protocol's `locked` event, so the next frame fails closed.
    pub fn session_locked(&self) -> bool {
        self.state.session_lock_phase.is_active()
    }

    /// Whether a secure frame has been physically presented and acknowledged.
    /// Power transitions must not rely on the earlier request-time state.
    pub fn session_lock_confirmed(&self) -> bool {
        self.state.session_lock_phase.is_confirmed()
    }

    /// A newly blanked/locked frame must be confirmed on every output before
    /// the protocol lock request can be acknowledged.
    pub fn lock_confirmation_pending(&self) -> bool {
        self.state.session_lock_phase.frame_pending()
    }

    /// Confirm that the just-submitted secure frame reached all outputs.
    pub fn presentation_complete(&mut self) {
        unsafe { extensions::session_lock_presented(self.state.as_mut()) };
        unsafe { ffi::wl_display_flush_clients(self.state.display) };
    }
}
