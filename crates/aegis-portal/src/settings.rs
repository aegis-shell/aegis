//! `org.freedesktop.impl.portal.Settings` v1.
//!
//! The compositor's revisioned IPC snapshot is the only input. This backend
//! exports the standardized `org.freedesktop.appearance` keys and a curated
//! `org.gnome.desktop.interface` compatibility namespace used by GTK
//! applications. It never reads Aegis TOML, dconf, or another desktop's
//! settings database.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use aegis_core::settings::{ColorScheme, Contrast, DesktopPreferences};
use zbus::zvariant::{OwnedValue, Str, Structure};

pub(crate) const APPEARANCE_NAMESPACE: &str = "org.freedesktop.appearance";
pub(crate) const GTK_INTERFACE_NAMESPACE: &str = "org.gnome.desktop.interface";
pub(crate) const COLOR_SCHEME_KEY: &str = "color-scheme";
pub(crate) const ACCENT_COLOR_KEY: &str = "accent-color";
pub(crate) const CONTRAST_KEY: &str = "contrast";
pub(crate) const REDUCED_MOTION_KEY: &str = "reduced-motion";
const SETTINGS_IFACE: &str = "org.freedesktop.impl.portal.Settings";
const RECONNECT_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);
const IPC_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

#[derive(Clone, Default)]
pub(crate) struct SettingsStore {
    preferences: Arc<RwLock<DesktopPreferences>>,
}

impl SettingsStore {
    fn snapshot(&self) -> DesktopPreferences {
        self.preferences
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn replace(&self, preferences: DesktopPreferences) -> DesktopPreferences {
        let mut current = self
            .preferences
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        std::mem::replace(&mut *current, preferences)
    }
}

#[derive(Clone)]
pub(crate) struct SettingsIface {
    store: SettingsStore,
}

impl SettingsIface {
    pub(crate) fn new(store: SettingsStore) -> Self {
        Self { store }
    }
}

#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.freedesktop.portal.Error")]
enum SettingsError {
    NotFound(String),
    #[zbus(error)]
    ZBus(zbus::Error),
}

fn string_value(value: &str) -> OwnedValue {
    OwnedValue::from(Str::from(value.to_owned()))
}

fn structure_value<T>(value: T) -> OwnedValue
where
    Structure<'static>: From<T>,
{
    OwnedValue::try_from(Structure::from(value)).expect("static setting structure is owned")
}

fn portal_color_scheme(value: ColorScheme) -> u32 {
    match value {
        ColorScheme::System => 0,
        ColorScheme::Dark => 1,
        ColorScheme::Light => 2,
    }
}

fn gtk_color_scheme(value: ColorScheme) -> &'static str {
    match value {
        ColorScheme::System => "default",
        ColorScheme::Dark => "prefer-dark",
        ColorScheme::Light => "prefer-light",
    }
}

fn setting_entries(
    preferences: &DesktopPreferences,
) -> Vec<(&'static str, &'static str, OwnedValue)> {
    let mut entries = vec![
        (
            APPEARANCE_NAMESPACE,
            COLOR_SCHEME_KEY,
            OwnedValue::from(portal_color_scheme(preferences.color_scheme)),
        ),
        (
            APPEARANCE_NAMESPACE,
            CONTRAST_KEY,
            OwnedValue::from(match preferences.contrast {
                Contrast::Normal => 0_u32,
                Contrast::High => 1_u32,
            }),
        ),
        (
            APPEARANCE_NAMESPACE,
            REDUCED_MOTION_KEY,
            OwnedValue::from(u32::from(preferences.reduced_motion)),
        ),
        (
            GTK_INTERFACE_NAMESPACE,
            COLOR_SCHEME_KEY,
            string_value(gtk_color_scheme(preferences.color_scheme)),
        ),
        (
            GTK_INTERFACE_NAMESPACE,
            "font-name",
            string_value(&preferences.font_name),
        ),
        (
            GTK_INTERFACE_NAMESPACE,
            "monospace-font-name",
            string_value(&preferences.monospace_font_name),
        ),
        (
            GTK_INTERFACE_NAMESPACE,
            "text-scaling-factor",
            OwnedValue::from(preferences.text_scale),
        ),
        (
            GTK_INTERFACE_NAMESPACE,
            "icon-theme",
            string_value(&preferences.icon_theme),
        ),
        (
            GTK_INTERFACE_NAMESPACE,
            "cursor-theme",
            string_value(&preferences.cursor_theme),
        ),
        (
            GTK_INTERFACE_NAMESPACE,
            "cursor-size",
            OwnedValue::from(preferences.cursor_size as i32),
        ),
        (
            GTK_INTERFACE_NAMESPACE,
            "enable-animations",
            OwnedValue::from(!preferences.reduced_motion),
        ),
    ];
    if let Some(accent) = preferences.accent_color {
        entries.push((
            APPEARANCE_NAMESPACE,
            ACCENT_COLOR_KEY,
            structure_value(accent.normalized()),
        ));
    }
    entries
}

pub(crate) fn lookup(
    preferences: &DesktopPreferences,
    namespace: &str,
    key: &str,
) -> Option<OwnedValue> {
    setting_entries(preferences).into_iter().find_map(
        |(candidate_namespace, candidate_key, value)| {
            (candidate_namespace == namespace && candidate_key == key).then_some(value)
        },
    )
}

fn namespace_matches(pattern: &str, namespace: &str) -> bool {
    pattern.is_empty()
        || pattern == namespace
        || pattern
            .strip_suffix('*')
            .is_some_and(|prefix| namespace.starts_with(prefix))
}

fn read_all_values(
    preferences: &DesktopPreferences,
    namespaces: &[String],
) -> HashMap<String, HashMap<String, OwnedValue>> {
    let mut out: HashMap<String, HashMap<String, OwnedValue>> = HashMap::new();
    for (namespace, key, value) in setting_entries(preferences) {
        let wanted = namespaces.is_empty()
            || namespaces
                .iter()
                .any(|pattern| namespace_matches(pattern, namespace));
        if wanted {
            out.entry(namespace.to_owned())
                .or_default()
                .insert(key.to_owned(), value);
        }
    }
    out
}

fn changed_settings(
    previous: &DesktopPreferences,
    current: &DesktopPreferences,
) -> Vec<(&'static str, &'static str, OwnedValue)> {
    let mut changed = Vec::new();
    if previous.color_scheme != current.color_scheme {
        changed.push((
            APPEARANCE_NAMESPACE,
            COLOR_SCHEME_KEY,
            OwnedValue::from(portal_color_scheme(current.color_scheme)),
        ));
        changed.push((
            GTK_INTERFACE_NAMESPACE,
            COLOR_SCHEME_KEY,
            string_value(gtk_color_scheme(current.color_scheme)),
        ));
    }
    if previous.accent_color != current.accent_color {
        let value = current
            .accent_color
            .map(|accent| structure_value(accent.normalized()))
            // The public portal treats out-of-range components as unset.
            .unwrap_or_else(|| structure_value((-1.0_f64, -1.0_f64, -1.0_f64)));
        changed.push((APPEARANCE_NAMESPACE, ACCENT_COLOR_KEY, value));
    }
    if previous.contrast != current.contrast {
        changed.push((
            APPEARANCE_NAMESPACE,
            CONTRAST_KEY,
            OwnedValue::from(match current.contrast {
                Contrast::Normal => 0_u32,
                Contrast::High => 1_u32,
            }),
        ));
    }
    if previous.reduced_motion != current.reduced_motion {
        changed.push((
            APPEARANCE_NAMESPACE,
            REDUCED_MOTION_KEY,
            OwnedValue::from(u32::from(current.reduced_motion)),
        ));
        changed.push((
            GTK_INTERFACE_NAMESPACE,
            "enable-animations",
            OwnedValue::from(!current.reduced_motion),
        ));
    }
    if previous.font_name != current.font_name {
        changed.push((
            GTK_INTERFACE_NAMESPACE,
            "font-name",
            string_value(&current.font_name),
        ));
    }
    if previous.monospace_font_name != current.monospace_font_name {
        changed.push((
            GTK_INTERFACE_NAMESPACE,
            "monospace-font-name",
            string_value(&current.monospace_font_name),
        ));
    }
    if previous.text_scale != current.text_scale {
        changed.push((
            GTK_INTERFACE_NAMESPACE,
            "text-scaling-factor",
            OwnedValue::from(current.text_scale),
        ));
    }
    if previous.icon_theme != current.icon_theme {
        changed.push((
            GTK_INTERFACE_NAMESPACE,
            "icon-theme",
            string_value(&current.icon_theme),
        ));
    }
    if previous.cursor_theme != current.cursor_theme {
        changed.push((
            GTK_INTERFACE_NAMESPACE,
            "cursor-theme",
            string_value(&current.cursor_theme),
        ));
    }
    if previous.cursor_size != current.cursor_size {
        changed.push((
            GTK_INTERFACE_NAMESPACE,
            "cursor-size",
            OwnedValue::from(current.cursor_size as i32),
        ));
    }
    changed
}

fn update_and_emit(
    conn: &zbus::blocking::Connection,
    store: &SettingsStore,
    preferences: DesktopPreferences,
) {
    let previous = store.replace(preferences.clone());
    for (namespace, key, value) in changed_settings(&previous, &preferences) {
        if let Err(error) = conn.emit_signal(
            None::<&str>,
            crate::DESKTOP_PATH,
            SETTINGS_IFACE,
            "SettingChanged",
            &(namespace, key, value),
        ) {
            log::warn!("portal: could not emit SettingChanged for {namespace}/{key}: {error}");
        }
    }
}

pub(crate) fn spawn_watcher(
    conn: zbus::blocking::Connection,
    socket: PathBuf,
    store: SettingsStore,
) {
    std::thread::Builder::new()
        .name("aegis-portal-settings".to_owned())
        .spawn(move || watch_loop(conn, socket, store))
        .map(|_| ())
        .unwrap_or_else(|error| log::warn!("portal: cannot spawn settings watcher: {error}"));
}

/// Bound the initial IPC query before the D-Bus name is acquired, preventing
/// the first portal read from racing the subscription thread's first sample.
pub(crate) fn prime_store(socket: &Path, store: &SettingsStore) {
    let result = aegis_ipc::Client::connect_with_timeout(
        socket,
        aegis_ipc::Capabilities::QUERY,
        IPC_TIMEOUT,
    )
    .and_then(|mut client| client.settings());
    match result {
        Ok(snapshot) => {
            store.replace(snapshot.preferences);
        }
        Err(error) => log::warn!(
            "portal: starting with default desktop preferences; {} is unavailable: {error}",
            socket.display()
        ),
    }
}

fn watch_loop(conn: zbus::blocking::Connection, socket: PathBuf, store: SettingsStore) {
    let mut reported_disconnect = false;
    loop {
        match watch_connection(&conn, &socket, &store) {
            Ok(()) => unreachable!("settings subscription only exits on an IPC error"),
            Err(error) => {
                if !reported_disconnect {
                    log::warn!(
                        "portal: compositor settings IPC unavailable at {}: {error}",
                        socket.display()
                    );
                    reported_disconnect = true;
                } else {
                    log::debug!("portal: compositor settings IPC still unavailable: {error}");
                }
            }
        }
        std::thread::sleep(RECONNECT_INTERVAL);
    }
}

fn watch_connection(
    conn: &zbus::blocking::Connection,
    socket: &Path,
    store: &SettingsStore,
) -> std::io::Result<()> {
    let mut events = aegis_ipc::Client::connect_with_timeout(
        socket,
        aegis_ipc::Capabilities::QUERY,
        IPC_TIMEOUT,
    )?;
    events.subscribe()?;
    events.set_io_timeout(None)?;

    let mut query = aegis_ipc::Client::connect_with_timeout(
        socket,
        aegis_ipc::Capabilities::QUERY,
        IPC_TIMEOUT,
    )?;
    update_and_emit(conn, store, query.settings()?.preferences);
    log::info!("portal: subscribed to compositor desktop preferences");

    loop {
        if let aegis_ipc::Event::SettingsChanged { .. } = events.next_event()? {
            update_and_emit(conn, store, query.settings()?.preferences);
        }
    }
}

#[zbus::interface(name = "org.freedesktop.impl.portal.Settings")]
impl SettingsIface {
    /// `v Read(s namespace, s key)`.
    async fn read(&self, namespace: &str, key: &str) -> Result<OwnedValue, SettingsError> {
        lookup(&self.store.snapshot(), namespace, key)
            .ok_or_else(|| SettingsError::NotFound(format!("{namespace} {key}")))
    }

    /// `a{sa{sv}} ReadAll(as namespaces)`. An empty list asks for every
    /// supported namespace. An empty string also matches all, and patterns
    /// ending in `*` select a namespace prefix.
    async fn read_all(
        &self,
        namespaces: Vec<String>,
    ) -> HashMap<String, HashMap<String, OwnedValue>> {
        read_all_values(&self.store.snapshot(), &namespaces)
    }

    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        // This is the backend interface version. The public frontend exposes
        // org.freedesktop.portal.Settings v2 (including ReadOne).
        1
    }

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
    use aegis_core::settings::AccentColor;

    #[test]
    fn standardized_appearance_values_have_portal_types() {
        let preferences = DesktopPreferences {
            color_scheme: ColorScheme::Dark,
            accent_color: Some(AccentColor {
                red: 51,
                green: 102,
                blue: 255,
            }),
            contrast: Contrast::High,
            reduced_motion: true,
            ..Default::default()
        };
        assert_eq!(
            u32::try_from(lookup(&preferences, APPEARANCE_NAMESPACE, COLOR_SCHEME_KEY).unwrap())
                .unwrap(),
            1
        );
        assert_eq!(
            u32::try_from(lookup(&preferences, APPEARANCE_NAMESPACE, CONTRAST_KEY).unwrap())
                .unwrap(),
            1
        );
        assert_eq!(
            u32::try_from(lookup(&preferences, APPEARANCE_NAMESPACE, REDUCED_MOTION_KEY).unwrap())
                .unwrap(),
            1
        );
        let accent: (f64, f64, f64) = lookup(&preferences, APPEARANCE_NAMESPACE, ACCENT_COLOR_KEY)
            .unwrap()
            .try_into()
            .unwrap();
        assert_eq!(accent, (0.2, 0.4, 1.0));
    }

    #[test]
    fn gtk_namespace_is_a_curated_compatibility_projection() {
        let preferences = DesktopPreferences {
            icon_theme: "Papirus".into(),
            reduced_motion: true,
            ..Default::default()
        };
        assert_eq!(
            String::try_from(lookup(&preferences, GTK_INTERFACE_NAMESPACE, "icon-theme").unwrap())
                .unwrap(),
            "Papirus"
        );
        assert!(
            !bool::try_from(
                lookup(&preferences, GTK_INTERFACE_NAMESPACE, "enable-animations").unwrap()
            )
            .unwrap()
        );
        assert!(lookup(&preferences, GTK_INTERFACE_NAMESPACE, "gtk-theme").is_none());
    }

    #[test]
    fn read_all_supports_exact_and_prefix_namespace_filters() {
        let preferences = DesktopPreferences::default();
        let exact = read_all_values(&preferences, &[APPEARANCE_NAMESPACE.into()]);
        assert!(exact.contains_key(APPEARANCE_NAMESPACE));
        assert!(!exact.contains_key(GTK_INTERFACE_NAMESPACE));

        let prefix = read_all_values(&preferences, &["org.gnome.*".into()]);
        assert!(prefix.contains_key(GTK_INTERFACE_NAMESPACE));
        assert!(!prefix.contains_key(APPEARANCE_NAMESPACE));

        let all = read_all_values(&preferences, &["".into()]);
        assert!(all.contains_key(APPEARANCE_NAMESPACE));
        assert!(all.contains_key(GTK_INTERFACE_NAMESPACE));
    }

    #[test]
    fn change_projection_emits_only_dependent_keys() {
        let previous = DesktopPreferences::default();
        let current = DesktopPreferences {
            reduced_motion: true,
            ..previous.clone()
        };
        let changed = changed_settings(&previous, &current);
        assert_eq!(changed.len(), 2);
        assert!(changed.iter().any(|(_, key, _)| *key == REDUCED_MOTION_KEY));
        assert!(
            changed
                .iter()
                .any(|(_, key, _)| *key == "enable-animations")
        );
    }
}
