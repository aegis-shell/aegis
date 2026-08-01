//! `org.freedesktop.impl.portal.Secret` v1: the native, permanent secret
//! portal interface.
//!
//! Sandboxed applications retrieve a master secret through this interface;
//! the portal frontend then encrypts per-application secrets with it. The
//! served secret is derived from the vault master key with HKDF-SHA256
//! (`info = b"aegis.portal.Secret/v1"`) so the raw vault key never leaves
//! the process, and it is written to the caller-supplied file descriptor
//! rather than returned over D-Bus.
//!
//! This module depends only on `SecretState` and the vault — never on the
//! transitional `compat` layer, so deleting `compat/` cannot affect it.
//!
//! Response codes follow the portal specification: 0 success, 1 cancelled
//! (the client called `Request.Close` first, or dismissed the unlock
//! prompt), 2 other error.

use std::collections::HashMap;
use std::os::fd::AsFd;
use std::sync::{Arc, Mutex};

use hkdf::Hkdf;
use sha2::Sha256;
use zbus::zvariant::{Fd, ObjectPath, Value};
use zeroize::Zeroize;

use super::SecretState;
use crate::request::RequestTracker;

/// The served interface version.
pub(crate) const SECRET_VERSION: u32 = 1;

/// HKDF-Expand info separating the portal secret from every other use of
/// the vault master key.
const PORTAL_SECRET_INFO: &[u8] = b"aegis.portal.Secret/v1";

/// Derive the 32-byte secret handed to the portal frontend. No salt (the
/// master key is already a uniform key), fixed info for domain separation.
pub(crate) fn derive_portal_secret(master_key: &[u8; 32]) -> [u8; 32] {
    let hk = Hkdf::<Sha256>::new(None, master_key);
    let mut out = [0u8; 32];
    hk.expand(PORTAL_SECRET_INFO, &mut out)
        .expect("a 32-byte HKDF-SHA256 output is always valid");
    out
}

/// The served secret interface.
pub(crate) struct SecretIface {
    /// Blocking handle onto the same connection; the unlock worker uses it
    /// for post-unlock registrations, served methods use `.inner()` (they
    /// already run on zbus's executor — screenshot precedent).
    pub(crate) conn: zbus::blocking::Connection,
    pub(crate) tracker: Arc<Mutex<RequestTracker>>,
    pub(crate) state: Arc<Mutex<SecretState>>,
    /// The compositor IPC socket the unlock prompt is served over.
    pub(crate) socket: std::path::PathBuf,
}

#[zbus::interface(name = "org.freedesktop.impl.portal.Secret")]
impl SecretIface {
    /// `RetrieveSecret(o handle, s app_id, h fd, a{sv} options)` with the
    /// spec-conformant `(response, results)` reply.
    async fn retrieve_secret(
        &self,
        handle: ObjectPath<'_>,
        app_id: String,
        fd: Fd<'_>,
        _options: HashMap<String, Value<'_>>,
    ) -> zbus::fdo::Result<(u32, HashMap<String, Value<'static>>)> {
        let path = handle.as_str().to_string();
        log::info!("portal: RetrieveSecret for '{app_id}' at {path}");

        crate::request::register(self.conn.inner(), &self.tracker, &path).await?;
        let response = self.run(&path, fd).await;
        crate::request::finish(self.conn.inner(), &self.tracker, &path).await;
        response
    }

    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        SECRET_VERSION
    }
}

impl SecretIface {
    /// The body of `RetrieveSecret`. An unlocked vault answers inline; a
    /// locked one queues behind the shared unlock prompt — the spec and the
    /// frontend (whose impl proxy has no timeout) both tolerate this taking
    /// as long as the user needs to type the password.
    async fn run(
        &self,
        path: &str,
        fd: Fd<'_>,
    ) -> zbus::fdo::Result<(u32, HashMap<String, Value<'static>>)> {
        if self.tracker.lock().unwrap().was_closed(path) {
            log::info!("portal: RetrieveSecret at {path} cancelled by Request.Close");
            return Ok((1, HashMap::new()));
        }

        if self.state.lock().unwrap().is_unlocked() {
            return Ok(self.deliver(fd));
        }

        // Locked (password-mode vault without a PAM token): hand the fd to
        // the unlock coordinator and await the outcome. The state guard is
        // never held across this await.
        let owned_fd = match fd.as_fd().try_clone_to_owned() {
            Ok(owned) => owned,
            Err(error) => {
                log::warn!("portal: could not clone the RetrieveSecret fd: {error}");
                return Ok((2, HashMap::new()));
            }
        };
        let (outcome_tx, outcome_rx) = async_channel::bounded(1);
        super::enqueue_unlock_request(
            &self.state,
            &self.conn,
            &self.socket,
            super::PendingUnlock::PortalRetrieve {
                fd: owned_fd,
                outcome: outcome_tx,
            },
        );
        let outcome = outcome_rx
            .recv()
            .await
            .unwrap_or(super::PortalUnlockOutcome::Failed);

        if self.tracker.lock().unwrap().was_closed(path) {
            log::info!("portal: RetrieveSecret at {path} cancelled while unlocking");
            return Ok((1, HashMap::new()));
        }
        let code = match outcome {
            super::PortalUnlockOutcome::Delivered => 0,
            super::PortalUnlockOutcome::Dismissed => 1,
            super::PortalUnlockOutcome::Failed => 2,
        };
        Ok((code, HashMap::new()))
    }

    /// The unlocked fast path: derive the portal secret and stream it into
    /// the caller's fd. The state guard is dropped before any fd I/O.
    fn deliver(&self, fd: Fd<'_>) -> (u32, HashMap<String, Value<'static>>) {
        let mut secret = {
            let state = self.state.lock().unwrap();
            let Some(vault) = &state.vault else {
                return (2, HashMap::new());
            };
            derive_portal_secret(vault.get_master_key())
        };

        // The fd passed through the frontend is usually a pipe, not a
        // socket; write+close delivers EOF for both shapes.
        let written = fd
            .as_fd()
            .try_clone_to_owned()
            .and_then(|owned| super::write_secret_fd(owned, &secret));
        secret.zeroize();
        match written {
            Ok(()) => (0, HashMap::new()),
            Err(error) => {
                log::warn!("portal: could not write the secret to the RetrieveSecret fd: {error}");
                (2, HashMap::new())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portal_secret_is_deterministic_and_distinct_from_the_master_key() {
        let master_key = [0x5au8; 32];
        let first = derive_portal_secret(&master_key);
        let second = derive_portal_secret(&master_key);
        assert_eq!(first, second);
        assert_ne!(first, master_key);
    }
}
