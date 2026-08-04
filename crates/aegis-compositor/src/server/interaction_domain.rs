use crate::*;

impl Server {
    /// Point-in-time authority state for the shell and IPC layers.
    pub fn interaction_domain_snapshot(&self) -> InteractionDomainSnapshot {
        self.state.authority.snapshot()
    }

    /// Current authority model revision. Cheap alternative to
    /// [`Self::interaction_domain_snapshot`] for per-frame change detection: the frame
    /// loop rebuilds the owned snapshot only when this moves.
    pub fn interaction_domain_revision(&self) -> u64 {
        self.state.authority.revision()
    }

    /// Prepare a capability listener for one compositor-mediated Interaction Domain launch.
    ///
    /// The randomized host pathname exists only while bubblewrap installs a
    /// bind mount of the socket inode. [`Self::activate_interaction_domain_portal`] refuses
    /// the portal until the launcher has removed that ambient pathname.
    pub fn prepare_interaction_domain_portal(
        &self,
        interaction_domain: InteractionDomainId,
    ) -> Result<InteractionDomainPortal, InteractionDomainRuntimeError> {
        let record = self
            .state
            .authority
            .interaction_domain(interaction_domain)
            .ok_or(InteractionDomainError::UnknownInteractionDomain(
                interaction_domain,
            ))?;
        if record.state != aegis_model::interaction_domain::InteractionDomainState::Active {
            return Err(
                InteractionDomainError::InteractionDomainNotActive(interaction_domain).into(),
            );
        }

        let runtime = std::env::var_os("XDG_RUNTIME_DIR").ok_or_else(|| {
            InteractionDomainRuntimeError::Portal("XDG_RUNTIME_DIR is not available".into())
        })?;
        let base = PathBuf::from(runtime).join("aegis-portals");
        std::fs::DirBuilder::new()
            .mode(0o700)
            .recursive(true)
            .create(&base)
            .map_err(|error| InteractionDomainRuntimeError::Portal(error.to_string()))?;
        std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o700))
            .map_err(|error| InteractionDomainRuntimeError::Portal(error.to_string()))?;

        let mut last_error = None;
        for _ in 0..8 {
            let token = random_portal_token()
                .map_err(|error| InteractionDomainRuntimeError::Portal(error.to_string()))?;
            let directory = base.join(format!(
                "interaction_domain-{}-{token}",
                interaction_domain.0
            ));
            match std::fs::DirBuilder::new().mode(0o700).create(&directory) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    last_error = Some(error);
                    continue;
                }
                Err(error) => return Err(InteractionDomainRuntimeError::Portal(error.to_string())),
            }
            let path = directory.join("wayland");
            let listener = match UnixListener::bind(&path) {
                Ok(listener) => listener,
                Err(error) => {
                    let _ = std::fs::remove_dir(&directory);
                    return Err(InteractionDomainRuntimeError::Portal(error.to_string()));
                }
            };
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
                .map_err(|error| InteractionDomainRuntimeError::Portal(error.to_string()))?;
            listener
                .set_nonblocking(true)
                .map_err(|error| InteractionDomainRuntimeError::Portal(error.to_string()))?;
            return Ok(InteractionDomainPortal {
                interaction_domain,
                path,
                listener,
            });
        }
        Err(InteractionDomainRuntimeError::Portal(format!(
            "could not allocate a unique portal path: {}",
            last_error
                .map(|error| error.to_string())
                .unwrap_or_else(|| "random-name collision".into())
        )))
    }

    /// Activate a portal after bubblewrap has made its socket inode private to
    /// the sandbox mount namespace.
    pub fn activate_interaction_domain_portal(
        &mut self,
        portal: InteractionDomainPortal,
    ) -> Result<(), InteractionDomainRuntimeError> {
        let record = self
            .state
            .authority
            .interaction_domain(portal.interaction_domain)
            .ok_or(InteractionDomainError::UnknownInteractionDomain(
                portal.interaction_domain,
            ))?;
        if record.state != aegis_model::interaction_domain::InteractionDomainState::Active {
            return Err(InteractionDomainError::InteractionDomainNotActive(
                portal.interaction_domain,
            )
            .into());
        }
        if portal.path.exists() {
            return Err(InteractionDomainRuntimeError::Portal(
                "sandbox launch gate did not remove the ambient portal path".into(),
            ));
        }
        self.interaction_domain_portals.push(portal);
        Ok(())
    }

    pub(crate) fn accept_interaction_domain_portal_clients(&mut self) {
        for portal in &self.interaction_domain_portals {
            loop {
                let (stream, _) = match portal.listener.accept() {
                    Ok(connection) => connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(error) => {
                        log::warn!(
                            "InteractionDomain {} portal accept failed: {error}",
                            portal.interaction_domain.0
                        );
                        break;
                    }
                };
                if let Err(error) = stream.set_nonblocking(true) {
                    log::warn!(
                        "InteractionDomain {} portal client could not become non-blocking: {error}",
                        portal.interaction_domain.0
                    );
                    continue;
                }
                let fd = stream.into_raw_fd();
                let wl_client = unsafe { ffi::wl_client_create(self.state.display, fd) };
                if wl_client.is_null() {
                    unsafe { libc_close(fd) };
                    log::warn!(
                        "InteractionDomain {} portal client was rejected",
                        portal.interaction_domain.0
                    );
                    continue;
                }
                unsafe {
                    self.state.ensure_client_with_interaction_domain(
                        wl_client,
                        Some(portal.interaction_domain),
                    );
                }
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn interaction_domain_portal_count(&self) -> usize {
        self.interaction_domain_portals.len()
    }

    pub fn configure_interaction_domain_output(
        &mut self,
        interaction_domain: InteractionDomainId,
        output: VirtualOutput,
    ) -> Result<(), InteractionDomainRuntimeError> {
        self.state
            .authority
            .configure_virtual_output(interaction_domain, output)?;
        unsafe {
            update_interaction_domain_output_global(self.state.as_mut(), interaction_domain, output)
        };
        self.layout_virtual_interaction_domain(interaction_domain)?;
        Ok(())
    }

    pub fn interaction_domain_output(
        &self,
        interaction_domain: InteractionDomainId,
    ) -> Option<VirtualOutput> {
        match self
            .state
            .authority
            .interaction_domain(interaction_domain)?
            .presentation
        {
            PresentationTarget::Virtual { output } => Some(output),
            _ => None,
        }
    }

    /// Window layout metadata corresponding to the directed Interaction Domain render.
    pub fn interaction_domain_window_placements(
        &self,
        interaction_domain: InteractionDomainId,
    ) -> Vec<aegis_model::interaction_domain::InteractionDomainWindowPlacement> {
        let mut placements = self
            .state
            .interaction_domain_placements
            .iter()
            .filter_map(|((placement_interaction_domain, window), output_rect)| {
                if *placement_interaction_domain != interaction_domain
                    || !self
                        .state
                        .authority
                        .interaction_domain_observes_window(interaction_domain, *window)
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
                    aegis_model::interaction_domain::InteractionDomainWindowPlacement {
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

    /// Build the semantic observation tree the compositor can prove from its
    /// own model. Application-internal accessibility nodes may be attached by
    /// a protocol adapter later; framebuffer pixels never synthesize nodes.
    pub fn interaction_domain_semantic_snapshot(
        &self,
        interaction_domain: InteractionDomainId,
    ) -> Result<aegis_model::semantic::SemanticSnapshot, InteractionDomainRuntimeError> {
        const MAX_SEMANTIC_OBJECTS: usize = 4_096;
        const MAX_SEMANTIC_LABEL_BYTES: usize = 1_024;
        use aegis_model::semantic::{
            SemanticAction, SemanticObject, SemanticObjectId, SemanticRole, SemanticSnapshot,
            SemanticSource, SemanticState,
        };

        let authority = self.interaction_domain_snapshot();
        let interaction_domain_record = authority
            .interaction_domains
            .iter()
            .find(|candidate| candidate.id == interaction_domain)
            .ok_or(InteractionDomainError::UnknownInteractionDomain(
                interaction_domain,
            ))?;
        if interaction_domain_record.state
            != aegis_model::interaction_domain::InteractionDomainState::Active
        {
            return Err(
                InteractionDomainError::InteractionDomainNotActive(interaction_domain).into(),
            );
        }
        let seats = authority
            .seats
            .iter()
            .filter(|seat| seat.interaction_domain == interaction_domain && seat.enabled)
            .collect::<Vec<_>>();
        let placements = self.interaction_domain_window_placements(interaction_domain);
        if placements.len() > MAX_SEMANTIC_OBJECTS {
            return Err(InteractionDomainRuntimeError::SemanticObservationTooLarge {
                limit: MAX_SEMANTIC_OBJECTS,
            });
        }
        let bounded_label = |value: Option<String>| {
            value.map(|mut value| {
                if value.len() > MAX_SEMANTIC_LABEL_BYTES {
                    let mut end = MAX_SEMANTIC_LABEL_BYTES;
                    while !value.is_char_boundary(end) {
                        end -= 1;
                    }
                    value.truncate(end);
                }
                value
            })
        };
        let mut objects = Vec::with_capacity(placements.len());
        for placement in placements {
            let rec = self.find_surface_by_window_id(placement.window);
            if rec.is_null() {
                continue;
            }
            let window = unsafe { (*rec).window.clone() };
            // One window semantic root represents its complete Wayland
            // surface tree. Fold every descendant generation into the
            // revision so a subsurface or popup commit invalidates an older
            // observation just like a root-buffer commit.
            let content_revision = self.state.surfaces.iter().enumerate().fold(
                0xcbf29ce484222325u64,
                |revision, (index, surface)| {
                    if surface.is_null() || unsafe { surface_root_toplevel(*surface) } != rec {
                        return revision;
                    }
                    let surface = unsafe { &**surface };
                    let word = surface.generation
                        ^ (index as u64).rotate_left(17)
                        ^ (surface.mapped as u64).rotate_left(31)
                        ^ (surface.width as u64).rotate_left(7)
                        ^ (surface.height as u64).rotate_left(23);
                    revision.wrapping_mul(0x100000001b3) ^ word
                },
            );
            let controlled = authority.interaction_groups.iter().any(|group| {
                group.control_interaction_domain == interaction_domain
                    && group.windows.contains(&placement.window)
            });
            let mut actions = Vec::new();
            if controlled {
                actions.push(SemanticAction::Focus);
                if seats.iter().any(|seat| seat.capabilities.pointer) {
                    actions.push(SemanticAction::Pointer);
                    actions.push(SemanticAction::Scroll);
                }
                if seats.iter().any(|seat| seat.capabilities.keyboard) {
                    actions.push(SemanticAction::TypeText);
                }
            }
            let focused = seats
                .iter()
                .any(|seat| self.seat_focuses_window(seat.id, placement.window));
            let app_id = bounded_label(window.app_id);
            objects.push(SemanticObject {
                id: SemanticObjectId::for_window(placement.window),
                parent: None,
                window: placement.window,
                source: SemanticSource::Compositor,
                role: SemanticRole::Window,
                name: bounded_label(window.title).or_else(|| app_id.clone()),
                description: None,
                value: None,
                app_id,
                bounds: placement.output_rect,
                local_size: placement.surface_size,
                state: SemanticState {
                    visible: true,
                    enabled: controlled && !seats.is_empty(),
                    focused,
                    read_only: !controlled,
                    minimized: window.minimized,
                },
                actions,
                revision: content_revision,
            });
            objects.extend(
                self.state
                    .semantic_trees
                    .objects_for_window(placement.window, placement.output_rect),
            );
            if objects.len() > MAX_SEMANTIC_OBJECTS {
                return Err(InteractionDomainRuntimeError::SemanticObservationTooLarge {
                    limit: MAX_SEMANTIC_OBJECTS,
                });
            }
        }
        objects.sort_by_key(|object| object.id);
        Ok(SemanticSnapshot {
            interaction_domain,
            authority_revision: authority.revision,
            objects,
        })
    }

    /// Publish one authenticated adapter's complete accessibility tree for a
    /// live toplevel. The update is validated before replacing the previous
    /// revision; partial or malformed trees never become observable.
    pub fn publish_accessibility_tree(
        &mut self,
        provider: aegis_semantic::SemanticProviderId,
        update: aegis_semantic::AccessibilityTreeUpdate,
    ) -> Result<(), String> {
        let rec = self.find_surface_by_window_id(update.window);
        if rec.is_null() || unsafe { !(*rec).mapped || (*rec).xdg_toplevel.is_null() } {
            return Err("accessibility tree targets an unknown or unmapped window".into());
        }
        let surface_size = unsafe {
            aegis_model::Size {
                w: (*rec).width,
                h: (*rec).height,
            }
        };
        self.state
            .semantic_trees
            .publish(provider, update, surface_size)
    }

    pub fn resolve_semantic_dispatch(
        &self,
        target: aegis_model::semantic::SemanticObjectId,
    ) -> Option<aegis_semantic::SemanticDispatchTarget> {
        self.state.semantic_trees.resolve(target)
    }

    pub fn revoke_semantic_provider(&mut self, provider: &aegis_semantic::SemanticProviderId) {
        self.state.semantic_trees.revoke_provider(provider);
    }

    /// Create an independently advertised agent seat and its authority interaction domain.
    ///
    /// The XKB state is prepared before the authority mutation. If advertising
    /// the Wayland global fails, the new interaction domain is immediately revoked so no
    /// active-but-unreachable authority survives the failed operation.
    pub fn create_agent_interaction_domain(
        &mut self,
        label: impl Into<String>,
        capabilities: SeatCapabilities,
    ) -> Result<InteractionDomainBundle, InteractionDomainRuntimeError> {
        self.create_agent_interaction_domain_for_subject(label, capabilities, None)
    }

    /// Create an agent Interaction Domain and bind it to the authenticated IPC subject
    /// that requested it. The subject comes from compositor-owned connection
    /// state, never from the Interaction Domain action payload.
    pub fn create_agent_interaction_domain_for_subject(
        &mut self,
        label: impl Into<String>,
        capabilities: SeatCapabilities,
        subject: Option<String>,
    ) -> Result<InteractionDomainBundle, InteractionDomainRuntimeError> {
        let keyboard = if capabilities.keyboard {
            Some(
                keyboard::Keyboard::new()
                    .map_err(|error| InteractionDomainRuntimeError::Keyboard(error.to_string()))?,
            )
        } else {
            None
        };
        let bundle = self
            .state
            .authority
            .create_agent_interaction_domain_for_subject(label, capabilities, subject);
        let mut runtime = Box::new(SeatRuntime::new(
            bundle.seat,
            bundle.interaction_domain,
            bundle.principal,
            capabilities,
        ));
        runtime.keyboard = keyboard;
        self.state.seats.insert(bundle.seat, runtime);

        let global = unsafe { create_seat_global(self.state.as_mut(), bundle.seat) };
        if global.is_null() {
            self.state
                .authority
                .revoke_interaction_domain(bundle.interaction_domain, HUMAN_INTERACTION_DOMAIN)
                .expect("new agent interaction_domain must be revocable");
            self.state.seats.remove(&bundle.seat);
            return Err(InteractionDomainRuntimeError::SeatGlobal);
        }
        let output = self
            .interaction_domain_output(bundle.interaction_domain)
            .expect("new agent interaction_domain must have a virtual output");
        if !unsafe {
            create_interaction_domain_output_global(
                self.state.as_mut(),
                bundle.interaction_domain,
                output,
            )
        } {
            let _ =
                self.revoke_interaction_domain(bundle.interaction_domain, HUMAN_INTERACTION_DOMAIN);
            return Err(InteractionDomainRuntimeError::OutputGlobal);
        }
        debug_assert!(self.state.authority.validate().is_ok());
        Ok(bundle)
    }

    /// Stop all input delivery from an interaction domain while preserving its identity and
    /// transferable window authority for a later resume.
    pub fn pause_interaction_domain(
        &mut self,
        interaction_domain: InteractionDomainId,
    ) -> Result<(), InteractionDomainRuntimeError> {
        let seat_ids = self.interaction_domain_seat_ids(interaction_domain)?;
        self.state
            .authority
            .pause_interaction_domain(interaction_domain)?;
        for seat in seat_ids {
            self.quiesce_seat(seat);
            self.publish_seat_capabilities(seat);
        }
        Ok(())
    }

    /// Resume a paused interaction domain with clean keyboard/modifier/grab state.
    pub fn resume_interaction_domain(
        &mut self,
        interaction_domain: InteractionDomainId,
    ) -> Result<(), InteractionDomainRuntimeError> {
        let seat_ids = self.interaction_domain_seat_ids(interaction_domain)?;
        for seat in &seat_ids {
            if let Some(runtime) = self.state.seat_runtime_mut(*seat)
                && runtime.capabilities.keyboard
                && runtime.keyboard.is_none()
            {
                runtime.keyboard =
                    Some(keyboard::Keyboard::new().map_err(|error| {
                        InteractionDomainRuntimeError::Keyboard(error.to_string())
                    })?);
            }
        }
        self.state
            .authority
            .resume_interaction_domain(interaction_domain)?;
        for seat in seat_ids {
            self.publish_seat_capabilities(seat);
        }
        self.state
            .queue_full_interaction_domain_damage(interaction_domain);
        Ok(())
    }

    /// Permanently revoke an agent interaction domain, remove its registry globals, quiesce
    /// every bound resource, and atomically return controlled groups to the
    /// fallback interaction domain in the core authority model.
    pub fn revoke_interaction_domain(
        &mut self,
        interaction_domain: InteractionDomainId,
        fallback: InteractionDomainId,
    ) -> Result<InteractionDomainRevocation, InteractionDomainRuntimeError> {
        let seat_ids = self.interaction_domain_seat_ids(interaction_domain)?;
        let fallback_seat = self
            .interaction_domain_seat_ids(fallback)?
            .into_iter()
            .next()
            .ok_or(InteractionDomainRuntimeError::InteractionDomainHasNoSeat(
                fallback,
            ))?;
        let groups = self.state.authority.snapshot().interaction_groups;
        let output_membership_before = groups
            .iter()
            .filter(|group| {
                group.control_interaction_domain == interaction_domain
                    || group
                        .observer_interaction_domains
                        .contains(&interaction_domain)
            })
            .map(|group| {
                (
                    group.id,
                    (
                        group.windows.iter().copied().collect::<Vec<_>>(),
                        std::iter::once(group.control_interaction_domain)
                            .chain(group.observer_interaction_domains.iter().copied())
                            .collect::<std::collections::BTreeSet<_>>(),
                    ),
                )
            })
            .collect::<std::collections::BTreeMap<_, _>>();
        let transfers = groups
            .into_iter()
            .filter(|group| group.control_interaction_domain == interaction_domain)
            .map(|group| (group.client, group.windows.into_iter().collect::<Vec<_>>()))
            .collect::<Vec<_>>();
        let all_windows = transfers
            .iter()
            .flat_map(|(_, windows)| windows.iter().copied())
            .collect::<Vec<_>>();
        // Close every sandbox-only listener before changing authority so no
        // connection can enter while revocation is in progress.
        self.interaction_domain_portals
            .retain(|portal| portal.interaction_domain != interaction_domain);
        self.clear_transferred_focus(interaction_domain, &all_windows);
        let receipt = self
            .state
            .authority
            .revoke_interaction_domain(interaction_domain, fallback)?;
        for (client, _) in &transfers {
            unsafe {
                self.state
                    .migrate_compatibility_resources(*client, fallback_seat);
            }
        }
        for (group, (windows, before)) in output_membership_before {
            let after = self.interaction_group_output_interaction_domains(group);
            unsafe {
                update_windows_output_membership(self.state.as_ref(), &windows, &before, &after);
            }
        }
        // Clients launched through this Interaction Domain's private portals are part of
        // the revoked sandbox, not transferable host applications. Disconnect
        // them synchronously so they cannot create new surfaces in the gap
        // before the process supervisor delivers SIGKILL.
        let launched_clients = self
            .state
            .clients
            .iter()
            .filter_map(|(raw, client)| {
                (self.state.client_initial_interaction_domains.get(client)
                    == Some(&interaction_domain))
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
            if output.interaction_domain == Some(interaction_domain) && output.active {
                unsafe { ffi::wl_global_destroy(output.global) };
                output.global = std::ptr::null_mut();
                output.active = false;
            }
        }
        let _ = self.layout_virtual_interaction_domain(interaction_domain);
        let _ = self.layout_virtual_interaction_domain(fallback);
        debug_assert!(self.state.authority.validate().is_ok());
        Ok(receipt)
    }

    /// Atomically move control of the target window's complete client
    /// interaction group to another interaction domain. aegis intentionally groups every
    /// toplevel on one Wayland client connection; a single-instance
    /// application therefore needs no app-side changes, while split authority
    /// can never create an ambiguous seat stream. Native multi-seat detection
    /// affects resource routing, not the transfer unit.
    pub fn transfer_window_control(
        &mut self,
        window: aegis_model::window::WindowId,
        target: InteractionDomainId,
        retain_source_as_observer: bool,
    ) -> Result<AuthorityTransfer, InteractionDomainRuntimeError> {
        let (group, client) = self
            .state
            .authority
            .interaction_group_for_window(window)
            .map(|group| (group.id, group.client))
            .ok_or(InteractionDomainError::UnknownWindow(window))?;
        let output_interaction_domains_before =
            self.interaction_group_output_interaction_domains(group);
        let target_seat = self
            .interaction_domain_seat_ids(target)?
            .into_iter()
            .next()
            .ok_or(InteractionDomainRuntimeError::InteractionDomainHasNoSeat(
                target,
            ))?;
        let receipt = self.state.authority.transfer_control(
            group,
            target,
            TransferOptions {
                retain_source_as_observer,
            },
        )?;
        let output_interaction_domains_after =
            self.interaction_group_output_interaction_domains(group);
        unsafe {
            update_windows_output_membership(
                self.state.as_ref(),
                &receipt.windows,
                &output_interaction_domains_before,
                &output_interaction_domains_after,
            );
        }
        self.clear_transferred_focus(receipt.from, &receipt.windows);
        unsafe {
            self.state
                .migrate_compatibility_resources(client, target_seat);
        }
        self.layout_virtual_interaction_domain(target)?;
        if receipt.from != HUMAN_INTERACTION_DOMAIN {
            self.layout_virtual_interaction_domain(receipt.from)?;
        }
        self.refresh_foreign_toplevel_visibility(&receipt.windows);
        debug_assert!(self.state.authority.validate().is_ok());
        Ok(receipt)
    }

    pub(crate) fn interaction_group_output_interaction_domains(
        &self,
        group: aegis_model::interaction_domain::InteractionGroupId,
    ) -> std::collections::BTreeSet<InteractionDomainId> {
        self.state
            .authority
            .interaction_group(group)
            .map(|group| {
                std::iter::once(group.control_interaction_domain)
                    .chain(group.observer_interaction_domains.iter().copied())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Commit a bounded, optimistic Interaction Domain transaction and then apply its
    /// infallible protocol/runtime consequences. XKB objects needed by resume
    /// operations and target-seat existence are prepared before the authority
    /// model commits, so an error cannot leave model and protocol state split.
    pub fn transact_interaction_domains(
        &mut self,
        expected_revision: Option<u64>,
        mutations: &[InteractionDomainMutation],
    ) -> Result<InteractionDomainTransactionReceipt, InteractionDomainRuntimeError> {
        let mut prepared_keyboards = std::collections::BTreeMap::new();
        let mut output_membership_before = std::collections::BTreeMap::new();
        for mutation in mutations {
            match *mutation {
                InteractionDomainMutation::TransferWindow { window, target, .. } => {
                    if self.interaction_domain_seat_ids(target)?.is_empty() {
                        return Err(InteractionDomainRuntimeError::InteractionDomainHasNoSeat(
                            target,
                        ));
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
                                self.interaction_group_output_interaction_domains(group),
                            )
                        });
                    }
                }
                InteractionDomainMutation::SetObserver { group, .. } => {
                    output_membership_before.entry(group).or_insert_with(|| {
                        (
                            self.state
                                .authority
                                .interaction_group(group)
                                .map(|group| group.windows.iter().copied().collect::<Vec<_>>())
                                .unwrap_or_default(),
                            self.interaction_group_output_interaction_domains(group),
                        )
                    });
                }
                InteractionDomainMutation::SetState {
                    interaction_domain,
                    state: aegis_model::interaction_domain::InteractionDomainState::Active,
                } => {
                    for seat in self.interaction_domain_seat_ids(interaction_domain)? {
                        let needs_keyboard = self.state.seat_runtime(seat).is_some_and(|runtime| {
                            runtime.capabilities.keyboard && runtime.keyboard.is_none()
                        });
                        if needs_keyboard && !prepared_keyboards.contains_key(&seat) {
                            prepared_keyboards.insert(
                                seat,
                                keyboard::Keyboard::new().map_err(|error| {
                                    InteractionDomainRuntimeError::Keyboard(error.to_string())
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
                InteractionDomainMutationResult::Transferred { receipt: transfer } => {
                    self.clear_transferred_focus(transfer.from, &transfer.windows);
                    let (client, target_seat) = self
                        .state
                        .authority
                        .interaction_group(transfer.group)
                        .and_then(|group| {
                            self.interaction_domain_seat_ids(transfer.to)
                                .ok()
                                .and_then(|seats| seats.into_iter().next())
                                .map(|seat| (group.client, seat))
                        })
                        .expect("transaction preflight guaranteed a target seat");
                    unsafe {
                        self.state
                            .migrate_compatibility_resources(client, target_seat);
                    }
                    let _ = self.layout_virtual_interaction_domain(transfer.to);
                    if transfer.from != HUMAN_INTERACTION_DOMAIN {
                        let _ = self.layout_virtual_interaction_domain(transfer.from);
                    }
                    self.refresh_foreign_toplevel_visibility(&transfer.windows);
                }
                InteractionDomainMutationResult::ObserverChanged {
                    group,
                    interaction_domain,
                    ..
                } => {
                    let _ = self.layout_virtual_interaction_domain(*interaction_domain);
                    let windows = self
                        .state
                        .authority
                        .interaction_group(*group)
                        .map(|group| group.windows.iter().copied().collect::<Vec<_>>())
                        .unwrap_or_default();
                    self.refresh_foreign_toplevel_visibility(&windows);
                }
                InteractionDomainMutationResult::OutputConfigured {
                    interaction_domain,
                    output,
                    ..
                } => {
                    unsafe {
                        update_interaction_domain_output_global(
                            self.state.as_mut(),
                            *interaction_domain,
                            *output,
                        );
                    }
                    let _ = self.layout_virtual_interaction_domain(*interaction_domain);
                }
                InteractionDomainMutationResult::StateChanged {
                    interaction_domain,
                    state,
                    ..
                } => {
                    let seats = self
                        .interaction_domain_seat_ids(*interaction_domain)
                        .expect("committed state mutation references a known interaction_domain");
                    match state {
                        aegis_model::interaction_domain::InteractionDomainState::Active => {
                            for seat in &seats {
                                if let Some(keyboard) = prepared_keyboards.remove(seat) {
                                    self.state
                                        .seat_runtime_mut(*seat)
                                        .expect("authority seat must have runtime")
                                        .keyboard = Some(keyboard);
                                }
                            }
                            self.state
                                .queue_full_interaction_domain_damage(*interaction_domain);
                        }
                        aegis_model::interaction_domain::InteractionDomainState::Paused => {
                            for seat in &seats {
                                self.quiesce_seat(*seat);
                            }
                        }
                        aegis_model::interaction_domain::InteractionDomainState::Revoked => {
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
                    InteractionDomainMutation::TransferWindow { .. },
                    InteractionDomainMutationResult::Transferred { .. }
                ) | (
                    InteractionDomainMutation::SetObserver { .. },
                    InteractionDomainMutationResult::ObserverChanged { .. },
                ) | (
                    InteractionDomainMutation::ConfigureOutput { .. },
                    InteractionDomainMutationResult::OutputConfigured { .. },
                ) | (
                    InteractionDomainMutation::SetState { .. },
                    InteractionDomainMutationResult::StateChanged { .. }
                )
            ));
        }
        for (group, (windows, before)) in output_membership_before {
            let after = self.interaction_group_output_interaction_domains(group);
            unsafe {
                update_windows_output_membership(self.state.as_ref(), &windows, &before, &after);
            }
        }
        debug_assert!(self.state.authority.validate().is_ok());
        Ok(receipt)
    }

    pub(crate) fn refresh_foreign_toplevel_visibility(
        &mut self,
        windows: &[aegis_model::window::WindowId],
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

    pub(crate) fn interaction_domain_seat_ids(
        &self,
        interaction_domain: InteractionDomainId,
    ) -> Result<Vec<SeatId>, InteractionDomainError> {
        if self
            .state
            .authority
            .interaction_domain(interaction_domain)
            .is_none()
        {
            return Err(InteractionDomainError::UnknownInteractionDomain(
                interaction_domain,
            ));
        }
        Ok(self
            .state
            .authority
            .snapshot()
            .seats
            .into_iter()
            .filter(|seat| seat.interaction_domain == interaction_domain)
            .map(|seat| seat.id)
            .collect())
    }

    pub(crate) fn clear_transferred_focus(
        &mut self,
        source_interaction_domain: InteractionDomainId,
        windows: &[aegis_model::window::WindowId],
    ) {
        let seats = self
            .interaction_domain_seat_ids(source_interaction_domain)
            .unwrap_or_default();
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
        windows: &[aegis_model::window::WindowId],
    ) -> bool {
        if resource.is_null() {
            return false;
        }
        let rec = unsafe { ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec };
        let root = unsafe { surface_root_toplevel(rec) };
        !root.is_null() && windows.contains(unsafe { &(*root).window.id })
    }

    pub(crate) fn layout_virtual_interaction_domain(
        &mut self,
        interaction_domain: InteractionDomainId,
    ) -> Result<(), InteractionDomainRuntimeError> {
        let Some(output) = self.interaction_domain_output(interaction_domain) else {
            if self
                .state
                .authority
                .interaction_domain(interaction_domain)
                .is_some()
            {
                return Ok(());
            }
            return Err(
                InteractionDomainError::UnknownInteractionDomain(interaction_domain).into(),
            );
        };
        let mut windows = self
            .state
            .authority
            .snapshot()
            .interaction_groups
            .into_iter()
            .filter(|group| {
                group.control_interaction_domain == interaction_domain
                    || group
                        .observer_interaction_domains
                        .contains(&interaction_domain)
            })
            .flat_map(|group| group.windows)
            .collect::<Vec<_>>();
        windows.sort_unstable();
        windows.dedup();
        self.state.interaction_domain_placements.retain(
            |(placement_interaction_domain, window), _| {
                *placement_interaction_domain != interaction_domain || windows.contains(window)
            },
        );
        let area = aegis_model::Rect::new(0, 0, output.width as i32, output.height as i32);
        let slots = aegis_model::overview::grid(area, windows.len());
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
            self.state.interaction_domain_placements.insert(
                (interaction_domain, window),
                aegis_model::overview::fit(slot, size),
            );
        }
        // Placements may have shifted for every window, so a full-output
        // invalidation is the only conservative damage until the new layout is
        // presented. Surface commits use window-local damage thereafter.
        self.state
            .queue_full_interaction_domain_damage(interaction_domain);
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
        runtime.depressed_mods = aegis_model::input::Mods::NONE;
        runtime.client_pressed_keys.clear();
        runtime.keyboard = None;
        runtime.cursor_surface = std::ptr::null_mut();
        runtime.cursor_hidden = false;
        runtime.cursor_shape = 1;
    }

    /// Process pending client events and flush queued events. Non-blocking.
    pub fn dispatch(&mut self) {
        self.accept_interaction_domain_portal_clients();
        unsafe {
            let loop_ = ffi::wl_display_get_event_loop(self.state.display);
            ffi::wl_event_loop_dispatch(loop_, 0);
            ffi::wl_display_flush_clients(self.state.display);
        }
        let pending_layouts = std::mem::take(&mut self.state.pending_interaction_domain_layouts);
        for interaction_domain in pending_layouts {
            if let Err(error) = self.layout_virtual_interaction_domain(interaction_domain) {
                log::warn!(
                    "could not update InteractionDomain {} layout: {error}",
                    interaction_domain.0
                );
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
        let pending_keyboard_focus = std::mem::take(&mut self.state.pending_keyboard_focus);
        for (seat, surface) in pending_keyboard_focus {
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
    /// A surface commit invalidates the complete Interaction Domain-local placement of its
    /// root toplevel. This is deliberately conservative: transform, viewport,
    /// subsurface and layout changes cannot under-report pixels, while agents
    /// can still avoid polling or recapturing unrelated outputs. Topology
    /// changes enqueue full-output damage. The queue is bounded to at most 64
    /// rectangles per Interaction Domain, collapsing excess entries to one bounding box.
    pub fn take_interaction_domain_damage(
        &mut self,
    ) -> std::collections::BTreeMap<InteractionDomainId, Vec<aegis_model::Rect>> {
        let changed_windows = std::mem::take(&mut self.state.damaged_windows);
        let mut damage = std::mem::take(&mut self.state.pending_interaction_domain_damage);

        if self.state.session_lock_phase.is_active() {
            return std::collections::BTreeMap::new();
        }

        for window in changed_windows {
            for ((interaction_domain, placement_window), placement) in
                &self.state.interaction_domain_placements
            {
                if *placement_window != window
                    || !self
                        .state
                        .authority
                        .interaction_domain_observes_window(*interaction_domain, window)
                {
                    continue;
                }
                let Some(record) = self.state.authority.interaction_domain(*interaction_domain)
                else {
                    continue;
                };
                if record.state != aegis_model::interaction_domain::InteractionDomainState::Active
                    || !matches!(record.presentation, PresentationTarget::Virtual { .. })
                {
                    continue;
                }
                damage
                    .entry(*interaction_domain)
                    .or_default()
                    .push(*placement);
            }
        }

        damage.retain(|interaction_domain, rects| {
            let Some(record) = self.state.authority.interaction_domain(*interaction_domain) else {
                return false;
            };
            let PresentationTarget::Virtual { output } = record.presentation else {
                return false;
            };
            if record.state != aegis_model::interaction_domain::InteractionDomainState::Active {
                return false;
            }
            normalize_interaction_domain_damage(
                rects,
                aegis_model::Rect::new(0, 0, output.width as i32, output.height as i32),
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
