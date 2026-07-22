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
                dock_surface: Color::rgba(255, 255, 255, 34),
                dock_border: Color::rgba(255, 255, 255, 64),
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
                dock: 18.0,
                control: 12.0,
                card: 16.0,
                scrollbar: 2.5,
            },
            strokes: Strokes {
                hairline: 1.0,
                scrollbar: 5.0,
            },
        }
    }
}

impl Default for Design {
    fn default() -> Self {
        Self::dark()
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
    pub dock_surface: Color,
    pub dock_border: Color,
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
    pub dock: f32,
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
}
