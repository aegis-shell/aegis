use tessera_model::interaction_domain::InteractionDomainId;
use tessera_model::window::WindowId;
use tessera_model::workspace::{OutputId, WorkspaceId};

use super::{ActorCapability, AuthorizationDecision};

/// Transport-neutral operation and GUI-resource ceiling for one Actor.
///
/// `None` on a resource axis means every resource on that axis. Operations
/// are intentionally asymmetric: ordinary trusted-local compatibility may
/// use `None`, but high-risk Actor operations must always be named explicitly
/// by the adapter that authorizes them.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ActorScope {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub windows: Option<Vec<WindowId>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspaces: Option<Vec<WorkspaceId>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outputs: Option<Vec<OutputId>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interaction_domains: Option<Vec<InteractionDomainId>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ops: Option<Vec<ActorCapability>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ask_ops: Option<Vec<ActorCapability>>,
}

impl ActorScope {
    pub fn unscoped() -> Self {
        Self::default()
    }

    pub fn permits_window(&self, window: WindowId) -> bool {
        allows(&self.windows, window)
    }

    pub fn permits_workspace(&self, workspace: WorkspaceId) -> bool {
        allows(&self.workspaces, workspace)
    }

    pub fn permits_output(&self, output: OutputId) -> bool {
        allows(&self.outputs, output)
    }

    pub fn permits_interaction_domain(&self, interaction_domain: InteractionDomainId) -> bool {
        allows(&self.interaction_domains, interaction_domain)
    }

    pub fn permits_actor_observation(&self, actor_scoped: bool, op: ActorCapability) -> bool {
        !actor_scoped || self.pregrants(op)
    }

    pub fn permits_interaction_domain_capture(
        &self,
        interaction_domain: InteractionDomainId,
    ) -> bool {
        self.pregrants(ActorCapability::CaptureInteractionDomain)
            && self.permits_interaction_domain(interaction_domain)
    }

    pub fn permits_interaction_domain_capture_target(
        &self,
        interaction_domain: InteractionDomainId,
    ) -> bool {
        self.permits_interaction_domain(interaction_domain)
    }

    pub fn pregrants(&self, op: ActorCapability) -> bool {
        self.ops
            .as_ref()
            .is_some_and(|operations| operations.contains(&op))
    }

    pub fn asks(&self, op: ActorCapability) -> bool {
        self.ask_ops
            .as_ref()
            .is_some_and(|operations| operations.contains(&op))
    }

    /// Decide one explicitly capability-classed Interaction Domain operation.
    /// Observation and action remain separate because the caller supplies the
    /// exact capability rather than relying on an ambient query permission.
    pub fn decide_interaction_domain_capability(
        &self,
        interaction_domain: InteractionDomainId,
        capability: ActorCapability,
    ) -> AuthorizationDecision {
        if !self.permits_interaction_domain(interaction_domain) {
            return AuthorizationDecision::Deny;
        }
        if self.pregrants(capability) {
            AuthorizationDecision::Permit
        } else if self.asks(capability) {
            AuthorizationDecision::Ask(capability)
        } else {
            AuthorizationDecision::Deny
        }
    }

    /// Decide one per-window content capture. The window axis bounds which
    /// windows may be captured; the operation itself must still be named
    /// explicitly as a pregrant or an ask.
    pub fn decide_window_capture(&self, window: WindowId) -> AuthorizationDecision {
        if !self.permits_window(window) {
            return AuthorizationDecision::Deny;
        }
        if self.pregrants(ActorCapability::CaptureWindow) {
            AuthorizationDecision::Permit
        } else if self.asks(ActorCapability::CaptureWindow) {
            AuthorizationDecision::Ask(ActorCapability::CaptureWindow)
        } else {
            AuthorizationDecision::Deny
        }
    }

    pub fn decide_interaction_domain_capture(
        &self,
        interaction_domain: InteractionDomainId,
    ) -> AuthorizationDecision {
        self.decide_interaction_domain_capability(
            interaction_domain,
            ActorCapability::CaptureInteractionDomain,
        )
    }

    pub fn decide_interaction_domain_observation(
        &self,
        interaction_domain: InteractionDomainId,
    ) -> AuthorizationDecision {
        self.decide_interaction_domain_capability(
            interaction_domain,
            ActorCapability::ObserveInteractionDomain,
        )
    }

    pub fn decide_interaction_domain_input(
        &self,
        interaction_domain: InteractionDomainId,
    ) -> AuthorizationDecision {
        self.decide_interaction_domain_capability(
            interaction_domain,
            ActorCapability::InjectInteractionDomainInput,
        )
    }
}

pub(crate) fn allows<T: PartialEq + Copy>(allowlist: &Option<Vec<T>>, value: T) -> bool {
    allowlist
        .as_ref()
        .is_none_or(|values| values.contains(&value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observation_and_action_capabilities_are_independent() {
        let scope = ActorScope {
            interaction_domains: Some(vec![InteractionDomainId(4)]),
            ops: Some(vec![ActorCapability::ObserveInteractionDomain]),
            ask_ops: Some(vec![ActorCapability::InjectInteractionDomainInput]),
            ..ActorScope::default()
        };
        assert_eq!(
            scope.decide_interaction_domain_capability(
                InteractionDomainId(4),
                ActorCapability::ObserveInteractionDomain,
            ),
            AuthorizationDecision::Permit
        );
        assert_eq!(
            scope.decide_interaction_domain_capability(
                InteractionDomainId(4),
                ActorCapability::InjectInteractionDomainInput,
            ),
            AuthorizationDecision::Ask(ActorCapability::InjectInteractionDomainInput)
        );
        assert_eq!(
            scope.decide_interaction_domain_capability(
                InteractionDomainId(9),
                ActorCapability::ObserveInteractionDomain,
            ),
            AuthorizationDecision::Deny
        );
    }

    #[test]
    fn window_capture_requires_window_and_named_operation() {
        let scope = ActorScope {
            windows: Some(vec![WindowId(7)]),
            ops: Some(vec![ActorCapability::CaptureWindow]),
            ..ActorScope::default()
        };
        assert_eq!(
            scope.decide_window_capture(WindowId(7)),
            AuthorizationDecision::Permit
        );
        assert_eq!(
            scope.decide_window_capture(WindowId(8)),
            AuthorizationDecision::Deny
        );

        let asking = ActorScope {
            ask_ops: Some(vec![ActorCapability::CaptureWindow]),
            ..ActorScope::default()
        };
        assert_eq!(
            asking.decide_window_capture(WindowId(7)),
            AuthorizationDecision::Ask(ActorCapability::CaptureWindow)
        );

        let unrelated = ActorScope {
            ops: Some(vec![ActorCapability::CaptureOutput]),
            ..ActorScope::default()
        };
        assert_eq!(
            unrelated.decide_window_capture(WindowId(7)),
            AuthorizationDecision::Deny
        );
    }
}
