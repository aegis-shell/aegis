//! The reference client for the aegis IPC.
//!
//! A thin synchronous client over a blocking unix stream. Power tools and
//! the agent layer build on the same schema; this is the canonical path for
//! "connect, read some state" in one process. See ADR-0027.

use std::io;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use crate::codec::{read_msg, write_msg};
use crate::journal::JournalSnapshot;
use crate::schema::{
    ActorActionIntent, ActorActionReceipt, ActorCapability, ActorResource, AgentGrantInfo,
    AgentHello, AgentIssued, AgentPrincipalInfo, AppPickResult, Command, ConfirmPickResult,
    ConnectionCapabilities, Event, InteractionDomainAction, InteractionDomainActionResult,
    LeaseGrant, LeaseRequest, ObservationToken, ObserveSnapshot, PROTOCOL_VERSION, PickKind,
    PickResult, Request, ResourceGrant, ResourceGrantId, Response, Scope, SecretPromptResult,
    SemanticObservation, SettingsAction, SettingsReceipt, SettingsSnapshot, StreamPixelFormat,
    StreamTarget, SystemAction, SystemStatus, TransactOp, TransactResult,
};

/// Decoded Interaction Domain observation returned by [`Client::capture_interaction_domain`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedInteractionDomain {
    pub interaction_domain: aegis_model::interaction_domain::InteractionDomainId,
    pub width: u32,
    pub height: u32,
    pub scale_milli: u32,
    pub region: aegis_model::Rect,
    pub placements: Vec<aegis_model::interaction_domain::InteractionDomainWindowPlacement>,
    pub observation: SemanticObservation,
    pub png: Vec<u8>,
    pub revision: u64,
}

/// Decoded window capture returned by [`Client::capture_window`] (protocol
/// 26). `rect` is the toplevel's logical rectangle at capture time; the
/// image's origin is the toplevel's origin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedWindow {
    pub window: aegis_model::window::WindowId,
    pub width: u32,
    pub height: u32,
    pub scale_milli: u32,
    pub rect: aegis_model::Rect,
    pub png: Vec<u8>,
}

/// Negotiated stream parameters returned by [`Client::start_output_stream`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamStarted {
    pub stream_id: u64,
    pub width: u32,
    pub height: u32,
    pub format: StreamPixelFormat,
}

/// One decoded output frame from [`Client::next_stream_message`]. `pixels`
/// are `height` tightly packed rows of `stride` bytes in `format` byte
/// order (ADR-0052).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamFrame {
    pub stream_id: u64,
    pub sequence: u64,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub format: StreamPixelFormat,
    pub damage: Vec<aegis_model::Rect>,
    pub dropped: u64,
    pub pixels: Vec<u8>,
}

/// A message arriving on a streaming connection after
/// [`Client::start_output_stream`]. Responses to write-only requests the
/// client issued itself (lease renewal) are surfaced as
/// [`StreamMessage::LeaseRenewed`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamMessage {
    Frame(StreamFrame),
    Ended { stream_id: u64, reason: String },
    LeaseRenewed,
}

/// A connected IPC client. The handshake is complete on construction; the
/// granted capabilities are available via [`Client::caps`].
pub struct Client {
    stream: UnixStream,
    caps: ConnectionCapabilities,
    scope: Scope,
    lease: Option<LeaseGrant>,
    session: Option<crate::ActorSessionSnapshot>,
    agent: Option<AgentIssued>,
}

impl Client {
    /// Connect requesting `query` only.
    pub fn connect(path: &Path) -> io::Result<Client> {
        Self::connect_inner(path, ConnectionCapabilities::QUERY, None)
    }

    /// Connect requesting a specific capability set. The server may grant a
    /// subset (intersected with its policy, with `query` forced on).
    pub fn connect_with(path: &Path, requested: ConnectionCapabilities) -> io::Result<Client> {
        Self::connect_inner(path, requested, None)
    }

    /// Connect with explicit capabilities and bound the handshake itself.
    /// Use this from GUI/background workers so an accepted but unresponsive
    /// local peer cannot retain the worker indefinitely.
    pub fn connect_with_timeout(
        path: &Path,
        requested: ConnectionCapabilities,
        timeout: Duration,
    ) -> io::Result<Client> {
        Self::connect_inner_with_timeout(path, requested, None, None, Some(timeout))
    }

    /// Connect requesting capabilities under a named, compositor-configured
    /// scope. An unknown scope is refused during the handshake instead of
    /// silently granting an unrestricted connection.
    pub fn connect_scoped(
        path: &Path,
        requested: ConnectionCapabilities,
        scope: impl Into<String>,
    ) -> io::Result<Client> {
        Self::connect_inner(path, requested, Some(scope.into()))
    }

    /// Connect under a named scope and apply a timeout before the handshake.
    /// This is the safe entry point for async adapters that execute the
    /// blocking client on a worker thread.
    pub fn connect_scoped_with_timeout(
        path: &Path,
        requested: ConnectionCapabilities,
        scope: impl Into<String>,
        timeout: Duration,
    ) -> io::Result<Client> {
        Self::connect_inner_with_timeout(path, requested, Some(scope.into()), None, Some(timeout))
    }

    /// Connect as a capability-borrowing agent (ADR-0088): present the
    /// self-declaration (display label, requested operation families, and
    /// the credential from an earlier pairing when held), optionally under a
    /// named built-in scope. The handshake may block on the interactive
    /// pairing prompt, so `timeout` should leave the user time to answer.
    /// When the server issues a new credential it is available through
    /// [`Client::agent_issued`].
    pub fn connect_agent_with_timeout(
        path: &Path,
        requested: ConnectionCapabilities,
        scope: Option<String>,
        agent: AgentHello,
        timeout: Duration,
    ) -> io::Result<Client> {
        Self::connect_inner_with_timeout(path, requested, scope, Some(agent), Some(timeout))
    }

    fn connect_inner(
        path: &Path,
        requested: ConnectionCapabilities,
        scope_name: Option<String>,
    ) -> io::Result<Client> {
        Self::connect_inner_with_timeout(path, requested, scope_name, None, None)
    }

    fn connect_inner_with_timeout(
        path: &Path,
        requested: ConnectionCapabilities,
        scope_name: Option<String>,
        agent: Option<AgentHello>,
        timeout: Option<Duration>,
    ) -> io::Result<Client> {
        let mut stream = UnixStream::connect(path)?;
        stream.set_read_timeout(timeout)?;
        stream.set_write_timeout(timeout)?;
        write_msg(
            &mut stream,
            &Request::Hello {
                version: PROTOCOL_VERSION,
                caps: requested,
                scope: scope_name,
                lease: requested.privileged().then(LeaseRequest::default),
                agent,
            },
        )?;
        let resp: Response = read_msg(&mut stream)?;
        let (caps, scope, lease, session, agent) = match resp {
            Response::Hello {
                version,
                caps,
                scope,
                lease,
                session,
                agent,
            } if version == PROTOCOL_VERSION => (caps, scope, lease, session, agent),
            Response::Error { message } => {
                return Err(io::Error::new(io::ErrorKind::ConnectionRefused, message));
            }
            other => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("expected Hello, got {other:?}"),
                ));
            }
        };
        Ok(Client {
            stream,
            caps,
            scope,
            lease,
            session,
            agent,
        })
    }

    /// The capabilities the server actually granted at the handshake.
    pub fn caps(&self) -> ConnectionCapabilities {
        self.caps
    }

    /// The resource/operation scope granted by the compositor at handshake.
    pub fn scope(&self) -> &Scope {
        &self.scope
    }

    pub fn lease(&self) -> Option<LeaseGrant> {
        self.lease
    }

    pub fn session(&self) -> Option<&crate::ActorSessionSnapshot> {
        self.session.as_ref()
    }

    /// The pairing outcome when this client connected with an `AgentHello`
    /// (ADR-0088). A present `credential` was newly issued and must be
    /// persisted by the caller.
    pub fn agent_issued(&self) -> Option<&AgentIssued> {
        self.agent.as_ref()
    }
    /// Bound blocking reads and writes on this connection.
    ///
    /// The reference client is intentionally synchronous. Async adapters
    /// should execute it on a blocking worker and set an I/O timeout so a
    /// stalled peer cannot retain that worker indefinitely.
    pub fn set_io_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.stream.set_read_timeout(timeout)?;
        self.stream.set_write_timeout(timeout)
    }

    pub fn renew_lease(&mut self, ttl_ms: u64) -> io::Result<LeaseGrant> {
        write_msg(&mut self.stream, &Request::RenewLease { ttl_ms })?;
        match read_msg::<_, Response>(&mut self.stream)? {
            Response::LeaseRenewed { lease } => {
                self.lease = Some(lease);
                Ok(lease)
            }
            Response::Error { message } => Err(io::Error::other(message)),
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("expected LeaseRenewed, got {other:?}"),
            )),
        }
    }

    /// List paired agent principals (ADR-0088).
    pub fn agent_principals(&mut self) -> io::Result<Vec<AgentPrincipalInfo>> {
        write_msg(&mut self.stream, &Request::GetAgentPrincipals)?;
        match read_msg::<_, Response>(&mut self.stream)? {
            Response::AgentPrincipals { principals } => Ok(principals),
            Response::Error { message } => Err(io::Error::other(message)),
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("expected AgentPrincipals, got {other:?}"),
            )),
        }
    }

    /// List recorded runtime grants, optionally filtered to one principal
    /// (ADR-0088).
    pub fn agent_grants(&mut self, principal: Option<&str>) -> io::Result<Vec<AgentGrantInfo>> {
        write_msg(
            &mut self.stream,
            &Request::GetAgentGrants {
                principal: principal.map(str::to_owned),
            },
        )?;
        match read_msg::<_, Response>(&mut self.stream)? {
            Response::AgentGrants { grants } => Ok(grants),
            Response::Error { message } => Err(io::Error::other(message)),
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("expected AgentGrants, got {other:?}"),
            )),
        }
    }

    /// Rename a principal's display label (`None` clears it).
    pub fn rename_agent_principal(
        &mut self,
        principal: &str,
        label: Option<&str>,
    ) -> io::Result<()> {
        write_msg(
            &mut self.stream,
            &Request::RenameAgentPrincipal {
                principal: principal.to_owned(),
                label: label.map(str::to_owned),
            },
        )?;
        match read_msg::<_, Response>(&mut self.stream)? {
            Response::Ok => Ok(()),
            Response::Error { message } => Err(io::Error::other(message)),
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("expected Ok, got {other:?}"),
            )),
        }
    }

    /// Forget a principal: its credential dies and its grants are dropped.
    pub fn forget_agent_principal(&mut self, principal: &str) -> io::Result<()> {
        write_msg(
            &mut self.stream,
            &Request::ForgetAgentPrincipal {
                principal: principal.to_owned(),
            },
        )?;
        match read_msg::<_, Response>(&mut self.stream)? {
            Response::Ok => Ok(()),
            Response::Error { message } => Err(io::Error::other(message)),
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("expected Ok, got {other:?}"),
            )),
        }
    }

    /// Replace a principal's approved ceiling.
    pub fn set_agent_ceiling(
        &mut self,
        principal: &str,
        pregranted: Vec<ActorCapability>,
        gated: Vec<ActorCapability>,
    ) -> io::Result<()> {
        write_msg(
            &mut self.stream,
            &Request::SetAgentCeiling {
                principal: principal.to_owned(),
                pregranted,
                gated,
            },
        )?;
        match read_msg::<_, Response>(&mut self.stream)? {
            Response::Ok => Ok(()),
            Response::Error { message } => Err(io::Error::other(message)),
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("expected Ok, got {other:?}"),
            )),
        }
    }

    /// Register a principal ahead of time (administrator pre-provisioning),
    /// returning the issued principal id and credential to plant in the
    /// agent's identity store.
    pub fn register_agent(
        &mut self,
        label: Option<&str>,
        pregranted: Vec<ActorCapability>,
        gated: Vec<ActorCapability>,
    ) -> io::Result<(String, String)> {
        write_msg(
            &mut self.stream,
            &Request::RegisterAgent {
                label: label.map(str::to_owned),
                pregranted,
                gated,
            },
        )?;
        match read_msg::<_, Response>(&mut self.stream)? {
            Response::AgentRegistered {
                principal,
                credential,
            } => Ok((principal, credential)),
            Response::Error { message } => Err(io::Error::other(message)),
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("expected AgentRegistered, got {other:?}"),
            )),
        }
    }

    /// Drop one recorded runtime grant.
    pub fn revoke_agent_grant(&mut self, principal: &str, op: ActorCapability) -> io::Result<()> {
        write_msg(
            &mut self.stream,
            &Request::RevokeAgentGrant {
                principal: principal.to_owned(),
                op,
            },
        )?;
        match read_msg::<_, Response>(&mut self.stream)? {
            Response::Ok => Ok(()),
            Response::Error { message } => Err(io::Error::other(message)),
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("expected Ok, got {other:?}"),
            )),
        }
    }

    /// Fetch the live toplevel snapshot.
    pub fn windows(&mut self) -> io::Result<Vec<aegis_model::window::Window>> {
        write_msg(&mut self.stream, &Request::GetWindows)?;
        match read_msg::<_, Response>(&mut self.stream)? {
            Response::Windows { windows } => Ok(windows),
            Response::Error { message } => Err(io::Error::other(message)),
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("expected Windows, got {other:?}"),
            )),
        }
    }

    /// Fetch the live workspace/output snapshot.
    pub fn workspaces(&mut self) -> io::Result<aegis_model::workspace::WorkspaceSnapshot> {
        write_msg(&mut self.stream, &Request::GetWorkspaces)?;
        match read_msg::<_, Response>(&mut self.stream)? {
            Response::Workspaces { snapshot } => Ok(snapshot),
            Response::Error { message } => Err(io::Error::other(message)),
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("expected Workspaces, got {other:?}"),
            )),
        }
    }

    /// Switch to an adjacent workspace on the focused output.
    pub fn switch_workspace(&mut self, dir: aegis_model::workspace::Switch) -> io::Result<()> {
        self.command(Command::SwitchWorkspace { dir })
    }

    /// Switch directly to a workspace by id.
    pub fn switch_workspace_to(
        &mut self,
        id: aegis_model::workspace::WorkspaceId,
    ) -> io::Result<()> {
        self.command(Command::SwitchWorkspaceTo { id })
    }

    /// Toggle the current workspace between tiled and floating (ADR-0024).
    pub fn toggle_tiling(&mut self) -> io::Result<()> {
        self.command(Command::ToggleTiling)
    }

    /// Set a floating toplevel's geometry in compositor logical coordinates.
    pub fn set_window_geometry(
        &mut self,
        id: aegis_model::window::WindowId,
        rect: aegis_model::Rect,
    ) -> io::Result<()> {
        self.command(Command::SetWindowGeometry { id, rect })
    }

    /// Inject bounded, target-local actions into a toplevel. The connection
    /// must have negotiated the `input` capability under a named scope.
    pub fn inject_input(
        &mut self,
        id: aegis_model::window::WindowId,
        actions: Vec<aegis_model::input::SyntheticInputAction>,
    ) -> io::Result<()> {
        self.command(Command::InjectInput { id, actions })
    }

    pub fn inject_interaction_domain_input(
        &mut self,
        interaction_domain: aegis_model::interaction_domain::InteractionDomainId,
        target: aegis_model::semantic::SemanticObjectId,
        observation: ObservationToken,
        actions: Vec<aegis_model::input::SyntheticInputAction>,
    ) -> io::Result<ActorActionReceipt> {
        self.act_in_interaction_domain(ActorActionIntent {
            interaction_domain,
            target,
            observation,
            actions: vec![aegis_model::semantic::SemanticActionIntent::SyntheticInput { actions }],
        })
    }

    pub fn launch_in_interaction_domain(
        &mut self,
        interaction_domain: aegis_model::interaction_domain::InteractionDomainId,
        desktop_id: impl Into<String>,
    ) -> io::Result<()> {
        self.command(Command::LaunchInInteractionDomain {
            interaction_domain,
            desktop_id: desktop_id.into(),
        })
    }

    /// Post a notification.
    pub fn notify(
        &mut self,
        summary: impl Into<String>,
        body: impl Into<String>,
        app_id: Option<String>,
    ) -> io::Result<()> {
        self.command(Command::Notify {
            summary: summary.into(),
            body: body.into(),
            app_id,
            external_id: None,
        })
    }

    /// Post a notification carrying the sender's own external id (the
    /// Notification portal's per-application id), so a later withdrawal can
    /// be matched by `(app_id, external_id)`.
    pub fn notify_external(
        &mut self,
        summary: impl Into<String>,
        body: impl Into<String>,
        app_id: Option<String>,
        external_id: Option<String>,
    ) -> io::Result<()> {
        self.command(Command::Notify {
            summary: summary.into(),
            body: body.into(),
            app_id,
            external_id,
        })
    }

    /// Dismiss a notification by id.
    pub fn dismiss_notification(&mut self, id: u64) -> io::Result<()> {
        self.command(Command::DismissNotification { id })
    }

    /// Capture the focused output and have the compositor write it as a PNG
    /// file (M9 screenshot path). Queued like every other command; the file
    /// appears once the main loop applies it.
    pub fn screenshot(&mut self, path: impl Into<String>) -> io::Result<()> {
        self.screenshot_region(path, None)
    }

    /// Capture a region of the focused output and have the compositor write it
    /// as a PNG file. `region` is in compositor logical pixels.
    pub fn screenshot_region(
        &mut self,
        path: impl Into<String>,
        region: Option<aegis_model::Rect>,
    ) -> io::Result<()> {
        self.command(Command::Screenshot {
            path: path.into(),
            region,
        })
    }

    /// Capture the focused output as a PNG, returning `(width, height, png
    /// bytes)` (M10 pixel capture). Requires the `control` capability and an
    /// explicit `CaptureOutput` op in the connection's scope.
    pub fn capture_output(&mut self) -> io::Result<(u32, u32, Vec<u8>)> {
        self.capture_output_region(None)
    }

    /// Capture a region of the focused output as a PNG, returning `(width,
    /// height, png bytes)`. `region` is in compositor logical pixels.
    pub fn capture_output_region(
        &mut self,
        region: Option<aegis_model::Rect>,
    ) -> io::Result<(u32, u32, Vec<u8>)> {
        write_msg(&mut self.stream, &Request::CaptureOutput { region })?;
        match read_msg::<_, Response>(&mut self.stream)? {
            Response::CaptureOutput {
                width,
                height,
                png_bytes,
            } => Ok((
                width,
                height,
                crate::blob::receive(&self.stream, png_bytes)?,
            )),
            Response::Error { message } => Err(io::Error::other(message)),
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("expected CaptureOutput, got {other:?}"),
            )),
        }
    }

    /// Fetch the live notification queue.
    pub fn notifications(&mut self) -> io::Result<Vec<aegis_model::notify::Notification>> {
        write_msg(&mut self.stream, &Request::GetNotifications)?;
        match read_msg::<_, Response>(&mut self.stream)? {
            Response::Notifications { notifications } => Ok(notifications),
            Response::Error { message } => Err(io::Error::other(message)),
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("expected Notifications, got {other:?}"),
            )),
        }
    }

    /// Fetch the live outputs (connector + geometry).
    pub fn outputs(&mut self) -> io::Result<Vec<aegis_model::output::OutputInfo>> {
        write_msg(&mut self.stream, &Request::GetOutputs)?;
        match read_msg::<_, Response>(&mut self.stream)? {
            Response::Outputs { outputs } => Ok(outputs),
            Response::Error { message } => Err(io::Error::other(message)),
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("expected Outputs, got {other:?}"),
            )),
        }
    }

    /// Fetch mutation-journal entries whose sequence is greater than `since`.
    pub fn journal(&mut self, since: u64) -> io::Result<JournalSnapshot> {
        write_msg(&mut self.stream, &Request::GetJournal { since })?;
        match read_msg::<_, Response>(&mut self.stream)? {
            Response::Journal { snapshot } => Ok(snapshot),
            Response::Error { message } => Err(io::Error::other(message)),
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("expected Journal, got {other:?}"),
            )),
        }
    }

    pub fn interaction_domains(
        &mut self,
    ) -> io::Result<aegis_model::interaction_domain::InteractionDomainSnapshot> {
        write_msg(&mut self.stream, &Request::GetInteractionDomains)?;
        match read_msg::<_, Response>(&mut self.stream)? {
            Response::InteractionDomains { snapshot } => Ok(snapshot),
            Response::Error { message } => Err(io::Error::other(message)),
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("expected InteractionDomains, got {other:?}"),
            )),
        }
    }

    /// Fetch the revisioned compositor-settings snapshot.
    pub fn settings(&mut self) -> io::Result<SettingsSnapshot> {
        write_msg(&mut self.stream, &Request::GetSettings)?;
        match read_msg::<_, Response>(&mut self.stream)? {
            Response::Settings { snapshot } => Ok(snapshot),
            Response::Error { message } => Err(io::Error::other(message)),
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("expected Settings, got {other:?}"),
            )),
        }
    }

    /// Fetch the live host and compositor-owned session status.
    pub fn system_status(&mut self) -> io::Result<SystemStatus> {
        write_msg(&mut self.stream, &Request::GetSystemStatus)?;
        match read_msg::<_, Response>(&mut self.stream)? {
            Response::SystemStatus { snapshot } => Ok(snapshot),
            Response::Error { message } => Err(io::Error::other(message)),
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("expected SystemStatus, got {other:?}"),
            )),
        }
    }

    /// Apply one live-system control and return only after the compositor main
    /// loop reports the authoritative result.
    pub fn apply_system_action(&mut self, action: SystemAction) -> io::Result<()> {
        self.command(Command::System { action })
    }

    /// Persist and apply a compositor setting, returning only after the main
    /// loop confirms the new revision.
    pub fn apply_settings(
        &mut self,
        expected_revision: Option<u64>,
        action: SettingsAction,
    ) -> io::Result<SettingsReceipt> {
        write_msg(
            &mut self.stream,
            &Request::Settings {
                expected_revision,
                action,
            },
        )?;
        match read_msg::<_, Response>(&mut self.stream)? {
            Response::SettingsApplied { receipt } => Ok(receipt),
            Response::Error { message } => Err(io::Error::other(message)),
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("expected SettingsApplied, got {other:?}"),
            )),
        }
    }

    pub fn interaction_domain_action(
        &mut self,
        action: InteractionDomainAction,
    ) -> io::Result<InteractionDomainActionResult> {
        write_msg(&mut self.stream, &Request::InteractionDomain { action })?;
        match read_msg::<_, Response>(&mut self.stream)? {
            Response::InteractionDomain { result } => Ok(result),
            Response::Error { message } => Err(io::Error::other(message)),
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("expected InteractionDomain, got {other:?}"),
            )),
        }
    }

    /// Read a consistent multi-class snapshot with the journal cursor in one
    /// round trip (protocol 28, ADR-0125; the Observe primitive).
    pub fn observe(&mut self) -> io::Result<ObserveSnapshot> {
        write_msg(&mut self.stream, &Request::Observe)?;
        match read_msg::<_, Response>(&mut self.stream)? {
            Response::Observed { snapshot } => Ok(snapshot),
            Response::Error { message } => Err(io::Error::other(message)),
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("expected Observed, got {other:?}"),
            )),
        }
    }

    /// Atomically authorize and apply an ordered op batch, returning the
    /// main loop's authoritative per-op receipt or a precondition conflict
    /// (protocol 28, ADR-0125; the Transact primitive).
    pub fn transact(
        &mut self,
        expected_journal_seq: Option<u64>,
        ops: Vec<TransactOp>,
    ) -> io::Result<TransactResult> {
        write_msg(
            &mut self.stream,
            &Request::Transact {
                expected_journal_seq,
                ops,
            },
        )?;
        match read_msg::<_, Response>(&mut self.stream)? {
            Response::Transact { result } => Ok(result),
            Response::Error { message } => Err(io::Error::other(message)),
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("expected Transact, got {other:?}"),
            )),
        }
    }

    pub fn observe_interaction_domain(
        &mut self,
        interaction_domain: aegis_model::interaction_domain::InteractionDomainId,
    ) -> io::Result<SemanticObservation> {
        write_msg(
            &mut self.stream,
            &Request::ObserveInteractionDomain { interaction_domain },
        )?;
        match read_msg::<_, Response>(&mut self.stream)? {
            Response::InteractionDomainObserved { observation }
                if observation.snapshot.interaction_domain == interaction_domain =>
            {
                Ok(observation)
            }
            Response::Error { message } => Err(io::Error::other(message)),
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("expected InteractionDomainObserved, got {other:?}"),
            )),
        }
    }

    pub fn act_in_interaction_domain(
        &mut self,
        intent: ActorActionIntent,
    ) -> io::Result<ActorActionReceipt> {
        write_msg(
            &mut self.stream,
            &Request::ActInInteractionDomain { intent },
        )?;
        match read_msg::<_, Response>(&mut self.stream)? {
            Response::ActorActionCommitted { receipt } => Ok(receipt),
            Response::Error { message } => Err(io::Error::other(message)),
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("expected ActorActionCommitted, got {other:?}"),
            )),
        }
    }

    pub fn capture_interaction_domain(
        &mut self,
        interaction_domain: aegis_model::interaction_domain::InteractionDomainId,
        region: Option<aegis_model::Rect>,
    ) -> io::Result<CapturedInteractionDomain> {
        write_msg(
            &mut self.stream,
            &Request::CaptureInteractionDomain {
                interaction_domain,
                region,
            },
        )?;
        match read_msg::<_, Response>(&mut self.stream)? {
            Response::CaptureInteractionDomain { capture }
                if capture.interaction_domain == interaction_domain =>
            {
                let png = crate::blob::receive(&self.stream, capture.png_bytes)?;
                Ok(CapturedInteractionDomain {
                    interaction_domain: capture.interaction_domain,
                    width: capture.width,
                    height: capture.height,
                    scale_milli: capture.scale_milli,
                    region: capture.region,
                    placements: capture.placements,
                    observation: capture.observation,
                    png,
                    revision: capture.revision,
                })
            }
            Response::Error { message } => Err(io::Error::other(message)),
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("expected CaptureInteractionDomain, got {other:?}"),
            )),
        }
    }

    /// Capture one window's real content as a PNG (protocol 26). The window
    /// is rendered offscreen, so it captures whether visible, occluded,
    /// minimized, or on another workspace. Requires the `control` capability
    /// and an explicit `CaptureWindow` scope decision for this window.
    pub fn capture_window(
        &mut self,
        window: aegis_model::window::WindowId,
    ) -> io::Result<CapturedWindow> {
        write_msg(&mut self.stream, &Request::CaptureWindow { window })?;
        match read_msg::<_, Response>(&mut self.stream)? {
            Response::CaptureWindow { capture } if capture.window == window => {
                let png = crate::blob::receive(&self.stream, capture.png_bytes)?;
                Ok(CapturedWindow {
                    window: capture.window,
                    width: capture.width,
                    height: capture.height,
                    scale_milli: capture.scale_milli,
                    rect: capture.rect,
                    png,
                })
            }
            Response::CaptureWindow { capture } => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "expected CaptureWindow for window {}, got window {}",
                    window.0, capture.window.0
                ),
            )),
            Response::Error { message } => Err(io::Error::other(message)),
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("expected CaptureWindow, got {other:?}"),
            )),
        }
    }

    /// Start a continuous frame stream of the focused output (ADR-0052).
    /// Requires `control` and an explicit `StreamOutput` op in the
    /// connection's scope. Frames arrive through
    /// [`Client::next_stream_message`]; stop with
    /// [`Client::stop_output_stream`] or by dropping the connection.
    pub fn start_output_stream(&mut self, max_fps: Option<u32>) -> io::Result<StreamStarted> {
        self.start_output_stream_target(max_fps, StreamTarget::Output)
    }

    /// Start a continuous frame stream with an explicit target (ADR-0054):
    /// the whole output, or one window's visible region cropped from the
    /// output frame. Window ids come from [`Client::pick_target`]; the
    /// compositor ends the stream when the window closes or its size
    /// changes.
    pub fn start_output_stream_target(
        &mut self,
        max_fps: Option<u32>,
        target: StreamTarget,
    ) -> io::Result<StreamStarted> {
        write_msg(
            &mut self.stream,
            &Request::StreamOutputStart {
                max_fps,
                target,
                dmabuf: None,
            },
        )?;
        match read_msg::<_, Response>(&mut self.stream)? {
            Response::StreamOutputStarted {
                stream_id,
                width,
                height,
                format,
                ..
            } => Ok(StreamStarted {
                stream_id,
                width,
                height,
                format,
            }),
            Response::Error { message } => Err(io::Error::other(message)),
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("expected StreamOutputStarted, got {other:?}"),
            )),
        }
    }

    /// Ask the user to interactively pick a screen target through
    /// compositor chrome (ADR-0054). Blocks until the user confirms or
    /// cancels (or the compositor's interaction timeout elapses), so this
    /// can take arbitrarily longer than any other request. Requires
    /// `control` and an explicit `PickTarget` op in the connection's scope.
    pub fn pick_target(&mut self, kind: PickKind) -> io::Result<PickResult> {
        write_msg(&mut self.stream, &Request::PickTarget { kind })?;
        match read_msg::<_, Response>(&mut self.stream)? {
            Response::Picked { result } => Ok(result),
            Response::Error { message } => Err(io::Error::other(message)),
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("expected Picked, got {other:?}"),
            )),
        }
    }

    /// Ask the user to choose one application out of `choices` through
    /// compositor chrome (the AppChooser portal's compositor side). Same
    /// blocking discipline as [`Client::pick_target`]. Requires `control`
    /// and an explicit `PickApp` op in the connection's scope.
    pub fn pick_app(
        &mut self,
        choices: Vec<String>,
        subject: Option<String>,
        last_choice: Option<String>,
    ) -> io::Result<AppPickResult> {
        write_msg(
            &mut self.stream,
            &Request::PickApp {
                choices,
                subject,
                last_choice,
            },
        )?;
        match read_msg::<_, Response>(&mut self.stream)? {
            Response::AppPicked { result } => Ok(result),
            Response::Error { message } => Err(io::Error::other(message)),
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("expected AppPicked, got {other:?}"),
            )),
        }
    }

    /// Ask the user for a secret (password, PIN, …) through a masked
    /// compositor prompt (the secret vault's password unlock). Same blocking
    /// discipline as [`Client::pick_target`]. Requires `control` and an
    /// explicit `PromptSecret` op in the connection's scope. Zeroize the
    /// returned value after use.
    pub fn prompt_secret(
        &mut self,
        title: String,
        reason: Option<String>,
    ) -> io::Result<SecretPromptResult> {
        let resource = ActorResource::secret_prompt(&title, reason.as_deref());
        let grant = self.request_resource_grant(resource, Duration::from_secs(5 * 60), 1)?;
        write_msg(
            &mut self.stream,
            &Request::PromptSecret {
                resource_grant: grant.id,
                title,
                reason,
            },
        )?;
        match read_msg::<_, Response>(&mut self.stream)? {
            Response::SecretPrompted { result } => Ok(result),
            Response::Error { message } => Err(io::Error::other(message)),
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("expected SecretPrompted, got {other:?}"),
            )),
        }
    }

    /// Request a short-lived, exact resource handle bound to this Actor
    /// session. Filesystem, network, secret, and payment authorities use this
    /// path rather than ambient booleans.
    pub fn request_resource_grant(
        &mut self,
        resource: ActorResource,
        ttl: Duration,
        uses: u32,
    ) -> io::Result<ResourceGrant> {
        let ttl_ms = u64::try_from(ttl.as_millis()).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidInput, "resource ttl is too large")
        })?;
        write_msg(
            &mut self.stream,
            &Request::RequestResourceGrant {
                resource,
                ttl_ms,
                uses,
            },
        )?;
        match read_msg::<_, Response>(&mut self.stream)? {
            Response::ResourceGranted { grant } => Ok(grant),
            Response::Error { message } => Err(io::Error::other(message)),
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("expected ResourceGranted, got {other:?}"),
            )),
        }
    }

    pub fn consume_resource_grant(
        &mut self,
        id: ResourceGrantId,
        resource: ActorResource,
    ) -> io::Result<ResourceGrant> {
        write_msg(
            &mut self.stream,
            &Request::ConsumeResourceGrant { id, resource },
        )?;
        match read_msg::<_, Response>(&mut self.stream)? {
            Response::ResourceGrantConsumed { grant } => Ok(grant),
            Response::Error { message } => Err(io::Error::other(message)),
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("expected ResourceGrantConsumed, got {other:?}"),
            )),
        }
    }

    pub fn revoke_resource_grant(&mut self, id: ResourceGrantId) -> io::Result<()> {
        write_msg(&mut self.stream, &Request::RevokeResourceGrant { id })?;
        match read_msg::<_, Response>(&mut self.stream)? {
            Response::ResourceGrantRevoked {} => Ok(()),
            Response::Error { message } => Err(io::Error::other(message)),
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("expected ResourceGrantRevoked, got {other:?}"),
            )),
        }
    }

    pub fn publish_accessibility_tree(
        &mut self,
        update: aegis_semantic::AccessibilityTreeUpdate,
    ) -> io::Result<()> {
        write_msg(
            &mut self.stream,
            &Request::PublishAccessibilityTree { update },
        )?;
        match read_msg::<_, Response>(&mut self.stream)? {
            Response::AccessibilityTreePublished {} => Ok(()),
            Response::Error { message } => Err(io::Error::other(message)),
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("expected AccessibilityTreePublished, got {other:?}"),
            )),
        }
    }

    /// Fetch kernel-process-bound windows for a trusted accessibility
    /// provider. Ordinary window observers cannot call this endpoint.
    pub fn accessibility_windows(
        &mut self,
    ) -> io::Result<Vec<aegis_semantic::AccessibilityWindowBinding>> {
        write_msg(&mut self.stream, &Request::GetAccessibilityWindows)?;
        match read_msg::<_, Response>(&mut self.stream)? {
            Response::AccessibilityWindows { windows } => Ok(windows),
            Response::Error { message } => Err(io::Error::other(message)),
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("expected AccessibilityWindows, got {other:?}"),
            )),
        }
    }

    pub fn next_accessibility_action(
        &mut self,
        timeout: Duration,
    ) -> io::Result<Option<aegis_semantic::SemanticActionRequest>> {
        let timeout_ms = u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX);
        write_msg(
            &mut self.stream,
            &Request::NextAccessibilityAction { timeout_ms },
        )?;
        match read_msg::<_, Response>(&mut self.stream)? {
            Response::AccessibilityAction { request } => Ok(request),
            Response::Error { message } => Err(io::Error::other(message)),
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("expected AccessibilityAction, got {other:?}"),
            )),
        }
    }

    pub fn complete_accessibility_action(
        &mut self,
        request_id: u64,
        result: Result<(), String>,
    ) -> io::Result<()> {
        let (success, message) = match result {
            Ok(()) => (true, None),
            Err(message) => (false, Some(message)),
        };
        write_msg(
            &mut self.stream,
            &Request::CompleteAccessibilityAction {
                request_id,
                success,
                message,
            },
        )?;
        match read_msg::<_, Response>(&mut self.stream)? {
            Response::AccessibilityActionCompleted {} => Ok(()),
            Response::Error { message } => Err(io::Error::other(message)),
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("expected AccessibilityActionCompleted, got {other:?}"),
            )),
        }
    }

    /// Ask the user a yes/no consent question through compositor chrome
    /// (portal consent dialogs). Same blocking discipline as
    /// [`Client::pick_target`]. Requires `control` and an explicit
    /// `PickConfirm` op in the connection's scope.
    pub fn pick_confirm(
        &mut self,
        title: String,
        body: String,
        accept_label: Option<String>,
    ) -> io::Result<ConfirmPickResult> {
        write_msg(
            &mut self.stream,
            &Request::PickConfirm {
                title,
                body,
                accept_label,
            },
        )?;
        match read_msg::<_, Response>(&mut self.stream)? {
            Response::ConfirmPicked { result } => Ok(result),
            Response::Error { message } => Err(io::Error::other(message)),
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("expected ConfirmPicked, got {other:?}"),
            )),
        }
    }

    /// Replace the desktop wallpaper with the image at `path` (the
    /// Wallpaper portal). The reply is the compositor's authoritative
    /// decode-and-swap receipt. Requires `control` and an explicit
    /// `SetWallpaper` op in the connection's scope.
    pub fn set_wallpaper(&mut self, path: std::path::PathBuf) -> io::Result<()> {
        write_msg(&mut self.stream, &Request::SetWallpaper { path })?;
        match read_msg::<_, Response>(&mut self.stream)? {
            Response::WallpaperSet {} => Ok(()),
            Response::Error { message } => Err(io::Error::other(message)),
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("expected WallpaperSet, got {other:?}"),
            )),
        }
    }

    /// Stop a stream owned by this connection.
    pub fn stop_output_stream(&mut self, stream_id: u64) -> io::Result<()> {
        write_msg(&mut self.stream, &Request::StreamOutputStop { stream_id })?;
        match read_msg::<_, Response>(&mut self.stream)? {
            Response::StreamOutputStopped { .. } => Ok(()),
            Response::Error { message } => Err(io::Error::other(message)),
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("expected StreamOutputStopped, got {other:?}"),
            )),
        }
    }

    /// Set or clear this connection's global idle inhibitor (the Inhibit
    /// portal, ADR-0075), returning the state the server confirmed. Requires
    /// `control` and an explicit `IdleInhibit` op in the connection's scope.
    /// The server releases the inhibitor when this connection drops.
    pub fn set_idle_inhibit(&mut self, inhibit: bool) -> io::Result<bool> {
        write_msg(&mut self.stream, &Request::SetIdleInhibit { inhibit })?;
        match read_msg::<_, Response>(&mut self.stream)? {
            Response::IdleInhibitSet { inhibited } => Ok(inhibited),
            Response::Error { message } => Err(io::Error::other(message)),
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("expected IdleInhibitSet, got {other:?}"),
            )),
        }
    }

    /// Send a lease renewal without reading its reply. On a streaming
    /// connection the reply arrives interleaved with frames; surface it from
    /// [`Client::next_stream_message`] as [`StreamMessage::LeaseRenewed`].
    pub fn request_lease_renewal(&mut self, ttl_ms: u64) -> io::Result<()> {
        write_msg(&mut self.stream, &Request::RenewLease { ttl_ms })
    }

    /// Read the next message on a streaming connection. Blocks until one
    /// arrives (subject to [`Client::set_io_timeout`]). Frame metadata is
    /// followed by its sealed pixel memfd, which this call receives and
    /// validates (ADR-0041). Unknown interleaved events are skipped.
    pub fn next_stream_message(&mut self) -> io::Result<StreamMessage> {
        loop {
            let value: serde_json::Value = read_msg(&mut self.stream)?;
            let event: io::Result<Event> = serde_json::from_value(value.clone()).map_err(|e| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("unexpected message on stream connection: {e}"),
                )
            });
            match event {
                Ok(Event::StreamFrame {
                    stream_id,
                    sequence,
                    width,
                    height,
                    stride,
                    format,
                    damage,
                    dropped,
                    byte_len,
                    slot,
                }) => {
                    if slot.is_some() {
                        // This client never opts into the dmabuf transport,
                        // so a slot-referenced frame has no blob to consume;
                        // reading one would desynchronize the framing.
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "dmabuf stream frames require slot-capable client",
                        ));
                    }
                    let pixels = crate::blob::receive(&self.stream, byte_len)?;
                    return Ok(StreamMessage::Frame(StreamFrame {
                        stream_id,
                        sequence,
                        width,
                        height,
                        stride,
                        format,
                        damage,
                        dropped,
                        pixels,
                    }));
                }
                Ok(Event::StreamEnded { stream_id, reason }) => {
                    return Ok(StreamMessage::Ended { stream_id, reason });
                }
                Ok(_) => {
                    // Unrelated events (the connection is not subscribed)
                    // are skipped.
                    continue;
                }
                Err(_) => {
                    // Not an event: try the responses a streaming client can
                    // still receive (lease renewal acknowledgement, error).
                    let response: Response = serde_json::from_value(value).map_err(|e| {
                        io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!("unexpected message on stream connection: {e}"),
                        )
                    })?;
                    match response {
                        Response::LeaseRenewed { lease } => {
                            self.lease = Some(lease);
                            return Ok(StreamMessage::LeaseRenewed);
                        }
                        Response::Error { message } => return Err(io::Error::other(message)),
                        other => {
                            return Err(io::Error::new(
                                io::ErrorKind::InvalidData,
                                format!("unexpected response on stream connection: {other:?}"),
                            ));
                        }
                    }
                }
            }
        }
    }

    /// Submit a control/session command. Most commands return once the server
    /// has queued them; [`Command::System`] is the exception and returns only
    /// after the compositor main loop reports its authoritative apply result.
    /// Re-query with [`Client::windows`] or subscribe to
    /// [`Event::WindowsChanged`] to observe fire-and-forget commands.
    pub fn command(&mut self, cmd: Command) -> io::Result<()> {
        write_msg(&mut self.stream, &Request::Do { cmd })?;
        match read_msg::<_, Response>(&mut self.stream)? {
            Response::Ok => Ok(()),
            Response::Error { message } => Err(io::Error::other(message)),
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("expected Ok, got {other:?}"),
            )),
        }
    }

    /// Opt into server-pushed events on this connection. After this returns,
    /// [`Client::next_event`] blocks until the next event arrives.
    pub fn subscribe(&mut self) -> io::Result<()> {
        write_msg(&mut self.stream, &Request::Subscribe)?;
        match read_msg::<_, Response>(&mut self.stream)? {
            Response::Subscribed => Ok(()),
            Response::Error { message } => Err(io::Error::other(message)),
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("expected Subscribed, got {other:?}"),
            )),
        }
    }

    /// Opt into the detailed mutation-journal stream on this connection.
    pub fn subscribe_journal(&mut self) -> io::Result<()> {
        write_msg(&mut self.stream, &Request::SubscribeJournal)?;
        match read_msg::<_, Response>(&mut self.stream)? {
            Response::Subscribed => Ok(()),
            Response::Error { message } => Err(io::Error::other(message)),
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("expected Subscribed, got {other:?}"),
            )),
        }
    }

    /// Block until the next server-pushed event arrives. Call only after
    /// [`Client::subscribe`]; an event may interleave with responses to
    /// other requests, so this reads one framed message and rejects it if it
    /// is not an event.
    pub fn next_event(&mut self) -> io::Result<Event> {
        let value: serde_json::Value = read_msg(&mut self.stream)?;
        serde_json::from_value(value)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))
    }
}

impl std::os::fd::AsRawFd for Client {
    /// Exposes the connection's descriptor so integrators running a foreign
    /// event loop (the portal's PipeWire main loop) can poll readability
    /// there instead of dedicating a thread to blocking reads.
    fn as_raw_fd(&self) -> std::os::fd::RawFd {
        self.stream.as_raw_fd()
    }
}
