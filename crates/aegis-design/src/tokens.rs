//! Semantic visual tokens for the built-in dark appearance.

use lens::Color;

/// The product design snapshot consumed by theme and material factories.
///
/// Components depend on semantic roles rather than literal color values. The
/// value is cheap to copy and leaves room for additional appearance variants
/// without changing component APIs.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct Design {
    pub colors: Colors,
    pub radii: Radii,
    pub strokes: Strokes,
    pub glass: GlassStyles,
    pub glass_focus: GlassFocus,
    pub preview: Preview,
    pub avatars: AvatarStyles,
    pub hud_foreground: HudForeground,
}

impl Design {
    /// The canonical dark appearance currently used by compositor chrome.
    #[must_use]
    pub fn dark() -> Self {
        Self {
            colors: Colors {
                menu_text: Color::rgba(238, 240, 248, 255),
                menu_heading: Color::rgba(183, 188, 207, 255),
                menu_disabled: Color::rgba(160, 168, 188, 255),
                menu_border: Color::rgba(255, 255, 255, 78),
                menu_hover: Color::rgba(255, 255, 255, 22),
                menu_active: Color::rgba(255, 255, 255, 36),
                popover_surface: Color::rgba(255, 255, 255, 38),
                popover_border: Color::rgba(255, 255, 255, 72),
                glass_surface: Color::rgba(255, 255, 255, 12),
                glass_border: Color::rgba(255, 255, 255, 0),
                application_surface: Color::rgba(25, 28, 40, 255),
                application_text: Color::rgba(244, 246, 252, 255),
                application_accent: Color::rgba(102, 156, 255, 255),
                application_border: Color::rgba(255, 255, 255, 42),
                application_hover: Color::rgba(255, 255, 255, 24),
                application_active: Color::rgba(102, 156, 255, 56),
                slider_track: Color::rgba(255, 255, 255, 30),
                slider_fill: Color::rgba(102, 156, 255, 255),
                slider_knob: Color::rgba(255, 255, 255, 255),
                card_surface: Color::rgba(255, 255, 255, 14),
            },
            radii: Radii {
                menu_item: 7.0,
                popover: 12.0,
                glass_panel: 18.0,
                control: 12.0,
                card: 16.0,
                scrollbar: 2.5,
            },
            strokes: Strokes {
                hairline: 1.0,
                scrollbar: 5.0,
            },
            glass: GlassStyles {
                chip: GlassStyle::new(0.16, 4.0, 2.0),
                tooltip: GlassStyle::new(0.14, 10.0, 5.0),
                floating_panel: GlassStyle::new(0.18, 16.0, 8.0),
                prominent_panel: GlassStyle::new(0.20, 18.0, 9.0),
                dock: GlassStyle::new(0.20, 12.0, 6.0),
            },
            glass_focus: GlassFocus {
                hover_tint: Color::rgba(255, 255, 255, 6),
                selected_tint: Color::rgba(255, 255, 255, 3),
                field_strength: 1.0,
            },
            preview: Preview {
                inactive_content_brightness: 0.74,
                focused: PreviewSelection {
                    scale: 1.0,
                    lift: 0.0,
                },
                staged: PreviewSelection {
                    scale: 1.06,
                    lift: 7.0,
                },
            },
            avatars: AvatarStyles {
                persona_header: AvatarStyle {
                    ring: Color::rgba(245, 158, 30, 132),
                    ring_width: 1.0,
                    fallback_surface: Color::rgba(24, 23, 22, 246),
                    fallback_foreground: Color::rgba(236, 232, 222, 238),
                    initials_scale: 22.0 / 72.0,
                },
                lock_hero: AvatarStyle {
                    ring: Color::rgba(255, 255, 255, 62),
                    ring_width: 1.0,
                    fallback_surface: Color::rgba(37, 49, 70, 255),
                    fallback_foreground: Color::rgba(250, 251, 254, 255),
                    initials_scale: 0.36,
                },
            },
            hud_foreground: HudForeground {
                primary: Color::rgba(248, 249, 252, 255),
                contour: Color::rgba(5, 7, 12, 48),
                text_contour_width: 0.75,
                glyph_contour_width: 1.0,
            },
        }
    }
}

impl Default for Design {
    fn default() -> Self {
        Self::dark()
    }
}

/// The SAO command-panel palette (ADR-0080): frosted white floating panels
/// with an amber accent over the standard dark scrim, after the Sword Art
/// Online menu language. Kept separate from [`Colors`] because the panel is
/// a light island inside the dark product appearance.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct Sao {
    /// Frosted white panel surface.
    pub surface: Color,
    /// Slightly deeper white for recessed areas inside a panel.
    pub surface_dim: Color,
    /// Panel edge against the dark scrim.
    pub border: Color,
    /// Primary text on the white surface.
    pub text: Color,
    /// Secondary text on the white surface.
    pub text_muted: Color,
    /// The signature amber accent: selected rows, rings, slider fill.
    pub accent: Color,
    /// Low-alpha accent tint for hover feedback on rows.
    pub accent_soft: Color,
    /// Text/icons drawn on top of a solid accent fill.
    pub on_accent: Color,
    /// Slider/checkbox track on the white surface.
    pub track: Color,
    /// Control knob on the white surface.
    pub knob: Color,
}

impl Sao {
    /// The canonical SAO palette.
    #[must_use]
    pub fn classic() -> Self {
        Self {
            surface: Color::rgba(248, 249, 252, 226),
            surface_dim: Color::rgba(236, 238, 244, 210),
            border: Color::rgba(255, 255, 255, 110),
            text: Color::rgba(32, 36, 48, 255),
            text_muted: Color::rgba(96, 102, 120, 255),
            accent: Color::rgba(245, 158, 30, 255),
            accent_soft: Color::rgba(245, 158, 30, 48),
            on_accent: Color::rgba(255, 255, 255, 255),
            track: Color::rgba(32, 36, 48, 28),
            knob: Color::rgba(255, 255, 255, 255),
        }
    }
}

impl Default for Sao {
    fn default() -> Self {
        Self::classic()
    }
}

/// Semantic color roles shared across compositor chrome.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct Colors {
    pub menu_text: Color,
    pub menu_heading: Color,
    pub menu_disabled: Color,
    pub menu_border: Color,
    pub menu_hover: Color,
    pub menu_active: Color,
    pub popover_surface: Color,
    pub popover_border: Color,
    pub glass_surface: Color,
    pub glass_border: Color,
    pub application_surface: Color,
    pub application_text: Color,
    pub application_accent: Color,
    pub application_border: Color,
    pub application_hover: Color,
    pub application_active: Color,
    pub slider_track: Color,
    pub slider_fill: Color,
    pub slider_knob: Color,
    pub card_surface: Color,
}

/// Shared radii in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct Radii {
    pub menu_item: f32,
    pub popover: f32,
    pub glass_panel: f32,
    pub control: f32,
    pub card: f32,
    pub scrollbar: f32,
}

/// Shared stroke widths in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct Strokes {
    pub hairline: f32,
    pub scrollbar: f32,
}

/// Semantic role of one analytic Liquid Glass body.
///
/// Roles describe the body's elevation and use, not a numbered material
/// intensity. Refraction, tint, and rim lighting remain one product-wide
/// optical identity; role-specific variation is limited to the body shadow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlassRole {
    Chip,
    Tooltip,
    FloatingPanel,
    ProminentPanel,
    Dock,
}

/// Per-body Liquid Glass shadow policy in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct GlassStyle {
    pub shadow_alpha: f32,
    pub shadow_blur: f32,
    pub shadow_offset_y: f32,
}

impl GlassStyle {
    const fn new(shadow_alpha: f32, shadow_blur: f32, shadow_offset_y: f32) -> Self {
        Self {
            shadow_alpha,
            shadow_blur,
            shadow_offset_y,
        }
    }
}

/// Role-indexed Liquid Glass policies for one appearance.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct GlassStyles {
    pub chip: GlassStyle,
    pub tooltip: GlassStyle,
    pub floating_panel: GlassStyle,
    pub prominent_panel: GlassStyle,
    pub dock: GlassStyle,
}

impl GlassStyles {
    #[must_use]
    pub fn for_role(self, role: GlassRole) -> GlassStyle {
        match role {
            GlassRole::Chip => self.chip,
            GlassRole::Tooltip => self.tooltip,
            GlassRole::FloatingPanel => self.floating_panel,
            GlassRole::ProminentPanel => self.prominent_panel,
            GlassRole::Dock => self.dock,
        }
    }
}

/// Focus hierarchy for interactive content hosted inside one glass body.
///
/// Hover is a quiet painted wash. Selection additionally drives the parent
/// body's optical focus field; it never creates a second glass body or a
/// painted outline. Sibling content dims just enough to make the focused
/// target read without tinting the material with an application accent.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct GlassFocus {
    pub hover_tint: Color,
    pub selected_tint: Color,
    pub field_strength: f32,
}

/// Shared presentation policy for live window previews.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct Preview {
    /// Opaque brightness of siblings while one preview is focused.
    pub inactive_content_brightness: f32,
    /// Quiet selection used inside an anchored preview panel.
    pub focused: PreviewSelection,
    /// Foreground staging used by the held-modifier window switcher.
    pub staged: PreviewSelection,
}

impl Preview {
    #[must_use]
    pub fn selection(self, style: PreviewSelectionStyle) -> PreviewSelection {
        match style {
            PreviewSelectionStyle::Focused => self.focused,
            PreviewSelectionStyle::Staged => self.staged,
        }
    }
}

/// Named selection treatments for preview cards.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewSelectionStyle {
    /// Optical focus only; card geometry remains stationary.
    Focused,
    /// Optical focus plus a restrained scale and upward lift.
    Staged,
}

/// Geometry adjustment associated with a preview selection treatment.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct PreviewSelection {
    pub scale: f32,
    pub lift: f32,
}

/// Semantic role of a persona portrait within product chrome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AvatarRole {
    PersonaHeader,
    LockHero,
}

/// Host-rendered frame and fallback policy for a persona portrait.
///
/// Portrait content, source precedence, and animation do not belong here;
/// they remain owned by `aegis-shell::persona`.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct AvatarStyle {
    pub ring: Color,
    pub ring_width: f32,
    pub fallback_surface: Color,
    pub fallback_foreground: Color,
    /// Initials font size as a fraction of the host-provided portrait size.
    pub initials_scale: f32,
}

/// Role-indexed persona portrait styles for one appearance.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct AvatarStyles {
    pub persona_header: AvatarStyle,
    pub lock_hero: AvatarStyle,
}

impl AvatarStyles {
    #[must_use]
    pub fn for_role(self, role: AvatarRole) -> AvatarStyle {
        match role {
            AvatarRole::PersonaHeader => self.persona_header,
            AvatarRole::LockHero => self.lock_hero,
        }
    }
}

/// Foreground-separation policy for the display-only HUD.
///
/// The HUD floats above arbitrary wallpaper and application content, so a
/// single foreground colour cannot guarantee local contrast. A restrained
/// dark contour keeps the light core legible on bright or visually busy
/// regions while disappearing naturally over dark regions. Text and glyphs
/// share one contour colour; only their geometry-specific widths differ.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct HudForeground {
    /// Light core shared by HUD labels, symbols, and active indicators.
    pub primary: Color,
    /// Dark contour/underlay shared by every floating HUD foreground form.
    pub contour: Color,
    /// Fine contour for compact text, in logical pixels.
    pub text_contour_width: f32,
    /// Contour for vector, raster, and geometric glyphs, in logical pixels.
    pub glyph_contour_width: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dark_tokens_preserve_the_existing_menu_palette() {
        let design = Design::dark();
        assert_eq!(design.colors.menu_text, Color::rgba(238, 240, 248, 255));
        assert_eq!(design.colors.menu_hover, Color::rgba(255, 255, 255, 22));
        assert_eq!(design.colors.menu_active, Color::rgba(255, 255, 255, 36));
        assert_eq!(design.radii.menu_item, 7.0);
    }

    #[test]
    fn sao_palette_is_a_light_island_with_amber_accent() {
        let sao = Sao::classic();
        assert_eq!(sao.accent, Color::rgba(245, 158, 30, 255));
        assert_eq!(sao.on_accent, Color::rgba(255, 255, 255, 255));
        let (_, _, _, surface_alpha) = sao.surface.components();
        assert!(surface_alpha > 200);
        let (r, g, b, _) = sao.text.components();
        assert!(r < 64 && g < 64 && b < 64);
    }

    #[test]
    fn hud_foreground_uses_one_restrained_contour_family() {
        let hud = Design::dark().hud_foreground;
        assert_eq!(hud.primary, Color::rgba(248, 249, 252, 255));
        assert_eq!(hud.contour, Color::rgba(5, 7, 12, 48));
        assert!(hud.text_contour_width > 0.0);
        assert!(hud.text_contour_width < hud.glyph_contour_width);
        assert!(hud.glyph_contour_width <= 1.0);
    }

    #[test]
    fn glass_focus_is_neutral_borderless_policy() {
        let design = Design::dark();
        let focus = design.glass_focus;
        assert_eq!(focus.hover_tint, Color::rgba(255, 255, 255, 6));
        assert_eq!(focus.selected_tint, Color::rgba(255, 255, 255, 3));
        assert_eq!(focus.field_strength, 1.0);
        assert_eq!(design.preview.inactive_content_brightness, 0.74);
    }

    #[test]
    fn glass_roles_name_elevation_without_changing_material_identity() {
        let glass = Design::dark().glass;
        assert_eq!(
            glass.for_role(GlassRole::FloatingPanel),
            GlassStyle::new(0.18, 16.0, 8.0)
        );
        assert!(glass.chip.shadow_blur < glass.floating_panel.shadow_blur);
        assert!(glass.floating_panel.shadow_blur < glass.prominent_panel.shadow_blur);
    }

    #[test]
    fn staged_preview_adds_geometry_without_inventing_a_second_focus_policy() {
        let preview = Design::dark().preview;
        assert_eq!(
            preview.selection(PreviewSelectionStyle::Focused),
            PreviewSelection {
                scale: 1.0,
                lift: 0.0
            }
        );
        assert_eq!(
            preview.selection(PreviewSelectionStyle::Staged),
            PreviewSelection {
                scale: 1.06,
                lift: 7.0
            }
        );
    }

    #[test]
    fn avatar_roles_keep_content_out_of_the_design_contract() {
        let avatars = Design::dark().avatars;
        let header = avatars.for_role(AvatarRole::PersonaHeader);
        let lock = avatars.for_role(AvatarRole::LockHero);
        assert_eq!(header.ring, Color::rgba(245, 158, 30, 132));
        assert_eq!(lock.ring, Color::rgba(255, 255, 255, 62));
        assert!(header.initials_scale > 0.0);
        assert!(lock.initials_scale > 0.0);
    }
}
