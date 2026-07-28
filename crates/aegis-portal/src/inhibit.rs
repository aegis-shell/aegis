//! `org.freedesktop.impl.portal.Inhibit` v1, idle-only path.
//!
//! Of the spec's four flags — 1 logout, 2 user switch, 4 idle, 8 suspend —
//! only 4 (idle) is servable: the other three belong to a session manager
//! aegis does not have, so they are logged and ignored. Flag 4 lands on the
//! compositor's surfaceless global idle inhibitor over the scoped IPC
//! (`SetIdleInhibit`, ADR-0053): the portal has no Wayland surface to hang
//! a `zwp_idle_inhibit_v1` object on.
//!
//! `Inhibit` is fire-and-forget (no `Response` signal), so the method only
//! validates and enqueues; the inhibit worker applies count transitions to
//! the IPC. Counts are kept per app_id; because v1 has no uninhibit call, a
//! sender's inhibits are released when its bus name vanishes (polled via
//! `NameHasOwner`), and the compositor releases everything when the portal
//! process dies. `QueryEndResponse` is declared for introspection but never
//! emitted — it is the session-end notification, and there is no session
//! manager to end the session.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use zbus::zvariant::{ObjectPath, Value};

use crate::ipc::PortalIdle;

/// The only flag this backend serves: idle inhibition.
pub(crate) const INHIBIT_IDLE: u32 = 4;
/// How often the sender monitor re-checks tracked bus names.
const MONITOR_INTERVAL: Duration = Duration::from_secs(2);

/// One job handed from the bus method or the sender monitor to the inhibit
/// worker.
pub(crate) enum InhibitJob {
    Inhibit {
        app_id: String,
        sender: String,
    },
    /// The sender's bus name vanished; drop every inhibit it holds.
    ReleaseSender {
        sender: String,
    },
}

/// Per-app_id inhibit counts. Multiple `Inhibit` calls from one app stack;
/// the compositor-facing flag is the fold over all of them.
#[derive(Default)]
pub(crate) struct InhibitCounts {
    /// app_id → (owning sender, count).
    counts: HashMap<String, (String, u32)>,
}

impl InhibitCounts {
    fn add(&mut self, app_id: &str, sender: &str) -> usize {
        let entry = self
            .counts
            .entry(app_id.to_string())
            .or_insert_with(|| (sender.to_string(), 0));
        // A restarted application re-inhibits from a new bus name; the
        // latest caller owns the stacked count for release purposes.
        entry.0 = sender.to_string();
        entry.1 += 1;
        self.total()
    }

    /// Drop every inhibit owned by `sender`, returning the remaining total.
    fn release_sender(&mut self, sender: &str) -> usize {
        self.counts.retain(|_, (owner, _)| owner != sender);
        self.total()
    }

    fn total(&self) -> usize {
        self.counts.values().map(|(_, count)| *count as usize).sum()
    }

    fn senders(&self) -> Vec<String> {
        let mut senders: Vec<String> = self
            .counts
            .values()
            .map(|(sender, _)| sender.clone())
            .collect();
        senders.sort();
        senders.dedup();
        senders
    }
}

/// The served Inhibit interface. Stateless apart from the job channel: the
/// method never blocks on the executor.
pub(crate) struct InhibitIface {
    pub(crate) jobs: mpsc::Sender<InhibitJob>,
}

#[zbus::interface(name = "org.freedesktop.impl.portal.Inhibit")]
impl InhibitIface {
    /// `Inhibit(o handle, s app_id, s window, u flags, a{sv} options)`.
    /// Fire-and-forget per the v1 contract: no `Response` signal.
    async fn inhibit(
        &self,
        _handle: ObjectPath<'_>,
        app_id: &str,
        _window: &str,
        flags: u32,
        _options: HashMap<String, Value<'_>>,
        #[zbus(header)] header: zbus::message::Header<'_>,
    ) -> zbus::fdo::Result<()> {
        let unsupported = flags & !INHIBIT_IDLE;
        if unsupported != 0 {
            log::info!(
                "portal: Inhibit for '{app_id}' requests unsupported flags {unsupported:#x} \
                 (logout/user-switch/suspend need a session manager); ignoring them"
            );
        }
        if flags & INHIBIT_IDLE == 0 {
            return Ok(());
        }
        let sender = header
            .sender()
            .map(|name| name.as_str().to_string())
            .unwrap_or_else(|| "unknown".to_string());
        log::info!("portal: idle inhibit for '{app_id}' from {sender}");
        self.jobs
            .send(InhibitJob::Inhibit {
                app_id: app_id.to_string(),
                sender,
            })
            .map_err(|_| zbus::fdo::Error::Failed("inhibit worker is gone".to_string()))
    }

    #[zbus(property)]
    fn version(&self) -> u32 {
        1
    }

    /// Declared so introspection advertises the signal; never emitted — it
    /// notifies applications of an impending session end, and aegis has no
    /// session manager to end the session.
    #[zbus(signal)]
    async fn query_end_response(
        emitter: &zbus::object_server::SignalEmitter<'_>,
    ) -> zbus::Result<()>;
}

/// Worker loop: one job at a time, folding per-app counts into the single
/// compositor-facing inhibitor on 0↔1 transitions.
pub(crate) fn inhibit_worker(
    rx: mpsc::Receiver<InhibitJob>,
    counts: Arc<Mutex<InhibitCounts>>,
    socket: PathBuf,
) {
    let mut idle = PortalIdle::new(socket);
    let mut ipc_inhibited = false;
    while let Ok(job) = rx.recv() {
        let total = {
            let mut counts = counts.lock().unwrap();
            match job {
                InhibitJob::Inhibit { app_id, sender } => counts.add(&app_id, &sender),
                InhibitJob::ReleaseSender { sender } => {
                    log::info!("portal: sender {sender} vanished; releasing its idle inhibits");
                    counts.release_sender(&sender)
                }
            }
        };
        let want = total > 0;
        if want == ipc_inhibited {
            continue;
        }
        match idle.set_inhibit(want) {
            Ok(()) => {
                ipc_inhibited = want;
                log::info!("portal: global idle inhibit {want} ({total} app inhibit(s))");
            }
            // Keep the old state: the next transition retries. A dead
            // compositor connection self-heals on the reconnect inside
            // `set_inhibit`; a persistent failure is logged, never fatal.
            Err(error) => log::warn!("portal: could not set idle inhibit {want}: {error}"),
        }
    }
}

/// Poll tracked senders and hand releases to the worker. v1 has no
/// uninhibit call, so a vanished bus name is the only per-app release
/// signal; the alternative (subscribing to `NameOwnerChanged`) would pull
/// zbus's async signal stream into a blocking-connection process for one
/// signal.
pub(crate) fn sender_monitor(
    conn: zbus::blocking::Connection,
    counts: Arc<Mutex<InhibitCounts>>,
    jobs: mpsc::Sender<InhibitJob>,
) {
    loop {
        std::thread::sleep(MONITOR_INTERVAL);
        let senders = counts.lock().unwrap().senders();
        for sender in senders {
            match name_has_owner(&conn, &sender) {
                Ok(true) => {}
                Ok(false) => {
                    if jobs.send(InhibitJob::ReleaseSender { sender }).is_err() {
                        return;
                    }
                }
                Err(error) => {
                    log::warn!("portal: NameHasOwner({sender}) failed: {error}");
                }
            }
        }
    }
}

/// `org.freedesktop.DBus.NameHasOwner` over the blocking connection.
fn name_has_owner(conn: &zbus::blocking::Connection, name: &str) -> zbus::Result<bool> {
    let reply = conn.call_method(
        Some("org.freedesktop.DBus"),
        "/org/freedesktop/DBus",
        Some("org.freedesktop.DBus"),
        "NameHasOwner",
        &name,
    )?;
    reply.body().deserialize()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_stack_per_app_and_fold_to_a_total() {
        let mut counts = InhibitCounts::default();
        assert_eq!(counts.add("org.example.A", ":1.1"), 1);
        assert_eq!(counts.add("org.example.A", ":1.1"), 2);
        assert_eq!(counts.add("org.example.B", ":1.2"), 3);
        assert_eq!(
            counts.senders(),
            vec![":1.1".to_string(), ":1.2".to_string()]
        );
    }

    #[test]
    fn releasing_a_sender_drops_only_its_apps() {
        let mut counts = InhibitCounts::default();
        counts.add("org.example.A", ":1.1");
        counts.add("org.example.B", ":1.2");
        counts.add("org.example.C", ":1.2");
        assert_eq!(counts.release_sender(":1.2"), 1);
        assert_eq!(counts.senders(), vec![":1.1".to_string()]);
        // Releasing twice is a no-op; releasing the last sender empties.
        assert_eq!(counts.release_sender(":1.2"), 1);
        assert_eq!(counts.release_sender(":1.1"), 0);
    }
}
