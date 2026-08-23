//! Versionable desktop-settings model shared by chrome, IPC, and clients.
//!
//! These types describe compositor-owned persistent settings. Instant system
//! controls such as volume and Wi-Fi are intentionally absent: their source
//! of truth is the corresponding system service, not the compositor config.

use crate::Point;
use crate::input::InputStatus;
use crate::output::{ModeSpec, OutputInfo};

/// Desktop-wide color-scheme preference, using the freedesktop Settings
/// portal vocabulary.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "kebab-case"))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ColorScheme {
    /// No preference; applications choose their own default.
    #[default]
    System,
    /// Prefer a dark appearance.
    Dark,
    /// Prefer a light appearance.
    Light,
}

impl ColorScheme {
    /// Resolve `System` to the compositor's built-in fallback appearance.
    ///
    /// The shell ships one concrete appearance per explicit scheme; until
    /// platform-level preference detection exists, "no preference" means the
    /// dark appearance, so an unset `color_scheme` never reaches the render
    /// side as an undecided value.
    #[must_use]
    pub fn or_dark(self) -> Self {
        match self {
            Self::System => Self::Dark,
            explicit => explicit,
        }
    }
}

/// Desktop-wide contrast preference.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "kebab-case"))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Contrast {
    #[default]
    Normal,
    High,
}

/// An sRGB accent color stored without an alpha channel.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccentColor {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
}

impl AccentColor {
    /// Parse the canonical configuration representation (`#RRGGBB`).
    pub fn parse_hex(value: &str) -> Result<Self, &'static str> {
        let bytes = value.as_bytes();
        if bytes.len() != 7 || bytes[0] != b'#' {
            return Err("accent color must use #RRGGBB");
        }
        let component = |start| {
            u8::from_str_radix(&value[start..start + 2], 16)
                .map_err(|_| "accent color must use #RRGGBB")
        };
        Ok(Self {
            red: component(1)?,
            green: component(3)?,
            blue: component(5)?,
        })
    }

    pub fn to_hex(self) -> String {
        format!("#{:02X}{:02X}{:02X}", self.red, self.green, self.blue)
    }

    /// Components normalized to the Settings portal `(ddd)` representation.
    pub fn normalized(self) -> (f64, f64, f64) {
        (
            f64::from(self.red) / 255.0,
            f64::from(self.green) / 255.0,
            f64::from(self.blue) / 255.0,
        )
    }
}

/// One concrete snapshot of all compositor-owned desktop preferences.
///
/// Configuration may omit values and explicit startup environment overrides
/// may replace them, but consumers only see this resolved representation.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq)]
pub struct DesktopPreferences {
    pub color_scheme: ColorScheme,
    pub accent_color: Option<AccentColor>,
    pub contrast: Contrast,
    pub reduced_motion: bool,
    pub font_name: String,
    pub monospace_font_name: String,
    pub text_scale: f64,
    pub icon_theme: String,
    pub cursor_theme: String,
    pub cursor_size: u32,
}

impl Default for DesktopPreferences {
    fn default() -> Self {
        Self {
            color_scheme: ColorScheme::System,
            accent_color: None,
            contrast: Contrast::Normal,
            reduced_motion: false,
            font_name: "Sans 10".into(),
            monospace_font_name: "Monospace 10".into(),
            text_scale: 1.0,
            icon_theme: "hicolor".into(),
            cursor_theme: "default".into(),
            cursor_size: 24,
        }
    }
}

impl DesktopPreferences {
    pub fn validate(&self) -> Result<(), &'static str> {
        if !self.text_scale.is_finite() || !(0.5..=3.0).contains(&self.text_scale) {
            return Err("text scale is outside 0.5..=3.0");
        }
        if !(8..=128).contains(&self.cursor_size) {
            return Err("cursor size is outside 8..=128");
        }
        for value in [
            &self.font_name,
            &self.monospace_font_name,
            &self.icon_theme,
            &self.cursor_theme,
        ] {
            if value.trim().is_empty() || value.len() > 256 {
                return Err("desktop preference string is empty or too long");
            }
        }
        Ok(())
    }
}

/// Staged inactivity policy owned by the Aegis session.
///
/// A timeout of zero disables that individual stage. Keeping inactive values
/// in the snapshot lets System Settings disable automatic idle handling and
/// later restore it without discarding the user's preferred timings.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default, deny_unknown_fields))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IdleSettings {
    /// Whether inactivity notifications may trigger the configured stages.
    /// Explicit locking and lock-before-sleep remain available when false.
    pub enabled: bool,
    pub dim_after_seconds: u32,
    pub lock_after_seconds: u32,
    pub display_off_after_seconds: u32,
    pub suspend_after_seconds: u32,
    /// Backlight level used by the dim stage, as a percentage.
    pub dim_percent: u8,
}

impl Default for IdleSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            dim_after_seconds: 5 * 60,
            lock_after_seconds: 10 * 60,
            display_off_after_seconds: 11 * 60,
            suspend_after_seconds: 30 * 60,
            dim_percent: 30,
        }
    }
}

impl IdleSettings {
    /// A generous upper bound that still catches a seconds/milliseconds typo
    /// before it reaches the Wayland protocol.
    pub const MAX_TIMEOUT_SECONDS: u32 = 7 * 24 * 60 * 60;

    pub fn validate(self) -> Result<(), &'static str> {
        if !(1..=100).contains(&self.dim_percent) {
            return Err("idle dim percentage is outside 1..=100");
        }
        let stages = [
            self.dim_after_seconds,
            self.lock_after_seconds,
            self.display_off_after_seconds,
            self.suspend_after_seconds,
        ];
        if stages
            .into_iter()
            .any(|seconds| seconds > Self::MAX_TIMEOUT_SECONDS)
        {
            return Err("idle timeout is longer than seven days");
        }
        if (self.display_off_after_seconds != 0 || self.suspend_after_seconds != 0)
            && self.lock_after_seconds == 0
        {
            return Err("locking must be enabled before display power-off or suspend");
        }
        let mut previous = 0;
        for current in stages.into_iter().filter(|seconds| *seconds != 0) {
            if current <= previous {
                return Err("enabled idle stages must be strictly increasing");
            }
            previous = current;
        }
        Ok(())
    }
}

/// Low-battery warning thresholds, as discharge percentages.
///
/// Each threshold raises the compositor's modal low-battery alert once per
/// discharge cycle; an empty list disables the feature.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default, deny_unknown_fields))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatterySettings {
    /// Percentages that raise the alert, highest first.
    pub warn_at: Vec<u8>,
}

impl Default for BatterySettings {
    fn default() -> Self {
        Self {
            warn_at: vec![20, 5],
        }
    }
}

impl BatterySettings {
    pub fn validate(&self) -> Result<(), &'static str> {
        let mut previous = 100;
        for threshold in &self.warn_at {
            if !(1..=99).contains(threshold) {
                return Err("battery warning percentage is outside 1..=99");
            }
            if *threshold >= previous {
                return Err("battery warning percentages must be strictly descending");
            }
            previous = *threshold;
        }
        Ok(())
    }
}

/// Live display capabilities and current effective configuration.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DisplayStatus {
    /// Whether this session owns physical display configuration.
    pub configurable: bool,
    pub outputs: Vec<OutputInfo>,
    /// Last persistence/application failure, cleared by a successful edit.
    pub error: Option<String>,
}

/// Compositor-owned dock preferences exposed to System Settings.
///
/// Pinning and dock position stay dock-context-menu edits; this struct
/// carries the settings-panel surface of the `[dock]` configuration table.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default, deny_unknown_fields))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DockSettings {
    /// The animation played when a window minimizes into the dock.
    pub minimize_animation: crate::dock::MinimizeAnimationStyle,
}

/// One complete, validated output edit.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq)]
pub struct DisplaySettings {
    pub connector: String,
    pub mode: ModeSpec,
    pub scale: f64,
    pub position: Point,
    pub primary: bool,
}

/// Coherent persistent-settings snapshot. `revision` changes after every
/// accepted mutation and lets clients reject stale drafts.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SettingsSnapshot {
    pub revision: u64,
    pub input: InputStatus,
    pub display: DisplayStatus,
    pub preferences: DesktopPreferences,
    pub idle: IdleSettings,
    pub dock: DockSettings,
}

/// One typed persistent-settings transaction.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(tag = "type"))]
#[derive(Debug, Clone, PartialEq)]
pub enum SettingsAction {
    SetInput { config: crate::input::InputConfig },
    SetDisplay { settings: DisplaySettings },
    SetDesktopPreferences { preferences: DesktopPreferences },
    SetIdle { settings: IdleSettings },
    SetDock { settings: DockSettings },
}

impl SettingsAction {
    /// Validate transport-level bounds. Hardware capability checks and mode
    /// membership remain authoritative main-loop work.
    pub fn validate(&self) -> Result<(), &'static str> {
        match self {
            Self::SetInput { config } => {
                if !config.touchpad.pointer_speed.is_finite()
                    || !(-1.0..=1.0).contains(&config.touchpad.pointer_speed)
                {
                    return Err("touchpad pointer speed is outside -1.0..=1.0");
                }
                if !config.touchpad.scroll_speed.is_finite()
                    || !crate::input::SCROLL_SPEED_RANGE.contains(&config.touchpad.scroll_speed)
                {
                    return Err("touchpad scroll speed is outside 0.1..=10.0");
                }
                if !config.mouse.pointer_speed.is_finite()
                    || !(-1.0..=1.0).contains(&config.mouse.pointer_speed)
                {
                    return Err("mouse pointer speed is outside -1.0..=1.0");
                }
                if !config.mouse.scroll_speed.is_finite()
                    || !crate::input::SCROLL_SPEED_RANGE.contains(&config.mouse.scroll_speed)
                {
                    return Err("mouse scroll speed is outside 0.1..=10.0");
                }
                if config.keyboard.repeat_rate > crate::input::MAX_REPEAT_RATE {
                    return Err("keyboard repeat rate is above 150 repeats per second");
                }
                if config.keyboard.repeat_delay_ms == 0
                    || config.keyboard.repeat_delay_ms > crate::input::MAX_REPEAT_DELAY_MS
                {
                    return Err("keyboard repeat delay is outside 1..=2000 ms");
                }
                Ok(())
            }
            Self::SetDisplay { settings }
                if settings.connector.trim().is_empty() || settings.connector.len() > 128 =>
            {
                Err("display connector is empty or too long")
            }
            Self::SetDisplay { settings }
                if !settings.scale.is_finite() || !(0.25..=4.0).contains(&settings.scale) =>
            {
                Err("display scale is outside 0.25..=4.0")
            }
            Self::SetDisplay { settings }
                if settings.mode.width <= 0
                    || settings.mode.height <= 0
                    || settings.mode.width > 32_768
                    || settings.mode.height > 32_768
                    || settings
                        .mode
                        .refresh_hz
                        .is_some_and(|hz| !(1..=1_000).contains(&hz)) =>
            {
                Err("display mode is outside the supported range")
            }
            Self::SetDesktopPreferences { preferences } => preferences.validate(),
            Self::SetIdle { settings } => settings.validate(),
            _ => Ok(()),
        }
    }
}

/// Confirmation returned after the main loop persisted and applied a
/// settings action.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SettingsReceipt {
    pub revision: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_validation_rejects_unbounded_values() {
        let config = crate::input::TouchpadConfig {
            pointer_speed: 1.5,
            ..Default::default()
        };
        assert!(
            SettingsAction::SetInput {
                config: crate::input::InputConfig {
                    touchpad: config,
                    ..Default::default()
                },
            }
            .validate()
            .is_err()
        );
        assert!(
            SettingsAction::SetInput {
                config: crate::input::InputConfig {
                    mouse: crate::input::MouseConfig {
                        scroll_speed: 0.0,
                        ..Default::default()
                    },
                    ..Default::default()
                },
            }
            .validate()
            .is_err()
        );
        assert!(
            SettingsAction::SetInput {
                config: crate::input::InputConfig {
                    keyboard: crate::input::KeyboardConfig {
                        repeat_rate: 500,
                        repeat_delay_ms: 250,
                    },
                    ..Default::default()
                },
            }
            .validate()
            .is_err()
        );
        assert!(
            SettingsAction::SetInput {
                config: crate::input::InputConfig::default(),
            }
            .validate()
            .is_ok()
        );

        let display = DisplaySettings {
            connector: "DP-1".into(),
            mode: ModeSpec {
                width: 2560,
                height: 1440,
                refresh_hz: Some(144),
            },
            scale: 1.5,
            position: Point { x: 0, y: 0 },
            primary: true,
        };
        assert!(
            SettingsAction::SetDisplay { settings: display }
                .validate()
                .is_ok()
        );

        let preferences = DesktopPreferences {
            text_scale: f64::NAN,
            ..Default::default()
        };
        assert!(
            SettingsAction::SetDesktopPreferences { preferences }
                .validate()
                .is_err()
        );

        let idle = IdleSettings {
            lock_after_seconds: 0,
            display_off_after_seconds: 60,
            ..Default::default()
        };
        assert!(
            SettingsAction::SetIdle { settings: idle }
                .validate()
                .is_err()
        );
    }

    #[test]
    fn system_color_scheme_resolves_to_the_dark_fallback() {
        assert_eq!(ColorScheme::System.or_dark(), ColorScheme::Dark);
        assert_eq!(ColorScheme::Dark.or_dark(), ColorScheme::Dark);
        assert_eq!(ColorScheme::Light.or_dark(), ColorScheme::Light);
        assert_eq!(
            DesktopPreferences::default().color_scheme.or_dark(),
            ColorScheme::Dark
        );
    }

    #[test]
    fn accent_color_round_trips_and_normalizes() {
        let accent = AccentColor::parse_hex("#1a80FF").unwrap();
        assert_eq!(accent.to_hex(), "#1A80FF");
        assert_eq!(accent.normalized(), (26.0 / 255.0, 128.0 / 255.0, 1.0));
        assert!(AccentColor::parse_hex("1A80FF").is_err());
        assert!(AccentColor::parse_hex("#xyzxyz").is_err());
    }

    #[test]
    fn battery_warning_thresholds_default_and_validate() {
        let defaults = BatterySettings::default();
        assert_eq!(defaults.warn_at, vec![20, 5]);
        assert!(defaults.validate().is_ok());
        // An empty list disables the feature and is valid.
        assert!(BatterySettings { warn_at: vec![] }.validate().is_ok());
        assert!(
            BatterySettings {
                warn_at: vec![30, 10, 5],
            }
            .validate()
            .is_ok()
        );
        assert!(BatterySettings { warn_at: vec![0] }.validate().is_err());
        assert!(BatterySettings { warn_at: vec![100] }.validate().is_err());
        // Ascending order and duplicates are both rejected.
        assert!(
            BatterySettings {
                warn_at: vec![5, 20],
            }
            .validate()
            .is_err()
        );
        assert!(
            BatterySettings {
                warn_at: vec![20, 20],
            }
            .validate()
            .is_err()
        );
    }
}
