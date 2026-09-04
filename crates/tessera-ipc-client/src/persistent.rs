//! One persistent, self-healing broker connection (ADR-0125).
//!
//! Where `connect` gives one handshake, `PersistentConnection` keeps the
//! connection alive for the process lifetime: the privileged lease is
//! renewed lazily before it lapses, and a wedged connection is re-paired
//! transparently on the next request. This is the connection policy a
//! long-lived broker consumer wants; short-lived, least-privilege-in-time
//! consumers can still open one connection per request with `connect`.
//!
//! `run` never retries a failed request: a mutation may have applied
//! before the connection died, so the error is surfaced and the caller
//! decides. The next `run` reconnects first.

use std::io;
use std::time::{Duration, Instant};

use tessera_ipc::Client;

use crate::connect::{ConnectError, ConnectParams, connect};

/// Whether an error means the connection itself is unusable — a broken or
/// reset stream, EOF, or a read/write timeout (a timed-out response may
/// still arrive later and desynchronize the framed stream). Server
/// `Response::Error` answers (`io::ErrorKind::Other`) are logical refusals
/// and keep the connection.
pub fn is_transport_failure(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::BrokenPipe
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::NotConnected
            | io::ErrorKind::UnexpectedEof
            | io::ErrorKind::TimedOut
            | io::ErrorKind::WouldBlock
    )
}

/// Classifies an error from a `run` closure as a transport failure (poison
/// the connection; the next call re-pairs) or a logical refusal (keep it).
pub trait ClassifyError {
    fn is_transport_failure(&self) -> bool;
}

impl ClassifyError for io::Error {
    fn is_transport_failure(&self) -> bool {
        is_transport_failure(self)
    }
}

/// A persistent broker connection with lazy lease renewal and transparent
/// re-pairing.
pub struct PersistentConnection {
    params: ConnectParams,
    client: Option<Client>,
    lease_deadline: Option<Instant>,
    principal: String,
    renewals: u64,
}

impl PersistentConnection {
    /// Connect now and keep the connection. Consumers that prefer a lazy
    /// first connect should defer calling this, as `tessera-mcp`'s lazy
    /// platform does.
    pub fn connect(params: ConnectParams) -> Result<Self, ConnectError> {
        let connected = connect(&params)?;
        let lease_deadline = connected
            .client
            .lease()
            .map(|lease| Instant::now() + Duration::from_millis(lease.ttl_ms));
        Ok(Self {
            params,
            client: Some(connected.client),
            lease_deadline,
            principal: connected.principal,
            renewals: 0,
        })
    }

    /// The authenticated principal bound by the latest handshake.
    pub fn principal(&self) -> &str {
        &self.principal
    }

    /// Bound per-request I/O from now on; re-applied after every reconnect.
    /// A failure poisons the wedged connection.
    pub fn set_post_timeout(&mut self, timeout: Duration) -> io::Result<()> {
        self.params.post_timeout = timeout;
        if let Some(client) = &self.client
            && let Err(error) = client.set_io_timeout(Some(timeout))
        {
            if is_transport_failure(&error) {
                self.client = None;
            }
            return Err(error);
        }
        Ok(())
    }

    /// Run one blocking request against the live connection. A transport
    /// failure poisons the connection (the next `run` re-pairs); a logical
    /// refusal keeps it. The failed request is never retried: a mutation
    /// may have applied before the connection died.
    pub fn run<T, E>(
        &mut self,
        f: impl FnOnce(&mut Client) -> Result<T, E>,
    ) -> Result<T, PersistentError<E>>
    where
        E: ClassifyError,
    {
        self.ensure_live().map_err(PersistentError::Connect)?;
        match f(self.client.as_mut().expect("ensure_live connects")) {
            Ok(value) => Ok(value),
            Err(error) => {
                if error.is_transport_failure() {
                    self.client = None;
                }
                Err(PersistentError::Call(error))
            }
        }
    }

    fn ensure_live(&mut self) -> Result<(), ConnectError> {
        if self.client.is_none() {
            self.reconnect()?;
        }
        self.renew_lease_due()
    }

    fn reconnect(&mut self) -> Result<(), ConnectError> {
        let connected = connect(&self.params)?;
        self.lease_deadline = connected
            .client
            .lease()
            .map(|lease| Instant::now() + Duration::from_millis(lease.ttl_ms));
        self.principal = connected.principal;
        self.client = Some(connected.client);
        Ok(())
    }

    /// Renew once half the granted TTL has elapsed. A renewal failure (an
    /// expired lease, a revoked session, or a wedge) is healed by
    /// re-pairing for a fresh lease rather than surfaced.
    fn renew_lease_due(&mut self) -> Result<(), ConnectError> {
        let Some(deadline) = self.lease_deadline else {
            return Ok(());
        };
        let client = self.client.as_mut().expect("ensure_live connects");
        let Some(lease) = client.lease() else {
            return Ok(());
        };
        if !lease.renewable {
            return Ok(());
        }
        let ttl = Duration::from_millis(lease.ttl_ms);
        if Instant::now() + ttl / 2 < deadline {
            return Ok(());
        }
        match client.renew_lease(lease.ttl_ms) {
            Ok(grant) => {
                self.lease_deadline = Some(Instant::now() + Duration::from_millis(grant.ttl_ms));
                self.renewals += 1;
                Ok(())
            }
            Err(_) => {
                self.client = None;
                self.reconnect()?;
                Ok(())
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn force_lease_deadline(&mut self, deadline: Instant) {
        self.lease_deadline = Some(deadline);
    }

    #[cfg(test)]
    pub(crate) fn renewals(&self) -> u64 {
        self.renewals
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PersistentError<E> {
    #[error(transparent)]
    Connect(#[from] ConnectError),
    #[error("Tessera IPC request failed: {0}")]
    Call(E),
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tessera_ipc::{ActorCapability, ConnectionCapabilities, Handler, Server};

    use super::*;
    use crate::CredentialSource;

    struct TestHandler;

    impl Handler for TestHandler {
        fn policy_caps(&self) -> ConnectionCapabilities {
            ConnectionCapabilities {
                query: true,
                control: true,
                input: false,
                session: false,
                interaction_domain: false,
            }
        }
        fn windows(&self) -> Vec<tessera_model::window::Window> {
            Vec::new()
        }
        fn workspaces(&self) -> tessera_model::workspace::WorkspaceSnapshot {
            tessera_model::workspace::WorkspaceSnapshot { outputs: vec![] }
        }
        fn notifications(&self) -> Vec<tessera_model::notify::Notification> {
            Vec::new()
        }
        fn outputs(&self) -> Vec<tessera_model::output::OutputInfo> {
            Vec::new()
        }
        fn journal_since(&self, _since: u64) -> tessera_ipc::JournalSnapshot {
            tessera_ipc::JournalSnapshot {
                entries: vec![],
                oldest_seq: 1,
                latest_seq: 0,
            }
        }
        fn command(&self, _conn_id: u64, _subject: Option<&str>, _cmd: tessera_ipc::Command) {}
        fn agent_lookup(&self, credential: &str) -> Option<tessera_ipc::AgentIdentity> {
            (credential == "cred_test").then(|| tessera_ipc::AgentIdentity {
                principal: tessera_ipc::ActorPrincipal::new("prin_test").unwrap(),
                pregranted: vec![ActorCapability::Focus, ActorCapability::ObserveWindows],
                gated: vec![],
            })
        }
        fn pair_agent(
            &self,
            _conn_id: u64,
            _label: Option<&str>,
            _requested: &[ActorCapability],
        ) -> Result<tessera_ipc::PairedAgent, String> {
            Ok(tessera_ipc::PairedAgent {
                principal: tessera_ipc::ActorPrincipal::new("prin_test").unwrap(),
                credential: "cred_test".into(),
                pregranted: vec![ActorCapability::Focus, ActorCapability::ObserveWindows],
                gated: vec![],
            })
        }
    }

    fn scratch() -> std::path::PathBuf {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        std::env::temp_dir().join(format!("tessera-ipc-client-{}-{n}.sock", std::process::id()))
    }

    fn params(socket: std::path::PathBuf) -> ConnectParams {
        ConnectParams {
            socket,
            capabilities: ConnectionCapabilities {
                query: true,
                control: true,
                input: false,
                session: false,
                interaction_domain: false,
            },
            label: "persistent-test".into(),
            requested: vec![ActorCapability::Focus],
            credential: CredentialSource::Paired(crate::IdentityStore::load(None, "test")),
            handshake_timeout: Duration::from_secs(5),
            post_timeout: Duration::from_secs(5),
        }
    }

    #[test]
    fn lease_renews_lazily_when_due() {
        let path = scratch();
        let handler = Arc::new(TestHandler);
        let _server = Server::start(&path, handler).expect("server");
        let mut conn = PersistentConnection::connect(params(path)).expect("connect");
        assert_eq!(conn.renewals(), 0);
        conn.run(|client| client.windows()).expect("first run");
        assert_eq!(conn.renewals(), 0, "a fresh lease is not renewed");

        conn.force_lease_deadline(Instant::now());
        conn.run(|client| client.windows()).expect("run past due");
        assert_eq!(conn.renewals(), 1, "a due lease is renewed transparently");
        conn.run(|client| client.windows())
            .expect("run after renewal");
        assert_eq!(conn.renewals(), 1, "the renewed lease is fresh again");
    }

    #[test]
    fn wedged_connection_reconnects_on_the_next_run() {
        use std::os::fd::AsRawFd as _;

        let path = scratch();
        let handler = Arc::new(TestHandler);
        let _server = Server::start(&path, handler).expect("server");
        let mut conn = PersistentConnection::connect(params(path)).expect("connect");
        conn.run(|client| client.windows())
            .expect("run before wedge");

        // Simulate a severed connection: the peer never learns why.
        let fd = conn.client.as_ref().expect("client").as_raw_fd();
        // SAFETY: the descriptor is valid and borrowed; shutdown only
        // disables further sends/receives on it.
        unsafe { libc::shutdown(fd, libc::SHUT_RDWR) };

        // The failed request is surfaced, never retried.
        let error = conn
            .run(|client| client.windows())
            .expect_err("the wedged connection surfaces an I/O error");
        assert!(matches!(error, PersistentError::Call(_)));

        // The next run transparently re-pairs with pairing continuity.
        conn.run(|client| client.windows())
            .expect("the next run reconnects");
        assert_eq!(conn.principal(), "prin_test");
    }
}
