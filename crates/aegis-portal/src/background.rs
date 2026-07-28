//! `org.freedesktop.impl.portal.Background` v1.
//!
//! `RequestBackground` follows the same request/response shape as
//! `Screenshot`: a `Request` object at the `handle_token`-derived path, the
//! decision computed on a dedicated worker, then `Response` with
//! `{background, autostart}` results and the object removed.
//!
//! aegis has no PermissionStore, so the authorization decision is persisted
//! by the backend itself as `$XDG_DATA_HOME/aegis-portal/background.json`
//! (ADR-0053) and repeated requests from the same app_id are answered from
//! the recorded decision. The policy is deliberately simple: there is no
//! running-application tracking, so a requested `background = true` is
//! granted by default and recorded (the grant is a bookkeeping answer, not
//! an enforced sandbox property — aegis does not confine background
//! execution). `autostart = true` is materialized the standard way, by
//! copying the application's desktop file into
//! `$XDG_CONFIG_HOME/autostart/`; it is reported `false` when no source
//! desktop file exists, and requesting `autostart = false` removes the
//! file. `dbus-activatable` needs no action (activation goes over the
//! session bus directly) and is accepted and ignored.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};

use zbus::zvariant::{ObjectPath, OwnedObjectPath, Value};

use crate::request::{RequestIface, RequestTracker};
use crate::screencast::fallback_token;
use crate::screenshot::{request_path, sanitize_token};
use crate::state;

const REQUEST_IFACE: &str = "org.freedesktop.impl.portal.Request";
/// The state document holding every recorded Background decision.
const BACKGROUND_DOC: &str = "background.json";

/// One background request handed from the bus method to the worker.
pub(crate) struct BackgroundJob {
    request_path: String,
    app_id: String,
    background: bool,
    autostart: bool,
}

/// Options parsed out of the `RequestBackground` argument.
pub(crate) struct BackgroundOptions {
    pub(crate) handle_token: Option<String>,
    pub(crate) background: bool,
    pub(crate) autostart: bool,
}

/// Parse the `RequestBackground` options dict. Unknown keys are ignored per
/// spec; `reason` and `dbus-activatable` are accepted but need no handling
/// (see the module docs).
pub(crate) fn parse_options(options: &HashMap<String, Value<'_>>) -> BackgroundOptions {
    let boolean = |key: &str| {
        options
            .get(key)
            .and_then(|value| bool::try_from(value).ok())
            .unwrap_or(false)
    };
    let handle_token = options
        .get("handle_token")
        .and_then(|value| match value {
            Value::Str(token) => Some(token.to_string()),
            _ => None,
        })
        .filter(|token| !token.is_empty() && sanitize_token(token) == *token);
    if let Some(Value::Str(reason)) = options.get("reason") {
        log::info!("portal: background reason: {reason}");
    }
    BackgroundOptions {
        handle_token,
        background: boolean("background"),
        autostart: boolean("autostart"),
    }
}

/// A portal app_id doubles as a desktop-file id, so it must stay inside the
/// desktop-entry character set — the same set that keeps the autostart file
/// name inside its directory.
pub(crate) fn valid_app_id(app_id: &str) -> bool {
    !app_id.is_empty()
        && app_id.len() <= 255
        && app_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// The recorded authorization decision for one app_id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Decision {
    pub(crate) background: bool,
    pub(crate) autostart: bool,
}

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct BackgroundDoc {
    decisions: HashMap<String, Decision>,
}

/// The persisted decision store. Owned by the background worker; every
/// mutation is written through atomically.
pub(crate) struct BackgroundStore {
    dir: Option<PathBuf>,
    doc: BackgroundDoc,
}

impl BackgroundStore {
    pub(crate) fn load(dir: Option<PathBuf>) -> Self {
        let doc = dir
            .as_ref()
            .and_then(|dir| state::read_json(dir, BACKGROUND_DOC))
            .unwrap_or_default();
        Self { dir, doc }
    }

    pub(crate) fn decision(&self, app_id: &str) -> Option<Decision> {
        self.doc.decisions.get(app_id).copied()
    }

    /// Record a decision and persist the document. A missing state
    /// directory degrades to in-memory-only decisions (logged once per
    /// write), never to a failed request.
    pub(crate) fn set(&mut self, app_id: &str, decision: Decision) {
        self.doc.decisions.insert(app_id.to_string(), decision);
        if let Some(dir) = &self.dir
            && let Err(error) = state::write_json(dir, BACKGROUND_DOC, &self.doc)
        {
            log::warn!("portal: cannot persist background decisions: {error}");
        }
    }
}

/// `$XDG_CONFIG_HOME/autostart`, else `$HOME/.config/autostart`.
fn autostart_dir() -> Option<PathBuf> {
    autostart_dir_from(
        std::env::var_os("XDG_CONFIG_HOME"),
        std::env::var_os("HOME"),
    )
}

/// Split out for tests: environment variables are process-global.
fn autostart_dir_from(
    config: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
) -> Option<PathBuf> {
    let base = config
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            home.filter(|dir| !dir.is_empty())
                .map(|home| PathBuf::from(home).join(".config"))
        })?;
    Some(base.join("autostart"))
}

/// Locate `<app_id>.desktop` in the applications directories
/// (`$XDG_DATA_HOME/applications`, then each `$XDG_DATA_DIRS` entry,
/// defaulting to `/usr/local/share:/usr/share`).
fn find_desktop_file(app_id: &str) -> Option<PathBuf> {
    let name = format!("{app_id}.desktop");
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(data_home) = std::env::var_os("XDG_DATA_HOME").filter(|dir| !dir.is_empty()) {
        dirs.push(PathBuf::from(data_home).join("applications"));
    } else if let Some(home) = std::env::var_os("HOME").filter(|dir| !dir.is_empty()) {
        dirs.push(PathBuf::from(home).join(".local/share/applications"));
    }
    let data_dirs = std::env::var("XDG_DATA_DIRS")
        .unwrap_or_else(|_| "/usr/local/share:/usr/share".to_string());
    for dir in data_dirs.split(':').filter(|dir| !dir.is_empty()) {
        dirs.push(PathBuf::from(dir).join("applications"));
    }
    dirs.into_iter()
        .map(|dir| dir.join(&name))
        .find(|candidate| candidate.is_file())
}

/// Apply an autostart decision idempotently. Enabling copies the source
/// desktop file (an existing file is left alone); disabling removes the
/// file. Returns whether the autostart entry is in place afterwards.
fn apply_autostart(
    dir: &Path,
    app_id: &str,
    enable: bool,
    source: Option<&Path>,
) -> std::io::Result<bool> {
    let name = format!("{app_id}.desktop");
    let target = dir.join(&name);
    if !enable {
        match std::fs::remove_file(&target) {
            Ok(()) => log::info!("portal: removed autostart entry {}", target.display()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        return Ok(false);
    }
    let Some(source) = source else {
        return Ok(false);
    };
    if target.exists() {
        return Ok(true);
    }
    let bytes = std::fs::read(source)?;
    state::write_atomic(dir, &name, &bytes, 0o644)?;
    log::info!(
        "portal: installed autostart entry {} from {}",
        target.display(),
        source.display()
    );
    Ok(true)
}

/// The served Background interface. Methods register the request object and
/// enqueue; the worker owns the decision store and the autostart file.
pub(crate) struct BackgroundIface {
    pub(crate) conn: zbus::Connection,
    pub(crate) tracker: Arc<Mutex<RequestTracker>>,
    pub(crate) jobs: mpsc::Sender<BackgroundJob>,
}

#[zbus::interface(name = "org.freedesktop.impl.portal.Background")]
impl BackgroundIface {
    /// `o RequestBackground(o handle, s app_id, s parent_window, a{sv} options)`.
    async fn request_background(
        &self,
        handle: ObjectPath<'_>,
        app_id: &str,
        _parent_window: &str,
        options: HashMap<String, Value<'_>>,
        #[zbus(header)] header: zbus::message::Header<'_>,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        if !valid_app_id(app_id) {
            return Err(zbus::fdo::Error::InvalidArgs(format!(
                "invalid app_id '{app_id}'"
            )));
        }
        let options = parse_options(&options);
        let sender = header.sender().map(|name| name.as_str());
        let token = options
            .handle_token
            .unwrap_or_else(|| fallback_token(&handle));
        let path = request_path(sender, &token);
        log::info!(
            "portal: RequestBackground for '{app_id}' (background={}, autostart={}) at {path}",
            options.background,
            options.autostart
        );

        self.conn
            .object_server()
            .at(
                path.as_str(),
                RequestIface {
                    path: path.clone(),
                    tracker: Arc::clone(&self.tracker),
                },
            )
            .await
            .map_err(zbus::fdo::Error::from)?;
        self.jobs
            .send(BackgroundJob {
                request_path: path.clone(),
                app_id: app_id.to_string(),
                background: options.background,
                autostart: options.autostart,
            })
            .map_err(|_| zbus::fdo::Error::Failed("background worker is gone".to_string()))?;
        OwnedObjectPath::try_from(path).map_err(|error| zbus::fdo::Error::Failed(error.to_string()))
    }

    #[zbus(property)]
    fn version(&self) -> u32 {
        1
    }
}

/// Worker loop: one request at a time. File writes are milliseconds, so
/// serializing background requests is the natural pacing.
pub(crate) fn background_worker(
    rx: mpsc::Receiver<BackgroundJob>,
    conn: zbus::blocking::Connection,
    tracker: Arc<Mutex<RequestTracker>>,
    mut store: BackgroundStore,
) {
    let autostart_dir = autostart_dir();
    while let Ok(job) = rx.recv() {
        let (code, results) = run_job(&mut store, autostart_dir.as_deref(), &tracker, &job);
        if let Err(error) = conn.emit_signal(
            None::<&str>,
            job.request_path.as_str(),
            REQUEST_IFACE,
            "Response",
            &(code, results),
        ) {
            log::warn!(
                "portal: could not emit Response for {}: {error}",
                job.request_path
            );
        }
        if let Err(error) = conn
            .object_server()
            .remove::<RequestIface, _>(job.request_path.as_str())
        {
            log::warn!("portal: could not remove {}: {error}", job.request_path);
        }
        tracker.lock().unwrap().forget(&job.request_path);
    }
}

/// Decide one request and produce the `(response_code, results)` pair.
fn run_job(
    store: &mut BackgroundStore,
    autostart_dir: Option<&Path>,
    tracker: &Arc<Mutex<RequestTracker>>,
    job: &BackgroundJob,
) -> (u32, HashMap<String, Value<'static>>) {
    if tracker.lock().unwrap().was_closed(&job.request_path) {
        return (1, HashMap::new());
    }
    // A recorded decision wins over fresh defaults: the user-facing policy
    // must not flip between identical requests (ADR-0053).
    let requested = Decision {
        background: job.background,
        autostart: job.autostart,
    };
    let mut decision = store.decision(&job.app_id).unwrap_or(requested);

    // Materialize the autostart half; the background half is a bookkeeping
    // answer (no running-application tracking exists to enforce it).
    let installed = match (decision.autostart, autostart_dir) {
        (true, Some(dir)) => {
            let source = find_desktop_file(&job.app_id);
            match apply_autostart(dir, &job.app_id, true, source.as_deref()) {
                Ok(installed) => installed,
                Err(error) => {
                    log::warn!("portal: autostart for '{}' failed: {error}", job.app_id);
                    false
                }
            }
        }
        (true, None) => {
            log::warn!("portal: no autostart directory; refusing autostart");
            false
        }
        (false, Some(dir)) => {
            let _ = apply_autostart(dir, &job.app_id, false, None);
            false
        }
        (false, None) => false,
    };
    if decision.autostart && !installed {
        log::info!(
            "portal: autostart for '{}' requested but unavailable",
            job.app_id
        );
        decision.autostart = false;
    }
    if store.decision(&job.app_id) != Some(decision) {
        store.set(&job.app_id, decision);
    }
    log::info!(
        "portal: background decision for '{}': background={}, autostart={}",
        job.app_id,
        decision.background,
        decision.autostart
    );
    (
        0,
        HashMap::from([
            ("background".to_string(), Value::from(decision.background)),
            ("autostart".to_string(), Value::from(decision.autostart)),
        ]),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(pairs: &[(&str, Value<'static>)]) -> HashMap<String, Value<'static>> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    fn scratch() -> PathBuf {
        std::env::temp_dir().join(format!(
            "aegis-portal-bg-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn options_default_to_false_without_token() {
        let parsed = parse_options(&HashMap::new());
        assert!(!parsed.background);
        assert!(!parsed.autostart);
        assert_eq!(parsed.handle_token, None);
    }

    #[test]
    fn options_parse_flags_and_token() {
        let parsed = parse_options(&options(&[
            ("background", Value::from(true)),
            ("autostart", Value::from(true)),
            ("handle_token", Value::from("bg1")),
            ("reason", Value::from("downloads")),
        ]));
        assert!(parsed.background && parsed.autostart);
        assert_eq!(parsed.handle_token.as_deref(), Some("bg1"));
    }

    #[test]
    fn app_ids_follow_desktop_file_id_rules() {
        assert!(valid_app_id("org.example.App"));
        assert!(valid_app_id("a-b_c"));
        assert!(!valid_app_id(""));
        assert!(!valid_app_id("../evil"));
        assert!(!valid_app_id("a/b"));
        assert!(!valid_app_id("with space"));
    }

    #[test]
    fn decision_store_round_trips_through_the_state_file() {
        let dir = scratch();
        let mut store = BackgroundStore::load(Some(dir.clone()));
        assert_eq!(store.decision("org.example.App"), None);
        store.set(
            "org.example.App",
            Decision {
                background: true,
                autostart: true,
            },
        );
        let reloaded = BackgroundStore::load(Some(dir.clone()));
        assert_eq!(
            reloaded.decision("org.example.App"),
            Some(Decision {
                background: true,
                autostart: true
            })
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn decisions_degrade_to_memory_without_a_state_dir() {
        let mut store = BackgroundStore::load(None);
        store.set(
            "org.example.App",
            Decision {
                background: true,
                autostart: false,
            },
        );
        assert_eq!(
            store.decision("org.example.App"),
            Some(Decision {
                background: true,
                autostart: false
            })
        );
    }

    #[test]
    fn autostart_file_is_written_and_removed() {
        let autostart = scratch();
        let source_dir = scratch();
        std::fs::create_dir_all(&source_dir).unwrap();
        let source = source_dir.join("org.example.App.desktop");
        std::fs::write(&source, b"[Desktop Entry]\nType=Application\n").unwrap();

        // Enabling without a source reports not-installed.
        assert!(!apply_autostart(&autostart, "org.example.App", true, None).unwrap());
        // Enabling copies the source; the copy is world-readable like every
        // desktop file.
        assert!(apply_autostart(&autostart, "org.example.App", true, Some(&source)).unwrap());
        let installed = autostart.join("org.example.App.desktop");
        assert_eq!(
            std::fs::read(&installed).unwrap(),
            b"[Desktop Entry]\nType=Application\n"
        );
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&installed).unwrap().permissions().mode() & 0o777,
            0o644
        );
        // Disabling removes the entry and is idempotent.
        assert!(!apply_autostart(&autostart, "org.example.App", false, None).unwrap());
        assert!(!installed.exists());
        assert!(!apply_autostart(&autostart, "org.example.App", false, None).unwrap());
        std::fs::remove_dir_all(&autostart).unwrap();
        std::fs::remove_dir_all(&source_dir).unwrap();
    }

    #[test]
    fn autostart_dir_prefers_config_home() {
        let config = std::ffi::OsString::from("/cfg");
        let home = std::ffi::OsString::from("/home/u");
        assert_eq!(
            autostart_dir_from(Some(config), Some(home.clone())),
            Some(PathBuf::from("/cfg/autostart"))
        );
        assert_eq!(
            autostart_dir_from(None, Some(home)),
            Some(PathBuf::from("/home/u/.config/autostart"))
        );
        assert_eq!(autostart_dir_from(None, None), None);
    }
}
