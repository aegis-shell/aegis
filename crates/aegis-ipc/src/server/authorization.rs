use super::*;

/// Resolve the interactive-grant path for an askable operation (ADR-0088):
/// a recorded grant short-circuits, a recorded denial refuses without
/// prompting, anything else asks the user through the handler. Callers
/// without a bound principal (anonymous compatibility connections) have no
/// grant store and are refused outright.
pub(super) fn grant_authorize<H: Handler>(
    handler: &H,
    conn_id: u64,
    principal: Option<&str>,
    op: ActorCapability,
) -> Result<bool, String> {
    let Some(principal) = principal else {
        return Err("out of scope: operation requires a paired agent".into());
    };
    match handler.grant_for(principal, op) {
        Some(true) => Ok(true),
        Some(false) => Ok(false),
        None => handler.request_grant(conn_id, principal, op),
    }
}

pub(super) fn effective_scope<H: Handler>(
    handler: &H,
    scope_name: Option<&str>,
    granted_scope: &Scope,
    principal: Option<&str>,
) -> Option<Scope> {
    if let Some(name) = scope_name {
        return handler.resolve_scope(name);
    }
    let Some(principal) = principal else {
        return Some(granted_scope.clone());
    };
    match handler.refresh_agent_identity(principal) {
        Ok(Some(identity)) => Some(Scope {
            ops: Some(identity.pregranted),
            ask_ops: Some(identity.gated),
            ..Scope::default()
        }),
        Ok(None) => Some(granted_scope.clone()),
        Err(_) => None,
    }
}

pub(super) fn scope_permits_stream(scope: &Scope, target: &crate::schema::StreamTarget) -> bool {
    scope.pregrants(ActorCapability::StreamOutput)
        && match target {
            // An output selector names a connector, not a scoped resource:
            // its existence is checked at stream start on the main loop.
            crate::schema::StreamTarget::Output { .. } => true,
            crate::schema::StreamTarget::Window { window } => scope.permits_window(*window),
        }
}

pub(super) fn bounded_text(value: &str, max_bytes: usize, allow_empty: bool) -> bool {
    (allow_empty || !value.trim().is_empty()) && value.len() <= max_bytes && !value.contains('\0')
}

pub(super) fn bounded_identifier_text(value: &str, max_bytes: usize) -> bool {
    bounded_text(value, max_bytes, false) && !value.chars().any(char::is_control)
}

pub(super) fn valid_capability_list(capabilities: &[ActorCapability]) -> bool {
    capabilities.len() <= 128
        && capabilities
            .iter()
            .enumerate()
            .all(|(index, capability)| !capabilities[..index].contains(capability))
}

pub(super) fn valid_agent_hello(agent: &crate::schema::AgentHello) -> bool {
    agent
        .label
        .as_deref()
        .is_none_or(|label| bounded_identifier_text(label, 256))
        && valid_capability_list(&agent.requested)
        && agent.credential.as_deref().is_none_or(|credential| {
            bounded_identifier_text(credential, 512) && credential.is_ascii()
        })
}

pub(super) fn valid_principal_id(principal: &str) -> bool {
    aegis_security::authority::ActorPrincipal::new(principal.to_owned()).is_ok()
}

pub(super) fn valid_app_pick_request(
    choices: &[String],
    subject: Option<&str>,
    last_choice: Option<&str>,
) -> bool {
    (1..=256).contains(&choices.len())
        && choices
            .iter()
            .all(|choice| bounded_text(choice, 512, false))
        && choices
            .iter()
            .enumerate()
            .all(|(index, choice)| !choices[..index].contains(choice))
        && subject.is_none_or(|value| bounded_text(value, 1_024, true))
        && last_choice.is_none_or(|value| {
            bounded_text(value, 512, false) && choices.iter().any(|choice| choice == value)
        })
}

pub(super) fn valid_wallpaper_path(path: &Path) -> bool {
    aegis_security::authority::ActorResource::FilesystemPath {
        path: path.to_path_buf(),
        access: aegis_security::authority::FilesystemAccess::Read,
    }
    .validate()
    .is_ok()
}

pub(super) fn subject_owns_interaction_domain(
    snapshot: &aegis_model::interaction_domain::InteractionDomainSnapshot,
    subject: &str,
    interaction_domain: aegis_model::interaction_domain::InteractionDomainId,
) -> bool {
    let Some(interaction_domain) = snapshot
        .interaction_domains
        .iter()
        .find(|candidate| candidate.id == interaction_domain)
    else {
        return false;
    };
    snapshot
        .principals
        .iter()
        .find(|principal| principal.id == interaction_domain.controller)
        .and_then(|principal| principal.subject.as_deref())
        == Some(subject)
}

pub(super) fn filter_windows(
    windows: Vec<aegis_model::window::Window>,
    scope: &Scope,
) -> Vec<aegis_model::window::Window> {
    windows
        .into_iter()
        .filter(|window| scope.permits_window(window.id))
        .collect()
}

pub(super) fn filter_accessibility_windows(
    windows: Vec<aegis_semantic::AccessibilityWindowBinding>,
    scope: &Scope,
) -> Vec<aegis_semantic::AccessibilityWindowBinding> {
    windows
        .into_iter()
        .filter(|binding| scope.permits_window(binding.window.id))
        .collect()
}

pub(super) fn filter_workspaces(
    mut snapshot: aegis_model::workspace::WorkspaceSnapshot,
    scope: &Scope,
) -> aegis_model::workspace::WorkspaceSnapshot {
    snapshot
        .outputs
        .retain(|output| scope.permits_output(output.id));
    for output in &mut snapshot.outputs {
        output
            .workspaces
            .retain(|workspace| scope.permits_workspace(workspace.id));
        for workspace in &mut output.workspaces {
            workspace
                .toplevels
                .retain(|window| scope.permits_window(*window));
        }
        if output
            .current
            .is_some_and(|current| !scope.permits_workspace(current))
        {
            output.current = output.workspaces.first().map(|workspace| workspace.id);
        }
    }
    snapshot
}

pub(super) fn filter_interaction_domains(
    mut snapshot: aegis_model::interaction_domain::InteractionDomainSnapshot,
    scope: &Scope,
    subject: Option<&str>,
) -> aegis_model::interaction_domain::InteractionDomainSnapshot {
    let principal_subjects = snapshot
        .principals
        .iter()
        .map(|principal| (principal.id, principal.subject.as_deref()))
        .collect::<std::collections::BTreeMap<_, _>>();
    snapshot.interaction_domains.retain(|interaction_domain| {
        if !scope.permits_interaction_domain(interaction_domain.id) {
            return false;
        }
        let Some(subject) = subject else {
            return true;
        };
        interaction_domain.kind == aegis_model::interaction_domain::InteractionDomainKind::Human
            || principal_subjects
                .get(&interaction_domain.controller)
                .copied()
                .flatten()
                == Some(subject)
    });
    let interaction_domains = snapshot
        .interaction_domains
        .iter()
        .map(|interaction_domain| interaction_domain.id)
        .collect::<std::collections::BTreeSet<_>>();
    let principals = snapshot
        .interaction_domains
        .iter()
        .map(|interaction_domain| interaction_domain.controller)
        .collect::<std::collections::BTreeSet<_>>();
    snapshot
        .principals
        .retain(|principal| principals.contains(&principal.id));
    snapshot
        .seats
        .retain(|seat| interaction_domains.contains(&seat.interaction_domain));
    snapshot.interaction_groups.retain_mut(|group| {
        if !interaction_domains.contains(&group.control_interaction_domain) {
            return false;
        }
        group
            .observer_interaction_domains
            .retain(|observer| interaction_domains.contains(observer));
        group.windows.retain(|window| scope.permits_window(*window));
        !group.windows.is_empty()
    });
    let clients = snapshot
        .interaction_groups
        .iter()
        .map(|group| group.client)
        .collect::<std::collections::BTreeSet<_>>();
    snapshot
        .clients
        .retain(|client| clients.contains(&client.id));
    snapshot
}

pub(super) fn filter_journal(
    mut snapshot: crate::journal::JournalSnapshot,
    scope: &Scope,
    subject: Option<&str>,
) -> crate::journal::JournalSnapshot {
    snapshot
        .entries
        .retain(|entry| journal_entry_permitted(entry, scope, subject));
    snapshot
}

/// Whether one journal entry may be delivered to a connection with this
/// scope and subject — the per-entry half of [`filter_journal`], shared with
/// the journal subscription lanes (ADR-0125).
pub(super) fn journal_entry_permitted(
    entry: &crate::journal::JournalEntry,
    scope: &Scope,
    subject: Option<&str>,
) -> bool {
    if let Some(subject) = subject
        && let crate::journal::Origin::Actor { principal, .. } = &entry.origin
        && principal != subject
    {
        return false;
    }
    match &entry.mutation {
        JournalMutation::Command { cmd } => cmd.permitted_by(scope),
        JournalMutation::InteractionDomain { action, .. } => {
            scope.permits_interaction_domain_action_resources(action)
        }
        JournalMutation::ActorAction {
            interaction_domain,
            window,
            ..
        } => {
            scope.permits_interaction_domain(*interaction_domain)
                && window.is_none_or(|window| scope.permits_window(window))
        }
        JournalMutation::Settings { .. } => true,
        JournalMutation::AgentAuth { principal, .. } => {
            subject.is_none_or(|subject| principal == subject)
        }
        // A platform scope claim is compositor-internal: principal-bound
        // agent lanes never observe it.
        JournalMutation::ScopeClaim { .. } => subject.is_none(),
        JournalMutation::ActorSession { principal, .. }
        | JournalMutation::ResourceGrant { principal, .. }
        | JournalMutation::ResourceGrantAttempt { principal, .. }
        | JournalMutation::CapabilityUse { principal, .. } => subject.is_none_or(|subject| {
            principal
                .as_ref()
                .is_some_and(|principal| principal.as_ref() == subject)
        }),
    }
}

pub(super) fn audit_resource_grant_refusal<H: Handler>(
    handler: &H,
    conn_id: u64,
    subject: Option<&str>,
    session: aegis_security::authority::ActorSessionId,
    action: crate::journal::ResourceGrantAttemptAction,
    resource: Option<&aegis_security::authority::ActorResource>,
) {
    let reason = match action {
        crate::journal::ResourceGrantAttemptAction::Issue => "resource grant issue refused",
        crate::journal::ResourceGrantAttemptAction::Consume => "resource grant consume refused",
        crate::journal::ResourceGrantAttemptAction::Revoke => "resource grant revoke refused",
    };
    handler.audit_refusal(
        conn_id,
        subject,
        JournalMutation::ResourceGrantAttempt {
            session,
            principal: subject
                .and_then(|value| aegis_security::authority::ActorPrincipal::new(value).ok()),
            action,
            capability: resource.map(aegis_security::authority::ActorResource::required_capability),
            resource_kind: resource.map(crate::journal::ResourceKind::from),
        },
        reason.to_owned(),
    );
}

pub(super) fn audit_capability_response<H: Handler>(
    handler: &H,
    binding: &LiveScopeBinding,
    capability: ActorCapability,
    action: crate::journal::CapabilityUseAction,
    response: &Response,
) {
    let effect = if matches!(response, Response::Error { .. }) {
        crate::journal::Effect::Refused {
            reason: "capability use refused".into(),
        }
    } else {
        crate::journal::Effect::Applied
    };
    audit_capability_effect(handler, binding, capability, action, effect);
}

pub(super) fn audit_capability_effect<H: Handler>(
    handler: &H,
    binding: &LiveScopeBinding,
    capability: ActorCapability,
    action: crate::journal::CapabilityUseAction,
    effect: crate::journal::Effect,
) {
    handler.audit_capability_use(
        binding.connection_id,
        binding.principal.as_ref().map(AsRef::as_ref),
        binding.session,
        capability,
        action,
        effect,
    );
}

pub(super) fn subject_may_transfer_through(
    snapshot: &aegis_model::interaction_domain::InteractionDomainSnapshot,
    subject: &str,
    interaction_domain: aegis_model::interaction_domain::InteractionDomainId,
) -> bool {
    interaction_domain == aegis_model::interaction_domain::HUMAN_INTERACTION_DOMAIN
        || subject_owns_interaction_domain(snapshot, subject, interaction_domain)
}

pub(super) fn authorize_subject_interaction_domain_action(
    subject: &str,
    action: &InteractionDomainAction,
    snapshot: &aegis_model::interaction_domain::InteractionDomainSnapshot,
) -> Result<(), String> {
    let owns =
        |interaction_domain| subject_owns_interaction_domain(snapshot, subject, interaction_domain);
    let transfer =
        |interaction_domain| subject_may_transfer_through(snapshot, subject, interaction_domain);
    let allowed = match action {
        InteractionDomainAction::Create { .. } => true,
        InteractionDomainAction::Revoke {
            interaction_domain,
            fallback,
            ..
        } => owns(*interaction_domain) && transfer(*fallback),
        InteractionDomainAction::Transact { mutations, .. } => {
            mutations.iter().all(|mutation| match mutation {
                aegis_model::interaction_domain::InteractionDomainMutation::TransferWindow {
                    window,
                    target,
                    ..
                } => snapshot
                    .interaction_groups
                    .iter()
                    .find(|group| group.windows.contains(window))
                    .is_some_and(|group| {
                        transfer(group.control_interaction_domain) && transfer(*target)
                    }),
                aegis_model::interaction_domain::InteractionDomainMutation::SetObserver {
                    group,
                    interaction_domain,
                    ..
                } => {
                    owns(*interaction_domain)
                        && snapshot
                            .interaction_groups
                            .iter()
                            .find(|candidate| candidate.id == *group)
                            .is_some_and(|group| transfer(group.control_interaction_domain))
                }
                aegis_model::interaction_domain::InteractionDomainMutation::ConfigureOutput {
                    interaction_domain,
                    ..
                }
                | aegis_model::interaction_domain::InteractionDomainMutation::SetState {
                    interaction_domain,
                    ..
                } => owns(*interaction_domain),
            })
        }
    };
    allowed
        .then_some(())
        .ok_or_else(|| "out of scope: InteractionDomain is owned by another principal".into())
}
