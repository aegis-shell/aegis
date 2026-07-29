//! Versionable desktop-settings model shared by chrome, IPC, and clients.
//!
//! These types describe compositor-owned persistent settings. Instant system
//! controls such as volume and Wi-Fi are intentionally absent: their source
//! of truth is the corresponding system service, not the compositor config.

use crate::Point;
use crate::input::{TouchpadConfig, TouchpadStatus};
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
    pub touchpad: TouchpadStatus,
    pub display: DisplayStatus,
    pub preferences: DesktopPreferences,
}

/// One typed persistent-settings transaction.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(tag = "type"))]
#[derive(Debug, Clone, PartialEq)]
pub enum SettingsAction {
    SetTouchpad { config: TouchpadConfig },
    SetDisplay { settings: DisplaySettings },
    SetDesktopPreferences { preferences: DesktopPreferences },
}

impl SettingsAction {
    /// Validate transport-level bounds. Hardware capability checks and mode
    /// membership remain authoritative main-loop work.
    pub fn validate(&self) -> Result<(), &'static str> {
        match self {
            Self::SetTouchpad { config }
                if !config.pointer_speed.is_finite()
                    || !(-1.0..=1.0).contains(&config.pointer_speed) =>
            {
                Err("touchpad pointer speed is outside -1.0..=1.0")
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
        let config = TouchpadConfig {
            pointer_speed: 1.5,
            ..Default::default()
        };
        assert!(SettingsAction::SetTouchpad { config }.validate().is_err());

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
    }

    #[test]
    fn accent_color_round_trips_and_normalizes() {
        let accent = AccentColor::parse_hex("#1a80FF").unwrap();
        assert_eq!(accent.to_hex(), "#1A80FF");
        assert_eq!(accent.normalized(), (26.0 / 255.0, 128.0 / 255.0, 1.0));
        assert!(AccentColor::parse_hex("1A80FF").is_err());
        assert!(AccentColor::parse_hex("#xyzxyz").is_err());
    }
}
