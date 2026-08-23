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
    MaximizeWindow,
    RestoreWindow,
    AlwaysOnTop,
    NotAlwaysOnTop,
    CloseWindow,
    CloseAllWindows,
    PinToDock,
    UnpinFromDock,
    BuiltInSystemApp,
    ConnectingToDesktop,
    Connectivity,
    Wifi,
    Bluetooth,
    DoNotDisturb,
    StatusAndControls,
    Desktop,
    SoundAndDisplay,
    TiledLayout,
    OpenApplications,
    QuitSession,
    LockNow,
    KeepAwake,
    AutoLock,
    QuickControls,
    Sound,
    Brightness,
    Display,
    Displays,
    DisplayDescription,
    DisplayHostManaged,
    ResolutionAndRefresh,
    Scale,
    Arrangement,
    RightOfPrimary,
    LeftOfPrimary,
    AbovePrimary,
    BelowPrimary,
    CustomPosition,
    HorizontalPosition,
    VerticalPosition,
    PrimaryDisplay,
    MakePrimary,
    ApplyDisplaySettings,
    ResetDisplaySettings,
    NoDisplays,
    InvalidPosition,
    DisplayApplyHint,
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
    Input,
    InputDescription,
    InputHostManaged,
    Touchpad,
    Mouse,
    Keyboard,
    Appearance,
    AppearanceDescription,
    AppearanceColorScheme,
    SystemDefault,
    PreferDark,
    PreferLight,
    AccentColor,
    Contrast,
    NormalContrast,
    HighContrast,
    ReducedMotion,
    ReducedMotionDescription,
    InterfaceFont,
    MonospaceFont,
    TextScale,
    IconTheme,
    CursorTheme,
    CursorSize,
    ApplyAppearanceSettings,
    ResetAppearanceSettings,
    InvalidAppearanceSettings,
    AppearanceApplyHint,
    Dock,
    DockDescription,
    MinimizeAnimation,
    MinimizeAnimationGenie,
    MinimizeAnimationScale,
    MinimizeAnimationSuck,
    PowerManagement,
    PowerManagementDescription,
    AutomaticIdle,
    AutomaticIdleDescription,
    DimDisplay,
    DimDisplayDescription,
    LockAfterIdle,
    LockAfterIdleDescription,
    TurnDisplayOff,
    TurnDisplayOffDescription,
    SuspendAutomatically,
    SuspendAutomaticallyDescription,
    DimLevel,
    ApplyPowerSettings,
    ResetPowerSettings,
    InvalidPowerSettings,
    PowerApplyHint,
    UserAccounts,
    UserAccountsDescription,
    WindowRules,
    WindowRulesDescription,
    SettingsBackendUnavailable,
    SettingsBackendUnavailableDescription,
    NoTouchpadDetected,
    PointingAndClicking,
    KeyboardRepeatDescription,
    RepeatRate,
    RepeatRateUnit,
    RepeatDelay,
    RepeatDisabled,
    Milliseconds,
    Scrolling,
    TapToClick,
    TapToClickDescription,
    TapAndDrag,
    TapAndDragDescription,
    DragLock,
    DragLockDescription,
    DisableWhileTyping,
    DisableWhileTypingDescription,
    NaturalScroll,
    NaturalScrollDescription,
    PointerSpeed,
    ScrollSpeed,
    Slow,
    Fast,
    ScrollMethod,
    TwoFingerScroll,
    EdgeScroll,
    AgentWorkspaces,
    NoActiveAgentWorkspaces,
    AgentWorkspacesPartiallyPaused,
    InteractionDomainActive,
    InteractionDomainPaused,
    InteractionDomainRevoked,
    PhysicalDesktop,
    DropWindowHere,
    MoveToInteractionDomain,
    ReadOnlyMirror,
    AgentBadge,
    AgentOperating,
    AgentPointerMove,
    AgentClick,
    AgentRightClick,
    AgentMiddleClick,
    AgentScrollUp,
    AgentScrollDown,
    AgentScrollLeft,
    AgentScrollRight,
    AgentKeyboard,
    ScreenshotConfirmHint,
    CommandPanel,
    System,
    Tray,
    NoTrayItems,
    Session,
    Cpu,
    Gpu,
    Memory,
    Disk,
    NetDown,
    NetUp,
    Laptop,
    /// The chassis form factor; [`Message::Desktop`] is the desktop metaphor.
    DesktopChassis,
    Charging,
    Offline,
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
            Message::MaximizeWindow => &catalog.maximize_window,
            Message::RestoreWindow => &catalog.restore_window,
            Message::AlwaysOnTop => &catalog.always_on_top,
            Message::NotAlwaysOnTop => &catalog.not_always_on_top,
            Message::CloseWindow => &catalog.close_window,
            Message::CloseAllWindows => &catalog.close_all_windows,
            Message::PinToDock => &catalog.pin_to_dock,
            Message::UnpinFromDock => &catalog.unpin_from_dock,
            Message::BuiltInSystemApp => &catalog.built_in_system_app,
            Message::ConnectingToDesktop => &catalog.connecting_to_desktop,
            Message::Connectivity => &catalog.connectivity,
            Message::Wifi => &catalog.wifi,
            Message::Bluetooth => &catalog.bluetooth,
            Message::DoNotDisturb => &catalog.do_not_disturb,
            Message::StatusAndControls => &catalog.status_and_controls,
            Message::Desktop => &catalog.desktop,
            Message::SoundAndDisplay => &catalog.sound_and_display,
            Message::TiledLayout => &catalog.tiled_layout,
            Message::OpenApplications => &catalog.open_applications,
            Message::QuitSession => &catalog.quit_session,
            Message::Session => &catalog.session,
            Message::LockNow => &catalog.lock_now,
            Message::KeepAwake => &catalog.keep_awake,
            Message::AutoLock => &catalog.auto_lock,
            Message::QuickControls => &catalog.quick_controls,
            Message::Sound => &catalog.sound,
            Message::Brightness => &catalog.brightness,
            Message::Display => &catalog.display,
            Message::Displays => &catalog.displays,
            Message::DisplayDescription => &catalog.display_description,
            Message::DisplayHostManaged => &catalog.display_host_managed,
            Message::ResolutionAndRefresh => &catalog.resolution_and_refresh,
            Message::Scale => &catalog.scale,
            Message::Arrangement => &catalog.arrangement,
            Message::RightOfPrimary => &catalog.right_of_primary,
            Message::LeftOfPrimary => &catalog.left_of_primary,
            Message::AbovePrimary => &catalog.above_primary,
            Message::BelowPrimary => &catalog.below_primary,
            Message::CustomPosition => &catalog.custom_position,
            Message::HorizontalPosition => &catalog.horizontal_position,
            Message::VerticalPosition => &catalog.vertical_position,
            Message::PrimaryDisplay => &catalog.primary_display,
            Message::MakePrimary => &catalog.make_primary,
            Message::ApplyDisplaySettings => &catalog.apply_display_settings,
            Message::ResetDisplaySettings => &catalog.reset_display_settings,
            Message::NoDisplays => &catalog.no_displays,
            Message::InvalidPosition => &catalog.invalid_position,
            Message::DisplayApplyHint => &catalog.display_apply_hint,
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
            Message::Input => &catalog.input,
            Message::InputDescription => &catalog.input_description,
            Message::InputHostManaged => &catalog.input_host_managed,
            Message::Touchpad => &catalog.touchpad,
            Message::Mouse => &catalog.mouse,
            Message::Keyboard => &catalog.keyboard,
            Message::Appearance => &catalog.appearance,
            Message::AppearanceDescription => &catalog.appearance_description,
            Message::AppearanceColorScheme => &catalog.appearance_color_scheme,
            Message::SystemDefault => &catalog.system_default,
            Message::PreferDark => &catalog.prefer_dark,
            Message::PreferLight => &catalog.prefer_light,
            Message::AccentColor => &catalog.accent_color,
            Message::Contrast => &catalog.contrast,
            Message::NormalContrast => &catalog.normal_contrast,
            Message::HighContrast => &catalog.high_contrast,
            Message::ReducedMotion => &catalog.reduced_motion,
            Message::ReducedMotionDescription => &catalog.reduced_motion_description,
            Message::InterfaceFont => &catalog.interface_font,
            Message::MonospaceFont => &catalog.monospace_font,
            Message::TextScale => &catalog.text_scale,
            Message::IconTheme => &catalog.icon_theme,
            Message::CursorTheme => &catalog.cursor_theme,
            Message::CursorSize => &catalog.cursor_size,
            Message::ApplyAppearanceSettings => &catalog.apply_appearance_settings,
            Message::ResetAppearanceSettings => &catalog.reset_appearance_settings,
            Message::InvalidAppearanceSettings => &catalog.invalid_appearance_settings,
            Message::AppearanceApplyHint => &catalog.appearance_apply_hint,
            Message::Dock => &catalog.dock,
            Message::DockDescription => &catalog.dock_description,
            Message::MinimizeAnimation => &catalog.minimize_animation,
            Message::MinimizeAnimationGenie => &catalog.minimize_animation_genie,
            Message::MinimizeAnimationScale => &catalog.minimize_animation_scale,
            Message::MinimizeAnimationSuck => &catalog.minimize_animation_suck,
            Message::PowerManagement => &catalog.power_management,
            Message::PowerManagementDescription => &catalog.power_management_description,
            Message::AutomaticIdle => &catalog.automatic_idle,
            Message::AutomaticIdleDescription => &catalog.automatic_idle_description,
            Message::DimDisplay => &catalog.dim_display,
            Message::DimDisplayDescription => &catalog.dim_display_description,
            Message::LockAfterIdle => &catalog.lock_after_idle,
            Message::LockAfterIdleDescription => &catalog.lock_after_idle_description,
            Message::TurnDisplayOff => &catalog.turn_display_off,
            Message::TurnDisplayOffDescription => &catalog.turn_display_off_description,
            Message::SuspendAutomatically => &catalog.suspend_automatically,
            Message::SuspendAutomaticallyDescription => &catalog.suspend_automatically_description,
            Message::DimLevel => &catalog.dim_level,
            Message::ApplyPowerSettings => &catalog.apply_power_settings,
            Message::ResetPowerSettings => &catalog.reset_power_settings,
            Message::InvalidPowerSettings => &catalog.invalid_power_settings,
            Message::PowerApplyHint => &catalog.power_apply_hint,
            Message::UserAccounts => &catalog.user_accounts,
            Message::UserAccountsDescription => &catalog.user_accounts_description,
            Message::WindowRules => &catalog.window_rules,
            Message::WindowRulesDescription => &catalog.window_rules_description,
            Message::SettingsBackendUnavailable => &catalog.settings_backend_unavailable,
            Message::SettingsBackendUnavailableDescription => {
                &catalog.settings_backend_unavailable_description
            }
            Message::NoTouchpadDetected => &catalog.no_touchpad_detected,
            Message::PointingAndClicking => &catalog.pointing_and_clicking,
            Message::KeyboardRepeatDescription => &catalog.keyboard_repeat_description,
            Message::RepeatRate => &catalog.repeat_rate,
            Message::RepeatRateUnit => &catalog.repeat_rate_unit,
            Message::RepeatDelay => &catalog.repeat_delay,
            Message::RepeatDisabled => &catalog.repeat_disabled,
            Message::Milliseconds => &catalog.milliseconds,
            Message::Scrolling => &catalog.scrolling,
            Message::TapToClick => &catalog.tap_to_click,
            Message::TapToClickDescription => &catalog.tap_to_click_description,
            Message::TapAndDrag => &catalog.tap_and_drag,
            Message::TapAndDragDescription => &catalog.tap_and_drag_description,
            Message::DragLock => &catalog.drag_lock,
            Message::DragLockDescription => &catalog.drag_lock_description,
            Message::DisableWhileTyping => &catalog.disable_while_typing,
            Message::DisableWhileTypingDescription => &catalog.disable_while_typing_description,
            Message::NaturalScroll => &catalog.natural_scroll,
            Message::NaturalScrollDescription => &catalog.natural_scroll_description,
            Message::PointerSpeed => &catalog.pointer_speed,
            Message::ScrollSpeed => &catalog.scroll_speed,
            Message::Slow => &catalog.slow,
            Message::Fast => &catalog.fast,
            Message::ScrollMethod => &catalog.scroll_method,
            Message::TwoFingerScroll => &catalog.two_finger_scroll,
            Message::EdgeScroll => &catalog.edge_scroll,
            Message::AgentWorkspaces => &catalog.agent_workspaces,
            Message::NoActiveAgentWorkspaces => &catalog.no_active_agent_workspaces,
            Message::AgentWorkspacesPartiallyPaused => &catalog.agent_workspaces_partially_paused,
            Message::InteractionDomainActive => &catalog.interaction_domain_active,
            Message::InteractionDomainPaused => &catalog.interaction_domain_paused,
            Message::InteractionDomainRevoked => &catalog.interaction_domain_revoked,
            Message::PhysicalDesktop => &catalog.physical_desktop,
            Message::DropWindowHere => &catalog.drop_window_here,
            Message::MoveToInteractionDomain => &catalog.move_to_interaction_domain,
            Message::ReadOnlyMirror => &catalog.read_only_mirror,
            Message::AgentBadge => &catalog.agent_badge,
            Message::AgentOperating => &catalog.agent_operating,
            Message::AgentPointerMove => &catalog.agent_pointer_move,
            Message::AgentClick => &catalog.agent_click,
            Message::AgentRightClick => &catalog.agent_right_click,
            Message::AgentMiddleClick => &catalog.agent_middle_click,
            Message::AgentScrollUp => &catalog.agent_scroll_up,
            Message::AgentScrollDown => &catalog.agent_scroll_down,
            Message::AgentScrollLeft => &catalog.agent_scroll_left,
            Message::AgentScrollRight => &catalog.agent_scroll_right,
            Message::AgentKeyboard => &catalog.agent_keyboard,
            Message::ScreenshotConfirmHint => &catalog.screenshot_confirm_hint,
            Message::CommandPanel => &catalog.command_panel,
            Message::System => &catalog.system,
            Message::Tray => &catalog.tray,
            Message::NoTrayItems => &catalog.no_tray_items,
            Message::Cpu => &catalog.cpu,
            Message::Gpu => &catalog.gpu,
            Message::Memory => &catalog.memory,
            Message::Disk => &catalog.disk,
            Message::NetDown => &catalog.net_down,
            Message::NetUp => &catalog.net_up,
            Message::Laptop => &catalog.laptop,
            Message::DesktopChassis => &catalog.desktop_chassis,
            Message::Charging => &catalog.charging,
            Message::Offline => &catalog.offline,
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

    pub fn agent_workspace_count(self, count: usize) -> String {
        interpolate(
            &catalog(self.language).many_agent_workspaces,
            "count",
            count,
        )
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

    /// The recording indicator's label: a localized "screen recording"
    /// marker carrying the live capture-stream count (ADR-0128).
    pub fn recording_stream_count(self, count: u32) -> String {
        interpolate(&catalog(self.language).recording_with_count, "count", count)
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
    maximize_window: String,
    restore_window: String,
    always_on_top: String,
    not_always_on_top: String,
    close_window: String,
    close_all_windows: String,
    pin_to_dock: String,
    unpin_from_dock: String,
    built_in_system_app: String,
    connecting_to_desktop: String,
    connectivity: String,
    wifi: String,
    bluetooth: String,
    do_not_disturb: String,
    status_and_controls: String,
    desktop: String,
    sound_and_display: String,
    tiled_layout: String,
    open_applications: String,
    quit_session: String,
    session: String,
    lock_now: String,
    keep_awake: String,
    auto_lock: String,
    sound: String,
    brightness: String,
    display: String,
    displays: String,
    display_description: String,
    display_host_managed: String,
    resolution_and_refresh: String,
    scale: String,
    arrangement: String,
    right_of_primary: String,
    left_of_primary: String,
    above_primary: String,
    below_primary: String,
    custom_position: String,
    horizontal_position: String,
    vertical_position: String,
    primary_display: String,
    make_primary: String,
    apply_display_settings: String,
    reset_display_settings: String,
    no_displays: String,
    invalid_position: String,
    display_apply_hint: String,
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
    input: String,
    input_description: String,
    input_host_managed: String,
    touchpad: String,
    mouse: String,
    keyboard: String,
    appearance: String,
    appearance_description: String,
    appearance_color_scheme: String,
    system_default: String,
    prefer_dark: String,
    prefer_light: String,
    accent_color: String,
    contrast: String,
    normal_contrast: String,
    high_contrast: String,
    reduced_motion: String,
    reduced_motion_description: String,
    interface_font: String,
    monospace_font: String,
    text_scale: String,
    icon_theme: String,
    cursor_theme: String,
    cursor_size: String,
    apply_appearance_settings: String,
    reset_appearance_settings: String,
    invalid_appearance_settings: String,
    appearance_apply_hint: String,
    dock: String,
    dock_description: String,
    minimize_animation: String,
    minimize_animation_genie: String,
    minimize_animation_scale: String,
    minimize_animation_suck: String,
    power_management: String,
    power_management_description: String,
    automatic_idle: String,
    automatic_idle_description: String,
    dim_display: String,
    dim_display_description: String,
    lock_after_idle: String,
    lock_after_idle_description: String,
    turn_display_off: String,
    turn_display_off_description: String,
    suspend_automatically: String,
    suspend_automatically_description: String,
    dim_level: String,
    apply_power_settings: String,
    reset_power_settings: String,
    invalid_power_settings: String,
    power_apply_hint: String,
    user_accounts: String,
    user_accounts_description: String,
    window_rules: String,
    window_rules_description: String,
    settings_backend_unavailable: String,
    settings_backend_unavailable_description: String,
    no_touchpad_detected: String,
    pointing_and_clicking: String,
    keyboard_repeat_description: String,
    repeat_rate: String,
    repeat_rate_unit: String,
    repeat_delay: String,
    repeat_disabled: String,
    milliseconds: String,
    scrolling: String,
    tap_to_click: String,
    tap_to_click_description: String,
    tap_and_drag: String,
    tap_and_drag_description: String,
    drag_lock: String,
    drag_lock_description: String,
    disable_while_typing: String,
    disable_while_typing_description: String,
    natural_scroll: String,
    natural_scroll_description: String,
    pointer_speed: String,
    scroll_speed: String,
    slow: String,
    fast: String,
    scroll_method: String,
    two_finger_scroll: String,
    edge_scroll: String,
    agent_workspaces: String,
    no_active_agent_workspaces: String,
    agent_workspaces_partially_paused: String,
    many_agent_workspaces: String,
    interaction_domain_active: String,
    interaction_domain_paused: String,
    interaction_domain_revoked: String,
    physical_desktop: String,
    drop_window_here: String,
    move_to_interaction_domain: String,
    read_only_mirror: String,
    agent_badge: String,
    agent_operating: String,
    agent_pointer_move: String,
    agent_click: String,
    agent_right_click: String,
    agent_middle_click: String,
    agent_scroll_up: String,
    agent_scroll_down: String,
    agent_scroll_left: String,
    agent_scroll_right: String,
    agent_keyboard: String,
    screenshot_confirm_hint: String,
    command_panel: String,
    system: String,
    tray: String,
    no_tray_items: String,
    cpu: String,
    gpu: String,
    memory: String,
    disk: String,
    net_down: String,
    net_up: String,
    laptop: String,
    desktop_chassis: String,
    charging: String,
    recording_with_count: String,
    quick_controls: String,
    offline: String,
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
        assert_eq!(en.text(Message::AgentPointerMove), "Pointer move");
        assert_eq!(zh.text(Message::AgentKeyboard), "键盘输入");
        assert_eq!(zh.text(Message::NoActiveAgentWorkspaces), "暂无活动工作区");
        assert_eq!(en.agent_workspace_count(2), "2 workspaces");
        assert_eq!(en.text(Message::QuickControls), "Quick Controls");
        assert_eq!(zh.text(Message::QuickControls), "快捷控制");
        assert_eq!(en.text(Message::Network), "Network");
        assert_eq!(zh.text(Message::Network), "网络");
        assert_eq!(en.text(Message::Offline), "Offline");
        assert_eq!(zh.text(Message::Offline), "离线");
    }
}
