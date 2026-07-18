//! The reference client for the ass IPC.
//!
//! A thin synchronous client over a blocking unix stream. Power tools and
//! the agent layer build on the same schema; this is the canonical path for
//! "connect, read some state" in one process. See ADR-0027.

use std::io;
use std::os::unix::net::UnixStream;
use std::path::Path;

use crate::codec::{read_msg, write_msg};
use crate::journal::JournalSnapshot;
use crate::schema::{Capabilities, Command, Event, Request, Response, Scope, PROTOCOL_VERSION};

/// A connected IPC client. The handshake is complete on construction; the
/// granted capabilities are available via [`Client::caps`].
pub struct Client {
    stream: UnixStream,
    caps: Capabilities,
    scope: Scope,
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

    fn connect_inner(
        path: &Path,
        requested: Capabilities,
        scope_name: Option<String>,
    ) -> io::Result<Client> {
        let mut stream = UnixStream::connect(path)?;
        write_msg(
            &mut stream,
            &Request::Hello {
                version: PROTOCOL_VERSION,
                caps: requested,
                scope: scope_name,
            },
        )?;
        let resp: Response = read_msg(&mut stream)?;
        let (caps, scope) = match resp {
            Response::Hello {
                version,
                caps,
                scope,
            } if version == PROTOCOL_VERSION => (caps, scope),
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

    /// Fetch the live toplevel snapshot.
    pub fn windows(&mut self) -> io::Result<Vec<ass_core::window::Window>> {
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
    pub fn workspaces(&mut self) -> io::Result<ass_core::workspace::WorkspaceSnapshot> {
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
    pub fn switch_workspace(&mut self, dir: ass_core::workspace::Switch) -> io::Result<()> {
        self.command(Command::SwitchWorkspace { dir })
    }

    /// Switch directly to a workspace by id.
    pub fn switch_workspace_to(&mut self, id: ass_core::workspace::WorkspaceId) -> io::Result<()> {
        self.command(Command::SwitchWorkspaceTo { id })
    }

    /// Toggle the current workspace between tiled and floating (ADR-0024).
    pub fn toggle_tiling(&mut self) -> io::Result<()> {
        self.command(Command::ToggleTiling)
    }

    /// Set a floating toplevel's geometry in compositor logical coordinates.
    pub fn set_window_geometry(
        &mut self,
        id: ass_core::window::WindowId,
        rect: ass_core::Rect,
    ) -> io::Result<()> {
        self.command(Command::SetWindowGeometry { id, rect })
    }

    /// Inject bounded, target-local actions into a toplevel. The connection
    /// must have negotiated the `input` capability under a named scope.
    pub fn inject_input(
        &mut self,
        id: ass_core::window::WindowId,
        actions: Vec<ass_core::input::SyntheticInputAction>,
    ) -> io::Result<()> {
        self.command(Command::InjectInput { id, actions })
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
        region: Option<ass_core::Rect>,
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
        region: Option<ass_core::Rect>,
    ) -> io::Result<(u32, u32, Vec<u8>)> {
        write_msg(&mut self.stream, &Request::CaptureOutput { region })?;
        match read_msg::<_, Response>(&mut self.stream)? {
            Response::CaptureOutput {
                width,
                height,
                png_base64,
            } => {
                let png = crate::base64::decode(&png_base64).ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        "invalid base64 in CaptureOutput",
                    )
                })?;
                Ok((width, height, png))
            }
            Response::Error { message } => Err(io::Error::other(message)),
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("expected CaptureOutput, got {other:?}"),
            )),
        }
    }

    /// Fetch the live notification queue.
    pub fn notifications(&mut self) -> io::Result<Vec<ass_core::notify::Notification>> {
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
    pub fn outputs(&mut self) -> io::Result<Vec<ass_core::output::OutputInfo>> {
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

    /// Submit a control/session command. Returns once the server has queued
    /// it (not once the compositor has applied it); re-query with [`Client::windows`]
    /// or subscribe to [`Event::WindowsChanged`] to observe the effect.
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
