//! `org.freedesktop.impl.portal.Email` v2: compose-email requests.
//!
//! `ComposeEmail` stages any attachment fds into the portal cache directory
//! (they are pipe ends whose contents vanish with the call) and hands the
//! message to the session's preferred mail client through `xdg-email`
//! (`--cc`/`--bcc`/`--subject`/`--body`/`--attach`; the recipient list goes
//! as a `mailto:` URI). The hand-off is fire-and-forget: the mail client
//! owns the compose window from there. `AEGIS_PORTAL_MAILER` overrides the
//! mailer command (tests, sessions without xdg-utils).
//!
//! The request is not interactive on our side, so no worker thread: the
//! `Request` object is exported for spec shape, the mailer is spawned, and
//! the method answers immediately. Response codes follow the portal
//! specification: 0 handed off, 1 cancelled (`Request.Close` raced in),
//! 2 other error.

use std::collections::HashMap;
use std::io::Read;
use std::os::fd::AsFd;
use std::sync::{Arc, Mutex};

use zbus::zvariant::{Fd, ObjectPath, Value};

use crate::files;
use crate::request::{PortalResponse, RequestTracker};

/// The served interface version: 2 added `activation_token` (accepted and
/// ignored — the mail client is not portal-activated).
pub(crate) const EMAIL_VERSION: u32 = 2;

/// The served email interface.
pub(crate) struct EmailIface {
    /// Async handle onto the same connection; only used inside served
    /// methods, which already run on zbus's executor (screenshot precedent).
    pub(crate) conn: zbus::Connection,
    pub(crate) tracker: Arc<Mutex<RequestTracker>>,
}

#[zbus::interface(name = "org.freedesktop.impl.portal.Email")]
impl EmailIface {
    async fn compose_email(
        &self,
        handle: ObjectPath<'_>,
        app_id: &str,
        _parent_window: &str,
        options: HashMap<String, Value<'_>>,
    ) -> zbus::fdo::Result<PortalResponse> {
        let path = handle.as_str().to_string();
        log::info!("portal: ComposeEmail for '{app_id}' at {path}");

        crate::request::register(&self.conn, &self.tracker, &path).await?;
        let response = compose(app_id, &path, &options, &self.tracker);
        crate::request::finish(&self.conn, &self.tracker, &path).await;
        response
    }

    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        EMAIL_VERSION
    }
}

/// Build the mailer invocation and spawn it. Split from the zbus method so
/// the whole flow is testable without a bus.
fn compose(
    app_id: &str,
    request_path: &str,
    options: &HashMap<String, Value<'_>>,
    tracker: &Arc<Mutex<RequestTracker>>,
) -> zbus::fdo::Result<PortalResponse> {
    if tracker.lock().unwrap().was_closed(request_path) {
        return Ok((1, HashMap::new()));
    }

    let mut parsed = ParsedOptions::from(options);
    let attachments = stage_attachments(std::mem::take(&mut parsed.attachments));
    let argv = mailer_argv(&parsed, &attachments);

    let program = mailer_command();
    log::info!(
        "portal: ComposeEmail for '{app_id}' → {program} ({} attachment(s), {} arg(s))",
        attachments.len(),
        argv.len()
    );
    match std::process::Command::new(&program).args(&argv).spawn() {
        // The mail client runs on its own; reaping it later would need a
        // reaper thread, so detach by simply dropping the handle — the
        // process reparents to init when short-lived, and a mail composer
        // outliving the portal is fine.
        Ok(_) => Ok((0, HashMap::new())),
        Err(error) => {
            log::warn!("portal: could not spawn {program}: {error}");
            Ok((2, HashMap::new()))
        }
    }
}

/// The mailer command: `xdg-email` unless overridden (tests point this at a
/// recorder script).
fn mailer_command() -> String {
    std::env::var("AEGIS_PORTAL_MAILER").unwrap_or_else(|_| "xdg-email".to_string())
}

/// Options parsed out of the `a{sv}` argument.
struct ParsedOptions {
    addresses: Vec<String>,
    cc: Vec<String>,
    bcc: Vec<String>,
    subject: Option<String>,
    body: Option<String>,
    attachments: Vec<Vec<u8>>,
}

impl ParsedOptions {
    fn from(options: &HashMap<String, Value<'_>>) -> Self {
        let string_list = |key: &str| {
            options
                .get(key)
                .and_then(|value| Vec::<String>::try_from(value.clone()).ok())
                .unwrap_or_default()
        };
        let mut addresses = string_list("addresses");
        if let Some(address) = options
            .get("address")
            .and_then(|value| String::try_from(value).ok())
        {
            addresses.push(address);
        }
        // Attachment fds are read eagerly: their contents are only valid for
        // the duration of the call.
        let attachments = options
            .get("attachment_fds")
            .and_then(|value| Vec::<Fd>::try_from(value.clone()).ok())
            .unwrap_or_default()
            .into_iter()
            .map(|fd| {
                let mut bytes = Vec::new();
                if let Ok(owned) = fd.as_fd().try_clone_to_owned() {
                    let _ = std::fs::File::from(owned).read_to_end(&mut bytes);
                }
                bytes
            })
            .filter(|bytes| !bytes.is_empty())
            .collect();

        ParsedOptions {
            addresses,
            cc: string_list("cc"),
            bcc: string_list("bcc"),
            subject: options
                .get("subject")
                .and_then(|value| String::try_from(value).ok()),
            body: options
                .get("body")
                .and_then(|value| String::try_from(value).ok()),
            attachments,
        }
    }
}

/// Write attachment payloads to the cache directory so the mail client can
/// open real paths; returns the staged paths. Failures are logged and the
/// message goes without that attachment rather than failing outright.
fn stage_attachments(attachments: Vec<Vec<u8>>) -> Vec<std::path::PathBuf> {
    let Some(dir) = files::cache_dir() else {
        if !attachments.is_empty() {
            log::warn!("portal: dropping attachment(s): no cache directory available");
        }
        return Vec::new();
    };
    stage_attachments_in(&dir, attachments)
}

/// The filesystem half of [`stage_attachments`], split out so tests can
/// point it at a temporary directory (the cache dir is environment-global).
fn stage_attachments_in(
    dir: &std::path::Path,
    attachments: Vec<Vec<u8>>,
) -> Vec<std::path::PathBuf> {
    attachments
        .into_iter()
        .enumerate()
        .filter_map(|(index, bytes)| {
            let path = files::write_blob(dir, &format!("attachment{index}"), &bytes)
                .map_err(|error| log::warn!("portal: could not stage attachment {index}: {error}"))
                .ok()?;
            Some(path)
        })
        .collect()
}

/// The mailer argument vector: recipients as a `mailto:` URI, everything
/// else as xdg-email flags.
fn mailer_argv(parsed: &ParsedOptions, attachments: &[std::path::PathBuf]) -> Vec<String> {
    let mut argv = Vec::new();
    for address in &parsed.cc {
        argv.push("--cc".to_string());
        argv.push(address.clone());
    }
    for address in &parsed.bcc {
        argv.push("--bcc".to_string());
        argv.push(address.clone());
    }
    if let Some(subject) = &parsed.subject {
        argv.push("--subject".to_string());
        argv.push(subject.clone());
    }
    if let Some(body) = &parsed.body {
        argv.push("--body".to_string());
        argv.push(body.clone());
    }
    for path in attachments {
        argv.push("--attach".to_string());
        argv.push(path.to_string_lossy().into_owned());
    }
    argv.push(format!("mailto:{}", parsed.addresses.join(",")));
    argv
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
    fn version_is_2() {
        assert_eq!(EMAIL_VERSION, 2);
    }

    #[test]
    fn address_and_addresses_merge_into_the_mailto_uri() {
        let parsed = ParsedOptions::from(&options(&[
            ("address", Value::from("first@example.com")),
            (
                "addresses",
                Value::from(vec!["second@example.com".to_string()]),
            ),
        ]));
        let argv = mailer_argv(&parsed, &[]);
        assert_eq!(
            argv.last().unwrap(),
            "mailto:second@example.com,first@example.com"
        );
    }

    #[test]
    fn flags_cover_cc_bcc_subject_body_and_attachments() {
        let parsed = ParsedOptions::from(&options(&[
            ("cc", Value::from(vec!["carbon@example.com".to_string()])),
            ("bcc", Value::from(vec!["blind@example.com".to_string()])),
            ("subject", Value::from("a subject")),
            ("body", Value::from("the body")),
        ]));
        let attachments = vec![std::path::PathBuf::from("/cache/aegis-portal/attachment0")];
        let argv = mailer_argv(&parsed, &attachments);
        let flag_value = |flag: &str| {
            let at = argv.iter().position(|arg| arg == flag).unwrap();
            argv[at + 1].clone()
        };
        assert_eq!(flag_value("--cc"), "carbon@example.com");
        assert_eq!(flag_value("--bcc"), "blind@example.com");
        assert_eq!(flag_value("--subject"), "a subject");
        assert_eq!(flag_value("--body"), "the body");
        assert_eq!(flag_value("--attach"), "/cache/aegis-portal/attachment0");
    }

    #[test]
    fn empty_options_still_produce_a_mailto_uri() {
        let parsed = ParsedOptions::from(&HashMap::new());
        let argv = mailer_argv(&parsed, &[]);
        assert_eq!(argv.as_slice(), ["mailto:"]);
    }

    #[test]
    fn staging_writes_payloads_into_the_cache_dir() {
        let dir =
            std::env::temp_dir().join(format!("aegis-portal-email-test-{}", std::process::id()));
        let staged = stage_attachments_in(&dir, vec![b"payload".to_vec()]);
        assert_eq!(staged.len(), 1);
        assert_eq!(std::fs::read(&staged[0]).unwrap(), b"payload");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
