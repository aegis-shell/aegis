//! `org.freedesktop.impl.portal.FileChooser` v3: native file picking.
//!
//! `OpenFile`, `SaveFile`, and `SaveFiles` export a
//! `org.freedesktop.impl.portal.Request` object at the caller's `handle`,
//! then hand the request to a dedicated worker which runs the compositor's
//! file-picker chrome over scoped IPC (`PickFile`, the user-consent pick
//! added in IPC version 13). The picker is ordinary modal chrome over the
//! live scene; no screen content is captured.
//!
//! Result URIs are always `file://` URIs, as the backend contract requires.
//! `SaveFiles` maps to a directory pick; the suggested file names from the
//! `files` option are appended to the chosen folder per the spec.
//!
//! Accepted-but-ignored options: `modal` and `parent_window` (the chrome is
//! always a session-modal overlay), `choices` combo boxes (no UI for them
//! yet), and `current_filter` preselection (the picker starts at the first
//! filter; the selected filter is still reported back).
//!
//! Response codes follow the portal specification: 0 success, 1 cancelled
//! (the client called `Request.Close` first, or the user dismissed the
//! picker), 2 other error.

use std::collections::HashMap;
use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};

use zbus::zvariant::{ObjectPath, Value};

use crate::files;
use crate::ipc::PortalCapture;
use crate::request::{PortalResponse, RequestTracker, ResponseSender};

/// The served interface version: 3 adds the `current_file` option to
/// `SaveFile` (2 added `SaveFiles`).
pub(crate) const FILE_CHOOSER_VERSION: u32 = 3;

/// One file-chooser request handed from the bus methods to the worker.
pub(crate) enum FileChooserJob {
    Choose {
        request_path: String,
        app_id: String,
        options: aegis_ipc::FilePickOptions,
        /// Suggested file names from the `SaveFiles` `files` option,
        /// appended to the chosen folder in the result URIs.
        save_files: Vec<OsString>,
        reply: ResponseSender,
    },
}

/// The served file-chooser interface. Methods only register the request
/// object and enqueue; the user-facing pick happens on the worker.
pub(crate) struct FileChooserIface {
    /// Async handle onto the same connection; only used inside served
    /// methods, which already run on zbus's executor (screenshot precedent).
    pub(crate) conn: zbus::Connection,
    pub(crate) tracker: Arc<Mutex<RequestTracker>>,
    pub(crate) jobs: mpsc::Sender<FileChooserJob>,
}

#[zbus::interface(name = "org.freedesktop.impl.portal.FileChooser")]
impl FileChooserIface {
    async fn open_file(
        &self,
        handle: ObjectPath<'_>,
        app_id: &str,
        _parent_window: &str,
        title: &str,
        options: HashMap<String, Value<'_>>,
    ) -> zbus::fdo::Result<PortalResponse> {
        let parsed = parse_options(&options);
        let pick = aegis_ipc::FilePickOptions {
            mode: aegis_ipc::FilePickMode::Open,
            multiple: parsed.multiple,
            directory: parsed.directory,
            title: dialog_title(title),
            accept_label: parsed.accept_label,
            current_folder: parsed.current_folder,
            current_name: None,
            filters: parsed.filters,
        };
        self.choose(handle, app_id, pick, Vec::new()).await
    }

    async fn save_file(
        &self,
        handle: ObjectPath<'_>,
        app_id: &str,
        _parent_window: &str,
        title: &str,
        options: HashMap<String, Value<'_>>,
    ) -> zbus::fdo::Result<PortalResponse> {
        let parsed = parse_options(&options);
        // `current_file` (saving an existing file) splits into the folder and
        // suggested name, overriding the individual options when present.
        let (current_folder, current_name) = match parsed.current_file {
            Some(file) => {
                let name = file
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .or(parsed.current_name);
                let folder = file.parent().map(PathBuf::from).or(parsed.current_folder);
                (folder, name)
            }
            None => (parsed.current_folder, parsed.current_name),
        };
        let pick = aegis_ipc::FilePickOptions {
            mode: aegis_ipc::FilePickMode::Save,
            multiple: false,
            directory: false,
            title: dialog_title(title),
            accept_label: parsed.accept_label,
            current_folder,
            current_name,
            filters: parsed.filters,
        };
        self.choose(handle, app_id, pick, Vec::new()).await
    }

    async fn save_files(
        &self,
        handle: ObjectPath<'_>,
        app_id: &str,
        _parent_window: &str,
        title: &str,
        options: HashMap<String, Value<'_>>,
    ) -> zbus::fdo::Result<PortalResponse> {
        let parsed = parse_options(&options);
        let pick = aegis_ipc::FilePickOptions {
            mode: aegis_ipc::FilePickMode::ChooseDir,
            multiple: false,
            directory: true,
            title: dialog_title(title),
            accept_label: parsed.accept_label,
            current_folder: parsed.current_folder,
            current_name: None,
            filters: Vec::new(),
        };
        self.choose(handle, app_id, pick, parsed.save_files).await
    }

    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        FILE_CHOOSER_VERSION
    }
}

impl FileChooserIface {
    /// Shared method body: register the request, enqueue the pick, await the
    /// worker (screenshot.rs precedent — zbus's executor never blocks).
    async fn choose(
        &self,
        handle: ObjectPath<'_>,
        app_id: &str,
        options: aegis_ipc::FilePickOptions,
        save_files: Vec<OsString>,
    ) -> zbus::fdo::Result<PortalResponse> {
        let path = handle.as_str().to_string();
        log::info!(
            "portal: FileChooser for '{app_id}' ({:?}) at {path}",
            options.mode
        );

        crate::request::register(&self.conn, &self.tracker, &path).await?;
        let (reply, response) = async_channel::bounded(1);
        let queued = self.jobs.send(FileChooserJob::Choose {
            request_path: path.clone(),
            app_id: app_id.to_string(),
            options,
            save_files,
            reply,
        });
        if queued.is_err() {
            crate::request::finish(&self.conn, &self.tracker, &path).await;
            return Err(zbus::fdo::Error::Failed(
                "file chooser worker is gone".to_string(),
            ));
        }
        let result = response.recv().await.map_err(|_| {
            zbus::fdo::Error::Failed("file chooser worker dropped its response".to_string())
        });
        crate::request::finish(&self.conn, &self.tracker, &path).await;
        result
    }
}

/// Worker loop: one pick at a time, serialized like captures. Each pick
/// blocks on user interaction, so a dedicated thread keeps it off both
/// zbus's executor and the capture worker.
pub(crate) fn file_chooser_worker(
    rx: mpsc::Receiver<FileChooserJob>,
    tracker: Arc<Mutex<RequestTracker>>,
    mut capture: PortalCapture,
) {
    while let Ok(FileChooserJob::Choose {
        request_path,
        app_id,
        options,
        save_files,
        reply,
    }) = rx.recv()
    {
        let result = run_pick(
            &mut capture,
            &tracker,
            &request_path,
            &app_id,
            options,
            save_files,
        );
        let _ = reply.send_blocking(result);
    }
}

/// Execute one pick and produce the `(response_code, results)` pair.
fn run_pick(
    capture: &mut PortalCapture,
    tracker: &Arc<Mutex<RequestTracker>>,
    request_path: &str,
    app_id: &str,
    options: aegis_ipc::FilePickOptions,
    save_files: Vec<OsString>,
) -> (u32, HashMap<String, Value<'static>>) {
    if tracker.lock().unwrap().was_closed(request_path) {
        return (1, HashMap::new());
    }
    let filters = options.filters.clone();
    match capture.pick_file(options) {
        Ok(aegis_ipc::FilePickResult::Paths { paths, filter }) => {
            // A Close racing the pick wins over a completed result.
            if tracker.lock().unwrap().was_closed(request_path) {
                return (1, HashMap::new());
            }
            let results = build_results(paths, filter, &filters, &save_files);
            log::info!("portal: FileChooser for '{app_id}' → {} uri(s)", results.0);
            (0, results.1)
        }
        Ok(aegis_ipc::FilePickResult::Cancelled) => (1, HashMap::new()),
        Err(error) => {
            log::warn!("portal: FileChooser for '{app_id}' failed: {error}");
            (2, HashMap::new())
        }
    }
}

/// Build the results vardict: `uris` plus the selected-filter echo
/// (`current_filter` and the `choices` pair) when filters were supplied.
/// Returns the uri count alongside for logging.
fn build_results(
    paths: Vec<PathBuf>,
    filter: Option<u32>,
    filters: &[aegis_ipc::FileFilter],
    save_files: &[OsString],
) -> (usize, HashMap<String, Value<'static>>) {
    // SaveFiles: the picker returned the chosen folder; append each
    // suggested name per the spec.
    let mut files: Vec<PathBuf> = if save_files.is_empty() {
        paths
    } else {
        let folder = paths.into_iter().next().unwrap_or_default();
        save_files.iter().map(|name| folder.join(name)).collect()
    };
    files.retain(|path| !path.as_os_str().is_empty());

    let uris: Vec<String> = files.iter().map(|path| files::file_uri(path)).collect();
    let count = uris.len();
    let mut results = HashMap::from([("uris".to_string(), Value::from(uris))]);

    if let Some(selected) = filter
        && let Some(active) = filters.get(selected as usize)
    {
        // Echo the selected filter as the spec's `(sa(us))` structure and as
        // the `choices` ("filters", label) pair; rules map back to the
        // (type, value) form, globs = 0, MIME = 1.
        let rules: Vec<(u32, String)> = active
            .patterns
            .iter()
            .map(|pattern| (pattern_type(pattern), pattern.clone()))
            .collect();
        results.insert(
            "current_filter".to_string(),
            Value::Structure(zbus::zvariant::Structure::from((
                active.label.clone(),
                rules,
            ))),
        );
        results.insert(
            "choices".to_string(),
            Value::from(vec![("filters".to_string(), active.label.clone())]),
        );
    }
    (count, results)
}

/// The serialized-filter rule type: 0 = glob pattern, 1 = MIME type.
fn pattern_type(pattern: &str) -> u32 {
    if pattern.contains('/') { 1 } else { 0 }
}

/// Options parsed out of the `a{sv}` argument, before the per-method mapping
/// onto `FilePickOptions`.
struct ParsedOptions {
    multiple: bool,
    directory: bool,
    accept_label: Option<String>,
    current_folder: Option<PathBuf>,
    current_name: Option<String>,
    current_file: Option<PathBuf>,
    filters: Vec<aegis_ipc::FileFilter>,
    save_files: Vec<OsString>,
}

/// Parse the shared FileChooser options dict. Wrongly typed keys are ignored
/// (screenshot.rs precedent); the per-method meaning is applied by the
/// caller.
fn parse_options(options: &HashMap<String, Value<'_>>) -> ParsedOptions {
    let get_bool = |key: &str| {
        options
            .get(key)
            .and_then(|value| bool::try_from(value).ok())
            .unwrap_or(false)
    };
    let get_string = |key: &str| {
        options
            .get(key)
            .and_then(|value| String::try_from(value).ok())
    };
    // Path options are NUL-terminated byte arrays on the wire.
    let get_path = |key: &str| {
        options
            .get(key)
            .and_then(|value| Vec::<u8>::try_from(value.clone()).ok())
            .map(|bytes| {
                let bytes: Vec<u8> = bytes.into_iter().take_while(|b| *b != 0).collect();
                PathBuf::from(OsString::from_vec(bytes))
            })
    };
    let filters = options
        .get("filters")
        .and_then(|value| Vec::<(String, Vec<(u32, String)>)>::try_from(value.clone()).ok())
        .unwrap_or_default()
        .into_iter()
        .map(|(label, rules)| aegis_ipc::FileFilter {
            label,
            patterns: rules.into_iter().map(|(_, value)| value).collect(),
        })
        .collect();
    let save_files = options
        .get("files")
        .and_then(|value| Vec::<Vec<u8>>::try_from(value.clone()).ok())
        .unwrap_or_default()
        .into_iter()
        .map(|bytes| {
            let bytes: Vec<u8> = bytes.into_iter().take_while(|b| *b != 0).collect();
            OsString::from_vec(bytes)
        })
        .collect();

    ParsedOptions {
        multiple: get_bool("multiple"),
        directory: get_bool("directory"),
        accept_label: get_string("accept_label"),
        current_folder: get_path("current_folder"),
        current_name: get_string("current_name"),
        current_file: get_path("current_file"),
        filters,
        save_files,
    }
}

/// An empty dialog title means "use the picker's per-mode default".
fn dialog_title(title: &str) -> Option<String> {
    if title.is_empty() {
        None
    } else {
        Some(title.to_string())
    }
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

    #[test]
    fn version_is_3() {
        assert_eq!(FILE_CHOOSER_VERSION, 3);
    }

    #[test]
    fn defaults_are_empty() {
        let parsed = parse_options(&HashMap::new());
        assert!(!parsed.multiple && !parsed.directory);
        assert!(parsed.filters.is_empty() && parsed.save_files.is_empty());
        assert!(parsed.current_folder.is_none() && parsed.current_file.is_none());
    }

    #[test]
    fn filters_parse_into_labels_and_patterns() {
        let rules: Vec<(u32, String)> = vec![(0, "*.png".into()), (1, "image/jpeg".into())];
        let parsed = parse_options(&options(&[(
            "filters",
            Value::from(vec![("Images".to_string(), rules)]),
        )]));
        assert_eq!(parsed.filters.len(), 1);
        assert_eq!(parsed.filters[0].label, "Images");
        assert_eq!(parsed.filters[0].patterns, ["*.png", "image/jpeg"]);
    }

    #[test]
    fn path_options_strip_trailing_nul() {
        let mut bytes = b"/tmp/docs".to_vec();
        bytes.push(0);
        let parsed = parse_options(&options(&[("current_folder", Value::from(bytes))]));
        assert_eq!(parsed.current_folder, Some(PathBuf::from("/tmp/docs")));
    }

    #[test]
    fn pattern_types_match_the_wire_encoding() {
        assert_eq!(pattern_type("*.png"), 0);
        assert_eq!(pattern_type("image/png"), 1);
    }

    #[test]
    fn results_contain_file_uris() {
        let (count, results) =
            build_results(vec![PathBuf::from("/tmp/a file.txt")], None, &[], &[]);
        assert_eq!(count, 1);
        let Value::Array(uris) = &results["uris"] else {
            panic!("uris must be an array");
        };
        assert_eq!(uris.len(), 1);
        let uri = String::try_from(uris.iter().next().unwrap()).unwrap();
        assert!(uri.starts_with("file://"), "{uri}");
    }

    #[test]
    fn save_files_appends_suggested_names_to_the_folder() {
        let (count, results) = build_results(
            vec![PathBuf::from("/chosen/dir")],
            None,
            &[],
            &[OsString::from("one.txt"), OsString::from("two.txt")],
        );
        assert_eq!(count, 2);
        let Value::Array(uris) = &results["uris"] else {
            panic!("uris must be an array");
        };
        let first = String::try_from(uris.iter().next().unwrap()).unwrap();
        assert!(first.ends_with("/chosen/dir/one.txt"), "{first}");
    }

    #[test]
    fn selected_filter_is_echoed_as_current_filter_and_choices() {
        let filters = vec![aegis_ipc::FileFilter {
            label: "Images".into(),
            patterns: vec!["*.png".into(), "image/jpeg".into()],
        }];
        let (_, results) = build_results(vec![PathBuf::from("/tmp/x.png")], Some(0), &filters, &[]);
        assert!(results.contains_key("current_filter"));
        let Value::Array(choices) = &results["choices"] else {
            panic!("choices must be an array");
        };
        assert_eq!(choices.len(), 1);
    }
}
