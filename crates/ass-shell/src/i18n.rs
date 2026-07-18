//! Locale negotiation and translated shell messages.
//!
//! Translation catalogs live in `locales/` and are embedded in the binary so
//! compositor chrome never performs filesystem I/O while rendering. The
//! strongly typed catalog makes a missing key a startup/test failure instead
//! of silently leaking an identifier into the UI.

use std::sync::OnceLock;

/// Languages for which the shell currently ships a complete catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    English,
    SimplifiedChinese,
}

impl Language {
    /// Negotiate a supported language from a POSIX locale or BCP-47 tag.
    /// Unknown locales fall back to English. Traditional-Chinese regions do
    /// not fall through to the Simplified-Chinese catalog.
    pub fn from_locale(locale: &str) -> Language {
        let base = locale
            .split('@')
            .next()
            .unwrap_or(locale)
            .split('.')
            .next()
            .unwrap_or(locale)
            .replace('_', "-")
            .to_ascii_lowercase();
        let subtags: Vec<&str> = base.split('-').filter(|part| !part.is_empty()).collect();
        match subtags.first().copied() {
            Some("en") => Language::English,
            Some("zh")
                if !subtags
                    .iter()
                    .any(|part| matches!(*part, "hant" | "tw" | "hk" | "mo")) =>
            {
                Language::SimplifiedChinese
            }
            _ => Language::English,
        }
    }

    /// Resolve the process locale using the POSIX message-locale precedence.
    pub fn from_env() -> Language {
        for variable in ["LC_ALL", "LC_MESSAGES", "LANG"] {
            if let Some(value) = std::env::var(variable)
                .ok()
                .filter(|value| !value.trim().is_empty())
            {
                return Language::from_locale(&value);
            }
        }
        Language::English
    }

    /// Canonical locale tag for logging, diagnostics, and host integration.
    pub fn locale(self) -> &'static str {
        match self {
            Language::English => "en-US",
            Language::SimplifiedChinese => "zh-CN",
        }
    }
}

/// A fixed shell message without runtime values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Message {
    Applications,
    Untitled,
    UntitledWindow,
    SearchApplications,
    NoApplicationsFound,
    TryAnotherSearch,
    PreviousWindows,
    MoreWindows,
    Open,
    NewWindow,
    MinimizeWindow,
    MinimizeAllWindows,
    CloseWindow,
    CloseAllWindows,
    PinToDock,
    UnpinFromDock,
    ControlCenter,
    BuiltInSystemApp,
    Connectivity,
    Wifi,
    Bluetooth,
    DoNotDisturb,
    Desktop,
    TiledLayout,
    OpenApplications,
    QuitSession,
    Sound,
    Brightness,
    Unavailable,
    Volume,
    WifiConnected,
    WiredConnected,
    Disconnected,
    Network,
    NoBatteryDetected,
    Battery,
    Notifications,
    RecentNotifications,
    NoNotifications,
    Muted,
}

/// Lightweight locale handle passed to chrome components for each frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Localizer {
    language: Language,
}

impl Localizer {
    pub fn new(locale: &str) -> Localizer {
        Localizer {
            language: Language::from_locale(locale),
        }
    }

    pub fn from_language(language: Language) -> Localizer {
        Localizer { language }
    }

    pub fn from_env() -> Localizer {
        Localizer::from_language(Language::from_env())
    }

    pub fn language(self) -> Language {
        self.language
    }

    pub fn locale(self) -> &'static str {
        self.language.locale()
    }

    /// Resolve a message whose translation has no runtime values.
    pub fn text(self, message: Message) -> &'static str {
        let catalog = catalog(self.language);
        match message {
            Message::Applications => &catalog.applications,
            Message::Untitled => &catalog.untitled,
            Message::UntitledWindow => &catalog.untitled_window,
            Message::SearchApplications => &catalog.search_applications,
            Message::NoApplicationsFound => &catalog.no_applications_found,
            Message::TryAnotherSearch => &catalog.try_another_search,
            Message::PreviousWindows => &catalog.previous_windows,
            Message::MoreWindows => &catalog.more_windows,
            Message::Open => &catalog.open,
            Message::NewWindow => &catalog.new_window,
            Message::MinimizeWindow => &catalog.minimize_window,
            Message::MinimizeAllWindows => &catalog.minimize_all_windows,
            Message::CloseWindow => &catalog.close_window,
            Message::CloseAllWindows => &catalog.close_all_windows,
            Message::PinToDock => &catalog.pin_to_dock,
            Message::UnpinFromDock => &catalog.unpin_from_dock,
            Message::ControlCenter => &catalog.control_center,
            Message::BuiltInSystemApp => &catalog.built_in_system_app,
            Message::Connectivity => &catalog.connectivity,
            Message::Wifi => &catalog.wifi,
            Message::Bluetooth => &catalog.bluetooth,
            Message::DoNotDisturb => &catalog.do_not_disturb,
            Message::Desktop => &catalog.desktop,
            Message::TiledLayout => &catalog.tiled_layout,
            Message::OpenApplications => &catalog.open_applications,
            Message::QuitSession => &catalog.quit_session,
            Message::Sound => &catalog.sound,
            Message::Brightness => &catalog.brightness,
            Message::Unavailable => &catalog.unavailable,
            Message::Volume => &catalog.volume,
            Message::WifiConnected => &catalog.wifi_connected,
            Message::WiredConnected => &catalog.wired_connected,
            Message::Disconnected => &catalog.disconnected,
            Message::Network => &catalog.network,
            Message::NoBatteryDetected => &catalog.no_battery_detected,
            Message::Battery => &catalog.battery,
            Message::Notifications => &catalog.notifications,
            Message::RecentNotifications => &catalog.recent_notifications,
            Message::NoNotifications => &catalog.no_notifications,
            Message::Muted => &catalog.muted,
        }
    }

    /// Format the locale-aware launcher result count.
    pub fn application_count(self, count: usize) -> String {
        let catalog = catalog(self.language);
        match count {
            0 => catalog.no_applications_found.clone(),
            1 => catalog.one_application.clone(),
            _ => interpolate(&catalog.many_applications, "count", count),
        }
    }

    pub fn recent_notification_count(self, count: usize) -> String {
        let catalog = catalog(self.language);
        if count == 1 {
            catalog.one_recent_notification.clone()
        } else {
            interpolate(&catalog.many_recent_notifications, "count", count)
        }
    }

    pub fn muted_volume(self, level: u8) -> String {
        interpolate(&catalog(self.language).muted_with_level, "level", level)
    }

    pub fn charging_battery(self, percent: u8) -> String {
        interpolate(
            &catalog(self.language).charging_with_level,
            "percent",
            percent,
        )
    }
}

impl Default for Localizer {
    fn default() -> Self {
        Localizer::from_env()
    }
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Catalog {
    applications: String,
    untitled: String,
    untitled_window: String,
    search_applications: String,
    no_applications_found: String,
    one_application: String,
    many_applications: String,
    try_another_search: String,
    previous_windows: String,
    more_windows: String,
    open: String,
    new_window: String,
    minimize_window: String,
    minimize_all_windows: String,
    close_window: String,
    close_all_windows: String,
    pin_to_dock: String,
    unpin_from_dock: String,
    control_center: String,
    built_in_system_app: String,
    connectivity: String,
    wifi: String,
    bluetooth: String,
    do_not_disturb: String,
    desktop: String,
    tiled_layout: String,
    open_applications: String,
    quit_session: String,
    sound: String,
    brightness: String,
    muted_with_level: String,
    unavailable: String,
    volume: String,
    wifi_connected: String,
    wired_connected: String,
    disconnected: String,
    network: String,
    charging_with_level: String,
    no_battery_detected: String,
    battery: String,
    notifications: String,
    one_recent_notification: String,
    many_recent_notifications: String,
    recent_notifications: String,
    no_notifications: String,
    muted: String,
}

static ENGLISH: OnceLock<Catalog> = OnceLock::new();
static SIMPLIFIED_CHINESE: OnceLock<Catalog> = OnceLock::new();

fn catalog(language: Language) -> &'static Catalog {
    let (cell, source, name) = match language {
        Language::English => (&ENGLISH, include_str!("../locales/en-US.toml"), "en-US"),
        Language::SimplifiedChinese => (
            &SIMPLIFIED_CHINESE,
            include_str!("../locales/zh-CN.toml"),
            "zh-CN",
        ),
    };
    cell.get_or_init(|| {
        toml::from_str(source)
            .unwrap_or_else(|error| panic!("invalid embedded {name} translation catalog: {error}"))
    })
}

fn interpolate<T: std::fmt::Display>(template: &str, name: &str, value: T) -> String {
    template.replace(&format!("{{{name}}}"), &value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn negotiates_posix_and_bcp47_locale_forms() {
        assert_eq!(
            Language::from_locale("zh_CN.UTF-8"),
            Language::SimplifiedChinese
        );
        assert_eq!(
            Language::from_locale("zh-Hans-CN"),
            Language::SimplifiedChinese
        );
        assert_eq!(Language::from_locale("zh-TW"), Language::English);
        assert_eq!(Language::from_locale("fr-FR"), Language::English);
        assert_eq!(Language::from_locale("C.UTF-8"), Language::English);
    }

    #[test]
    fn both_embedded_catalogs_are_complete_and_format_values() {
        let en = Localizer::new("en-US");
        let zh = Localizer::new("zh-CN");
        assert_eq!(en.text(Message::SearchApplications), "Search applications");
        assert_eq!(zh.text(Message::SearchApplications), "搜索应用");
        assert_eq!(en.application_count(2), "2 applications");
        assert_eq!(zh.application_count(2), "2 个应用");
        assert_eq!(en.recent_notification_count(1), "1 recent notification");
        assert_eq!(zh.charging_battery(80), "80% · 充电中");
    }
}
