//! `org.freedesktop.impl.portal.Inhibit`, backed by scoped idle inhibition.
//!
//! Aegis can inhibit idle only. The freedesktop backend ABI assigns idle to
//! flag 8; logout (1), user switch (2), and suspend (4) require session
//! services Aegis does not own and are rejected instead of being silently
//! reported as active.
//!
//! Every accepted call exports an `org.freedesktop.impl.portal.Request` at
//! the exact `handle` supplied by the frontend. `Request.Close` releases that
//! one inhibitor. The worker also periodically reasserts active inhibition,
//! which renews the scoped IPC lease and recovers after a compositor restart.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use zbus::zvariant::{ObjectPath, Value};

use crate::ipc::PortalIdle;

/// The freedesktop Inhibit flag for idle.
pub(crate) const INHIBIT_IDLE: u32 = 8;
pub(crate) const INHIBIT_VERSION: u32 = 1;
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(30);

pub(crate) enum InhibitJob {
    Add {
        request_path: String,
        app_id: String,
    },
    Release {
        request_path: String,
    },
}

/// Active idle inhibitors keyed by backend Request object path.
#[derive(Default)]
pub(crate) struct InhibitCounts {
    requests: HashMap<String, String>,
}

impl InhibitCounts {
    fn add(&mut self, request_path: &str, app_id: &str) -> usize {
        self.requests
            .insert(request_path.to_string(), app_id.to_string());
        self.requests.len()
    }

    fn release(&mut self, request_path: &str) -> usize {
        self.requests.remove(request_path);
        self.requests.len()
    }

    fn total(&self) -> usize {
        self.requests.len()
    }
}

pub(crate) struct InhibitIface {
    pub(crate) conn: zbus::Connection,
    pub(crate) jobs: mpsc::Sender<InhibitJob>,
}

#[zbus::interface(name = "org.freedesktop.impl.portal.Inhibit")]
impl InhibitIface {
    /// `Inhibit(o handle, s app_id, s window, u flags, a{sv} options)`.
    async fn inhibit(
        &self,
        handle: ObjectPath<'_>,
        app_id: &str,
        _window: &str,
        flags: u32,
        options: HashMap<String, Value<'_>>,
    ) -> zbus::fdo::Result<()> {
        let unsupported = flags & !INHIBIT_IDLE;
        if unsupported != 0 {
            return Err(zbus::fdo::Error::NotSupported(format!(
                "Aegis supports idle inhibition only; unsupported flags {unsupported:#x}"
            )));
        }
        if flags & INHIBIT_IDLE == 0 {
            return Err(zbus::fdo::Error::InvalidArgs(
                "Inhibit requires the idle flag (8)".to_string(),
            ));
        }
        if let Some(Value::Str(reason)) = options.get("reason") {
            log::info!("portal: idle inhibit for '{app_id}': {reason}");
        }

        let path = handle.as_str().to_string();
        let inserted = self
            .conn
            .object_server()
            .at(
                path.as_str(),
                InhibitRequestIface {
                    path: path.clone(),
                    jobs: self.jobs.clone(),
                },
            )
            .await
            .map_err(zbus::fdo::Error::from)?;
        if !inserted {
            return Err(zbus::fdo::Error::Failed(format!(
                "request handle {path} is already active"
            )));
        }
        if self
            .jobs
            .send(InhibitJob::Add {
                request_path: path.clone(),
                app_id: app_id.to_string(),
            })
            .is_err()
        {
            let _ = self
                .conn
                .object_server()
                .remove::<InhibitRequestIface, _>(path.as_str())
                .await;
            return Err(zbus::fdo::Error::Failed(
                "inhibit worker is gone".to_string(),
            ));
        }
        log::info!("portal: idle inhibit for '{app_id}' at {path}");
        Ok(())
    }

    /// Aegis has no session-state monitor, so fail explicitly instead of
    /// exposing a partial Session object.
    async fn create_monitor(
        &self,
        _handle: ObjectPath<'_>,
        _session_handle: ObjectPath<'_>,
        app_id: &str,
        _window: &str,
    ) -> u32 {
        log::info!("portal: session monitor for '{app_id}' is unsupported");
        2
    }

    async fn query_end_response(&self, _session_handle: ObjectPath<'_>) -> zbus::fdo::Result<()> {
        Err(zbus::fdo::Error::NotSupported(
            "Aegis has no session-end monitor".to_string(),
        ))
    }

    #[zbus(property)]
    fn version(&self) -> u32 {
        INHIBIT_VERSION
    }
}

/// The request object owned by one accepted idle-inhibit call.
pub(crate) struct InhibitRequestIface {
    path: String,
    jobs: mpsc::Sender<InhibitJob>,
}

#[zbus::interface(name = "org.freedesktop.impl.portal.Request")]
impl InhibitRequestIface {
    async fn close(&self) -> zbus::fdo::Result<()> {
        log::info!("portal: releasing idle inhibit {}", self.path);
        self.jobs
            .send(InhibitJob::Release {
                request_path: self.path.clone(),
            })
            .map_err(|_| zbus::fdo::Error::Failed("inhibit worker is gone".to_string()))
    }
}

pub(crate) fn inhibit_worker(
    rx: mpsc::Receiver<InhibitJob>,
    counts: Arc<Mutex<InhibitCounts>>,
    socket: PathBuf,
    conn: zbus::blocking::Connection,
) {
    let mut idle = PortalIdle::new(socket);
    let mut ipc_inhibited = false;
    loop {
        let job = match rx.recv_timeout(KEEPALIVE_INTERVAL) {
            Ok(job) => Some(job),
            Err(mpsc::RecvTimeoutError::Timeout) => None,
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        };
        let keepalive = job.is_none();

        let mut remove_path = None;
        let total = {
            let mut counts = counts.lock().unwrap();
            match job {
                Some(InhibitJob::Add {
                    request_path,
                    app_id,
                }) => counts.add(&request_path, &app_id),
                Some(InhibitJob::Release { request_path }) => {
                    let total = counts.release(&request_path);
                    remove_path = Some(request_path);
                    total
                }
                None => counts.total(),
            }
        };
        let want = total > 0;

        // A timeout deliberately reasserts `true`: this renews the scoped
        // lease and reconnects after compositor restarts.
        if want != ipc_inhibited || (keepalive && want) {
            match idle.set_inhibit(want) {
                Ok(()) => {
                    ipc_inhibited = want;
                    log::info!("portal: global idle inhibit {want} ({total} request(s))");
                }
                Err(error) => {
                    log::warn!("portal: could not set idle inhibit {want}: {error}");
                }
            }
        }

        if let Some(path) = remove_path
            && let Err(error) = conn
                .object_server()
                .remove::<InhibitRequestIface, _>(path.as_str())
        {
            log::warn!("portal: could not remove inhibit request {path}: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_uses_the_freedesktop_flag_value() {
        assert_eq!(INHIBIT_IDLE, 8);
    }

    #[test]
    fn inhibit_version_is_one() {
        assert_eq!(INHIBIT_VERSION, 1);
    }

    #[test]
    fn requests_are_independent_and_idempotently_released() {
        let mut counts = InhibitCounts::default();
        assert_eq!(counts.add("/r/1", "org.example.A"), 1);
        assert_eq!(counts.add("/r/2", "org.example.A"), 2);
        assert_eq!(counts.add("/r/3", "org.example.B"), 3);
        assert_eq!(counts.release("/r/2"), 2);
        assert_eq!(counts.release("/r/2"), 2);
        assert_eq!(counts.release("/r/1"), 1);
        assert_eq!(counts.release("/r/3"), 0);
    }
}
