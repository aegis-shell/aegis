//! `org.freedesktop.impl.portal.Settings` v1.
//!
//! The single supported key is `color-scheme` in the
//! `org.freedesktop.appearance` namespace, mapped from the compositor
//! configuration's `[appearance] color_scheme` (see
//! `docs/reference/config.md`). The file is re-read per call: it is small,
//! reads are rare, and re-reading keeps the backend honest across the
//! compositor's live reload without any watcher thread.
//!
//! `SettingChanged` is emitted by a light watcher thread that polls the
//! configuration file's mtime (ADR-0053): appearance lives in the config
//! file rather than the revisioned IPC settings snapshot, so there is no
//! IPC event to subscribe to. The poll interval is two seconds — enough
//! for a live-reload-driven theme flip, cheap enough to run forever.

use std::collections::HashMap;

use zbus::zvariant::OwnedValue;

/// The freedesktop appearance namespace.
pub(crate) const APPEARANCE_NAMESPACE: &str = "org.freedesktop.appearance";
/// The color-scheme key: 0 = no preference, 1 = prefer dark, 2 = prefer
/// light.
pub(crate) const COLOR_SCHEME_KEY: &str = "color-scheme";
/// The settings interface name, used by the watcher's signal emission.
const SETTINGS_IFACE: &str = "org.freedesktop.impl.portal.Settings";
/// How often the watcher re-stats the configuration file.
const WATCH_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);

/// The served settings interface. Stateless: every call re-reads the
/// configuration file.
pub(crate) struct SettingsIface;

/// Resolve one setting against a parsed configuration. `None` means the
/// namespace/key pair is not ours and the caller answers `NotFound`-style.
pub(crate) fn lookup(
    config: Option<&aegis_config::Config>,
    namespace: &str,
    key: &str,
) -> Option<u32> {
    match (namespace, key) {
        (APPEARANCE_NAMESPACE, COLOR_SCHEME_KEY) => Some(color_scheme(config)),
        _ => None,
    }
}

/// Map the configured preference onto the portal enum. A missing or invalid
/// configuration means "no preference".
pub(crate) fn color_scheme(config: Option<&aegis_config::Config>) -> u32 {
    match config.map(|c| c.appearance.color_scheme) {
        Some(aegis_config::ColorScheme::Dark) => 1,
        Some(aegis_config::ColorScheme::Light) => 2,
        _ => 0,
    }
}

/// Load the compositor configuration, tolerating every failure mode as "no
/// preference": a missing file is the common case and a malformed one is
/// already reported by the compositor's own diagnostics.
fn load_config() -> Option<aegis_config::Config> {
    let path = aegis_config::default_path()?;
    load_config_at(&path)
}

/// The path-parameterized half of [`load_config`], split out for the
/// watcher and tests.
fn load_config_at(path: &std::path::Path) -> Option<aegis_config::Config> {
    match aegis_config::load(path) {
        Ok(config) => config,
        Err(error) => {
            log::warn!(
                "portal: cannot read {} for settings: {error}",
                path.display()
            );
            None
        }
    }
}

/// The effective color-scheme at `path` — the watcher's sample.
fn scheme_at(path: &std::path::Path) -> u32 {
    color_scheme(load_config_at(path).as_ref())
}

/// Spawn the mtime watcher that emits `SettingChanged` when the mapped
/// color-scheme value changes. No configuration path means no watcher (the
/// `Read` half answers "no preference" from defaults anyway).
pub(crate) fn spawn_watcher(conn: zbus::blocking::Connection) {
    let Some(path) = aegis_config::default_path() else {
        log::info!("portal: no config path; SettingChanged watcher disabled");
        return;
    };
    std::thread::Builder::new()
        .name("aegis-portal-settings".to_string())
        .spawn(move || watch_loop(conn, path))
        .map(|_| ())
        .unwrap_or_else(|error| log::warn!("portal: cannot spawn settings watcher: {error}"));
}

/// Poll the file's mtime and emit on an effective-value change, including
/// the file appearing or disappearing (both change the mapped value).
fn watch_loop(conn: zbus::blocking::Connection, path: std::path::PathBuf) {
    let mut last_mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
    let mut scheme = scheme_at(&path);
    loop {
        std::thread::sleep(WATCH_INTERVAL);
        let mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        if mtime == last_mtime {
            continue;
        }
        last_mtime = mtime;
        let current = scheme_at(&path);
        if current == scheme {
            continue;
        }
        scheme = current;
        log::info!("portal: color-scheme changed → {scheme}");
        if let Err(error) = conn.emit_signal(
            None::<&str>,
            crate::DESKTOP_PATH,
            SETTINGS_IFACE,
            "SettingChanged",
            &(
                APPEARANCE_NAMESPACE,
                COLOR_SCHEME_KEY,
                zbus::zvariant::Value::from(scheme),
            ),
        ) {
            log::warn!("portal: could not emit SettingChanged: {error}");
        }
    }
}

#[zbus::interface(name = "org.freedesktop.impl.portal.Settings")]
impl SettingsIface {
    /// `v Read(s namespace, s key)`.
    async fn read(&self, namespace: &str, key: &str) -> zbus::fdo::Result<OwnedValue> {
        let config = load_config();
        lookup(config.as_ref(), namespace, key)
            .map(OwnedValue::from)
            .ok_or_else(|| zbus::fdo::Error::Failed(format!("unknown setting {namespace} {key}")))
    }

    /// `a{sa{sv}} ReadAll(as namespaces)`. An empty list asks for every
    /// supported namespace; namespaces we do not implement are skipped.
    async fn read_all(
        &self,
        namespaces: Vec<String>,
    ) -> HashMap<String, HashMap<String, OwnedValue>> {
        let config = load_config();
        let mut out = HashMap::new();
        let wanted = |ns: &str| namespaces.is_empty() || namespaces.iter().any(|n| n == ns);
        if wanted(APPEARANCE_NAMESPACE) {
            out.insert(
                APPEARANCE_NAMESPACE.to_string(),
                HashMap::from([(
                    COLOR_SCHEME_KEY.to_string(),
                    OwnedValue::from(color_scheme(config.as_ref())),
                )]),
            );
        }
        out
    }

    #[zbus(property)]
    fn version(&self) -> u32 {
        1
    }

    /// Emitted by the watcher thread when the mapped color-scheme changes.
    #[zbus(signal)]
    async fn setting_changed(
        emitter: &zbus::object_server::SignalEmitter<'_>,
        namespace: &str,
        key: &str,
        value: zbus::zvariant::Value<'_>,
    ) -> zbus::Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_with(toml: &str) -> Option<aegis_config::Config> {
        aegis_config::Config::parse(toml).ok()
    }

    #[test]
    fn color_scheme_maps_config_to_portal_enum() {
        assert_eq!(color_scheme(None), 0);
        assert_eq!(color_scheme(config_with("schema_version = 1").as_ref()), 0);
        assert_eq!(
            color_scheme(
                config_with("schema_version = 1\n[appearance]\ncolor_scheme = \"dark\"").as_ref()
            ),
            1
        );
        assert_eq!(
            color_scheme(
                config_with("schema_version = 1\n[appearance]\ncolor_scheme = \"light\"").as_ref()
            ),
            2
        );
        assert_eq!(
            color_scheme(
                config_with("schema_version = 1\n[appearance]\ncolor_scheme = \"system\"").as_ref()
            ),
            0
        );
    }

    #[test]
    fn lookup_only_knows_the_appearance_namespace() {
        let config = config_with("schema_version = 1\n[appearance]\ncolor_scheme = \"dark\"");
        assert_eq!(
            lookup(config.as_ref(), APPEARANCE_NAMESPACE, COLOR_SCHEME_KEY),
            Some(1)
        );
        assert_eq!(
            lookup(config.as_ref(), APPEARANCE_NAMESPACE, "accent-color"),
            None
        );
        assert_eq!(
            lookup(
                config.as_ref(),
                "org.gnome.desktop.interface",
                "color-scheme"
            ),
            None
        );
    }

    #[test]
    fn scheme_at_tracks_file_content_and_absence() {
        let dir = std::env::temp_dir().join(format!(
            "aegis-portal-settings-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        // Missing file → no preference.
        assert_eq!(scheme_at(&path), 0);
        std::fs::write(
            &path,
            "schema_version = 1\n[appearance]\ncolor_scheme = \"dark\"\n",
        )
        .unwrap();
        assert_eq!(scheme_at(&path), 1);
        std::fs::write(
            &path,
            "schema_version = 1\n[appearance]\ncolor_scheme = \"light\"\n",
        )
        .unwrap();
        assert_eq!(scheme_at(&path), 2);
        // Deleted again → no preference.
        std::fs::remove_file(&path).unwrap();
        assert_eq!(scheme_at(&path), 0);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
