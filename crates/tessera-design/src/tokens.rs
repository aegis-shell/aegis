//! Semantic visual tokens for the built-in dark and light appearances.

use tessera_model::settings::ColorScheme;
use lens::Color;

use crate::colors::{DockColors, ProductColors, SceneColors};

/// The product design snapshot consumed by theme and material factories.
///
/// Components depend on semantic roles rather than literal color values. The
/// value is cheap to copy and leaves room for additional appearance variants
/// without changing component APIs.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct Design {
    /// The resolved color scheme this snapshot implements. Always explicit —
    /// `System` never survives [`Design::for_scheme`].
    pub scheme: ColorScheme,
    pub colors: ProductColors,
    pub radii: Radii,
    pub strokes: Strokes,
    pub glass: GlassStyles,
    pub glass_focus: GlassFocus,
    pub preview: Preview,
    pub avatars: AvatarStyles,
    pub hud_foreground: HudForeground,
    pub typography: TypeScale,
    pub scene: SceneColors,
    pub dock: DockColors,
}

impl Design {
    /// The canonical dark appearance currently used by compositor chrome.
    #[must_use]
    pub fn dark() -> Self {
        let colors = crate::colors::dark_appearance();
        Self {
            scheme: ColorScheme::Dark,
            colors: colors.product,
            radii: Radii {
                menu_item: 7.0,
                popover: 12.0,
                glass_panel: 18.0,
                control: 12.0,
                card: 16.0,
                scrollbar: 2.5,
                chip: 16.0,
                cell: 22.0,
                application: 24.0,
            },
            strokes: Strokes {
                hairline: 1.0,
                scrollbar: 5.0,
            },
            glass: GlassStyles {
                chip: GlassStyle::new(0.16, 4.0, 2.0).with_material(1.0, 1.0, 1.0, 0.0),
                tooltip: GlassStyle::new(0.14, 10.0, 5.0).with_material(3.0, 3.0, 0.85, 0.0),
                menu: GlassStyle::new(0.18, 16.0, 8.0).with_material(5.0, 3.6, 0.7, 0.0),
                floating_panel: GlassStyle::new(0.18, 16.0, 8.0).with_material(3.5, 3.0, 0.85, 0.0),
                prominent_panel: GlassStyle::new(0.20, 18.0, 9.0).with_material(4.0, 3.2, 0.8, 0.0),
                dock: GlassStyle::new(0.20, 12.0, 6.0).with_material(3.0, 2.5, 0.9, 0.0),
            },
            glass_focus: GlassFocus {
                hover_tint: colors.glass_focus.surface_hover,
                selected_tint: colors.glass_focus.surface_selected,
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
                    ring: colors.avatars.persona_header.ring,
                    ring_width: 1.0,
                    fallback_surface: colors.avatars.persona_header.fallback_surface,
                    fallback_foreground: colors.avatars.persona_header.fallback_foreground,
                    initials_scale: 22.0 / 72.0,
                },
                lock_hero: AvatarStyle {
                    ring: colors.avatars.lock_hero.ring,
                    ring_width: 1.0,
                    fallback_surface: colors.avatars.lock_hero.fallback_surface,
                    fallback_foreground: colors.avatars.lock_hero.fallback_foreground,
                    initials_scale: 0.36,
                },
            },
            hud_foreground: HudForeground {
                primary: colors.hud_foreground.primary,
                contour: colors.hud_foreground.contour,
                text_contour_width: 0.75,
                glyph_contour_width: 1.0,
            },
            typography: TypeScale {
                caption: 10.0,
                footnote: 11.0,
                label: 12.0,
                body: 13.0,
                headline: 15.0,
                title: 20.0,
                hero: 24.0,
            },
            scene: colors.scene,
            dock: colors.dock,
        }
    }

    /// The canonical light appearance: the same geometry and optical identity
    /// as [`Design::dark`], re-toned as dark ink on white frosted glass.
    ///
    /// The lens light theme is generalized to whole-product surfaces: white
    /// bodies keep the glass whisper, tints become dark washes, and shadows
    /// deepen slightly so pale panels still separate from bright content.
    /// Radii, strokes, and preview policy are scheme-invariant and shared
    /// with the dark appearance.
    #[must_use]
    pub fn light() -> Self {
        let colors = crate::colors::light_appearance();
        Self {
            scheme: ColorScheme::Light,
            colors: colors.product,
            glass: GlassStyles {
                chip: GlassStyle::new(0.18, 4.0, 2.0).with_material(1.0, 1.0, 1.0, 1.0),
                tooltip: GlassStyle::new(0.16, 10.0, 5.0).with_material(3.0, 3.5, 0.9, 1.0),
                menu: GlassStyle::new(0.20, 16.0, 8.0).with_material(5.0, 4.5, 0.8, 1.0),
                floating_panel: GlassStyle::new(0.20, 16.0, 8.0).with_material(3.5, 3.5, 0.9, 1.0),
                prominent_panel: GlassStyle::new(0.22, 18.0, 9.0)
                    .with_material(4.0, 4.0, 0.85, 1.0),
                dock: GlassStyle::new(0.22, 12.0, 6.0).with_material(3.0, 3.0, 0.95, 1.0),
            },
            glass_focus: GlassFocus {
                hover_tint: colors.glass_focus.surface_hover,
                selected_tint: colors.glass_focus.surface_selected,
                field_strength: 1.0,
            },
            avatars: AvatarStyles {
                persona_header: AvatarStyle {
                    ring: colors.avatars.persona_header.ring,
                    ring_width: 1.0,
                    fallback_surface: colors.avatars.persona_header.fallback_surface,
                    fallback_foreground: colors.avatars.persona_header.fallback_foreground,
                    initials_scale: 22.0 / 72.0,
                },
                lock_hero: AvatarStyle {
                    ring: colors.avatars.lock_hero.ring,
                    ring_width: 1.0,
                    fallback_surface: colors.avatars.lock_hero.fallback_surface,
                    fallback_foreground: colors.avatars.lock_hero.fallback_foreground,
                    initials_scale: 0.36,
                },
            },
            // The HUD core turns dark on light; the contour inverts with it
            // so legibility survives arbitrary bright wallpaper the same way
            // the dark appearance survives dark regions.
            hud_foreground: HudForeground {
                primary: colors.hud_foreground.primary,
                contour: colors.hud_foreground.contour,
                text_contour_width: 0.75,
                glyph_contour_width: 1.0,
            },
            // The scene floor inverts to the light surface gray; scrims stay
            // pale so bright content recedes behind modal chrome the same
            // way the dark appearance dims it.
            scene: colors.scene,
            dock: colors.dock,
            ..Self::dark()
        }
    }

    /// The design snapshot for one desktop color-scheme preference. `System`
    /// resolves through [`ColorScheme::or_dark`], so the returned snapshot's
    /// [`Design::scheme`] is always explicit.
    #[must_use]
    pub fn for_scheme(scheme: ColorScheme) -> Self {
        match scheme.or_dark() {
            ColorScheme::Dark | ColorScheme::System => Self::dark(),
            ColorScheme::Light => Self::light(),
        }
    }

    /// Whether this snapshot implements the light appearance.
    #[must_use]
    pub fn is_light(&self) -> bool {
        self.scheme == ColorScheme::Light
    }
}

impl Default for Design {
    fn default() -> Self {
        Self::dark()
    }
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
    /// HUD chips and pills.
    pub chip: f32,
    /// Launcher grid cells.
    pub cell: f32,
    /// Application-level modal panels.
    pub application: f32,
}

/// Shared stroke widths in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct Strokes {
    pub hairline: f32,
    pub scrollbar: f32,
}

/// The only permitted label sizes in chrome, in logical pixels. Components
/// snap their text to the nearest step instead of choosing arbitrary sizes,
/// keeping every surface on one readable hierarchy. Scheme-invariant.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct TypeScale {
    /// Smallest auxiliary text (badges, timestamps, fine print).
    pub caption: f32,
    /// Secondary annotations below body text.
    pub footnote: f32,
    /// Control and row labels.
    pub label: f32,
    /// Default reading size.
    pub body: f32,
    /// Section headers and emphasized rows.
    pub headline: f32,
    /// Panel and dialog titles.
    pub title: f32,
    /// The single largest display step, reserved for hero text.
    pub hero: f32,
}

/// Semantic role of one analytic Liquid Glass body.
///
/// Roles describe the body's elevation and use, not a numbered material
/// intensity. Refraction, rim lighting, and the material's curve shapes
/// remain one product-wide optical identity; role-specific variation is
/// limited to the body shadow and the material-strength multipliers that
/// keep text-bearing bodies legible over arbitrary backdrops.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlassRole {
    Chip,
    Tooltip,
    /// Text-bearing transient surfaces (context menus). Frost and tint run
    /// stronger so rows stay readable over busy content; the liquid identity
    /// lives in the rim, not the interior.
    Menu,
    FloatingPanel,
    ProminentPanel,
    Dock,
}

/// Per-body Liquid Glass policy. The shadow is configured in logical pixels;
/// `shadow_alpha` 0 disables it. The material strengths are multipliers on
/// the shared optical recipe (1.0 = the reference look): `frost_strength`
/// raises interior scattering, `tint_strength` the adaptive body tint, and
/// `saturation` the backdrop's surviving chroma — the three legibility
/// levers for text-bearing bodies. `plate_polarity` pins the tint direction
/// for the whole body (0 = always the smoke plate, 1 = always pearl);
/// -1 keeps the shader's per-pixel adaptive polarity. Text-bearing roles
/// pin polarity so the plate always opposes their text tone — a per-pixel
/// polarity would zebra over mixed content and can lift the plate toward
/// the text's own tone.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct GlassStyle {
    pub shadow_alpha: f32,
    pub shadow_blur: f32,
    pub shadow_offset_y: f32,
    pub frost_strength: f32,
    pub tint_strength: f32,
    pub saturation: f32,
    pub plate_polarity: f32,
}

impl GlassStyle {
    const fn new(shadow_alpha: f32, shadow_blur: f32, shadow_offset_y: f32) -> Self {
        Self {
            shadow_alpha,
            shadow_blur,
            shadow_offset_y,
            frost_strength: 1.0,
            tint_strength: 1.0,
            saturation: 1.0,
            plate_polarity: -1.0,
        }
    }

    /// Override the material policy: frost/tint strength multipliers,
    /// surviving backdrop saturation, and the pinned plate polarity.
    const fn with_material(
        mut self,
        frost: f32,
        tint: f32,
        saturation: f32,
        polarity: f32,
    ) -> Self {
        self.frost_strength = frost;
        self.tint_strength = tint;
        self.saturation = saturation;
        self.plate_polarity = polarity;
        self
    }
}

/// Role-indexed Liquid Glass policies for one appearance.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct GlassStyles {
    pub chip: GlassStyle,
    pub tooltip: GlassStyle,
    pub menu: GlassStyle,
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
            GlassRole::Menu => self.menu,
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
/// they remain owned by `tessera-shell::persona`.
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
/// single foreground color cannot guarantee local contrast. A restrained
/// dark contour keeps the light core legible on bright or visually busy
/// regions while disappearing naturally over dark regions. Text and glyphs
/// share one contour color; only their geometry-specific widths differ.
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
    fn dark_design_uses_the_dark_color_appearance() {
        let design = Design::dark();
        assert_eq!(design.scheme, ColorScheme::Dark);
        assert!(!design.is_light());
        assert_eq!(design.colors, crate::colors::dark_appearance().product);
        assert_eq!(design.radii.menu_item, 7.0);
    }

    #[test]
    fn for_scheme_resolves_system_to_the_dark_fallback() {
        assert_eq!(Design::for_scheme(ColorScheme::System), Design::dark());
        assert_eq!(Design::for_scheme(ColorScheme::Dark), Design::dark());
        assert_eq!(Design::for_scheme(ColorScheme::Light), Design::light());
        assert_eq!(Design::light().scheme, ColorScheme::Light);
        assert!(Design::light().is_light());
    }

    #[test]
    fn light_palette_inverts_ink_and_surface_while_sharing_geometry() {
        let light = Design::light();
        let dark = Design::dark();
        // Dark ink on white-ish surfaces.
        let (r, g, b, a) = light.colors.application_text.components();
        assert!(r < 80 && g < 80 && b < 90 && a == 255);
        let (r, g, b, a) = light.colors.application_surface.components();
        assert!(r > 220 && g > 220 && b > 220 && a == 255);
        // Tints and tracks become dark washes instead of white ones.
        let (r, g, b, _) = light.colors.menu_surface_hover.components();
        assert!(r < 64 && g < 64 && b < 64);
        // Radii, strokes, and preview policy are scheme-invariant.
        assert_eq!(light.radii, dark.radii);
        assert_eq!(light.strokes, dark.strokes);
        assert_eq!(light.preview, dark.preview);
        // Every semantic role stays populated.
        assert_eq!(light.colors.slider_fill, light.colors.application_accent);
        let (_, _, _, popover_alpha) = light.colors.popover_surface.components();
        assert!(popover_alpha > 200);
    }

    #[test]
    fn modal_scrim_text_stays_light_in_both_appearances() {
        // The scrim veil stays a dark wash in both appearances, so content
        // drawn directly on it keeps a light tone in both too — the light
        // scheme's page-appropriate ink would be illegible over the wash.
        for design in [Design::dark(), Design::light()] {
            let (r, g, b, a) = design.colors.modal_scrim_text.components();
            assert!(r > 220 && g > 220 && b > 220 && a == 255);
        }
    }

    #[test]
    fn parent_modal_scrim_and_attention_pulse_tokens_are_valid() {
        for design in [Design::dark(), Design::light()] {
            let (_, _, _, scrim_alpha) = design.colors.parent_modal_scrim.components();
            assert!(scrim_alpha >= 100 && scrim_alpha <= 160);
            let (_, _, _, pulse_alpha) = design.colors.attention_pulse_border.components();
            assert!(pulse_alpha >= 200);
        }
    }

    #[test]
    fn generic_icon_surface_is_scheme_invariant() {
        // The no-icon app chip is a mid-tone slate in both appearances.
        assert_eq!(
            Design::light().colors.generic_icon_surface,
            Design::dark().colors.generic_icon_surface
        );
        let (r, g, b, a) = Design::dark().colors.generic_icon_surface.components();
        assert!(r > 40 && r < 130 && g > 50 && g < 140 && b > 90 && b < 190 && a > 200);
    }

    #[test]
    fn launcher_field_glass_follows_the_scheme() {
        // Dark appearance: translucent dark glass, not the popover white —
        // over the blurred desktop veil a translucent-white bar reads as an
        // opaque bright bar. (Colors store premultiplied bytes, so the
        // assertions divide back out the alpha.)
        let dark = Design::dark().colors;
        let (r, g, b, a) = dark.launcher_field_surface.components();
        let unmul = |c: u8| u32::from(c) * 255 / u32::from(a.max(1));
        assert!(
            unmul(r) < 60 && unmul(g) < 60 && unmul(b) < 80,
            "dark field is dark glass: {}",
            dark.launcher_field_surface.components().0
        );
        assert!(a > 80 && a < 190, "and translucent: {a}");

        // Light appearance: white glass, matching its page tone.
        let light = Design::light().colors;
        let (r, g, b, a) = light.launcher_field_surface.components();
        let unmul = |c: u8| u32::from(c) * 255 / u32::from(a.max(1));
        assert!(
            unmul(r) > 240 && unmul(g) > 240 && unmul(b) > 240,
            "light field is white glass"
        );

        // The grid selection wash follows the scheme the same way: dark ink
        // in the dark appearance, white in the light one. (Stored bytes are
        // premultiplied, so divide the alpha back out.)
        let (dr, dg, db, da) = dark.launcher_selection_surface.components();
        let dunmul = |c: u8| u32::from(c) * 255 / u32::from(da.max(1));
        assert!(
            dunmul(dr) < 40 && dunmul(dg) < 40 && dunmul(db) < 50,
            "dark selection is a dark wash"
        );
        let (lr, lg, lb, la) = light.launcher_selection_surface.components();
        let lunmul = |c: u8| u32::from(c) * 255 / u32::from(la.max(1));
        assert!(
            lunmul(lr) > 240 && lunmul(lg) > 240 && lunmul(lb) > 240,
            "light selection is a light wash"
        );
    }

    #[test]
    fn hud_foreground_uses_one_restrained_contour_family() {
        let hud = Design::dark().hud_foreground;
        let colors = crate::colors::dark_appearance().hud_foreground;
        assert_eq!(hud.primary, colors.primary);
        assert_eq!(hud.contour, colors.contour);
        assert!(hud.text_contour_width > 0.0);
        assert!(hud.text_contour_width < hud.glyph_contour_width);
        assert!(hud.glyph_contour_width <= 1.0);
    }

    #[test]
    fn glass_focus_is_neutral_borderless_policy() {
        let design = Design::dark();
        let focus = design.glass_focus;
        let colors = crate::colors::dark_appearance().glass_focus;
        assert_eq!(focus.hover_tint, colors.surface_hover);
        assert_eq!(focus.selected_tint, colors.surface_selected);
        assert_eq!(focus.field_strength, 1.0);
        assert_eq!(design.preview.inactive_content_brightness, 0.74);
    }

    #[test]
    fn glass_roles_name_elevation_without_changing_material_identity() {
        let glass = Design::dark().glass;
        assert_eq!(
            glass.for_role(GlassRole::FloatingPanel),
            GlassStyle::new(0.18, 16.0, 8.0).with_material(3.5, 3.0, 0.85, 0.0)
        );
        assert!(glass.chip.shadow_blur < glass.floating_panel.shadow_blur);
        assert!(glass.floating_panel.shadow_blur < glass.prominent_panel.shadow_blur);
    }

    #[test]
    fn menu_and_tooltip_roles_carry_legibility_material_strengths() {
        let glass = Design::dark().glass;
        // Text-bearing bodies boost interior frost and adaptive tint, damp
        // backdrop chroma, and pin the plate polarity against their text
        // tone; decorative bodies keep the reference recipe.
        let menu = glass.for_role(GlassRole::Menu);
        assert_eq!(
            menu,
            GlassStyle::new(0.18, 16.0, 8.0).with_material(5.0, 3.6, 0.7, 0.0)
        );
        assert!(menu.frost_strength > glass.floating_panel.frost_strength);
        assert!(menu.tint_strength > glass.floating_panel.tint_strength);
        assert!(menu.saturation < 1.0);
        // Dark appearance pins plates to smoke (0); the shader's
        // per-pixel pearl-over-dark would wash the plate toward the light
        // text and undo the legibility the strengths buy.
        assert_eq!(menu.plate_polarity, 0.0);
        let tooltip = glass.for_role(GlassRole::Tooltip);
        assert!(tooltip.frost_strength > 1.0 && tooltip.tint_strength > 1.0);
        assert!(tooltip.frost_strength < menu.frost_strength);
        assert_eq!(tooltip.plate_polarity, 0.0);
        for role in [
            GlassRole::Chip,
            GlassRole::Tooltip,
            GlassRole::Menu,
            GlassRole::FloatingPanel,
            GlassRole::ProminentPanel,
            GlassRole::Dock,
        ] {
            let style = glass.for_role(role);
            assert_eq!(style.plate_polarity, 0.0);
        }
        // Light appearance pins the opposite polarity (pearl plate under dark
        // text) with strengths of its own.
        let light = Design::light().glass;
        assert_eq!(light.menu.plate_polarity, 1.0);
        assert_eq!(light.for_role(GlassRole::Tooltip).plate_polarity, 1.0);
        assert_eq!(light.for_role(GlassRole::FloatingPanel).plate_polarity, 1.0);
        assert!(light.menu.tint_strength > menu.tint_strength);
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
        let colors = crate::colors::dark_appearance().avatars;
        let header = avatars.for_role(AvatarRole::PersonaHeader);
        let lock = avatars.for_role(AvatarRole::LockHero);
        assert_eq!(header.ring, colors.persona_header.ring);
        assert_eq!(lock.ring, colors.lock_hero.ring);
        assert!(header.initials_scale > 0.0);
        assert!(lock.initials_scale > 0.0);
    }

    #[test]
    fn type_scale_is_monotonic_with_exactly_the_spec_steps() {
        let type_scale = Design::dark().typography;
        assert_eq!(type_scale.caption, 10.0);
        assert_eq!(type_scale.footnote, 11.0);
        assert_eq!(type_scale.label, 12.0);
        assert_eq!(type_scale.body, 13.0);
        assert_eq!(type_scale.headline, 15.0);
        assert_eq!(type_scale.title, 20.0);
        assert_eq!(type_scale.hero, 24.0);
        let steps = [
            type_scale.caption,
            type_scale.footnote,
            type_scale.label,
            type_scale.body,
            type_scale.headline,
            type_scale.title,
            type_scale.hero,
        ];
        assert!(steps.windows(2).all(|pair| pair[0] < pair[1]));
        // Typography is scheme-invariant.
        assert_eq!(Design::light().typography, type_scale);
    }

    #[test]
    fn new_radii_are_present_and_scheme_invariant() {
        let dark = Design::dark();
        assert_eq!(dark.radii.chip, 16.0);
        assert_eq!(dark.radii.cell, 22.0);
        assert_eq!(dark.radii.application, 24.0);
        assert_eq!(Design::light().radii, dark.radii);
    }

    #[test]
    fn critical_and_validation_stay_in_hue_family_per_scheme() {
        let dark = Design::dark();
        let light = Design::light();
        // Same hue families, darkened so they read on light surfaces.
        let (r, g, b, a) = light.colors.critical.components();
        assert!(r > g && r > b && a == 255);
        let (r, g, b, a) = light.colors.validation.components();
        assert!(b > r && b > g && a == 255);
        assert_ne!(light.colors.critical, dark.colors.critical);
        assert_ne!(light.colors.validation, dark.colors.validation);
    }

    #[test]
    fn dark_scene_colors_match_the_compositor_palette() {
        let scene = Design::dark().scene;
        assert_eq!(scene, crate::colors::dark_appearance().scene);
        assert_eq!(scene.glass_tint, [255, 255, 255]);
        // `System` resolves through the dark arm, as in `runtime::scheme`.
        assert_eq!(Design::for_scheme(ColorScheme::System).scene, scene);
    }

    #[test]
    fn light_scene_colors_match_the_compositor_palette() {
        let scene = Design::light().scene;
        assert_eq!(scene, crate::colors::light_appearance().scene);
        assert_eq!(scene.glass_tint, [243, 245, 249]);
    }

    #[test]
    fn dock_palette_matches_the_dock_literals_and_is_scheme_invariant() {
        let dock = Design::dark().dock;
        assert_eq!(dock, crate::colors::dark_appearance().dock);
        // Both schemes share the dock palette (light inherits via `..Self::dark()`).
        assert_eq!(Design::light().dock, dock);
    }
}
