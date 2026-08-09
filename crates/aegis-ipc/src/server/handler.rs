use super::*;

/// Live compositor state the IPC serves. The main loop updates the snapshot
/// each frame; connection threads read it. `Send + Sync + 'static` so it can
/// live behind an `Arc` shared across threads.
pub trait Handler: Send + Sync {
    /// ConnectionCapabilities this server grants by policy before per-client
    /// intersection. `query` is added back unconditionally (ADR-0027).
    fn policy_caps(&self) -> ConnectionCapabilities {
        ConnectionCapabilities::QUERY
    }
    /// Snapshot of live toplevels, in z-order — the same `Window` the
    /// renderer and chrome read.
    fn windows(&self) -> Vec<aegis_model::window::Window>;
    /// Process-bound toplevels for the authenticated first-party semantic
    /// provider. The default exposes nothing.
    fn accessibility_windows(&self) -> Vec<aegis_semantic::AccessibilityWindowBinding> {
        Vec::new()
    }
    /// Snapshot of the workspace/output model. Same shape the chrome and the
    /// agent read.
    fn workspaces(&self) -> aegis_model::workspace::WorkspaceSnapshot;
    /// Snapshot of the live notification queue.
    fn notifications(&self) -> Vec<aegis_model::notify::Notification>;
    /// Snapshot of the live outputs (connector + geometry).
    fn outputs(&self) -> Vec<aegis_model::output::OutputInfo>;
    /// Snapshot of journal entries with `seq > since` (ADR-0033).
    fn journal_since(&self, since: u64) -> crate::journal::JournalSnapshot;
    /// Complete Interaction Domain authority snapshot.
    fn interaction_domains(&self) -> aegis_model::interaction_domain::InteractionDomainSnapshot {
        aegis_model::interaction_domain::InteractionDomainModel::new().snapshot()
    }
    /// Compositor-owned persistent settings snapshot.
    fn settings(&self) -> SettingsSnapshot {
        SettingsSnapshot::default()
    }
    /// Live host and compositor-owned session status.
    fn system_status(&self) -> aegis_model::system::SystemStatus {
        aegis_model::system::SystemStatus::default()
    }
    /// Resolve a named scope from configuration (ADR-0034). Returns `None`
    /// if the name is unknown; an explicitly named connection is refused.
    fn resolve_scope(&self, _name: &str) -> Option<Scope> {
        None
    }
    /// Look up a previously paired agent by the credential it presents
    /// (ADR-0088). The default recognizes nothing, so every agenting
    /// connection falls through to [`Handler::pair_agent`].
    fn agent_lookup(&self, _credential: &str) -> Option<AgentIdentity> {
        None
    }
    /// Refresh an authenticated principal's live ceiling for an existing
    /// connection. `Ok(None)` lets simple embedders retain the handshake
    /// snapshot; production registries return `Err` when the principal has
    /// been forgotten and `Ok(Some(_))` otherwise.
    fn refresh_agent_identity(&self, _principal: &str) -> Result<Option<AgentIdentity>, String> {
        Ok(None)
    }
    /// Create the explicit, bounded execution context for this connection.
    /// Production implementations retain it in the authority registry; the
    /// default still returns a correctly bounded session for embedders.
    fn start_actor_session(
        &self,
        conn_id: u64,
        principal: Option<&str>,
        policy: aegis_security::authority::ActorSessionPolicy,
    ) -> Result<aegis_security::authority::ActorSessionSnapshot, String> {
        let principal = principal
            .map(aegis_security::authority::ActorPrincipal::new)
            .transpose()
            .map_err(str::to_owned)?;
        let policy = policy.validate().map_err(str::to_owned)?;
        Ok(aegis_security::authority::ActorSessionSnapshot {
            id: aegis_security::authority::ActorSessionId(conn_id),
            principal,
            connection_id: conn_id,
            state: aegis_security::authority::ActorSessionState::Active,
            ttl_ms: policy.ttl.as_millis() as u64,
            idle_timeout_ms: policy.idle_timeout.as_millis() as u64,
            max_pending_actions: policy.max_pending_actions as u32,
            max_observations: policy.max_observations as u32,
        })
    }
    /// Refresh and authorize one live Actor session before dispatching a
    /// request. Expired, suspended, or revoked sessions fail closed.
    fn authorize_actor_session(
        &self,
        _session: aegis_security::authority::ActorSessionId,
    ) -> Result<(), String> {
        Ok(())
    }
    /// Issue an exact, short-lived resource authority after the protocol
    /// adapter has checked the Actor's capability ceiling. When
    /// `confirm_exact_resource` is true, production implementations must
    /// obtain fresh human consent describing this exact resource.
    fn issue_resource_grant(
        &self,
        _session: aegis_security::authority::ActorSessionId,
        _principal: Option<&str>,
        _resource: aegis_security::authority::ActorResource,
        _ttl: std::time::Duration,
        _uses: u32,
        _confirm_exact_resource: bool,
    ) -> Result<aegis_security::authority::ResourceGrant, String> {
        Err("dynamic resource grants are not supported by this server".into())
    }
    /// Consume one use of an exact resource authority.
    fn consume_resource_grant(
        &self,
        _session: aegis_security::authority::ActorSessionId,
        _principal: Option<&str>,
        _id: &aegis_security::authority::ResourceGrantId,
        _resource: &aegis_security::authority::ActorResource,
    ) -> Result<aegis_security::authority::ResourceGrant, String> {
        Err("dynamic resource grants are not supported by this server".into())
    }
    /// Revoke one resource authority owned by this live Actor.
    fn revoke_resource_grant(
        &self,
        _session: aegis_security::authority::ActorSessionId,
        _principal: Option<&str>,
        _id: &aegis_security::authority::ResourceGrantId,
    ) -> Result<(), String> {
        Err("dynamic resource grants are not supported by this server".into())
    }
    /// Commit a validated accessibility-tree revision on the compositor main
    /// loop. `principal` must be the authenticated provider identity.
    fn publish_accessibility_tree(
        &self,
        _principal: &str,
        _update: aegis_semantic::AccessibilityTreeUpdate,
    ) -> Result<(), String> {
        Err("accessibility-tree publishing is not supported by this server".into())
    }
    /// Long-poll one semantic action routed to this authenticated provider.
    /// `Ok(None)` is a normal timeout, not an error.
    fn next_accessibility_action(
        &self,
        _session: aegis_security::authority::ActorSessionId,
        _principal: &str,
        _timeout: std::time::Duration,
    ) -> Result<Option<aegis_semantic::SemanticActionRequest>, String> {
        Err("accessibility action dispatch is not supported by this server".into())
    }
    fn complete_accessibility_action(
        &self,
        _session: aegis_security::authority::ActorSessionId,
        _principal: &str,
        _request_id: u64,
        _result: Result<(), String>,
    ) -> Result<(), String> {
        Err("accessibility action dispatch is not supported by this server".into())
    }
    /// Interactively pair a capability-borrowing agent: ask the user to
    /// approve the declared ceiling and return the issued principal,
    /// credential, and the approved ceiling split into pregranted and
    /// runtime-gated operations. Called from a connection thread during the
    /// handshake; the implementation forwards to the compositor main loop
    /// and blocks for the answer. The default refuses pairing.
    fn pair_agent(
        &self,
        _conn_id: u64,
        _label: Option<&str>,
        _requested: &[ActorCapability],
    ) -> Result<PairedAgent, String> {
        Err("agent pairing is not supported by this server".into())
    }
    /// Whether to strip privileged capabilities from connections that
    /// neither present a built-in scope nor pair as an agent (`[agent]
    /// lockdown`, ADR-0088). The default `false` keeps the anonymous
    /// owner-tool channel working.
    fn lockdown(&self) -> bool {
        false
    }
    /// Look up the recorded runtime grant for (principal, op) (ADR-0088):
    /// `Some(true)` allows, `Some(false)` refuses without prompting again,
    /// `None` asks the user interactively through
    /// [`Handler::request_grant`].
    fn grant_for(&self, _principal: &str, _op: ActorCapability) -> Option<bool> {
        None
    }
    /// Interactively ask the user to grant `op` to the agent bound to
    /// `principal` (ADR-0088). Called from a connection thread; the
    /// implementation forwards to the compositor main loop and blocks for
    /// the answer. Returns whether the operation may proceed; durable and
    /// session decisions are recorded by the implementation.
    fn request_grant(
        &self,
        _conn_id: u64,
        _principal: &str,
        _op: ActorCapability,
    ) -> Result<bool, String> {
        Err("runtime grants are not supported by this server".into())
    }
    /// Reauthorize one Interaction Domain action whose operation family was approved by a
    /// runtime grant (ADR-0088). The operation allowlist is satisfied by the
    /// grant, so only resource allowlists and implementation-specific checks
    /// apply. Compositors should mirror [`Handler::authorize_interaction_domain_action`]'s
    /// interaction-group expansion here.
    fn authorize_interaction_domain_action_granted(
        &self,
        scope: &Scope,
        action: &InteractionDomainAction,
    ) -> Result<(), String> {
        scope
            .permits_interaction_domain_action_resources(action)
            .then_some(())
            .ok_or_else(|| "out of scope".into())
    }
    /// List paired agent principals (ADR-0088). Default: none.
    fn agent_principals(&self) -> Vec<AgentPrincipalInfo> {
        Vec::new()
    }
    /// List recorded runtime grants, optionally filtered to one principal
    /// (ADR-0088). Default: none.
    fn agent_grants(&self, _principal: Option<&str>) -> Vec<AgentGrantInfo> {
        Vec::new()
    }
    /// Rename a principal's display label (`None` clears it).
    fn rename_agent_principal(&self, _principal: &str, _label: Option<&str>) -> Result<(), String> {
        Err("agent management is not supported by this server".into())
    }
    /// Forget a principal: its credential dies and its grants are dropped.
    fn forget_agent_principal(&self, _principal: &str) -> Result<(), String> {
        Err("agent management is not supported by this server".into())
    }
    /// Replace a principal's approved ceiling.
    fn set_agent_ceiling(
        &self,
        _principal: &str,
        _pregranted: &[ActorCapability],
        _gated: &[ActorCapability],
    ) -> Result<(), String> {
        Err("agent management is not supported by this server".into())
    }
    /// Register a principal ahead of time (administrator pre-provisioning),
    /// returning the issued principal id and credential.
    fn register_agent(
        &self,
        _label: Option<&str>,
        _pregranted: &[ActorCapability],
        _gated: &[ActorCapability],
    ) -> Result<(String, String), String> {
        Err("agent management is not supported by this server".into())
    }
    /// Drop one recorded runtime grant.
    fn revoke_agent_grant(&self, _principal: &str, _op: ActorCapability) -> Result<(), String> {
        Err("agent management is not supported by this server".into())
    }
    /// Reauthorize one Interaction Domain action against live interaction-group state.
    ///
    /// The default performs the schema-level scope check. Compositors should
    /// additionally expand group-level mutations to every affected window so
    /// an allowlisted member cannot smuggle sibling windows across interaction domains.
    fn authorize_interaction_domain_action(
        &self,
        scope: &Scope,
        action: &InteractionDomainAction,
    ) -> Result<(), String> {
        scope
            .permits_interaction_domain_action(action)
            .then_some(())
            .ok_or_else(|| "out of scope".into())
    }
    /// Enforce credential-bound ownership for an Interaction Domain lifecycle action.
    ///
    /// `None` denotes a compositor-local or named built-in component. A
    /// paired subject may create new authority, but every existing non-human
    /// Interaction Domain touched by the action must be controlled by a core principal
    /// carrying that same authenticated subject id.
    fn authorize_agent_interaction_domain_action(
        &self,
        subject: Option<&str>,
        action: &InteractionDomainAction,
    ) -> Result<(), String> {
        let Some(subject) = subject else {
            return Ok(());
        };
        authorize_subject_interaction_domain_action(subject, action, &self.interaction_domains())
    }
    /// Enforce credential-bound ownership for commands that target an Interaction Domain.
    fn authorize_agent_interaction_domain_command(
        &self,
        subject: Option<&str>,
        command: &Command,
    ) -> Result<(), String> {
        let Some(subject) = subject else {
            return Ok(());
        };
        let interaction_domain = match command {
            Command::LaunchInInteractionDomain {
                interaction_domain, ..
            } => *interaction_domain,
            _ => return Ok(()),
        };
        subject_owns_interaction_domain(&self.interaction_domains(), subject, interaction_domain)
            .then_some(())
            .ok_or_else(|| "out of scope: InteractionDomain is owned by another principal".into())
    }
    /// Enforce credential-bound ownership for directed Interaction Domain capture.
    fn authorize_agent_interaction_domain_capture(
        &self,
        subject: Option<&str>,
        interaction_domain: aegis_model::interaction_domain::InteractionDomainId,
    ) -> Result<(), String> {
        let Some(subject) = subject else {
            return Ok(());
        };
        subject_owns_interaction_domain(&self.interaction_domains(), subject, interaction_domain)
            .then_some(())
            .ok_or_else(|| "out of scope: InteractionDomain is owned by another principal".into())
    }
    /// Record a mutation rejected by the IPC capability/scope/lease layer.
    ///
    /// Production implementations persist this before returning the error
    /// response and serialize append plus subscriber broadcast with every
    /// other producer. The default keeps embedders source-compatible without
    /// an audit sink.
    fn audit_refusal(
        &self,
        _conn_id: u64,
        _subject: Option<&str>,
        _mutation: JournalMutation,
        _reason: String,
    ) {
    }
    /// Persist a privacy-minimized decision for a capability endpoint that
    /// has no richer command/action mutation. Production implementations
    /// commit the event before the corresponding response or payload.
    fn audit_capability_use(
        &self,
        _conn_id: u64,
        _subject: Option<&str>,
        _session: aegis_security::authority::ActorSessionId,
        _capability: ActorCapability,
        _action: crate::journal::CapabilityUseAction,
        _effect: crate::journal::Effect,
    ) {
    }
    /// Live lock/VT security gate checked by the writer immediately before it
    /// attaches a capture memfd.
    fn capture_security_active(&self) -> bool {
        true
    }
    /// Receive a control/session [`Command`]. Called from a connection
    /// thread; the implementation must forward to the compositor's main loop
    /// because the Wayland server state is not `Send`. Fire-and-forget: the
    /// caller acknowledges queuing, not completion. [`Command::System`] is
    /// dispatched through [`Handler::system_action`] instead.
    fn command(&self, conn_id: u64, subject: Option<&str>, cmd: Command);
    /// Commit one live-system action on the compositor main thread and return
    /// its authoritative apply result.
    fn system_action(
        &self,
        _conn_id: u64,
        _subject: Option<&str>,
        _action: crate::schema::SystemAction,
    ) -> Result<(), String> {
        Err("system control unsupported".into())
    }
    /// Commit one synchronous Interaction Domain lifecycle request on the compositor main
    /// thread and return its authoritative receipt.
    fn interaction_domain_action(
        &self,
        _conn_id: u64,
        _subject: Option<&str>,
        _action: InteractionDomainAction,
    ) -> Result<InteractionDomainActionResult, String> {
        Err("interaction_domain control unsupported".into())
    }
    /// Persist and apply one settings transaction on the compositor main
    /// thread, returning only after the authoritative state is updated.
    fn settings_action(
        &self,
        _conn_id: u64,
        _subject: Option<&str>,
        _expected_revision: Option<u64>,
        _action: SettingsAction,
    ) -> Result<SettingsReceipt, String> {
        Err("settings control unsupported".into())
    }
    /// Capture the focused output as a PNG. Called from a connection thread;
    /// the implementation forwards to the main loop and blocks briefly for
    /// the reply. The writer transfers the PNG through a sealed memfd after
    /// the small JSON response. `region` is in compositor logical pixels;
    /// `None` captures the whole output.
    fn capture_output(
        &self,
        _region: Option<aegis_model::Rect>,
    ) -> Result<CaptureOutputPayload, String> {
        Err("capture unsupported".into())
    }
    /// Capture one directed virtual output.
    fn capture_interaction_domain(
        &self,
        _conn_id: u64,
        _subject: Option<&str>,
        _interaction_domain: aegis_model::interaction_domain::InteractionDomainId,
        _region: Option<aegis_model::Rect>,
    ) -> Result<CaptureInteractionDomainPayload, String> {
        Err("interaction_domain capture unsupported".into())
    }
    /// Read compositor-owned semantic objects without transferring pixels.
    fn observe_interaction_domain(
        &self,
        _conn_id: u64,
        _subject: Option<&str>,
        _interaction_domain: aegis_model::interaction_domain::InteractionDomainId,
    ) -> Result<SemanticObservation, String> {
        Err("InteractionDomain semantic observation unsupported".into())
    }
    /// Revalidate and commit one observation-bound action on the compositor
    /// main loop. The implementation must consume the observation even when
    /// a precondition fails.
    fn act_in_interaction_domain(
        &self,
        _conn_id: u64,
        _subject: Option<&str>,
        _scope_name: Option<&str>,
        _scope: Scope,
        _intent: ActorActionIntent,
    ) -> Result<ActorActionReceipt, String> {
        Err("observation-bound Interaction Domain actions unsupported".into())
    }
    /// Revoke a syntactically plausible observation token after an action is
    /// refused before main-loop dispatch. Implementations must verify the
    /// connection and Actor binding rather than treating the token as a
    /// globally revocable identifier.
    fn discard_observation(
        &self,
        _conn_id: u64,
        _subject: Option<&str>,
        _token: &crate::schema::ObservationToken,
    ) {
    }
    /// Notification that an IPC connection ended. Implementations use this
    /// to revoke connection-bound Actor context such as outstanding
    /// observation leases. It is invoked for every connection after the
    /// reader exits, regardless of whether the connection owned a stream or
    /// idle inhibitor.
    fn connection_disconnected(&self, _conn_id: u64) {}
    /// Start a continuous frame stream of the focused output (ADR-0052).
    /// Called from a connection thread after capability, lease, and scope
    /// checks; the implementation forwards to the main loop and blocks
    /// briefly for the reply. Frames are pushed back through
    /// [`Server::push_stream_frame`]. `target` (version 6) selects the whole
    /// output or one window's visible region (ADR-0054); an unknown window
    /// id is an error. `allow_dmabuf` (version 25) is the client's explicit
    /// zero-copy opt-in: only then may the reply announce
    /// [`StreamPixelFormat::Dmabuf`] and carry a slot table; any reason the
    /// transport is unavailable falls back to SHM pixels instead of failing.
    fn stream_output_start(
        &self,
        _conn_id: u64,
        _max_fps: Option<u32>,
        _target: crate::schema::StreamTarget,
        _allow_dmabuf: bool,
    ) -> Result<StreamInfo, String> {
        Err("streaming unsupported".into())
    }
    /// Notification that a stream was torn down — a `StreamOutputStop`
    /// request, a per-frame authorization failure in the writer, or a
    /// disconnect. Fire-and-forget; the main loop drops its stream state.
    fn stream_output_stop(&self, _stream_id: u64) {}
    /// The consumer finished reading a dmabuf stream's slot (version 25);
    /// the compositor may reuse it for a later frame. Fire-and-forget, and
    /// only ever sent for a stream the releasing connection owns.
    fn stream_buffer_release(&self, _stream_id: u64, _slot: u32) {}
    /// Notification that a connection owning one or more streams
    /// disconnected. The server has already unregistered the delivery lanes;
    /// the main loop drops its stream state.
    fn streams_disconnected(&self, _conn_id: u64) {}
    /// Set or clear the calling connection's global idle inhibitor
    /// (ADR-0075). Called from a connection thread after capability, lease,
    /// and scope checks; the implementation forwards to the main loop and
    /// blocks briefly for the reply, which carries the inhibitor state the
    /// connection now holds.
    fn set_idle_inhibit(&self, _conn_id: u64, _inhibit: bool) -> Result<bool, String> {
        Err("idle inhibit unsupported".into())
    }
    /// Notification that a connection holding an idle inhibitor
    /// disconnected. The server releases the inhibitor; the main loop drops
    /// its per-connection state.
    fn idle_inhibit_disconnected(&self, _conn_id: u64) {}
    /// Run one user-consent interactive pick (ADR-0054). Called from a
    /// connection thread after capability, lease, scope, and lock/VT-gate
    /// checks; the implementation forwards to the compositor main loop,
    /// which freezes the screen, opens the matching selector chrome, and
    /// answers when the user confirms or cancels. The reply may therefore
    /// block for as long as the user takes, bounded by the compositor's
    /// interaction timeout.
    fn pick_target(
        &self,
        _conn_id: u64,
        _kind: crate::schema::PickKind,
    ) -> Result<crate::schema::PickResult, String> {
        Err("interactive picking unsupported".into())
    }
    /// Run one user-consent application pick (the AppChooser portal's
    /// compositor side). It is gated by a live lease and an explicit scope
    /// operation, and may block until user interaction completes.
    fn pick_app(
        &self,
        _conn_id: u64,
        _choices: Vec<String>,
        _subject: Option<String>,
        _last_choice: Option<String>,
    ) -> Result<crate::schema::AppPickResult, String> {
        Err("application picking unsupported".into())
    }
    /// Run one user-consent secret prompt (the secret vault's password
    /// unlock). The typed secret crosses this channel and the implementation
    /// must zeroize its copy once answered.
    fn prompt_secret(
        &self,
        _conn_id: u64,
        _title: String,
        _reason: Option<String>,
    ) -> Result<crate::schema::SecretPromptResult, String> {
        Err("secret prompting unsupported".into())
    }
    /// Run one user-consent yes/no confirmation (portal consent dialogs).
    /// Uses the same lease, scope, and interaction-timeout discipline as the
    /// other compositor-owned prompts.
    fn pick_confirm(
        &self,
        _conn_id: u64,
        _title: String,
        _body: String,
        _accept_label: Option<String>,
    ) -> Result<crate::schema::ConfirmPickResult, String> {
        Err("confirmation prompting unsupported".into())
    }
    /// Replace the desktop wallpaper (the Wallpaper portal). Called from a
    /// connection thread after the mutation gate; the implementation
    /// forwards to the compositor main loop and returns its authoritative
    /// decode-and-swap receipt.
    fn set_wallpaper(&self, _conn_id: u64, _path: std::path::PathBuf) -> Result<(), String> {
        Err("wallpaper setting unsupported".into())
    }
}
