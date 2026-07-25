//! The inward half of the bridge: a scoped ass-IPC capture client.
//!
//! The portal connects under the built-in owner-only `aegis-portal` scope
//! (`aegis_ipc::LOCAL_PORTAL_SCOPE`), which the compositor resolves to exactly
//! one operation — `CaptureOutput` — with the `control` capability and its
//! time-bounded lease. The wrapper keeps the connection alive across idle
//! periods by renewing the lease at half its TTL, and reconnects once on any
//! failure so a compositor restart or an expired lease self-heals on the
//! next screenshot instead of killing the D-Bus service.

use std::io;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use aegis_ipc::{Capabilities, Client, LOCAL_PORTAL_SCOPE};

/// Lease TTL requested at handshake and renewal; matches the reference
/// client's default (`LeaseRequest::default`).
const LEASE_TTL_MS: u64 = 900_000;

/// A lazily connected, lease-renewing `CaptureOutput` client. One instance
/// lives on the capture worker thread; it is not `Sync` by design.
pub(crate) struct PortalCapture {
    socket: PathBuf,
    client: Option<Client>,
    renewed_at: Instant,
}

impl PortalCapture {
    pub(crate) fn new(socket: PathBuf) -> Self {
        Self {
            socket,
            client: None,
            renewed_at: Instant::now(),
        }
    }

    fn connect(&self) -> io::Result<Client> {
        let caps = Capabilities {
            query: true,
            control: true,
            input: false,
            session: false,
            realm: false,
        };
        Client::connect_scoped(&self.socket, caps, LOCAL_PORTAL_SCOPE)
    }

    /// Hand out a live client, renewing an ageing lease or reconnecting an
    /// expired/broken one.
    fn client(&mut self) -> io::Result<&mut Client> {
        if let Some(client) = self.client.as_mut()
            && self.renewed_at.elapsed() >= Duration::from_millis(LEASE_TTL_MS / 2)
        {
            match client.renew_lease(LEASE_TTL_MS) {
                Ok(_) => self.renewed_at = Instant::now(),
                Err(error) => {
                    // An expired lease cannot be renewed; reconnecting is the
                    // recovery path (ADR-0035's fail-closed rule also makes a
                    // vanished scope refuse the new handshake, which surfaces
                    // here as a normal error).
                    log::info!("portal: lease renewal failed ({error}); reconnecting IPC");
                    self.client = None;
                }
            }
        }
        if self.client.is_none() {
            self.client = Some(self.connect()?);
            self.renewed_at = Instant::now();
        }
        Ok(self.client.as_mut().expect("connected above"))
    }

    /// Capture the focused output as PNG bytes. One automatic reconnect +
    /// retry hides transient failures (compositor restart, raced lease
    /// expiry); persistent failures surface as errors to the caller.
    pub(crate) fn capture_png(&mut self) -> io::Result<Vec<u8>> {
        match self.client()?.capture_output() {
            Ok((_, _, png)) => Ok(png),
            Err(first) => {
                log::info!("portal: capture failed ({first}); reconnecting IPC");
                self.client = None;
                let (_, _, png) = self.client()?.capture_output()?;
                Ok(png)
            }
        }
    }

    /// Capture a region of the focused output as PNG bytes (compositor
    /// logical pixels), with the same reconnect + retry discipline as
    /// [`PortalCapture::capture_png`].
    pub(crate) fn capture_region_png(&mut self, region: aegis_core::Rect) -> io::Result<Vec<u8>> {
        match self.client()?.capture_output_region(Some(region)) {
            Ok((_, _, png)) => Ok(png),
            Err(first) => {
                log::info!("portal: region capture failed ({first}); reconnecting IPC");
                self.client = None;
                let (_, _, png) = self.client()?.capture_output_region(Some(region))?;
                Ok(png)
            }
        }
    }

    /// Run one interactive pick through compositor chrome (ADR-0054). Blocks
    /// until the user confirms or cancels, so this can take far longer than
    /// any other call. No automatic retry: a reconnect would orphan the
    /// user-facing picker, and the compositor bounds the wait itself.
    pub(crate) fn pick(&mut self, kind: aegis_ipc::PickKind) -> io::Result<aegis_ipc::PickResult> {
        self.client()?.pick_target(kind)
    }

    /// Fetch the live window list (the query capability every scoped
    /// connection holds), with one automatic reconnect + retry.
    pub(crate) fn windows(&mut self) -> io::Result<Vec<aegis_core::window::Window>> {
        match self.client()?.windows() {
            Ok(windows) => Ok(windows),
            Err(first) => {
                log::info!("portal: window query failed ({first}); reconnecting IPC");
                self.client = None;
                self.client()?.windows()
            }
        }
    }
}

/// A lazily connected, lease-renewing `SetIdleInhibit` client behind the
/// same built-in `aegis-portal` scope (ADR-0053). One instance lives on the
/// inhibit worker thread; it is not `Sync` by design. The compositor
/// releases the inhibitor if this connection dies, so a crashed backend can
/// never wedge the session out of idle.
pub(crate) struct PortalIdle {
    socket: PathBuf,
    client: Option<Client>,
    renewed_at: Instant,
}

impl PortalIdle {
    pub(crate) fn new(socket: PathBuf) -> Self {
        Self {
            socket,
            client: None,
            renewed_at: Instant::now(),
        }
    }

    fn connect(&self) -> io::Result<Client> {
        let caps = Capabilities {
            query: true,
            control: true,
            input: false,
            session: false,
            realm: false,
        };
        Client::connect_scoped(&self.socket, caps, LOCAL_PORTAL_SCOPE)
    }

    /// Same lease-renewal discipline as [`PortalCapture::client`].
    fn client(&mut self) -> io::Result<&mut Client> {
        if let Some(client) = self.client.as_mut()
            && self.renewed_at.elapsed() >= Duration::from_millis(LEASE_TTL_MS / 2)
        {
            match client.renew_lease(LEASE_TTL_MS) {
                Ok(_) => self.renewed_at = Instant::now(),
                Err(error) => {
                    log::info!("portal: lease renewal failed ({error}); reconnecting IPC");
                    self.client = None;
                }
            }
        }
        if self.client.is_none() {
            self.client = Some(self.connect()?);
            self.renewed_at = Instant::now();
        }
        Ok(self.client.as_mut().expect("connected above"))
    }

    /// Set or clear the global idle inhibitor, with one automatic reconnect
    /// and retry on transient failure (the same recovery shape as
    /// [`PortalCapture::capture_png`]).
    pub(crate) fn set_inhibit(&mut self, inhibit: bool) -> io::Result<()> {
        match self.client()?.set_idle_inhibit(inhibit) {
            Ok(_) => Ok(()),
            Err(first) => {
                log::info!("portal: idle inhibit failed ({first}); reconnecting IPC");
                self.client = None;
                self.client()?.set_idle_inhibit(inhibit)?;
                Ok(())
            }
        }
    }
}
