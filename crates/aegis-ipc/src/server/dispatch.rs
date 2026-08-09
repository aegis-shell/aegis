use super::*;

/// Drive the protocol on the read half. Returns `(coarse_sub_id,
/// journal_sub_id, idle_inhibited)` for cleanup; any may be absent/false.
#[allow(clippy::too_many_arguments)]
pub(super) fn drive_read_loop<H: Handler>(
    read: &mut UnixStream,
    tx: &SyncSender<Outbound>,
    handler: &H,
    subs: &Mutex<HashMap<SubId, SubscriptionLane>>,
    journal_subs: &Mutex<HashMap<SubId, SubscriptionLane>>,
    streams: &Mutex<HashMap<u64, StreamLane>>,
    next_sub: &AtomicU64,
    next_lease: &AtomicU64,
    shutdown: &Arc<UnixStream>,
    conn_id: u64,
) -> (Option<SubId>, Option<SubId>, bool) {
    const MIN_LEASE_MS: u64 = 1_000;
    const MAX_LEASE_MS: u64 = 86_400_000;
    let (granted, granted_scope, scope_name, mut active_lease, principal, agent_reply, version) =
        match read_msg::<_, Request>(read) {
            Ok(Request::Hello {
                version,
                caps,
                scope,
                lease,
                agent,
            }) => {
                if version > PROTOCOL_VERSION {
                    let _ = tx.send(Outbound::Response(Response::Error {
                    message: format!(
                        "unsupported protocol version {version} (server supports {PROTOCOL_VERSION})"
                    ),
                }));
                    return (None, None, false);
                }
                if scope
                    .as_deref()
                    .is_some_and(|name| !bounded_identifier_text(name, 256))
                    || agent
                        .as_ref()
                        .is_some_and(|agent| !valid_agent_hello(agent))
                {
                    let _ = tx.send(Outbound::Response(Response::Error {
                        message: "invalid scope name or Agent declaration".into(),
                    }));
                    return (None, None, false);
                }
                // Resolve a declared scope first: an explicitly named but
                // unknown scope is refused before any pairing happens.
                let declared = match scope.as_deref() {
                    Some(name) => match handler.resolve_scope(name) {
                        Some(scope) => Some(scope),
                        None => {
                            let _ = tx.send(Outbound::Response(Response::Error {
                                message: format!("unknown scope '{name}'"),
                            }));
                            return (None, None, false);
                        }
                    },
                    None => None,
                };
                // Pairing (ADR-0088): an agent self-declaration is bound to a
                // principal before any request runs. Built-in scopes are
                // platform components and never pair.
                let builtin = matches!(
                    scope.as_deref(),
                    Some(
                        LOCAL_AGENT_ADMIN_SCOPE
                            | LOCAL_OWNER_ADMIN_SCOPE
                            | LOCAL_INTERACTION_DOMAIN_ADMIN_SCOPE
                            | LOCAL_PORTAL_SCOPE
                    )
                );
                let mut principal = None;
                let mut agent_reply = None;
                let mut registry_ceiling = None;
                if let Some(agent_hello) = agent
                    && !builtin
                {
                    let identity = agent_hello
                        .credential
                        .as_deref()
                        .and_then(|credential| handler.agent_lookup(credential));
                    match identity {
                        Some(identity) => {
                            principal = Some(identity.principal.clone());
                            registry_ceiling = Some((identity.pregranted, identity.gated));
                            agent_reply = Some(AgentIssued {
                                principal: identity.principal.to_string(),
                                credential: None,
                            });
                        }
                        None => {
                            match handler.pair_agent(
                                conn_id,
                                agent_hello.label.as_deref(),
                                &agent_hello.requested,
                            ) {
                                Ok(mut paired) => {
                                    agent_reply = Some(AgentIssued {
                                        principal: paired.principal.to_string(),
                                        credential: Some(std::mem::take(&mut paired.credential)),
                                    });
                                    principal = Some(paired.principal.clone());
                                    registry_ceiling =
                                        Some((paired.pregranted.clone(), paired.gated.clone()));
                                }
                                Err(message) => {
                                    let _ =
                                        tx.send(Outbound::Response(Response::Error { message }));
                                    return (None, None, false);
                                }
                            }
                        }
                    }
                }
                // A declared scope is the ceiling when present; a paired
                // self-declared agent gets a synthetic scope from its approved
                // ceiling; anything else is the anonymous compatibility scope.
                let gs = declared.unwrap_or_else(|| match registry_ceiling {
                    Some((pregranted, gated)) => Scope {
                        ops: Some(pregranted),
                        ask_ops: Some(gated),
                        ..Scope::default()
                    },
                    None => Scope::unscoped(),
                });
                let mut gc = handler.policy_caps().intersect(caps).with_query_always();
                // Synthetic input is intentionally unavailable to unscoped
                // compatibility clients. A caller must name a compositor-owned
                // scope so every injected action has a revocable resource and
                // operation bound (ADR-0035). Paired agents are treated like
                // scoped callers here: their approved ceiling, not the
                // capability class, decides what input they may inject.
                let anonymous = principal.is_none();
                if scope.is_none() && anonymous {
                    gc.input = false;
                }
                // Lockdown strips privileges from connections that neither
                // present a built-in scope nor pair; platform components are
                // exempt (ADR-0088).
                if lease.is_none() || (anonymous && !builtin && handler.lockdown()) {
                    gc.control = false;
                    gc.input = false;
                    gc.session = false;
                    gc.interaction_domain = false;
                }
                let active_lease = if gc.privileged() {
                    let requested = lease.expect("privileged capabilities require a lease");
                    if !(MIN_LEASE_MS..=MAX_LEASE_MS).contains(&requested.ttl_ms) {
                        let _ = tx.send(Outbound::Response(Response::Error {
                            message: format!(
                                "lease ttl must be between {MIN_LEASE_MS} and {MAX_LEASE_MS} ms"
                            ),
                        }));
                        return (None, None, false);
                    }
                    let grant = LeaseGrant {
                        id: next_lease.fetch_add(1, Ordering::Relaxed),
                        ttl_ms: requested.ttl_ms,
                        renewable: true,
                    };
                    let deadline = std::time::Instant::now()
                        .checked_add(std::time::Duration::from_millis(grant.ttl_ms))
                        .expect("bounded lease duration overflowed");
                    Some((grant, deadline))
                } else {
                    None
                };
                (gc, gs, scope, active_lease, principal, agent_reply, version)
            }
            Ok(_) => {
                let _ = tx.send(Outbound::Response(Response::Error {
                    message: "expected Hello first".into(),
                }));
                return (None, None, false);
            }
            Err(_) => return (None, None, false),
        };
    let session_ttl = active_lease
        .as_ref()
        .map(|(grant, _)| std::time::Duration::from_millis(grant.ttl_ms))
        .unwrap_or_else(|| std::time::Duration::from_secs(15 * 60));
    let session_policy = aegis_security::authority::ActorSessionPolicy {
        ttl: session_ttl,
        idle_timeout: session_ttl.min(std::time::Duration::from_secs(5 * 60)),
        ..aegis_security::authority::ActorSessionPolicy::default()
    };
    let session = match handler.start_actor_session(conn_id, principal.as_deref(), session_policy) {
        Ok(session) => session,
        Err(message) => {
            let _ = tx.send(Outbound::Response(Response::Error { message }));
            return (None, None, false);
        }
    };
    if read
        .set_read_timeout(Some(std::time::Duration::from_millis(
            session.idle_timeout_ms,
        )))
        .is_err()
    {
        return (None, None, false);
    }
    // Older clients are answered at their own version; version-gated
    // behavior (such as dmabuf streams) keys off the negotiated version.
    if tx
        .send(Outbound::Response(Response::Hello {
            version: version.min(PROTOCOL_VERSION),
            caps: granted,
            scope: granted_scope.clone(),
            lease: active_lease.as_ref().map(|(grant, _)| *grant),
            session: Some(session.clone()),
            agent: agent_reply,
        }))
        .is_err()
    {
        return (None, None, false);
    }

    // Streams outlive individual requests, so their delivery-time lease
    // check reads a deadline shared with lease renewals (ADR-0052).
    let lease_deadline_shared = Arc::new(Mutex::new(
        active_lease
            .as_ref()
            .map(|(_, deadline)| *deadline)
            .unwrap_or_else(std::time::Instant::now),
    ));
    let live_scope = LiveScopeBinding {
        connection_id: conn_id,
        session: session.id,
        name: scope_name.clone(),
        principal: principal.clone(),
        fallback: granted_scope.clone(),
    };

    let mut sub_id: Option<SubId> = None;
    let mut journal_sub_id: Option<SubId> = None;
    // Whether this connection currently holds a global idle inhibitor;
    // released through the handler on disconnect (ADR-0075).
    let mut idle_inhibited = false;
    while let Ok(req) = read_msg::<_, Request>(read) {
        if let Err(message) = handler.authorize_actor_session(session.id) {
            let _ = tx.send(Outbound::Response(Response::Error { message }));
            break;
        }
        let lease_alive = active_lease
            .as_ref()
            .is_some_and(|(_, deadline)| std::time::Instant::now() < *deadline);
        let agent_admin = scope_name.as_deref() == Some(LOCAL_AGENT_ADMIN_SCOPE)
            && handler.resolve_scope(LOCAL_AGENT_ADMIN_SCOPE).is_some();
        // Set by the StreamOutputStart arm when a zero-copy stream began:
        // the reply then carries the slot table's descriptors on the blob
        // channel right after the JSON (protocol 25).
        let mut stream_start_table: Option<StreamSlotTable> = None;
        let resp = match req {
            Request::Hello { .. } => Response::Error {
                message: "Hello already exchanged".into(),
            },
            Request::RequestResourceGrant {
                resource,
                ttl_ms,
                uses,
            } => {
                let audit_resource = resource.clone();
                let current_scope = live_scope.resolve(handler);
                let capability = resource.required_capability();
                let decision =
                    current_scope
                        .as_ref()
                        .map_or(AuthorizationDecision::Deny, |scope| {
                            if scope.pregrants(capability) {
                                AuthorizationDecision::Permit
                            } else if scope.asks(capability) {
                                AuthorizationDecision::Ask(capability)
                            } else {
                                AuthorizationDecision::Deny
                            }
                        });
                let response = if !granted.control {
                    Response::Error {
                        message: "resource grants require the control capability".into(),
                    }
                } else if !lease_alive {
                    Response::Error {
                        message: "privileged capability lease expired".into(),
                    }
                } else if let Err(message) = resource.validate().map_err(str::to_owned) {
                    Response::Error { message }
                } else if decision == AuthorizationDecision::Deny {
                    Response::Error {
                        message: "resource capability is out of scope".into(),
                    }
                } else {
                    let confirm_exact_resource =
                        matches!(
                            resource,
                            aegis_security::authority::ActorResource::PaymentRequest { .. }
                        ) || matches!(decision, AuthorizationDecision::Ask(_));
                    match handler.issue_resource_grant(
                        session.id,
                        principal.as_deref(),
                        resource,
                        std::time::Duration::from_millis(ttl_ms),
                        uses,
                        confirm_exact_resource,
                    ) {
                        Ok(grant) => Response::ResourceGranted { grant },
                        Err(message) => Response::Error { message },
                    }
                };
                if matches!(&response, Response::Error { .. }) {
                    audit_resource_grant_refusal(
                        handler,
                        conn_id,
                        principal.as_deref(),
                        session.id,
                        crate::journal::ResourceGrantAttemptAction::Issue,
                        Some(&audit_resource),
                    );
                }
                response
            }
            Request::ConsumeResourceGrant { id, resource } => {
                let capability = resource.required_capability();
                let scope_still_allows = live_scope
                    .resolve(handler)
                    .is_some_and(|scope| scope.pregrants(capability) || scope.asks(capability));
                let response = if !granted.control {
                    Response::Error {
                        message: "resource grant consumption requires the control capability"
                            .into(),
                    }
                } else if !lease_alive {
                    Response::Error {
                        message: "privileged capability lease expired".into(),
                    }
                } else if let Err(message) = resource.validate().map_err(str::to_owned) {
                    Response::Error { message }
                } else if !scope_still_allows {
                    Response::Error {
                        message: "resource capability is out of scope".into(),
                    }
                } else {
                    match handler.consume_resource_grant(
                        session.id,
                        principal.as_deref(),
                        &id,
                        &resource,
                    ) {
                        Ok(grant) => Response::ResourceGrantConsumed { grant },
                        Err(message) => Response::Error { message },
                    }
                };
                if matches!(&response, Response::Error { .. }) {
                    audit_resource_grant_refusal(
                        handler,
                        conn_id,
                        principal.as_deref(),
                        session.id,
                        crate::journal::ResourceGrantAttemptAction::Consume,
                        Some(&resource),
                    );
                }
                response
            }
            Request::RevokeResourceGrant { id } => {
                let response =
                    match handler.revoke_resource_grant(session.id, principal.as_deref(), &id) {
                        Ok(()) => Response::ResourceGrantRevoked {},
                        Err(message) => Response::Error { message },
                    };
                if matches!(&response, Response::Error { .. }) {
                    audit_resource_grant_refusal(
                        handler,
                        conn_id,
                        principal.as_deref(),
                        session.id,
                        crate::journal::ResourceGrantAttemptAction::Revoke,
                        None,
                    );
                }
                response
            }
            Request::GetAccessibilityWindows => {
                let current_scope = effective_scope(
                    handler,
                    scope_name.as_deref(),
                    &granted_scope,
                    principal.as_deref(),
                );
                let authorized = granted.query
                    && granted.control
                    && lease_alive
                    && principal.is_some()
                    && current_scope.as_ref().is_some_and(|scope| {
                        scope.pregrants(ActorCapability::ObserveWindows)
                            && scope.pregrants(ActorCapability::PublishAccessibilityTree)
                    });
                let response = if authorized {
                    Response::AccessibilityWindows {
                        windows: filter_accessibility_windows(
                            handler.accessibility_windows(),
                            current_scope.as_ref().expect("scope checked"),
                        ),
                    }
                } else {
                    Response::Error {
                        message: "accessibility process bindings are out of scope".into(),
                    }
                };
                audit_capability_response(
                    handler,
                    &live_scope,
                    ActorCapability::PublishAccessibilityTree,
                    crate::journal::CapabilityUseAction::Observe,
                    &response,
                );
                response
            }
            Request::PublishAccessibilityTree { update } => {
                let current_scope = effective_scope(
                    handler,
                    scope_name.as_deref(),
                    &granted_scope,
                    principal.as_deref(),
                );
                let authorized = granted.control
                    && lease_alive
                    && current_scope.as_ref().is_some_and(|scope| {
                        scope.pregrants(ActorCapability::PublishAccessibilityTree)
                    });
                let response = match (authorized, principal.as_deref()) {
                    (false, _) => Response::Error {
                        message: "publishing accessibility trees is out of scope".into(),
                    },
                    (true, None) => Response::Error {
                        message: "accessibility providers require an authenticated principal"
                            .into(),
                    },
                    (true, Some(principal)) => {
                        match handler.publish_accessibility_tree(principal, update) {
                            Ok(()) => Response::AccessibilityTreePublished {},
                            Err(message) => Response::Error { message },
                        }
                    }
                };
                audit_capability_response(
                    handler,
                    &live_scope,
                    ActorCapability::PublishAccessibilityTree,
                    crate::journal::CapabilityUseAction::Publish,
                    &response,
                );
                response
            }
            Request::NextAccessibilityAction { timeout_ms } => {
                let current_scope = effective_scope(
                    handler,
                    scope_name.as_deref(),
                    &granted_scope,
                    principal.as_deref(),
                );
                let authorized = granted.control
                    && lease_alive
                    && current_scope.as_ref().is_some_and(|scope| {
                        scope.pregrants(ActorCapability::DispatchAccessibilityAction)
                    });
                let response = match (authorized, principal.as_deref()) {
                    (false, _) => Response::Error {
                        message: "accessibility action dispatch is out of scope".into(),
                    },
                    (true, None) => Response::Error {
                        message: "accessibility providers require an authenticated principal"
                            .into(),
                    },
                    (true, Some(principal)) => match handler.next_accessibility_action(
                        session.id,
                        principal,
                        std::time::Duration::from_millis(timeout_ms.clamp(1, 30_000)),
                    ) {
                        Ok(request) => Response::AccessibilityAction { request },
                        Err(message) => Response::Error { message },
                    },
                };
                audit_capability_response(
                    handler,
                    &live_scope,
                    ActorCapability::DispatchAccessibilityAction,
                    crate::journal::CapabilityUseAction::Await,
                    &response,
                );
                response
            }
            Request::CompleteAccessibilityAction {
                request_id,
                success,
                message,
            } => {
                let message_valid = message
                    .as_ref()
                    .is_none_or(|message| message.len() <= 1_024 && !message.contains('\0'));
                let current_scope = effective_scope(
                    handler,
                    scope_name.as_deref(),
                    &granted_scope,
                    principal.as_deref(),
                );
                let authorized = granted.control
                    && lease_alive
                    && message_valid
                    && current_scope.as_ref().is_some_and(|scope| {
                        scope.pregrants(ActorCapability::DispatchAccessibilityAction)
                    });
                let response = match (authorized, principal.as_deref()) {
                    (false, _) => Response::Error {
                        message: "accessibility action completion is invalid or out of scope"
                            .into(),
                    },
                    (true, None) => Response::Error {
                        message: "accessibility providers require an authenticated principal"
                            .into(),
                    },
                    (true, Some(principal)) => {
                        let result = if success {
                            Ok(())
                        } else {
                            Err(message.unwrap_or_else(|| {
                                "accessibility adapter refused the action".into()
                            }))
                        };
                        match handler.complete_accessibility_action(
                            session.id, principal, request_id, result,
                        ) {
                            Ok(()) => Response::AccessibilityActionCompleted {},
                            Err(message) => Response::Error { message },
                        }
                    }
                };
                audit_capability_response(
                    handler,
                    &live_scope,
                    ActorCapability::DispatchAccessibilityAction,
                    crate::journal::CapabilityUseAction::Complete,
                    &response,
                );
                response
            }
            Request::GetWindows => {
                let scope = effective_scope(
                    handler,
                    scope_name.as_deref(),
                    &granted_scope,
                    principal.as_deref(),
                );
                if granted.query
                    && scope.as_ref().is_some_and(|scope| {
                        scope.permits_actor_observation(
                            principal.is_some(),
                            ActorCapability::ObserveWindows,
                        )
                    })
                {
                    Response::Windows {
                        windows: filter_windows(
                            handler.windows(),
                            scope.as_ref().expect("scope checked"),
                        ),
                    }
                } else {
                    Response::Error {
                        message: "GetWindows requires query and a live scope".into(),
                    }
                }
            }
            Request::GetWorkspaces => {
                let scope = effective_scope(
                    handler,
                    scope_name.as_deref(),
                    &granted_scope,
                    principal.as_deref(),
                );
                if granted.query
                    && scope.as_ref().is_some_and(|scope| {
                        scope.permits_actor_observation(
                            principal.is_some(),
                            ActorCapability::ObserveWorkspaces,
                        )
                    })
                {
                    Response::Workspaces {
                        snapshot: filter_workspaces(
                            handler.workspaces(),
                            scope.as_ref().expect("scope checked"),
                        ),
                    }
                } else {
                    Response::Error {
                        message: "GetWorkspaces requires query and a live scope".into(),
                    }
                }
            }
            Request::GetNotifications => {
                let scope = effective_scope(
                    handler,
                    scope_name.as_deref(),
                    &granted_scope,
                    principal.as_deref(),
                );
                if granted.query
                    && scope.as_ref().is_some_and(|scope| {
                        scope.permits_actor_observation(
                            principal.is_some(),
                            ActorCapability::ObserveNotifications,
                        )
                    })
                {
                    Response::Notifications {
                        notifications: handler.notifications(),
                    }
                } else {
                    Response::Error {
                        message: "GetNotifications requires the query capability".into(),
                    }
                }
            }
            Request::GetOutputs => {
                let scope = effective_scope(
                    handler,
                    scope_name.as_deref(),
                    &granted_scope,
                    principal.as_deref(),
                );
                if granted.query
                    && scope.as_ref().is_some_and(|scope| {
                        scope.permits_actor_observation(
                            principal.is_some(),
                            ActorCapability::ObserveOutputs,
                        )
                    })
                {
                    let scope = scope.as_ref().expect("scope checked");
                    let mut outputs = handler.outputs();
                    if scope.outputs.is_some() {
                        let allowed_connectors = handler
                            .workspaces()
                            .outputs
                            .into_iter()
                            .filter(|output| scope.permits_output(output.id))
                            .map(|output| output.connector)
                            .collect::<std::collections::HashSet<_>>();
                        outputs.retain(|output| allowed_connectors.contains(&output.connector));
                    }
                    Response::Outputs { outputs }
                } else {
                    Response::Error {
                        message: "GetOutputs requires the query capability".into(),
                    }
                }
            }
            Request::GetJournal { since } => {
                let scope = effective_scope(
                    handler,
                    scope_name.as_deref(),
                    &granted_scope,
                    principal.as_deref(),
                );
                if granted.query
                    && scope.as_ref().is_some_and(|scope| {
                        scope.permits_actor_observation(
                            principal.is_some(),
                            ActorCapability::ObserveJournal,
                        )
                    })
                {
                    Response::Journal {
                        snapshot: filter_journal(
                            handler.journal_since(since),
                            scope.as_ref().expect("scope checked"),
                            principal.as_deref(),
                        ),
                    }
                } else {
                    Response::Error {
                        message: "GetJournal requires the query capability".into(),
                    }
                }
            }
            Request::GetInteractionDomains => {
                let scope = effective_scope(
                    handler,
                    scope_name.as_deref(),
                    &granted_scope,
                    principal.as_deref(),
                );
                if granted.query
                    && scope.as_ref().is_some_and(|scope| {
                        scope.permits_actor_observation(
                            principal.is_some(),
                            ActorCapability::ObserveInteractionDomains,
                        )
                    })
                {
                    Response::InteractionDomains {
                        snapshot: filter_interaction_domains(
                            handler.interaction_domains(),
                            scope.as_ref().expect("scope checked"),
                            principal.as_deref(),
                        ),
                    }
                } else {
                    Response::Error {
                        message: "GetInteractionDomains requires the query capability".into(),
                    }
                }
            }
            Request::GetAgentPrincipals => {
                if granted.query && agent_admin {
                    Response::AgentPrincipals {
                        principals: handler.agent_principals(),
                    }
                } else {
                    Response::Error {
                        message: "GetAgentPrincipals requires the agent-admin scope".into(),
                    }
                }
            }
            Request::GetAgentGrants { principal: filter } => {
                if filter
                    .as_deref()
                    .is_some_and(|principal| !valid_principal_id(principal))
                {
                    Response::Error {
                        message: "invalid Actor principal".into(),
                    }
                } else if granted.query && agent_admin {
                    Response::AgentGrants {
                        grants: handler.agent_grants(filter.as_deref()),
                    }
                } else {
                    Response::Error {
                        message: "GetAgentGrants requires the agent-admin scope".into(),
                    }
                }
            }
            Request::RenameAgentPrincipal {
                principal: id,
                label,
            } => {
                if !agent_admin {
                    Response::Error {
                        message: "RenameAgentPrincipal requires the agent-admin scope".into(),
                    }
                } else if !granted.control {
                    Response::Error {
                        message: "RenameAgentPrincipal requires the control capability".into(),
                    }
                } else if !lease_alive {
                    Response::Error {
                        message: "privileged capability lease expired".into(),
                    }
                } else if !valid_principal_id(&id)
                    || label
                        .as_deref()
                        .is_some_and(|label| !bounded_identifier_text(label, 256))
                {
                    Response::Error {
                        message: "invalid Actor principal or display label".into(),
                    }
                } else {
                    match handler.rename_agent_principal(&id, label.as_deref()) {
                        Ok(()) => Response::Ok,
                        Err(message) => Response::Error { message },
                    }
                }
            }
            Request::ForgetAgentPrincipal { principal: id } => {
                if !agent_admin {
                    Response::Error {
                        message: "ForgetAgentPrincipal requires the agent-admin scope".into(),
                    }
                } else if !granted.control {
                    Response::Error {
                        message: "ForgetAgentPrincipal requires the control capability".into(),
                    }
                } else if !lease_alive {
                    Response::Error {
                        message: "privileged capability lease expired".into(),
                    }
                } else if !valid_principal_id(&id) {
                    Response::Error {
                        message: "invalid Actor principal".into(),
                    }
                } else {
                    match handler.forget_agent_principal(&id) {
                        Ok(()) => Response::Ok,
                        Err(message) => Response::Error { message },
                    }
                }
            }
            Request::SetAgentCeiling {
                principal: id,
                pregranted,
                gated,
            } => {
                if !agent_admin {
                    Response::Error {
                        message: "SetAgentCeiling requires the agent-admin scope".into(),
                    }
                } else if !granted.control {
                    Response::Error {
                        message: "SetAgentCeiling requires the control capability".into(),
                    }
                } else if !lease_alive {
                    Response::Error {
                        message: "privileged capability lease expired".into(),
                    }
                } else if !valid_principal_id(&id)
                    || !valid_capability_list(&pregranted)
                    || !valid_capability_list(&gated)
                {
                    Response::Error {
                        message: "invalid Actor principal or capability ceiling".into(),
                    }
                } else {
                    match handler.set_agent_ceiling(&id, &pregranted, &gated) {
                        Ok(()) => Response::Ok,
                        Err(message) => Response::Error { message },
                    }
                }
            }
            Request::RegisterAgent {
                label,
                pregranted,
                gated,
            } => {
                if !agent_admin {
                    Response::Error {
                        message: "RegisterAgent requires the agent-admin scope".into(),
                    }
                } else if !granted.control {
                    Response::Error {
                        message: "RegisterAgent requires the control capability".into(),
                    }
                } else if !lease_alive {
                    Response::Error {
                        message: "privileged capability lease expired".into(),
                    }
                } else if label
                    .as_deref()
                    .is_some_and(|label| !bounded_identifier_text(label, 256))
                    || !valid_capability_list(&pregranted)
                    || !valid_capability_list(&gated)
                {
                    Response::Error {
                        message: "invalid Agent label or capability ceiling".into(),
                    }
                } else {
                    match handler.register_agent(label.as_deref(), &pregranted, &gated) {
                        Ok((principal, credential)) => Response::AgentRegistered {
                            principal,
                            credential,
                        },
                        Err(message) => Response::Error { message },
                    }
                }
            }
            Request::RevokeAgentGrant { principal: id, op } => {
                if !agent_admin {
                    Response::Error {
                        message: "RevokeAgentGrant requires the agent-admin scope".into(),
                    }
                } else if !granted.control {
                    Response::Error {
                        message: "RevokeAgentGrant requires the control capability".into(),
                    }
                } else if !lease_alive {
                    Response::Error {
                        message: "privileged capability lease expired".into(),
                    }
                } else if !valid_principal_id(&id) {
                    Response::Error {
                        message: "invalid Actor principal".into(),
                    }
                } else {
                    match handler.revoke_agent_grant(&id, op) {
                        Ok(()) => Response::Ok,
                        Err(message) => Response::Error { message },
                    }
                }
            }
            Request::GetSettings => {
                let scope = effective_scope(
                    handler,
                    scope_name.as_deref(),
                    &granted_scope,
                    principal.as_deref(),
                );
                if granted.query
                    && scope.as_ref().is_some_and(|scope| {
                        scope.permits_actor_observation(
                            principal.is_some(),
                            ActorCapability::ObserveSettings,
                        )
                    })
                {
                    Response::Settings {
                        snapshot: handler.settings(),
                    }
                } else {
                    Response::Error {
                        message: "GetSettings requires the query capability".into(),
                    }
                }
            }
            Request::GetSystemStatus => {
                let scope = effective_scope(
                    handler,
                    scope_name.as_deref(),
                    &granted_scope,
                    principal.as_deref(),
                );
                if granted.query
                    && scope.as_ref().is_some_and(|scope| {
                        scope.permits_actor_observation(
                            principal.is_some(),
                            ActorCapability::ObserveSystem,
                        )
                    })
                {
                    Response::SystemStatus {
                        snapshot: handler.system_status(),
                    }
                } else {
                    Response::Error {
                        message: "GetSystemStatus requires the query capability".into(),
                    }
                }
            }
            Request::Settings {
                expected_revision,
                action,
            } => {
                let before_revision = handler.settings().revision;
                let rejection = if !granted.session {
                    Some("Settings requires the session capability".to_owned())
                } else if !lease_alive {
                    Some("privileged capability lease expired".to_owned())
                } else {
                    action.validate().err().map(str::to_owned)
                };
                if let Some(message) = rejection {
                    handler.audit_refusal(
                        conn_id,
                        principal.as_deref(),
                        JournalMutation::Settings {
                            action,
                            before_revision,
                            after_revision: before_revision,
                        },
                        message.clone(),
                    );
                    Response::Error { message }
                } else {
                    match handler.settings_action(
                        conn_id,
                        principal.as_deref(),
                        expected_revision,
                        action,
                    ) {
                        Ok(receipt) => Response::SettingsApplied { receipt },
                        Err(message) => Response::Error { message },
                    }
                }
            }
            Request::InteractionDomain { action } => {
                let current_scope = effective_scope(
                    handler,
                    scope_name.as_deref(),
                    &granted_scope,
                    principal.as_deref(),
                );
                let mut rejection = if !granted.interaction_domain {
                    Some("InteractionDomain requires the interaction_domain capability".to_owned())
                } else if !lease_alive {
                    Some("privileged capability lease expired".to_owned())
                } else {
                    match current_scope.as_ref() {
                        None => Some("out of scope: named scope was revoked".into()),
                        Some(scope) => match scope.decide_interaction_domain_action(&action) {
                            AuthorizationDecision::Permit => {
                                match handler.authorize_interaction_domain_action(scope, &action) {
                                    Err(message) => Some(message),
                                    Ok(()) => action.validate().err().map(str::to_owned),
                                }
                            }
                            AuthorizationDecision::Deny => Some("out of scope".into()),
                            AuthorizationDecision::Ask(op) => {
                                match grant_authorize(handler, conn_id, principal.as_deref(), op) {
                                    Ok(true) => {
                                        match handler.authorize_interaction_domain_action_granted(
                                            scope, &action,
                                        ) {
                                            Err(message) => Some(message),
                                            Ok(()) => action.validate().err().map(str::to_owned),
                                        }
                                    }
                                    Ok(false) => {
                                        Some("out of scope: the user denied this operation".into())
                                    }
                                    Err(message) => Some(message),
                                }
                            }
                        },
                    }
                };
                if rejection.is_none() {
                    rejection = handler
                        .authorize_agent_interaction_domain_action(principal.as_deref(), &action)
                        .err();
                }
                if let Some(message) = rejection {
                    let revision = handler.interaction_domains().revision;
                    handler.audit_refusal(
                        conn_id,
                        principal.as_deref(),
                        JournalMutation::InteractionDomain {
                            action,
                            before_revision: revision,
                            after_revision: revision,
                        },
                        message.clone(),
                    );
                    Response::Error { message }
                } else {
                    match handler.interaction_domain_action(conn_id, principal.as_deref(), action) {
                        Ok(result) => Response::InteractionDomain { result },
                        Err(message) => Response::Error { message },
                    }
                }
            }
            Request::Do { cmd } => {
                let need = cmd.required_cap();
                let allowed = (need.control && granted.control)
                    || (need.input && granted.input)
                    || (need.session && granted.session)
                    || (need.interaction_domain && granted.interaction_domain);
                let current_scope = effective_scope(
                    handler,
                    scope_name.as_deref(),
                    &granted_scope,
                    principal.as_deref(),
                );
                let mut rejection = if !allowed {
                    Some("command requires a capability not granted".to_owned())
                } else if !lease_alive {
                    Some("privileged capability lease expired".to_owned())
                } else {
                    match current_scope.as_ref() {
                        None => Some("out of scope: named scope was revoked".into()),
                        Some(scope) => match scope.decide_command(&cmd) {
                            AuthorizationDecision::Permit => {
                                cmd.validate().err().map(str::to_owned)
                            }
                            AuthorizationDecision::Deny => Some("out of scope".to_owned()),
                            AuthorizationDecision::Ask(op) => {
                                match grant_authorize(handler, conn_id, principal.as_deref(), op) {
                                    Ok(true) => {
                                        if scope.permits_resources(&cmd) {
                                            cmd.validate().err().map(str::to_owned)
                                        } else {
                                            Some("out of scope".to_owned())
                                        }
                                    }
                                    Ok(false) => {
                                        Some("out of scope: the user denied this operation".into())
                                    }
                                    Err(message) => Some(message),
                                }
                            }
                        },
                    }
                };
                if rejection.is_none() {
                    rejection = handler
                        .authorize_agent_interaction_domain_command(principal.as_deref(), &cmd)
                        .err();
                }
                if let Some(message) = rejection {
                    handler.audit_refusal(
                        conn_id,
                        principal.as_deref(),
                        JournalMutation::Command {
                            cmd: crate::journal::AuditedCommand::from(&cmd),
                        },
                        message.clone(),
                    );
                    Response::Error { message }
                } else if let Command::System { action } = cmd {
                    match handler.system_action(conn_id, principal.as_deref(), action) {
                        Ok(()) => Response::Ok,
                        Err(message) => Response::Error { message },
                    }
                } else {
                    handler.command(conn_id, principal.as_deref(), cmd);
                    Response::Ok
                }
            }
            Request::Subscribe => {
                if principal.is_some() {
                    Response::Error {
                        message: "Actor subscriptions require filtered lanes; use scoped snapshots"
                            .into(),
                    }
                } else {
                    if sub_id.is_none() {
                        let id = next_sub.fetch_add(1, Ordering::Relaxed);
                        subs.lock().unwrap().insert(
                            id,
                            SubscriptionLane {
                                tx: tx.clone(),
                                shutdown: Some(Arc::clone(shutdown)),
                            },
                        );
                        sub_id = Some(id);
                    }
                    Response::Subscribed
                }
            }
            Request::SubscribeJournal => {
                if principal.is_some() {
                    Response::Error {
                        message:
                            "Actor journal subscriptions require filtered lanes; use GetJournal"
                                .into(),
                    }
                } else {
                    if journal_sub_id.is_none() {
                        let id = next_sub.fetch_add(1, Ordering::Relaxed);
                        journal_subs.lock().unwrap().insert(
                            id,
                            SubscriptionLane {
                                tx: tx.clone(),
                                shutdown: Some(Arc::clone(shutdown)),
                            },
                        );
                        journal_sub_id = Some(id);
                    }
                    Response::Subscribed
                }
            }
            Request::RenewLease { ttl_ms } => {
                if !(MIN_LEASE_MS..=MAX_LEASE_MS).contains(&ttl_ms) {
                    Response::Error {
                        message: format!(
                            "lease ttl must be between {MIN_LEASE_MS} and {MAX_LEASE_MS} ms"
                        ),
                    }
                } else if !lease_alive {
                    Response::Error {
                        message: "lease is absent or already expired".into(),
                    }
                } else {
                    let (grant, deadline) =
                        active_lease.as_mut().expect("lease_alive checked presence");
                    grant.ttl_ms = ttl_ms;
                    *deadline = std::time::Instant::now()
                        .checked_add(std::time::Duration::from_millis(ttl_ms))
                        .expect("bounded lease duration overflowed");
                    *lease_deadline_shared.lock().unwrap() = *deadline;
                    Response::LeaseRenewed { lease: *grant }
                }
            }
            Request::CaptureOutput { region } => {
                // Pixel capture reads the screen back to the client, so it is
                // fail-closed like InjectInput: `control` plus an explicit
                // CaptureOutput op in the granted scope — never inherited
                // through None-means-all (ADR-0034).
                let current_scope = live_scope.resolve(handler);
                let op_allowed = current_scope.as_ref().is_some_and(|scope| {
                    scope.ops.as_ref().is_some_and(|ops| {
                        ops.contains(&crate::schema::ActorCapability::CaptureOutput)
                    })
                });
                let response = if !granted.control {
                    Response::Error {
                        message: "CaptureOutput requires the control capability".into(),
                    }
                } else if !lease_alive {
                    Response::Error {
                        message: "privileged capability lease expired".into(),
                    }
                } else if !op_allowed {
                    Response::Error {
                        message: "out of scope".into(),
                    }
                } else {
                    match handler.capture_output(region) {
                        Ok(payload) => {
                            let scope_still_allows =
                                live_scope.resolve(handler).is_some_and(|scope| {
                                    scope.ops.as_ref().is_some_and(|ops| {
                                        ops.contains(&crate::schema::ActorCapability::CaptureOutput)
                                    })
                                });
                            let lease_deadline = active_lease
                                .as_ref()
                                .map(|(_, deadline)| *deadline)
                                .expect("granted control has an active lease");
                            if !scope_still_allows {
                                Response::Error {
                                    message: "out of scope before capture delivery".into(),
                                }
                            } else if std::time::Instant::now() >= lease_deadline {
                                Response::Error {
                                    message: "privileged capability lease expired".into(),
                                }
                            } else {
                                if tx
                                    .send(Outbound::CaptureOutput {
                                        payload,
                                        lease_deadline,
                                        scope: live_scope.clone(),
                                    })
                                    .is_err()
                                {
                                    break;
                                }
                                continue;
                            }
                        }
                        Err(message) => Response::Error { message },
                    }
                };
                audit_capability_response(
                    handler,
                    &live_scope,
                    ActorCapability::CaptureOutput,
                    crate::journal::CapabilityUseAction::Capture,
                    &response,
                );
                response
            }
            Request::CaptureInteractionDomain {
                interaction_domain,
                region,
            } => {
                let current_scope = live_scope.resolve(handler);
                let (authorized, via_grant) = match current_scope
                    .as_ref()
                    .map(|scope| scope.decide_interaction_domain_capture(interaction_domain))
                {
                    Some(AuthorizationDecision::Permit) => (true, false),
                    Some(AuthorizationDecision::Ask(op)) => {
                        match grant_authorize(handler, conn_id, principal.as_deref(), op) {
                            Ok(true) => (
                                current_scope.as_ref().is_some_and(|scope| {
                                    scope.permits_interaction_domain_capture_target(
                                        interaction_domain,
                                    )
                                }),
                                true,
                            ),
                            Ok(false) | Err(_) => (false, false),
                        }
                    }
                    _ => (false, false),
                };
                let response = if !granted.interaction_domain {
                    Response::Error {
                        message:
                            "CaptureInteractionDomain requires the interaction_domain capability"
                                .into(),
                    }
                } else if !lease_alive {
                    Response::Error {
                        message: "privileged capability lease expired".into(),
                    }
                } else if !authorized {
                    Response::Error {
                        message: "out of scope".into(),
                    }
                } else if let Err(message) = handler.authorize_agent_interaction_domain_capture(
                    principal.as_deref(),
                    interaction_domain,
                ) {
                    Response::Error { message }
                } else {
                    match handler.capture_interaction_domain(
                        conn_id,
                        principal.as_deref(),
                        interaction_domain,
                        region,
                    ) {
                        Ok(payload) if payload.capture.interaction_domain == interaction_domain => {
                            let lease_deadline = active_lease
                                .as_ref()
                                .map(|(_, deadline)| *deadline)
                                .expect("granted InteractionDomain capability has an active lease");
                            if std::time::Instant::now() >= lease_deadline {
                                Response::Error {
                                    message: "privileged capability lease expired".into(),
                                }
                            } else if tx
                                .send(Outbound::CaptureInteractionDomain {
                                    payload,
                                    lease_deadline,
                                    scope: live_scope.clone(),
                                    via_grant,
                                })
                                .is_err()
                            {
                                break;
                            } else {
                                continue;
                            }
                        }
                        Ok(payload) => Response::Error {
                            message: format!(
                                "capture handler returned InteractionDomain {} for requested InteractionDomain {}",
                                payload.capture.interaction_domain.0, interaction_domain.0
                            ),
                        },
                        Err(message) => Response::Error { message },
                    }
                };
                audit_capability_response(
                    handler,
                    &live_scope,
                    ActorCapability::CaptureInteractionDomain,
                    crate::journal::CapabilityUseAction::Capture,
                    &response,
                );
                response
            }
            Request::CaptureWindow { window } => {
                // Per-window pixel capture is fail-closed like CaptureOutput:
                // `control` plus an explicit CaptureWindow scope decision for
                // this exact window — never inherited through None-means-all.
                let current_scope = live_scope.resolve(handler);
                let (authorized, via_grant) = match current_scope
                    .as_ref()
                    .map(|scope| scope.decide_window_capture(window))
                {
                    Some(AuthorizationDecision::Permit) => (true, false),
                    Some(AuthorizationDecision::Ask(op)) => {
                        match grant_authorize(handler, conn_id, principal.as_deref(), op) {
                            Ok(true) => (
                                current_scope
                                    .as_ref()
                                    .is_some_and(|scope| scope.permits_window(window)),
                                true,
                            ),
                            Ok(false) | Err(_) => (false, false),
                        }
                    }
                    _ => (false, false),
                };
                let response = if !granted.control {
                    Response::Error {
                        message: "CaptureWindow requires the control capability".into(),
                    }
                } else if !lease_alive {
                    Response::Error {
                        message: "privileged capability lease expired".into(),
                    }
                } else if !authorized {
                    Response::Error {
                        message: "out of scope".into(),
                    }
                } else {
                    match handler.capture_window(conn_id, principal.as_deref(), window) {
                        Ok(payload) if payload.capture.window == window => {
                            let lease_deadline = active_lease
                                .as_ref()
                                .map(|(_, deadline)| *deadline)
                                .expect("granted control has an active lease");
                            if std::time::Instant::now() >= lease_deadline {
                                Response::Error {
                                    message: "privileged capability lease expired".into(),
                                }
                            } else if tx
                                .send(Outbound::CaptureWindow {
                                    payload,
                                    lease_deadline,
                                    scope: live_scope.clone(),
                                    via_grant,
                                })
                                .is_err()
                            {
                                break;
                            } else {
                                continue;
                            }
                        }
                        Ok(payload) => Response::Error {
                            message: format!(
                                "capture handler returned window {} for requested window {}",
                                payload.capture.window.0, window.0
                            ),
                        },
                        Err(message) => Response::Error { message },
                    }
                };
                audit_capability_response(
                    handler,
                    &live_scope,
                    ActorCapability::CaptureWindow,
                    crate::journal::CapabilityUseAction::Capture,
                    &response,
                );
                response
            }
            Request::ObserveInteractionDomain { interaction_domain } => {
                let current_scope = effective_scope(
                    handler,
                    scope_name.as_deref(),
                    &granted_scope,
                    principal.as_deref(),
                );
                let authorized = match current_scope
                    .as_ref()
                    .map(|scope| scope.decide_interaction_domain_observation(interaction_domain))
                {
                    Some(AuthorizationDecision::Permit) => true,
                    Some(AuthorizationDecision::Ask(op)) => {
                        grant_authorize(handler, conn_id, principal.as_deref(), op).unwrap_or(false)
                    }
                    _ => false,
                };
                let response = if !granted.query {
                    Response::Error {
                        message: "ObserveInteractionDomain requires the query capability".into(),
                    }
                } else if !authorized {
                    Response::Error {
                        message: "out of scope".into(),
                    }
                } else if let Err(message) = handler.authorize_agent_interaction_domain_capture(
                    principal.as_deref(),
                    interaction_domain,
                ) {
                    Response::Error { message }
                } else {
                    match handler.observe_interaction_domain(
                        conn_id,
                        principal.as_deref(),
                        interaction_domain,
                    ) {
                        Ok(observation)
                            if observation.snapshot.interaction_domain == interaction_domain =>
                        {
                            Response::InteractionDomainObserved { observation }
                        }
                        Ok(_) => Response::Error {
                            message: "observation handler returned a different InteractionDomain"
                                .into(),
                        },
                        Err(message) => Response::Error { message },
                    }
                };
                audit_capability_response(
                    handler,
                    &live_scope,
                    ActorCapability::ObserveInteractionDomain,
                    crate::journal::CapabilityUseAction::Observe,
                    &response,
                );
                response
            }
            Request::ActInInteractionDomain { intent } => {
                let validation = intent.validate().map_err(str::to_owned);
                let current_scope = effective_scope(
                    handler,
                    scope_name.as_deref(),
                    &granted_scope,
                    principal.as_deref(),
                );
                let mut rejection = validation.err();
                if rejection.is_none() && !granted.input {
                    rejection = Some("ActInInteractionDomain requires the input capability".into());
                }
                if rejection.is_none() && !lease_alive {
                    rejection = Some("privileged capability lease expired".into());
                }
                if rejection.is_none() {
                    rejection = match current_scope.as_ref().map(|scope| {
                        scope.decide_interaction_domain_input(intent.interaction_domain)
                    }) {
                        Some(AuthorizationDecision::Permit) => None,
                        Some(AuthorizationDecision::Deny) | None => Some("out of scope".into()),
                        Some(AuthorizationDecision::Ask(op)) => {
                            match grant_authorize(handler, conn_id, principal.as_deref(), op) {
                                Ok(true) => None,
                                Ok(false) => {
                                    Some("out of scope: the user denied this operation".into())
                                }
                                Err(message) => Some(message),
                            }
                        }
                    };
                }
                if rejection.is_none() {
                    rejection = handler
                        .authorize_agent_interaction_domain_capture(
                            principal.as_deref(),
                            intent.interaction_domain,
                        )
                        .err();
                }
                if let Some(message) = rejection {
                    if (32..=128).contains(&intent.observation.0.len()) {
                        handler.discard_observation(
                            conn_id,
                            principal.as_deref(),
                            &intent.observation,
                        );
                    }
                    let actions_truncated = intent.actions.len() > 64;
                    handler.audit_refusal(
                        conn_id,
                        principal.as_deref(),
                        JournalMutation::ActorAction {
                            action_id: None,
                            interaction_domain: intent.interaction_domain,
                            target: intent.target,
                            window: None,
                            actions: crate::journal::audit_semantic_actions(
                                &intent.actions.iter().take(64).cloned().collect::<Vec<_>>(),
                            ),
                            actions_truncated,
                            authority_revision: None,
                        },
                        message.clone(),
                    );
                    Response::Error { message }
                } else {
                    let expected_interaction_domain = intent.interaction_domain;
                    let expected_target = intent.target;
                    let expected_actions = intent.actions.len() as u32;
                    match handler.act_in_interaction_domain(
                        conn_id,
                        principal.as_deref(),
                        scope_name.as_deref(),
                        current_scope.expect("authorized action has a live scope"),
                        intent,
                    ) {
                        Ok(receipt)
                            if receipt.action_id != 0
                                && receipt.interaction_domain == expected_interaction_domain
                                && receipt.target == expected_target
                                && receipt.window.0 != 0
                                && receipt.actions_applied == expected_actions =>
                        {
                            Response::ActorActionCommitted { receipt }
                        }
                        Ok(_) => Response::Error {
                            message: "action handler returned an inconsistent receipt".into(),
                        },
                        Err(message) => Response::Error { message },
                    }
                }
            }
            Request::StreamOutputStart {
                max_fps,
                target,
                dmabuf,
            } => {
                // Fail-closed exactly like CaptureOutput: `control`, a live
                // lease, and an explicit StreamOutput op in the granted
                // scope — never inherited through None-means-all (ADR-0052).
                let current_scope = live_scope.resolve(handler);
                let op_allowed = current_scope
                    .as_ref()
                    .is_some_and(|scope| scope_permits_stream(scope, target));
                let response = if !granted.control {
                    Response::Error {
                        message: "StreamOutputStart requires the control capability".into(),
                    }
                } else if !lease_alive {
                    Response::Error {
                        message: "privileged capability lease expired".into(),
                    }
                } else if !op_allowed {
                    Response::Error {
                        message: "out of scope".into(),
                    }
                } else if !handler.capture_security_active() {
                    Response::Error {
                        message: "session is locked or inactive".into(),
                    }
                } else {
                    // The zero-copy transport requires an explicit opt-in at
                    // protocol 25 or later: a client that did not opt in must
                    // never receive a dmabuf announcement, or its framing
                    // desynchronizes.
                    let allow_dmabuf = dmabuf == Some(true) && version >= 25;
                    match handler.stream_output_start(conn_id, max_fps, target, allow_dmabuf) {
                        Ok(info) => {
                            streams.lock().unwrap().insert(
                                info.stream_id,
                                StreamLane {
                                    conn_id,
                                    tx: tx.clone(),
                                    scope: live_scope.clone(),
                                    target,
                                    lease_deadline: Arc::clone(&lease_deadline_shared),
                                    queued: Arc::new(AtomicU32::new(0)),
                                },
                            );
                            match info.slots {
                                Some(table) => {
                                    let slots = table.fds.len() as u32;
                                    let slot_stride = table.stride;
                                    let slot_bytes = table.byte_len;
                                    stream_start_table = Some(table);
                                    Response::StreamOutputStarted {
                                        stream_id: info.stream_id,
                                        width: info.width,
                                        height: info.height,
                                        format: info.format,
                                        slots: Some(slots),
                                        slot_stride: Some(slot_stride),
                                        slot_bytes: Some(slot_bytes),
                                    }
                                }
                                None => Response::StreamOutputStarted {
                                    stream_id: info.stream_id,
                                    width: info.width,
                                    height: info.height,
                                    format: info.format,
                                    slots: None,
                                    slot_stride: None,
                                    slot_bytes: None,
                                },
                            }
                        }
                        Err(message) => Response::Error { message },
                    }
                };
                audit_capability_response(
                    handler,
                    &live_scope,
                    ActorCapability::StreamOutput,
                    crate::journal::CapabilityUseAction::Start,
                    &response,
                );
                response
            }
            Request::StreamBufferRelease { stream_id, slot } => {
                // A connection may release slots only of a stream it owns
                // (same ownership rule as StreamOutputStop). Releases arrive
                // per consumed frame, so they are deliberately not journaled.
                let owned = streams
                    .lock()
                    .unwrap()
                    .get(&stream_id)
                    .is_some_and(|lane| lane.conn_id == conn_id);
                if owned {
                    handler.stream_buffer_release(stream_id, slot);
                    Response::StreamBufferReleased { stream_id, slot }
                } else {
                    Response::Error {
                        message: format!("unknown stream {stream_id}"),
                    }
                }
            }
            Request::StreamOutputStop { stream_id } => {
                // A connection may stop only a stream it owns.
                let owned = streams
                    .lock()
                    .unwrap()
                    .get(&stream_id)
                    .is_some_and(|lane| lane.conn_id == conn_id);
                let response = if owned {
                    streams.lock().unwrap().remove(&stream_id);
                    handler.stream_output_stop(stream_id);
                    Response::StreamOutputStopped { stream_id }
                } else {
                    Response::Error {
                        message: format!("unknown stream {stream_id}"),
                    }
                };
                audit_capability_response(
                    handler,
                    &live_scope,
                    ActorCapability::StreamOutput,
                    crate::journal::CapabilityUseAction::Stop,
                    &response,
                );
                response
            }
            Request::SetIdleInhibit { inhibit } => {
                // Fail-closed exactly like StreamOutputStart: `control`, a
                // live lease, and an explicit IdleInhibit op in the granted
                // scope — never inherited through None-means-all (ADR-0075).
                let current_scope = live_scope.resolve(handler);
                let op_allowed = current_scope.as_ref().is_some_and(|scope| {
                    scope
                        .ops
                        .as_ref()
                        .is_some_and(|ops| ops.contains(&ActorCapability::IdleInhibit))
                });
                let response = if !granted.control {
                    Response::Error {
                        message: "SetIdleInhibit requires the control capability".into(),
                    }
                } else if !lease_alive {
                    Response::Error {
                        message: "privileged capability lease expired".into(),
                    }
                } else if !op_allowed {
                    Response::Error {
                        message: "out of scope".into(),
                    }
                } else {
                    match handler.set_idle_inhibit(conn_id, inhibit) {
                        Ok(inhibited) => {
                            idle_inhibited = inhibited;
                            Response::IdleInhibitSet { inhibited }
                        }
                        Err(message) => Response::Error { message },
                    }
                };
                audit_capability_response(
                    handler,
                    &live_scope,
                    ActorCapability::IdleInhibit,
                    if inhibit {
                        crate::journal::CapabilityUseAction::Enable
                    } else {
                        crate::journal::CapabilityUseAction::Disable
                    },
                    &response,
                );
                response
            }
            Request::PickTarget { kind } => {
                // Fail-closed exactly like StreamOutputStart (ADR-0054):
                // `control`, a live lease, and an explicit PickTarget op in
                // the granted scope — never inherited — plus the lock/VT
                // gate, since a pick presents and reads screen content. The
                // user's click is the interactive half of the authorization.
                let current_scope = live_scope.resolve(handler);
                let op_allowed = current_scope.as_ref().is_some_and(|scope| {
                    scope
                        .ops
                        .as_ref()
                        .is_some_and(|ops| ops.contains(&ActorCapability::PickTarget))
                });
                let response = if !granted.control {
                    Response::Error {
                        message: "PickTarget requires the control capability".into(),
                    }
                } else if !lease_alive {
                    Response::Error {
                        message: "privileged capability lease expired".into(),
                    }
                } else if !op_allowed {
                    Response::Error {
                        message: "out of scope".into(),
                    }
                } else if !handler.capture_security_active() {
                    Response::Error {
                        message: "session is locked or inactive".into(),
                    }
                } else {
                    match handler.pick_target(conn_id, kind) {
                        Ok(result) => {
                            // The pick blocked on user interaction; policy may
                            // have changed meanwhile, so re-check before
                            // delivering the picked content (ADR-0054).
                            let scope_still_allows =
                                live_scope.resolve(handler).is_some_and(|scope| {
                                    scope.ops.as_ref().is_some_and(|ops| {
                                        ops.contains(&ActorCapability::PickTarget)
                                    })
                                });
                            if !scope_still_allows {
                                Response::Error {
                                    message: "out of scope before pick delivery".into(),
                                }
                            } else if active_lease
                                .as_ref()
                                .is_some_and(|(_, deadline)| std::time::Instant::now() >= *deadline)
                            {
                                Response::Error {
                                    message: "privileged capability lease expired".into(),
                                }
                            } else {
                                Response::Picked { result }
                            }
                        }
                        Err(message) => Response::Error { message },
                    }
                };
                audit_capability_response(
                    handler,
                    &live_scope,
                    ActorCapability::PickTarget,
                    crate::journal::CapabilityUseAction::Pick,
                    &response,
                );
                response
            }
            Request::PickApp {
                choices,
                subject,
                last_choice,
            } => {
                // Fail-closed like the other interactive prompts: `control`, a live lease,
                // an explicit PickApp op (never inherited), the lock/VT gate,
                // and a scope+lease re-check before delivery.
                let current_scope = live_scope.resolve(handler);
                let op_allowed = current_scope.as_ref().is_some_and(|scope| {
                    scope
                        .ops
                        .as_ref()
                        .is_some_and(|ops| ops.contains(&ActorCapability::PickApp))
                });
                let request_valid =
                    valid_app_pick_request(&choices, subject.as_deref(), last_choice.as_deref());
                let response = if !granted.control {
                    Response::Error {
                        message: "PickApp requires the control capability".into(),
                    }
                } else if !lease_alive {
                    Response::Error {
                        message: "privileged capability lease expired".into(),
                    }
                } else if !op_allowed {
                    Response::Error {
                        message: "out of scope".into(),
                    }
                } else if !request_valid {
                    Response::Error {
                            message: "application picker request is empty, oversized, duplicate, or inconsistent".into(),
                        }
                } else if !handler.capture_security_active() {
                    Response::Error {
                        message: "session is locked or inactive".into(),
                    }
                } else {
                    match handler.pick_app(conn_id, choices, subject, last_choice) {
                        Ok(result) => {
                            let scope_still_allows =
                                live_scope.resolve(handler).is_some_and(|scope| {
                                    scope
                                        .ops
                                        .as_ref()
                                        .is_some_and(|ops| ops.contains(&ActorCapability::PickApp))
                                });
                            if !scope_still_allows {
                                Response::Error {
                                    message: "out of scope before pick delivery".into(),
                                }
                            } else if active_lease
                                .as_ref()
                                .is_some_and(|(_, deadline)| std::time::Instant::now() >= *deadline)
                            {
                                Response::Error {
                                    message: "privileged capability lease expired".into(),
                                }
                            } else {
                                Response::AppPicked { result }
                            }
                        }
                        Err(message) => Response::Error { message },
                    }
                };
                audit_capability_response(
                    handler,
                    &live_scope,
                    ActorCapability::PickApp,
                    crate::journal::CapabilityUseAction::Pick,
                    &response,
                );
                response
            }
            Request::PromptSecret {
                resource_grant,
                title,
                reason,
            } => {
                // Fail-closed like the other interactive prompts: `control`, a live lease,
                // an explicit PromptSecret op (never inherited), the lock/VT
                // gate, and a scope+lease re-check before delivery.
                let current_scope = live_scope.resolve(handler);
                let op_allowed = current_scope.as_ref().is_some_and(|scope| {
                    scope
                        .ops
                        .as_ref()
                        .is_some_and(|ops| ops.contains(&ActorCapability::PromptSecret))
                });
                let prompt_valid = bounded_text(&title, 256, false)
                    && reason
                        .as_deref()
                        .is_none_or(|value| bounded_text(value, 256, true));
                let response = if !granted.control {
                    Response::Error {
                        message: "PromptSecret requires the control capability".into(),
                    }
                } else if !lease_alive {
                    Response::Error {
                        message: "privileged capability lease expired".into(),
                    }
                } else if !op_allowed {
                    Response::Error {
                        message: "out of scope".into(),
                    }
                } else if !prompt_valid {
                    Response::Error {
                        message: "secret prompt labels are empty, oversized, or contain NUL".into(),
                    }
                } else if !handler.capture_security_active() {
                    Response::Error {
                        message: "session is locked or inactive".into(),
                    }
                } else {
                    let resource = aegis_security::authority::ActorResource::secret_prompt(
                        &title,
                        reason.as_deref(),
                    );
                    match handler.consume_resource_grant(
                        session.id,
                        principal.as_deref(),
                        &resource_grant,
                        &resource,
                    ) {
                        Err(message) => {
                            audit_resource_grant_refusal(
                                handler,
                                conn_id,
                                principal.as_deref(),
                                session.id,
                                crate::journal::ResourceGrantAttemptAction::Consume,
                                Some(&resource),
                            );
                            Response::Error { message }
                        }
                        Ok(_) => match handler.prompt_secret(conn_id, title, reason) {
                            Ok(result) => {
                                let scope_still_allows =
                                    live_scope.resolve(handler).is_some_and(|scope| {
                                        scope.ops.as_ref().is_some_and(|ops| {
                                            ops.contains(&ActorCapability::PromptSecret)
                                        })
                                    });
                                if !scope_still_allows {
                                    Response::Error {
                                        message: "out of scope before prompt delivery".into(),
                                    }
                                } else if active_lease.as_ref().is_some_and(|(_, deadline)| {
                                    std::time::Instant::now() >= *deadline
                                }) {
                                    Response::Error {
                                        message: "privileged capability lease expired".into(),
                                    }
                                } else {
                                    Response::SecretPrompted { result }
                                }
                            }
                            Err(message) => Response::Error { message },
                        },
                    }
                };
                audit_capability_response(
                    handler,
                    &live_scope,
                    ActorCapability::PromptSecret,
                    crate::journal::CapabilityUseAction::Prompt,
                    &response,
                );
                response
            }
            Request::PickConfirm {
                title,
                body,
                accept_label,
            } => {
                // Fail-closed exactly like the other picks: `control`, a
                // live lease, an explicit PickConfirm op (never
                // inherited), the lock/VT gate, and a scope+lease
                // re-check before delivery.
                let current_scope = live_scope.resolve(handler);
                let op_allowed = current_scope.as_ref().is_some_and(|scope| {
                    scope
                        .ops
                        .as_ref()
                        .is_some_and(|ops| ops.contains(&ActorCapability::PickConfirm))
                });
                let prompt_valid = bounded_text(&title, 256, false)
                    && bounded_text(&body, 4_096, false)
                    && accept_label
                        .as_deref()
                        .is_none_or(|value| bounded_text(value, 128, false));
                let response = if !granted.control {
                    Response::Error {
                        message: "PickConfirm requires the control capability".into(),
                    }
                } else if !lease_alive {
                    Response::Error {
                        message: "privileged capability lease expired".into(),
                    }
                } else if !op_allowed {
                    Response::Error {
                        message: "out of scope".into(),
                    }
                } else if !prompt_valid {
                    Response::Error {
                        message: "confirmation labels are empty, oversized, or contain NUL".into(),
                    }
                } else if !handler.capture_security_active() {
                    Response::Error {
                        message: "session is locked or inactive".into(),
                    }
                } else {
                    match handler.pick_confirm(conn_id, title, body, accept_label) {
                        Ok(result) => {
                            let scope_still_allows =
                                live_scope.resolve(handler).is_some_and(|scope| {
                                    scope.ops.as_ref().is_some_and(|ops| {
                                        ops.contains(&ActorCapability::PickConfirm)
                                    })
                                });
                            if !scope_still_allows {
                                Response::Error {
                                    message: "out of scope before confirm delivery".into(),
                                }
                            } else if active_lease
                                .as_ref()
                                .is_some_and(|(_, deadline)| std::time::Instant::now() >= *deadline)
                            {
                                Response::Error {
                                    message: "privileged capability lease expired".into(),
                                }
                            } else {
                                Response::ConfirmPicked { result }
                            }
                        }
                        Err(message) => Response::Error { message },
                    }
                };
                audit_capability_response(
                    handler,
                    &live_scope,
                    ActorCapability::PickConfirm,
                    crate::journal::CapabilityUseAction::Pick,
                    &response,
                );
                response
            }
            Request::SetWallpaper { path } => {
                // Mutation gate: `control`, a live lease, an explicit
                // SetWallpaper op (never inherited), and the lock/VT
                // gate. The reply is the main loop's authoritative
                // decode-and-swap receipt.
                let current_scope = live_scope.resolve(handler);
                let op_allowed = current_scope.as_ref().is_some_and(|scope| {
                    scope
                        .ops
                        .as_ref()
                        .is_some_and(|ops| ops.contains(&ActorCapability::SetWallpaper))
                });
                let path_valid = valid_wallpaper_path(&path);
                let response = if !granted.control {
                    Response::Error {
                        message: "SetWallpaper requires the control capability".into(),
                    }
                } else if !lease_alive {
                    Response::Error {
                        message: "privileged capability lease expired".into(),
                    }
                } else if !op_allowed {
                    Response::Error {
                        message: "out of scope".into(),
                    }
                } else if !path_valid {
                    Response::Error {
                        message:
                            "wallpaper path must be bounded, absolute, and lexically normalized"
                                .into(),
                    }
                } else if !handler.capture_security_active() {
                    Response::Error {
                        message: "session is locked or inactive".into(),
                    }
                } else {
                    match handler.set_wallpaper(conn_id, path) {
                        Ok(()) => Response::WallpaperSet {},
                        Err(message) => Response::Error { message },
                    }
                };
                audit_capability_response(
                    handler,
                    &live_scope,
                    ActorCapability::SetWallpaper,
                    crate::journal::CapabilityUseAction::Apply,
                    &response,
                );
                response
            }
        };
        let outbound = match stream_start_table {
            Some(table) => Outbound::StreamStarted {
                response: resp,
                table,
            },
            None => Outbound::Response(resp),
        };
        if tx.send(outbound).is_err() {
            break;
        }
    }
    (sub_id, journal_sub_id, idle_inhibited)
}
