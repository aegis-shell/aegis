//! The agent connection discipline (ADR-0125): one pairing handshake, one
//! credential policy, one timeout split, for every agent client.
//!
//! The first connection may block on the interactive pairing prompt, so the
//! handshake gets a generous bound; per-request I/O falls to the configured
//! timeout right after. Credential handling splits into two policies:
//! durable paired identities that persist or confirm, and launcher-injected
//! ephemeral credentials that are presented as-is.

use std::io;
use std::path::PathBuf;
use std::time::Duration;

use tessera_ipc::{ActorCapability, AgentHello, Client, ConnectionCapabilities};

use crate::identity::{IdentityError, IdentityStore};

/// How an agent presents its pairing credential at the handshake.
pub enum CredentialSource {
    /// A durable paired identity: the stored credential is presented, a
    /// newly issued credential is persisted atomically, and a recognized
    /// credential is confirmed against the recorded principal.
    Paired(IdentityStore),
    /// A launcher-injected credential, for example a compositor-spawned
    /// adapter that received an ephemeral credential over stdin. Presented
    /// as-is and never persisted; continuity is the launcher's
    /// responsibility.
    Injected(String),
}

/// One agent connection attempt.
pub struct ConnectParams {
    /// Path to the compositor IPC socket.
    pub socket: PathBuf,
    /// Coarse connection capabilities to request; the compositor intersects
    /// them with policy.
    pub capabilities: ConnectionCapabilities,
    /// Human-facing agent label shown in pairing prompts and principals.
    pub label: String,
    /// The operation ceiling this agent can ever request; the pairing
    /// prompt shows it and the approved ceiling is checked against it.
    pub requested: Vec<ActorCapability>,
    /// Credential policy for this connection.
    pub credential: CredentialSource,
    /// Bound for the handshake itself; the first connection may block on
    /// the interactive pairing prompt.
    pub handshake_timeout: Duration,
    /// Per-request I/O timeout installed immediately after the handshake.
    pub post_timeout: Duration,
}

/// An established agent connection plus the identity the compositor bound.
pub struct Connected {
    pub client: Client,
    /// The authenticated principal bound by the handshake.
    pub principal: String,
}

/// Connect to the compositor as an authenticated agent.
pub fn connect(params: &ConnectParams) -> Result<Connected, ConnectError> {
    let credential = match &params.credential {
        CredentialSource::Paired(store) => store.credential(),
        CredentialSource::Injected(credential) => Some(credential.clone()),
    };
    let client = Client::connect_agent_with_timeout(
        &params.socket,
        params.capabilities,
        None,
        AgentHello {
            label: Some(params.label.clone()),
            requested: params.requested.clone(),
            credential,
        },
        params.handshake_timeout,
    )
    .map_err(|source| ConnectError::Ipc {
        socket: params.socket.clone(),
        label: params.label.clone(),
        source,
    })?;
    client
        .set_io_timeout(Some(params.post_timeout))
        .map_err(|source| ConnectError::Ipc {
            socket: params.socket.clone(),
            label: params.label.clone(),
            source,
        })?;
    let issued = client.agent_issued().ok_or(ConnectError::MissingIdentity)?;
    let principal = issued.principal.clone();
    if let CredentialSource::Paired(store) = &params.credential {
        if let Some(credential) = &issued.credential {
            store.store(&issued.principal, credential)?;
        } else {
            store.confirm_principal(&issued.principal)?;
        }
    }
    Ok(Connected { client, principal })
}

/// Read a launcher-injected credential from stdin: one line, bounded and
/// validated as the compositor-issued hex token.
pub fn read_credential_from_stdin() -> io::Result<String> {
    use std::io::{BufRead as _, Read as _};
    let mut credential = String::new();
    std::io::stdin()
        .lock()
        .take(257)
        .read_line(&mut credential)?;
    let credential = credential.trim_end_matches(['\r', '\n']).to_owned();
    if credential.len() < 32
        || credential.len() > 256
        || !credential.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid compositor-issued credential",
        ));
    }
    Ok(credential)
}

#[derive(Debug, thiserror::Error)]
pub enum ConnectError {
    #[error("cannot connect to Tessera IPC socket {socket:?} as agent {label:?}: {source}")]
    Ipc {
        socket: PathBuf,
        label: String,
        #[source]
        source: io::Error,
    },
    #[error("the compositor handshake did not bind an authenticated agent identity")]
    MissingIdentity,
    #[error("agent identity continuity failed: {0}")]
    Identity(#[from] IdentityError),
}
