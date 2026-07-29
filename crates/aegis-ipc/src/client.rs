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
    Capabilities, Command, Event, LeaseGrant, LeaseRequest, PROTOCOL_VERSION, PickKind, PickResult,
    RealmAction, RealmActionResult, Request, Response, Scope, SettingsAction, SettingsReceipt,
    SettingsSnapshot, StreamPixelFormat, StreamTarget, SystemAction, SystemStatus,
};

/// Decoded Realm observation returned by [`Client::capture_realm`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedRealm {
    pub realm: aegis_core::realm::RealmId,
    pub width: u32,
    pub height: u32,
    pub scale_milli: u32,
    pub region: aegis_core::Rect,
    pub placements: Vec<aegis_core::realm::RealmWindowPlacement>,
    pub png: Vec<u8>,
    pub revision: u64,
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
    pub damage: Vec<aegis_core::Rect>,
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
    caps: Capabilities,
    scope: Scope,
    lease: Option<LeaseGrant>,
}

impl Client {
    /// Connect requesting `query` only.
    pub fn connect(path: &Path) -> io::Result<Client> {
        Self::connect_inner(path, Capabilities::QUERY, None)
    }

    /// Connect requesting a specific capability set. The server may grant a
    /// subset (intersected with its policy, with `query` forced on).
    pub fn connect_with(path: &Path, requested: Capabilities) -> io::Result<Client> {
        Self::connect_inner(path, requested, None)
    }

    /// Connect with explicit capabilities and bound the handshake itself.
    /// Use this from GUI/background workers so an accepted but unresponsive
    /// local peer cannot retain the worker indefinitely.
    pub fn connect_with_timeout(
        path: &Path,
        requested: Capabilities,
        timeout: Duration,
    ) -> io::Result<Client> {
        Self::connect_inner_with_timeout(path, requested, None, Some(timeout))
    }

    /// Connect requesting capabilities under a named, compositor-configured
    /// scope. An unknown scope is refused during the handshake instead of
    /// silently granting an unrestricted connection.
    pub fn connect_scoped(
        path: &Path,
        requested: Capabilities,
        scope: impl Into<String>,
    ) -> io::Result<Client> {
        Self::connect_inner(path, requested, Some(scope.into()))
    }

    /// Connect under a named scope and apply a timeout before the handshake.
    /// This is the safe entry point for async adapters that execute the
    /// blocking client on a worker thread.
    pub fn connect_scoped_with_timeout(
        path: &Path,
        requested: Capabilities,
        scope: impl Into<String>,
        timeout: Duration,
    ) -> io::Result<Client> {
        Self::connect_inner_with_timeout(path, requested, Some(scope.into()), Some(timeout))
    }

    fn connect_inner(
        path: &Path,
        requested: Capabilities,
        scope_name: Option<String>,
    ) -> io::Result<Client> {
        Self::connect_inner_with_timeout(path, requested, scope_name, None)
    }

    fn connect_inner_with_timeout(
        path: &Path,
        requested: Capabilities,
        scope_name: Option<String>,
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
            },
        )?;
        let resp: Response = read_msg(&mut stream)?;
        let (caps, scope, lease) = match resp {
            Response::Hello {
                version,
                caps,
                scope,
                lease,
            } if version == PROTOCOL_VERSION => (caps, scope, lease),
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
        })
    }

    /// The capabilities the server actually granted at the handshake.
    pub fn caps(&self) -> Capabilities {
        self.caps
    }

    /// The resource/operation scope granted by the compositor at handshake.
    pub fn scope(&self) -> &Scope {
        &self.scope
    }

    pub fn lease(&self) -> Option<LeaseGrant> {
        self.lease
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

    /// Fetch the live toplevel snapshot.
    pub fn windows(&mut self) -> io::Result<Vec<aegis_core::window::Window>> {
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
    pub fn workspaces(&mut self) -> io::Result<aegis_core::workspace::WorkspaceSnapshot> {
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
    pub fn switch_workspace(&mut self, dir: aegis_core::workspace::Switch) -> io::Result<()> {
        self.command(Command::SwitchWorkspace { dir })
    }

    /// Switch directly to a workspace by id.
    pub fn switch_workspace_to(
        &mut self,
        id: aegis_core::workspace::WorkspaceId,
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
        id: aegis_core::window::WindowId,
        rect: aegis_core::Rect,
    ) -> io::Result<()> {
        self.command(Command::SetWindowGeometry { id, rect })
    }

    /// Inject bounded, target-local actions into a toplevel. The connection
    /// must have negotiated the `input` capability under a named scope.
    pub fn inject_input(
        &mut self,
        id: aegis_core::window::WindowId,
        actions: Vec<aegis_core::input::SyntheticInputAction>,
    ) -> io::Result<()> {
        self.command(Command::InjectInput { id, actions })
    }

    pub fn inject_realm_input(
        &mut self,
        realm: aegis_core::realm::RealmId,
        id: aegis_core::window::WindowId,
        actions: Vec<aegis_core::input::SyntheticInputAction>,
    ) -> io::Result<()> {
        self.command(Command::InjectRealmInput { realm, id, actions })
    }

    pub fn launch_in_realm(
        &mut self,
        realm: aegis_core::realm::RealmId,
        desktop_id: impl Into<String>,
    ) -> io::Result<()> {
        self.command(Command::LaunchInRealm {
            realm,
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
        region: Option<aegis_core::Rect>,
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
        region: Option<aegis_core::Rect>,
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
    pub fn notifications(&mut self) -> io::Result<Vec<aegis_core::notify::Notification>> {
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
    pub fn outputs(&mut self) -> io::Result<Vec<aegis_core::output::OutputInfo>> {
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

    pub fn realms(&mut self) -> io::Result<aegis_core::realm::RealmSnapshot> {
        write_msg(&mut self.stream, &Request::GetRealms)?;
        match read_msg::<_, Response>(&mut self.stream)? {
            Response::Realms { snapshot } => Ok(snapshot),
            Response::Error { message } => Err(io::Error::other(message)),
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("expected Realms, got {other:?}"),
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

    pub fn realm_action(&mut self, action: RealmAction) -> io::Result<RealmActionResult> {
        write_msg(&mut self.stream, &Request::Realm { action })?;
        match read_msg::<_, Response>(&mut self.stream)? {
            Response::Realm { result } => Ok(result),
            Response::Error { message } => Err(io::Error::other(message)),
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("expected Realm, got {other:?}"),
            )),
        }
    }

    pub fn capture_realm(
        &mut self,
        realm: aegis_core::realm::RealmId,
        region: Option<aegis_core::Rect>,
    ) -> io::Result<CapturedRealm> {
        write_msg(&mut self.stream, &Request::CaptureRealm { realm, region })?;
        match read_msg::<_, Response>(&mut self.stream)? {
            Response::CaptureRealm { capture } if capture.realm == realm => {
                let png = crate::blob::receive(&self.stream, capture.png_bytes)?;
                Ok(CapturedRealm {
                    realm: capture.realm,
                    width: capture.width,
                    height: capture.height,
                    scale_milli: capture.scale_milli,
                    region: capture.region,
                    placements: capture.placements,
                    png,
                    revision: capture.revision,
                })
            }
            Response::Error { message } => Err(io::Error::other(message)),
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("expected CaptureRealm, got {other:?}"),
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
            &Request::StreamOutputStart { max_fps, target },
        )?;
        match read_msg::<_, Response>(&mut self.stream)? {
            Response::StreamOutputStarted {
                stream_id,
                width,
                height,
                format,
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
                }) => {
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
