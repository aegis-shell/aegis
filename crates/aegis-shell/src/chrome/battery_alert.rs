//! The low-battery alert: a modal, centered panel raised by the compositor
//! itself when the battery crosses a configured `[battery] warn_at`
//! threshold. Like the user-consent prompts it owns the chrome band until
//! dismissed — full-screen scrim, analytic-glass panel, exclusive
//! presentation, keyboard and pointer captured — but it is compositor-owned:
//! dismissal simply closes it, no answer travels back through
//! [`ChromeEvents`], and it never enters the notification queue (a battery
//! warning must not be DND-suppressible).
//!
//! [`ChromeCommand::StartBatteryAlert`] opens or updates the panel;
//! [`ChromeCommand::CancelBatteryAlert`] closes it without a reply.

use lens::{Color, Frame, Input, LayoutOpts, Rect};

use crate::{
    BackdropRegion, Chrome, ChromeCommand, ChromeEvents, ChromeUpdate, CursorShape,
    LiquidGlassRegion, Localizer, Reserved, ellipsize, modal_scrim_backdrop,
};
use aegis_design::{Design, GlassRole, materials, themes};
use aegis_model::input::{KeyAction, KeyChar, key_action};
use aegis_model::window::Window;
use aegis_ui::{
    ActionButtonStyle, DEFAULT_BACKDROP_BLUR_SIGMA, DEFAULT_BUTTON_HEIGHT, DEFAULT_BUTTON_WIDTH,
    DEFAULT_MODAL_PAD, DEFAULT_TITLE_HEIGHT, contains, place_modal_panel, render_action_button,
    stretch, stretch_top,
};

const PANEL_W: f32 = 400.0;
const PANEL_PAD: f32 = DEFAULT_MODAL_PAD;
const GAUGE_H: f32 = 40.0;
const GLYPH_W: f32 = 64.0;
const GLYPH_H: f32 = 30.0;
const GLYPH_NUB_W: f32 = 4.0;
const TITLE_H: f32 = DEFAULT_TITLE_HEIGHT;
const BODY_H: f32 = 20.0;
const BUTTON_H: f32 = DEFAULT_BUTTON_HEIGHT;
const BUTTON_W: f32 = DEFAULT_BUTTON_WIDTH;
const BACKDROP_BLUR_SIGMA: f32 = DEFAULT_BACKDROP_BLUR_SIGMA;

/// Parameters of one low-battery alert, produced by the compositor runtime
/// when a configured threshold fires.
#[derive(Debug, Clone, Copy)]
pub struct BatteryAlertParams {
    /// Charge level shown in the panel, in percent.
    pub percent: u8,
    /// The lowest configured threshold fired: the alert uses the critical
    /// wording and the rejection-red fill.
    pub critical: bool,
}

/// The resolved geometry of the panel for one frame.
#[derive(Debug, Clone, Copy)]
struct AlertLayout {
    panel: Rect,
    glyph: Rect,
    percent: Rect,
    title: Rect,
    body: Rect,
    ok: Rect,
}

impl AlertLayout {
    fn for_display(display: (f32, f32), reserved: Reserved) -> AlertLayout {
        let left = reserved.left.max(0) as f32;
        let top = reserved.top.max(0) as f32;
        let usable_w = (display.0 - left - reserved.right.max(0) as f32).max(1.0);
        let usable_h = (display.1 - top - reserved.bottom.max(0) as f32).max(1.0);

        let panel_w = PANEL_W.min((usable_w - 32.0).max(240.0));
        let panel_h =
            PANEL_PAD + GAUGE_H + 4.0 + TITLE_H + 4.0 + BODY_H + 10.0 + BUTTON_H + PANEL_PAD;
        let panel = Rect {
            x: left + ((usable_w - panel_w) * 0.5).max(0.0),
            y: top + ((usable_h - panel_h) * 0.5).max(0.0),
            w: panel_w,
            h: panel_h,
        };

        let inner_x = panel.x + PANEL_PAD;
        let inner_w = panel.w - 2.0 * PANEL_PAD;
        let glyph = Rect {
            x: inner_x,
            y: panel.y + PANEL_PAD,
            w: GLYPH_W + 2.0 + GLYPH_NUB_W,
            h: GAUGE_H,
        };
        let percent = Rect {
            x: glyph.x + glyph.w + 12.0,
            y: glyph.y,
            w: (inner_x + inner_w - glyph.x - glyph.w - 12.0).max(0.0),
            h: GAUGE_H,
        };
        let title = Rect {
            x: inner_x,
            y: glyph.y + GAUGE_H + 4.0,
            w: inner_w,
            h: TITLE_H,
        };
        let body = Rect {
            x: inner_x,
            y: title.y + title.h + 4.0,
            w: inner_w,
            h: BODY_H,
        };
        let ok = Rect {
            x: panel.x + panel.w - PANEL_PAD - BUTTON_W,
            y: panel.y + panel.h - PANEL_PAD - BUTTON_H,
            w: BUTTON_W,
            h: BUTTON_H,
        };
        AlertLayout {
            panel,
            glyph,
            percent,
            title,
            body,
            ok,
        }
    }
}

/// The battery-alert chrome component. Inert until the runtime opens it with
/// [`ChromeCommand::StartBatteryAlert`].
pub struct BatteryAlert {
    active: bool,
    percent: u8,
    critical: bool,
    modal_reserved: Reserved,
    /// The design snapshot the alert paints from, from
    /// [`ChromeUpdate::Appearance`]. Seeded on registration by
    /// [`crate::Shell::add`] and refreshed when the desktop color scheme
    /// changes; defaults to the dark appearance until the first update arrives.
    design: Design,
}

impl BatteryAlert {
    pub fn new() -> BatteryAlert {
        BatteryAlert {
            active: false,
            percent: 0,
            critical: false,
            modal_reserved: Reserved::default(),
            design: Design::dark(),
        }
    }

    fn start_battery_alert(&mut self, params: BatteryAlertParams) {
        self.percent = params.percent;
        self.critical = params.critical;
        self.active = true;
    }

    /// Dismiss the alert. Fire-and-forget: no answer travels back.
    fn dismiss(&mut self) {
        self.active = false;
    }

    /// Handle one primary-button press at output-space `(x, y)`: dismisses
    /// on the OK button. Clicks anywhere else — including the scrim — are
    /// ignored: a low-battery warning must leave through a deliberate
    /// action, never through an accidental click.
    fn press_at(&mut self, x: f32, y: f32, display: (f32, f32)) {
        let layout = AlertLayout::for_display(display, self.modal_reserved);
        if contains(layout.ok, x, y) {
            self.dismiss();
        }
    }
}

impl Default for BatteryAlert {
    fn default() -> Self {
        BatteryAlert::new()
    }
}

impl Chrome for BatteryAlert {
    fn render(
        &mut self,
        frame: &mut Frame,
        input: &Input,
        _windows: &[Window],
        _workspaces: &crate::WorkspaceSnapshot,
        _i18n: &Localizer,
        _out: &mut ChromeEvents,
    ) {
        if !self.active {
            return;
        }
        let raw = input.as_raw();
        let display = (raw.display_size.x, raw.display_size.y);
        let cursor = raw.cursor;
        let pressed = raw.mouse_pressed.first().copied().unwrap_or(false);
        let design = self.design;
        let layout = AlertLayout::for_display(display, self.modal_reserved);

        let original_theme = frame.theme();
        frame.set_theme(themes::application(&design));

        // Minimal foreground tint only. The compositor-owned analytic pass
        // supplies the body, refraction, rim light, and shadow.
        place_modal_panel(frame, "aegis-battery-alert-panel", layout.panel, &design);

        // The gauge: a rounded outline with a charge-proportional fill and
        // the battery's terminal nub. The fill carries the meaning: accent
        // for a plain warning, the shared critical red when critical.
        let fill_color = if self.critical {
            design.colors.critical
        } else {
            design.colors.application_accent
        };
        let outline = Rect {
            x: layout.glyph.x,
            y: layout.glyph.y + (layout.glyph.h - GLYPH_H) * 0.5,
            w: GLYPH_W,
            h: GLYPH_H,
        };
        frame.place(
            "aegis-battery-alert-glyph-outline",
            &materials::chrome_place(
                outline,
                LayoutOpts {
                    bg: Color::TRANSPARENT,
                    border: design.colors.application_text,
                    border_width: 2.0,
                    radius: 6.0,
                    pad: 0.0,
                    ..materials::surface_layout()
                },
            ),
            |_| {},
        );
        let charge = (f32::from(self.percent) / 100.0).clamp(0.0, 1.0);
        frame.place(
            "aegis-battery-alert-glyph-fill",
            &materials::chrome_place(
                Rect {
                    x: outline.x + 4.0,
                    y: outline.y + 4.0,
                    w: ((GLYPH_W - 8.0) * charge).max(0.0),
                    h: GLYPH_H - 8.0,
                },
                LayoutOpts {
                    bg: fill_color,
                    radius: 3.0,
                    pad: 0.0,
                    ..materials::surface_layout()
                },
            ),
            |_| {},
        );
        frame.place(
            "aegis-battery-alert-glyph-nub",
            &materials::chrome_place(
                Rect {
                    x: outline.x + GLYPH_W + 2.0,
                    y: outline.y + (GLYPH_H - 10.0) * 0.5,
                    w: GLYPH_NUB_W,
                    h: 10.0,
                },
                LayoutOpts {
                    bg: design.colors.application_text,
                    radius: 2.0,
                    pad: 0.0,
                    ..materials::surface_layout()
                },
            ),
            |_| {},
        );

        let percent = format!("{}%", self.percent);
        frame.place(
            "aegis-battery-alert-percent",
            &materials::chrome_place(layout.percent, materials::transparent()),
            |frame| {
                frame.row_ex(&stretch(layout.percent), |frame| {
                    frame.label_sized(&percent, design.typography.hero);
                });
            },
        );

        let title = if self.critical {
            "Battery critically low"
        } else {
            "Battery low"
        };
        let title = ellipsize(
            frame,
            title,
            design.typography.headline,
            (layout.title.w - frame.theme().padding() * 2.0).max(0.0),
        );
        frame.place(
            "aegis-battery-alert-title",
            &materials::chrome_place(layout.title, materials::transparent()),
            |frame| {
                frame.row_ex(&stretch(layout.title), |frame| {
                    frame.label_sized(&title, design.typography.headline);
                });
            },
        );

        let body = if self.critical {
            "Connect a charger immediately."
        } else {
            "Connect a charger."
        };
        let body = ellipsize(frame, body, design.typography.label, layout.body.w);
        frame.place(
            "aegis-battery-alert-body",
            &materials::chrome_place(layout.body, materials::transparent()),
            |frame| {
                frame.row_ex(&stretch_top(layout.body), |frame| {
                    frame.label_compact_sized(&body, design.typography.label);
                });
            },
        );

        render_action_button(
            frame,
            "aegis-battery-alert-ok",
            layout.ok,
            "OK",
            ActionButtonStyle::Accented,
            contains(layout.ok, cursor.x, cursor.y),
            &design,
        );

        frame.set_theme(original_theme);

        if pressed {
            self.press_at(cursor.x, cursor.y, display);
        }
    }

    fn captures_keyboard(&self) -> bool {
        self.active
    }

    fn captures_pointer(
        &self,
        _x: f32,
        _y: f32,
        _display: (f32, f32),
        _windows: &[Window],
        _workspaces: &crate::WorkspaceSnapshot,
    ) -> bool {
        self.active
    }

    fn modal_active(&self) -> bool {
        self.active
    }

    // A battery warning owns the complete chrome band: the Dock, HUD, and
    // toasts stay suppressed until the alert is dismissed.
    fn exclusive_presentation_active(&self) -> bool {
        self.active
    }

    fn visible_during_modal(&self) -> bool {
        true
    }

    fn requires_composition(&self) -> bool {
        self.active
    }

    fn cursor_shape_at(
        &self,
        x: f32,
        y: f32,
        display: (f32, f32),
        _windows: &[Window],
        _workspaces: &crate::WorkspaceSnapshot,
    ) -> Option<CursorShape> {
        if !self.active {
            return None;
        }
        let layout = AlertLayout::for_display(display, self.modal_reserved);
        Some(if contains(layout.ok, x, y) {
            CursorShape::Pointer
        } else {
            CursorShape::Default
        })
    }

    fn update(&mut self, update: ChromeUpdate<'_>) {
        match update {
            ChromeUpdate::ModalReserved(reserved) => self.modal_reserved = reserved,
            ChromeUpdate::Appearance(design) => self.design = *design,
            _ => {}
        }
    }

    fn key_char(&mut self, key: &KeyChar, _out: &mut ChromeEvents) {
        if !self.active {
            return;
        }
        match key_action(key.keysym, key.ch) {
            KeyAction::Enter | KeyAction::Escape => self.dismiss(),
            _ => {}
        }
    }

    fn command(&mut self, command: &ChromeCommand<'_>, _out: &mut ChromeEvents) {
        match command {
            ChromeCommand::StartBatteryAlert(params) => self.start_battery_alert(**params),
            ChromeCommand::CancelBatteryAlert if self.active => self.active = false,
            ChromeCommand::DismissModal if self.active => self.dismiss(),
            _ => {}
        }
    }

    fn battery_alert_active(&self) -> bool {
        self.active
    }

    fn backdrop_blur_sigma(&self) -> f32 {
        if self.active {
            BACKDROP_BLUR_SIGMA
        } else {
            0.0
        }
    }

    fn backdrop_regions(
        &self,
        display: (f32, f32),
        _windows: &[Window],
        _workspaces: &crate::WorkspaceSnapshot,
    ) -> Vec<BackdropRegion> {
        if !self.active {
            return Vec::new();
        }
        let layout = AlertLayout::for_display(display, self.modal_reserved);
        // The modal's full-display dim is a wash INTO the frost (beneath the
        // panel's glass body) — it used to be painted above the glass, which
        // hid the lens's refraction and split the layer stack. The second
        // region exactly matches the glass body below: the runtime drops it
        // from the rectangular frost set, so the analytic pass alone owns
        // the rounded panel.
        vec![
            modal_scrim_backdrop(display, &self.design),
            BackdropRegion::from(layout.panel),
        ]
    }

    fn liquid_glass_regions(
        &self,
        display: (f32, f32),
        _windows: &[Window],
        _workspaces: &crate::WorkspaceSnapshot,
    ) -> Vec<LiquidGlassRegion> {
        if !self.active {
            return Vec::new();
        }
        let layout = AlertLayout::for_display(display, self.modal_reserved);
        vec![LiquidGlassRegion::from_role(
            &self.design,
            GlassRole::ProminentPanel,
            BackdropRegion::from(layout.panel),
            self.design.radii.glass_panel,
            1.0,
        )]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> BatteryAlertParams {
        BatteryAlertParams {
            percent: 20,
            critical: false,
        }
    }

    fn key(keysym: u32) -> KeyChar {
        KeyChar {
            keysym,
            ch: None,
            mods: aegis_model::input::Mods::NONE,
        }
    }

    #[test]
    fn start_populates_the_panel() {
        let mut alert = BatteryAlert::new();
        assert!(!alert.battery_alert_active());
        alert.start_battery_alert(BatteryAlertParams {
            percent: 5,
            critical: true,
        });
        assert!(alert.battery_alert_active());
        assert_eq!(alert.percent, 5);
        assert!(alert.critical);
    }

    #[test]
    fn enter_dismisses() {
        let mut alert = BatteryAlert::new();
        alert.start_battery_alert(params());
        alert.key_char(
            &key(aegis_model::input::XKB_KEY_Return),
            &mut ChromeEvents::default(),
        );
        assert!(!alert.battery_alert_active());
    }

    #[test]
    fn escape_dismisses() {
        let mut alert = BatteryAlert::new();
        alert.start_battery_alert(params());
        alert.key_char(
            &key(aegis_model::input::XKB_KEY_Escape),
            &mut ChromeEvents::default(),
        );
        assert!(!alert.battery_alert_active());
    }

    #[test]
    fn clicking_ok_dismisses() {
        let mut alert = BatteryAlert::new();
        alert.start_battery_alert(params());
        let display = (1280.0, 800.0);
        let ok = AlertLayout::for_display(display, Reserved::default()).ok;
        alert.press_at(ok.x + 4.0, ok.y + 4.0, display);
        assert!(!alert.battery_alert_active());
    }

    #[test]
    fn clicking_outside_keeps_the_alert() {
        let mut alert = BatteryAlert::new();
        alert.start_battery_alert(params());
        alert.press_at(4.0, 4.0, (1280.0, 800.0));
        assert!(alert.battery_alert_active());
    }

    #[test]
    fn the_panic_chord_command_dismisses() {
        let mut alert = BatteryAlert::new();
        alert.start_battery_alert(params());
        alert.command(&ChromeCommand::DismissModal, &mut ChromeEvents::default());
        assert!(!alert.battery_alert_active());
    }

    #[test]
    fn clicking_the_panel_body_keeps_the_alert() {
        let mut alert = BatteryAlert::new();
        alert.start_battery_alert(params());
        let display = (1280.0, 800.0);
        let layout = AlertLayout::for_display(display, Reserved::default());
        alert.press_at(layout.title.x + 4.0, layout.title.y + 4.0, display);
        assert!(alert.battery_alert_active());
    }

    #[test]
    fn the_active_panel_is_one_analytic_glass_body() {
        let mut alert = BatteryAlert::new();
        let display = (1280.0, 800.0);
        let workspaces = crate::WorkspaceSnapshot {
            outputs: Vec::new(),
        };
        assert!(
            alert
                .liquid_glass_regions(display, &[], &workspaces)
                .is_empty()
        );
        assert!(!alert.exclusive_presentation_active());

        alert.start_battery_alert(params());
        let backdrop = alert.backdrop_regions(display, &[], &workspaces);
        let glass = alert.liquid_glass_regions(display, &[], &workspaces);
        // Fullscreen veil wash + the panel region the glass body equals.
        assert_eq!(backdrop.len(), 2);
        assert!(
            backdrop[0].wash.is_some(),
            "the dim is a wash into the frost"
        );
        assert_eq!(glass.len(), 1);
        assert_eq!(glass[0].bounds, backdrop[1]);
        assert_eq!(glass[0].corner_radius, Design::dark().radii.glass_panel);
        assert_eq!(glass[0].opacity, 1.0);
        assert!(alert.exclusive_presentation_active());
    }
}
