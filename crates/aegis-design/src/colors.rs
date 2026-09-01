//! Semantic color roles for every built-in Aegis appearance.
//!
//! This module is the only production source of literal product colors in
//! `aegis-design`. Public roles use the grammar
//! `[scope_]role[_variant][_state]`: `menu_text_disabled`,
//! `application_surface_hover`, and `launcher_selection_surface`. The type
//! supplies the scope when a palette is already component-specific, as with
//! [`CommandPanelColors::surface_recessed`].
//!
//! Names describe product meaning, never pigments, numbered intensity,
//! opacity, an inspiration source, or a temporary visual treatment. Callers
//! consume these roles through [`crate::Design`] instead of constructing
//! literal colors or selecting an appearance themselves.

use aegis_model::settings::ColorScheme;
use lens::Color;

/// Semantic color roles shared across Aegis-owned surfaces.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct ProductColors {
    pub menu_text: Color,
    pub menu_text_heading: Color,
    pub menu_text_disabled: Color,
    pub menu_border: Color,
    pub menu_surface_hover: Color,
    pub menu_surface_active: Color,
    pub popover_surface: Color,
    pub popover_border: Color,
    pub glass_surface: Color,
    pub glass_border: Color,
    pub application_surface: Color,
    pub application_text: Color,
    pub application_accent: Color,
    pub application_border: Color,
    pub application_surface_hover: Color,
    pub application_surface_active: Color,
    pub slider_track: Color,
    pub slider_fill: Color,
    pub slider_knob: Color,
    pub card_surface: Color,
    /// Neutral slate of the generic app-icon chip drawn when an entry ships
    /// no icon. This identity fallback is scheme-invariant.
    pub generic_icon_surface: Color,
    /// Full-screen veil behind modal chrome.
    pub modal_scrim: Color,
    /// Foreground for content drawn directly on [`Self::modal_scrim`].
    pub modal_scrim_text: Color,
    /// Translucent scrim wash over a parent window suspended by an active modal descendant.
    pub parent_modal_scrim: Color,
    /// Luminous halo border color for an active modal child during an attention pulse.
    pub attention_pulse_border: Color,
    pub launcher_field_surface: Color,
    pub launcher_field_border: Color,
    pub launcher_selection_surface: Color,
    /// Critical emphasis for destructive actions, errors, and alerts.
    pub critical: Color,
    /// Validation emphasis for successful checks and completed operations.
    pub validation: Color,
}

impl ProductColors {
    fn dark() -> Self {
        Self {
            menu_text: Color::rgba(238, 240, 248, 255),
            menu_text_heading: Color::rgba(183, 188, 207, 255),
            menu_text_disabled: Color::rgba(160, 168, 188, 255),
            menu_border: Color::rgba(255, 255, 255, 78),
            menu_surface_hover: Color::rgba(255, 255, 255, 22),
            menu_surface_active: Color::rgba(255, 255, 255, 36),
            popover_surface: Color::rgba(255, 255, 255, 110),
            popover_border: Color::rgba(255, 255, 255, 72),
            glass_surface: Color::rgba(18, 22, 34, 32),
            glass_border: Color::rgba(255, 255, 255, 0),
            application_surface: Color::rgba(25, 28, 40, 255),
            application_text: Color::rgba(244, 246, 252, 255),
            application_accent: Color::rgba(102, 156, 255, 255),
            application_border: Color::rgba(255, 255, 255, 42),
            application_surface_hover: Color::rgba(255, 255, 255, 24),
            application_surface_active: Color::rgba(102, 156, 255, 56),
            slider_track: Color::rgba(255, 255, 255, 30),
            slider_fill: Color::rgba(102, 156, 255, 255),
            slider_knob: Color::rgba(255, 255, 255, 255),
            card_surface: Color::rgba(255, 255, 255, 14),
            generic_icon_surface: Color::rgba(76, 85, 116, 224),
            modal_scrim: Color::rgba(8, 10, 18, 118),
            modal_scrim_text: Color::rgba(248, 250, 253, 255),
            parent_modal_scrim: Color::rgba(10, 12, 22, 140),
            attention_pulse_border: Color::rgba(120, 175, 255, 245),
            launcher_field_surface: Color::rgba(16, 19, 30, 122),
            launcher_field_border: Color::rgba(255, 255, 255, 44),
            launcher_selection_surface: Color::rgba(12, 15, 26, 96),
            critical: Color::rgba(255, 72, 84, 255),
            validation: Color::rgba(190, 226, 255, 255),
        }
    }

    fn light() -> Self {
        Self {
            menu_text: Color::rgba(34, 38, 50, 255),
            menu_text_heading: Color::rgba(99, 105, 123, 255),
            menu_text_disabled: Color::rgba(133, 139, 156, 255),
            menu_border: Color::rgba(28, 32, 44, 36),
            menu_surface_hover: Color::rgba(28, 32, 44, 12),
            menu_surface_active: Color::rgba(28, 32, 44, 22),
            popover_surface: Color::rgba(250, 251, 253, 216),
            popover_border: Color::rgba(28, 32, 44, 30),
            glass_surface: Color::rgba(255, 255, 255, 72),
            glass_border: Color::rgba(255, 255, 255, 0),
            application_surface: Color::rgba(243, 245, 249, 255),
            application_text: Color::rgba(29, 33, 44, 255),
            application_accent: Color::rgba(43, 101, 232, 255),
            application_border: Color::rgba(28, 32, 44, 32),
            application_surface_hover: Color::rgba(28, 32, 44, 12),
            application_surface_active: Color::rgba(43, 101, 232, 44),
            slider_track: Color::rgba(28, 32, 44, 32),
            slider_fill: Color::rgba(43, 101, 232, 255),
            slider_knob: Color::rgba(255, 255, 255, 255),
            card_surface: Color::rgba(255, 255, 255, 96),
            generic_icon_surface: Color::rgba(76, 85, 116, 224),
            modal_scrim: Color::rgba(28, 32, 44, 104),
            modal_scrim_text: Color::rgba(248, 250, 253, 255),
            parent_modal_scrim: Color::rgba(30, 36, 52, 110),
            attention_pulse_border: Color::rgba(45, 125, 245, 245),
            launcher_field_surface: Color::rgba(250, 251, 253, 208),
            launcher_field_border: Color::rgba(28, 32, 44, 30),
            launcher_selection_surface: Color::rgba(255, 255, 255, 72),
            critical: Color::rgba(210, 40, 55, 255),
            validation: Color::rgba(30, 90, 200, 255),
        }
    }
}

/// Scheme-adaptive colors for the command panel's solid-surface hierarchy.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct CommandPanelColors {
    pub scheme: ColorScheme,
    pub background: Color,
    pub surface: Color,
    pub surface_recessed: Color,
    pub border: Color,
    pub text: Color,
    pub text_muted: Color,
    pub accent: Color,
    pub selection_surface: Color,
    pub accent_surface_hover: Color,
    pub accent_foreground: Color,
    pub control_track: Color,
    pub control_knob: Color,
}

impl CommandPanelColors {
    /// The canonical dark command-panel colors.
    #[must_use]
    pub fn dark() -> Self {
        Self {
            scheme: ColorScheme::Dark,
            background: Color::rgba(18, 18, 20, 255),
            surface: Color::rgba(28, 28, 30, 255),
            surface_recessed: Color::rgba(22, 22, 24, 255),
            border: Color::rgba(255, 255, 255, 28),
            text: Color::rgba(242, 242, 247, 255),
            text_muted: Color::rgba(174, 174, 178, 255),
            accent: Color::rgba(10, 132, 255, 255),
            selection_surface: Color::rgba(24, 55, 86, 255),
            accent_surface_hover: Color::rgba(10, 132, 255, 36),
            accent_foreground: Color::rgba(255, 255, 255, 255),
            control_track: Color::rgba(120, 120, 128, 64),
            control_knob: Color::rgba(242, 242, 247, 255),
        }
    }

    /// The canonical light command-panel colors.
    #[must_use]
    pub fn light() -> Self {
        Self {
            scheme: ColorScheme::Light,
            background: Color::rgba(242, 242, 247, 255),
            surface: Color::rgba(255, 255, 255, 255),
            surface_recessed: Color::rgba(247, 247, 249, 255),
            border: Color::rgba(60, 60, 67, 46),
            text: Color::rgba(29, 29, 31, 255),
            text_muted: Color::rgba(99, 99, 102, 255),
            accent: Color::rgba(0, 122, 255, 255),
            selection_surface: Color::rgba(225, 238, 255, 255),
            accent_surface_hover: Color::rgba(0, 122, 255, 26),
            accent_foreground: Color::rgba(255, 255, 255, 255),
            control_track: Color::rgba(120, 120, 128, 48),
            control_knob: Color::rgba(255, 255, 255, 255),
        }
    }

    /// Resolve colors from a desktop color-scheme preference.
    #[must_use]
    pub fn for_scheme(scheme: ColorScheme) -> Self {
        match scheme.or_dark() {
            ColorScheme::Dark | ColorScheme::System => Self::dark(),
            ColorScheme::Light => Self::light(),
        }
    }

    #[must_use]
    pub fn is_light(&self) -> bool {
        self.scheme == ColorScheme::Light
    }
}

impl Default for CommandPanelColors {
    fn default() -> Self {
        Self::dark()
    }
}

/// Colors consumed by compositor render passes instead of components.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct SceneColors {
    pub clear_color: Color,
    pub overview_scrim: Color,
    pub window_switcher_scrim: Color,
    pub interaction_domain_clear: Color,
    pub glass_tint: [u8; 3],
}

/// Painted foreground colors of the Dock's analytic glass body.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct DockColors {
    pub launchpad_tile_bg: Color,
    pub launchpad_tile_border: Color,
    pub launchpad_grid: Color,
    pub running_dot_active: Color,
    pub running_dot_inactive: Color,
    pub section_divider: Color,
    pub bar_surface_expanded: Color,
    pub bar_surface_collapsed: Color,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct GlassFocusColors {
    pub surface_hover: Color,
    pub surface_selected: Color,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct AvatarRoleColors {
    pub ring: Color,
    pub fallback_surface: Color,
    pub fallback_foreground: Color,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct AvatarColors {
    pub persona_header: AvatarRoleColors,
    pub lock_hero: AvatarRoleColors,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct HudForegroundColors {
    pub primary: Color,
    pub contour: Color,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct AppearanceColors {
    pub product: ProductColors,
    pub glass_focus: GlassFocusColors,
    pub avatars: AvatarColors,
    pub hud_foreground: HudForegroundColors,
    pub scene: SceneColors,
    pub dock: DockColors,
}

pub(crate) fn dark_appearance() -> AppearanceColors {
    AppearanceColors {
        product: ProductColors::dark(),
        glass_focus: GlassFocusColors {
            surface_hover: Color::rgba(255, 255, 255, 6),
            surface_selected: Color::rgba(255, 255, 255, 3),
        },
        avatars: AvatarColors {
            persona_header: AvatarRoleColors {
                ring: Color::rgba(245, 158, 30, 132),
                fallback_surface: Color::rgba(24, 23, 22, 246),
                fallback_foreground: Color::rgba(236, 232, 222, 238),
            },
            lock_hero: AvatarRoleColors {
                ring: Color::rgba(255, 255, 255, 62),
                fallback_surface: Color::rgba(37, 49, 70, 255),
                fallback_foreground: Color::rgba(250, 251, 254, 255),
            },
        },
        hud_foreground: HudForegroundColors {
            primary: Color::rgba(248, 249, 252, 255),
            contour: Color::rgba(5, 7, 12, 48),
        },
        scene: SceneColors {
            clear_color: Color::rgba(30, 30, 46, 255),
            overview_scrim: Color::rgba(8, 10, 20, 255),
            window_switcher_scrim: Color::rgba(5, 7, 12, 255),
            interaction_domain_clear: Color::rgba(17, 20, 27, 255),
            glass_tint: [255, 255, 255],
        },
        dock: dock_colors(),
    }
}

pub(crate) fn light_appearance() -> AppearanceColors {
    AppearanceColors {
        product: ProductColors::light(),
        glass_focus: GlassFocusColors {
            surface_hover: Color::rgba(28, 32, 44, 7),
            surface_selected: Color::rgba(28, 32, 44, 4),
        },
        avatars: AvatarColors {
            persona_header: AvatarRoleColors {
                ring: Color::rgba(245, 158, 30, 132),
                fallback_surface: Color::rgba(246, 242, 234, 246),
                fallback_foreground: Color::rgba(56, 48, 36, 238),
            },
            lock_hero: AvatarRoleColors {
                ring: Color::rgba(28, 32, 44, 48),
                fallback_surface: Color::rgba(216, 222, 232, 255),
                fallback_foreground: Color::rgba(26, 31, 43, 255),
            },
        },
        hud_foreground: HudForegroundColors {
            primary: Color::rgba(30, 34, 46, 255),
            contour: Color::rgba(255, 255, 255, 72),
        },
        scene: SceneColors {
            clear_color: Color::rgba(243, 245, 249, 255),
            overview_scrim: Color::rgba(243, 245, 249, 255),
            window_switcher_scrim: Color::rgba(243, 245, 249, 255),
            interaction_domain_clear: Color::rgba(243, 245, 249, 255),
            glass_tint: [243, 245, 249],
        },
        dock: dock_colors(),
    }
}

fn dock_colors() -> DockColors {
    DockColors {
        launchpad_tile_bg: Color::rgba(70, 78, 110, 240),
        launchpad_tile_border: Color::rgba(150, 160, 195, 180),
        launchpad_grid: Color::rgba(236, 238, 248, 245),
        running_dot_active: Color::rgba(236, 238, 245, 255),
        running_dot_inactive: Color::rgba(200, 204, 220, 170),
        section_divider: Color::rgba(255, 255, 255, 80),
        bar_surface_expanded: Color::rgba(255, 255, 255, 12),
        bar_surface_collapsed: Color::rgba(240, 243, 252, 64),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use super::*;

    #[test]
    fn adopted_appearances_preserve_exact_role_values() {
        let dark = dark_appearance();
        let light = light_appearance();

        assert_eq!(dark.product.menu_text, Color::rgba(238, 240, 248, 255));
        assert_eq!(
            dark.product.menu_surface_hover,
            Color::rgba(255, 255, 255, 22)
        );
        assert_eq!(light.product.critical, Color::rgba(210, 40, 55, 255));
        assert_eq!(light.product.validation, Color::rgba(30, 90, 200, 255));
        assert_eq!(dark.scene.clear_color, Color::rgba(30, 30, 46, 255));
        assert_eq!(light.scene.glass_tint, [243, 245, 249]);
        assert_eq!(dark.dock, light.dock);
    }

    #[test]
    fn command_panel_colors_are_opaque_and_follow_the_scheme() {
        let dark = CommandPanelColors::dark();
        let light = CommandPanelColors::light();

        assert_eq!(dark.scheme, ColorScheme::Dark);
        assert_eq!(light.scheme, ColorScheme::Light);
        assert_eq!(dark.accent, Color::rgba(10, 132, 255, 255));
        assert_eq!(light.accent, Color::rgba(0, 122, 255, 255));
        assert_eq!(CommandPanelColors::for_scheme(ColorScheme::System), dark);

        for colors in [dark, light] {
            assert_eq!(colors.background.components().3, 255);
            assert_eq!(colors.surface.components().3, 255);
            assert_eq!(colors.surface_recessed.components().3, 255);
            assert_eq!(colors.selection_surface.components().3, 255);
        }
    }

    #[test]
    fn product_color_literals_stay_in_the_color_module() {
        fn inspect(path: &Path) {
            for entry in fs::read_dir(path).expect("read design source directory") {
                let path = entry.expect("read design source entry").path();
                if path.is_dir() {
                    inspect(&path);
                } else if path.extension().and_then(|value| value.to_str()) == Some("rs")
                    && path.file_name().and_then(|value| value.to_str()) != Some("colors.rs")
                {
                    let source = fs::read_to_string(&path).expect("read design source file");
                    assert!(
                        !source.contains("Color::rgba("),
                        "literal product color escaped colors.rs: {}",
                        path.display()
                    );
                }
            }
        }

        inspect(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"));
    }
}
