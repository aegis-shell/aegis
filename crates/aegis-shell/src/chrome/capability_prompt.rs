//! The agent capability-borrowing consent dialog: a modal, centered panel
//! listing the requested capability groups with one checkbox each, so the
//! user approves a subset instead of an all-or-nothing Allow/Deny
//! (ADR-0088 agent pairing).
//!
//! The flow mirrors the confirmation dialog: [`ChromeCommand::StartCapabilityPick`]
//! opens the panel, and the user's answer travels back through
//! [`ChromeEvents::capability_pick_answered`] (`approved: Some(keys)` = the
//! checked groups the user allowed, `approved: None` = denied). Ordinary
//! modal chrome over the live scene: no freeze, no screen-content capture.

use lens::{Align, Color, Frame, Input, LayoutOpts, Rect};

use crate::{
    BackdropRegion, Chrome, ChromeCommand, ChromeEvents, ChromeUpdate, CursorShape,
    LiquidGlassRegion, Localizer, Reserved, ellipsize,
};
use aegis_design::{Design, GlassRole, materials, themes};
use aegis_model::input::{KeyAction, KeyChar, key_action};
use aegis_model::window::Window;

const PANEL_W: f32 = 460.0;
const PANEL_PAD: f32 = 16.0;
const TITLE_H: f32 = 24.0;
const WARNING_H: f32 = 24.0;
const ROW_H: f32 = 26.0;
const CHECK: f32 = 15.0;
const BUTTON_H: f32 = 30.0;
const BUTTON_W: f32 = 96.0;
const BACKDROP_BLUR_SIGMA: f32 = 18.0;

/// Trailing note on runtime-gated groups: however the pairing was approved,
/// first use confirms again interactively (ADR-0088).
const GATED_NOTE: &str = "confirmed again on first use";

/// Parameters of one capability-borrowing checklist, mapped from the agent
/// pairing request by the compositor runtime.
#[derive(Debug, Clone)]
pub struct CapabilityPickParams {
    /// Dialog heading (e.g. "Codex wants to borrow desktop capabilities").
    pub title: String,
    /// Look-alike installation warning (ADR-0088 TOFU continuity), shown as
    /// a highlighted row under the title when present.
    pub warning: Option<String>,
    /// One row per requested capability group, in display order.
    pub groups: Vec<CapabilityGroup>,
}

/// One checkable capability group row.
#[derive(Debug, Clone)]
pub struct CapabilityGroup {
    /// Stable machine key the runtime maps back to an operation family.
    pub key: String,
    /// Human-readable capability description (e.g. "Focus windows").
    pub label: String,
    /// High-risk group: first use is confirmed again interactively.
    pub gated: bool,
    /// Initially checked.
    pub enabled: bool,
}

/// The user's answer: the checked group keys on Allow, or `None` on
/// Deny/Escape/click-away.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityPickResult {
    pub approved: Option<Vec<String>>,
}

/// The resolved geometry of the panel for one frame.
#[derive(Debug, Clone)]
struct PromptLayout {
    panel: Rect,
    title: Rect,
    warning: Option<Rect>,
    rows: Vec<Rect>,
    deny: Rect,
    allow: Rect,
}

impl PromptLayout {
    fn for_display(
        display: (f32, f32),
        reserved: Reserved,
        groups: usize,
        has_warning: bool,
    ) -> PromptLayout {
        let left = reserved.left.max(0) as f32;
        let top = reserved.top.max(0) as f32;
        let usable_w = (display.0 - left - reserved.right.max(0) as f32).max(1.0);
        let usable_h = (display.1 - top - reserved.bottom.max(0) as f32).max(1.0);

        let panel_w = PANEL_W.min((usable_w - 32.0).max(240.0));
        let warning_h = if has_warning { WARNING_H + 4.0 } else { 0.0 };
        let rows_h = groups as f32 * ROW_H;
        let panel_h = PANEL_PAD + TITLE_H + 4.0 + warning_h + rows_h + 10.0 + BUTTON_H + PANEL_PAD;
        let panel = Rect {
            x: left + ((usable_w - panel_w) * 0.5).max(0.0),
            y: top + ((usable_h - panel_h) * 0.5).max(0.0),
            w: panel_w,
            h: panel_h,
        };

        let inner_x = panel.x + PANEL_PAD;
        let inner_w = panel.w - 2.0 * PANEL_PAD;
        let title = Rect {
            x: inner_x,
            y: panel.y + PANEL_PAD,
            w: inner_w,
            h: TITLE_H,
        };
        let warning = has_warning.then_some(Rect {
            x: inner_x,
            y: title.y + title.h + 4.0,
            w: inner_w,
            h: WARNING_H,
        });
        let rows_y = title.y + title.h + 4.0 + warning_h;
        let rows = (0..groups)
            .map(|index| Rect {
                x: inner_x,
                y: rows_y + index as f32 * ROW_H,
                w: inner_w,
                h: ROW_H,
            })
            .collect();
        let buttons_y = panel.y + panel.h - PANEL_PAD - BUTTON_H;
        let allow = Rect {
            x: panel.x + panel.w - PANEL_PAD - BUTTON_W,
            y: buttons_y,
            w: BUTTON_W,
            h: BUTTON_H,
        };
        let deny = Rect {
            x: allow.x - BUTTON_W - 8.0,
            y: buttons_y,
            w: BUTTON_W,
            h: BUTTON_H,
        };
        PromptLayout {
            panel,
            title,
            warning,
            rows,
            deny,
            allow,
        }
    }
}

/// The capability-checklist chrome component. Inert until the runtime opens
/// it with [`ChromeCommand::StartCapabilityPick`].
pub struct CapabilityPrompt {
    active: bool,
    title: String,
    warning: Option<String>,
    groups: Vec<CapabilityGroup>,
    modal_reserved: Reserved,
    /// The design snapshot the prompt paints from, from
    /// [`ChromeUpdate::Appearance`]. Seeded on registration by
    /// [`crate::Shell::add`] and refreshed when the desktop color scheme
    /// changes; defaults to the dark appearance until the first update arrives.
    design: Design,
}

impl CapabilityPrompt {
    pub fn new() -> CapabilityPrompt {
        CapabilityPrompt {
            active: false,
            title: String::new(),
            warning: None,
            groups: Vec::new(),
            modal_reserved: Reserved::default(),
            design: Design::dark(),
        }
    }

    fn layout(&self, display: (f32, f32)) -> PromptLayout {
        PromptLayout::for_display(
            display,
            self.modal_reserved,
            self.groups.len(),
            self.warning.is_some(),
        )
    }

    /// Answer the dialog and close.
    fn answer(&mut self, approved: Option<Vec<String>>, out: &mut ChromeEvents) {
        out.capability_pick_answered = Some(CapabilityPickResult { approved });
        self.active = false;
    }

    fn start_capability_pick(&mut self, params: CapabilityPickParams) {
        self.title = params.title;
        self.warning = params.warning;
        self.groups = params.groups;
        self.active = true;
    }

    /// Allow the currently checked groups and close.
    fn allow(&mut self, out: &mut ChromeEvents) {
        let keys = self
            .groups
            .iter()
            .filter(|group| group.enabled)
            .map(|group| group.key.clone())
            .collect();
        self.answer(Some(keys), out);
    }

    /// Handle one primary-button press at output-space `(x, y)`: toggles a
    /// row, answers on the buttons, or denies on a click outside the panel.
    fn press_at(&mut self, x: f32, y: f32, display: (f32, f32), out: &mut ChromeEvents) {
        let layout = self.layout(display);
        if contains(layout.deny, x, y) {
            self.answer(None, out);
            return;
        }
        if contains(layout.allow, x, y) {
            self.allow(out);
            return;
        }
        if !contains(layout.panel, x, y) {
            self.answer(None, out);
            return;
        }
        for (index, row) in layout.rows.iter().enumerate() {
            if contains(*row, x, y) {
                if let Some(group) = self.groups.get_mut(index) {
                    group.enabled = !group.enabled;
                }
                return;
            }
        }
    }
}

impl Default for CapabilityPrompt {
    fn default() -> Self {
        CapabilityPrompt::new()
    }
}

impl Chrome for CapabilityPrompt {
    fn render(
        &mut self,
        frame: &mut Frame,
        input: &Input,
        _windows: &[Window],
        _workspaces: &crate::WorkspaceSnapshot,
        _i18n: &Localizer,
        out: &mut ChromeEvents,
    ) {
        if !self.active {
            return;
        }
        let raw = input.as_raw();
        let display = (raw.display_size.x, raw.display_size.y);
        let cursor = raw.cursor;
        let pressed = raw.mouse_pressed.first().copied().unwrap_or(false);
        let design = self.design;
        let layout = self.layout(display);

        frame.place(
            "aegis-capability-prompt-scrim",
            &materials::chrome_place(
                Rect {
                    x: 0.0,
                    y: 0.0,
                    w: display.0,
                    h: display.1,
                },
                LayoutOpts {
                    bg: design.colors.scrim,
                    ..materials::surface_layout()
                },
            ),
            |_| {},
        );

        let original_theme = frame.theme();
        frame.set_theme(themes::application(&design));

        // Minimal foreground tint only. The compositor-owned analytic pass
        // supplies the body, refraction, rim light, and shadow.
        frame.place(
            "aegis-capability-prompt-panel",
            &materials::chrome_place(layout.panel, materials::glass_panel(&design)),
            |_| {},
        );

        let title = ellipsize(
            frame,
            &self.title,
            15.0,
            (layout.title.w - frame.theme().padding() * 2.0).max(0.0),
        );
        frame.place(
            "aegis-capability-prompt-title",
            &materials::chrome_place(layout.title, transparent()),
            |frame| {
                frame.row_ex(&stretch(layout.title), |frame| {
                    frame.label_sized(&title, 15.0);
                });
            },
        );

        if let (Some(warning), Some(rect)) = (&self.warning, layout.warning) {
            let warning = ellipsize(
                frame,
                &format!("Warning: {warning}"),
                12.0,
                (rect.w - 12.0).max(0.0),
            );
            frame.place(
                "aegis-capability-prompt-warning",
                &materials::chrome_place(
                    rect,
                    LayoutOpts {
                        bg: design.colors.application_hover,
                        border: design.colors.application_border,
                        border_width: design.strokes.hairline,
                        radius: design.radii.control,
                        pad: 0.0,
                        ..materials::surface_layout()
                    },
                ),
                |frame| {
                    frame.row_ex(&stretch_pad(rect), |frame| {
                        frame.label_compact_sized(&warning, 12.0);
                    });
                },
            );
        }

        for (index, group) in self.groups.iter().enumerate() {
            let row = layout.rows[index];
            let hovered = contains(row, cursor.x, cursor.y);
            frame.place(
                &format!("aegis-capability-prompt-row-{index}"),
                &materials::chrome_place(
                    row,
                    if hovered {
                        materials::glass_focus(&design, false, 1.0)
                    } else {
                        LayoutOpts {
                            bg: Color::TRANSPARENT,
                            radius: design.radii.control,
                            pad: 0.0,
                            ..materials::surface_layout()
                        }
                    },
                ),
                |_| {},
            );
            let check = Rect {
                x: row.x + 6.0,
                y: row.y + (ROW_H - CHECK) * 0.5,
                w: CHECK,
                h: CHECK,
            };
            let enabled = group.enabled;
            frame.place(
                &format!("aegis-capability-prompt-check-{index}"),
                &materials::chrome_place(
                    check,
                    LayoutOpts {
                        bg: if enabled {
                            design.colors.application_accent
                        } else {
                            design.colors.card_surface
                        },
                        border: design.colors.application_border,
                        border_width: design.strokes.hairline,
                        radius: design.radii.control,
                        pad: 0.0,
                        cross: Align::Center,
                        ..materials::surface_layout()
                    },
                ),
                |frame| {
                    if enabled {
                        frame.column_ex(&stretch(check), |frame| {
                            frame.label_sized("✓", 12.0);
                        });
                    }
                },
            );
            let text = Rect {
                x: check.x + CHECK + 8.0,
                y: row.y,
                w: (row.x + row.w - check.x - CHECK - 14.0).max(0.0),
                h: ROW_H,
            };
            let gated = group.gated;
            let gated_width = if gated {
                frame.measure_text(GATED_NOTE, 11.0).width + 6.0
            } else {
                0.0
            };
            let label = ellipsize(
                frame,
                &group.label,
                13.0,
                (text.w - gated_width - frame.theme().padding() * 2.0).max(0.0),
            );
            frame.place(
                &format!("aegis-capability-prompt-label-{index}"),
                &materials::chrome_place(text, transparent()),
                |frame| {
                    frame.row_ex(&stretch_gap(text), |frame| {
                        frame.label_sized(&label, 13.0);
                        if gated {
                            frame.label_compact_sized(GATED_NOTE, 11.0);
                        }
                    });
                },
            );
        }

        let deny_hovered = contains(layout.deny, cursor.x, cursor.y);
        frame.place(
            "aegis-capability-prompt-deny",
            &materials::chrome_place(
                layout.deny,
                LayoutOpts {
                    bg: if deny_hovered {
                        design.colors.application_hover
                    } else {
                        design.colors.card_surface
                    },
                    radius: design.radii.control,
                    pad: 0.0,
                    cross: Align::Center,
                    ..materials::surface_layout()
                },
            ),
            |frame| {
                frame.column_ex(&stretch(layout.deny), |frame| {
                    frame.label_sized("Deny", 13.0);
                });
            },
        );
        frame.place(
            "aegis-capability-prompt-allow",
            &materials::chrome_place(
                layout.allow,
                LayoutOpts {
                    bg: design.colors.application_accent,
                    radius: design.radii.control,
                    pad: 0.0,
                    cross: Align::Center,
                    ..materials::surface_layout()
                },
            ),
            |frame| {
                frame.column_ex(&stretch(layout.allow), |frame| {
                    frame.label_sized("Allow", 13.0);
                });
            },
        );

        frame.set_theme(original_theme);

        if pressed {
            self.press_at(cursor.x, cursor.y, display, out);
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

    // A pending consent owns the complete chrome band: the Dock, HUD, and
    // toasts stay suppressed until the prompt is answered.
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
        let layout = self.layout(display);
        Some(
            if contains(layout.allow, x, y)
                || contains(layout.deny, x, y)
                || layout.rows.iter().any(|row| contains(*row, x, y))
            {
                CursorShape::Pointer
            } else {
                CursorShape::Default
            },
        )
    }

    fn update(&mut self, update: ChromeUpdate<'_>) {
        match update {
            ChromeUpdate::ModalReserved(reserved) => self.modal_reserved = reserved,
            ChromeUpdate::Appearance(design) => self.design = *design,
            _ => {}
        }
    }

    fn key_char(&mut self, key: &KeyChar, out: &mut ChromeEvents) {
        if !self.active {
            return;
        }
        match key_action(key.keysym, key.ch) {
            KeyAction::Enter => self.allow(out),
            KeyAction::Escape => self.answer(None, out),
            _ => {}
        }
    }

    fn command(&mut self, command: &ChromeCommand<'_>, _out: &mut ChromeEvents) {
        match command {
            ChromeCommand::StartCapabilityPick(params) => {
                self.start_capability_pick((**params).clone());
            }
            ChromeCommand::CancelCapabilityPick if self.active => self.active = false,
            _ => {}
        }
    }

    fn capability_pick_active(&self) -> bool {
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
        let layout = self.layout(display);
        // One region exactly matching the glass body below: the runtime drops
        // it from the rectangular frost set, so the analytic pass alone owns
        // the rounded panel.
        vec![BackdropRegion::from(layout.panel)]
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
        let layout = self.layout(display);
        vec![LiquidGlassRegion::from_role(
            &self.design,
            GlassRole::ProminentPanel,
            BackdropRegion::from(layout.panel),
            self.design.radii.glass_panel,
            1.0,
        )]
    }
}

fn contains(rect: Rect, x: f32, y: f32) -> bool {
    x >= rect.x && y >= rect.y && x < rect.x + rect.w && y < rect.y + rect.h
}

fn stretch(rect: Rect) -> LayoutOpts {
    LayoutOpts {
        width: rect.w,
        height: rect.h,
        cross: Align::Center,
        ..Default::default()
    }
}

fn stretch_pad(rect: Rect) -> LayoutOpts {
    LayoutOpts {
        width: rect.w,
        height: rect.h,
        cross: Align::Center,
        pad: 6.0,
        ..Default::default()
    }
}

fn stretch_gap(rect: Rect) -> LayoutOpts {
    LayoutOpts {
        width: rect.w,
        height: rect.h,
        cross: Align::Center,
        gap: 6.0,
        ..Default::default()
    }
}

fn transparent() -> LayoutOpts {
    LayoutOpts {
        bg: Color::TRANSPARENT,
        pad: 0.0,
        ..materials::surface_layout()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> CapabilityPickParams {
        CapabilityPickParams {
            title: "Codex wants to borrow desktop capabilities".to_string(),
            warning: Some(
                "A different installation already registered under this name.".to_string(),
            ),
            groups: vec![
                CapabilityGroup {
                    key: "Focus".to_string(),
                    label: "Focus windows".to_string(),
                    gated: false,
                    enabled: true,
                },
                CapabilityGroup {
                    key: "CaptureInteractionDomain".to_string(),
                    label: "Capture its Interaction Domain".to_string(),
                    gated: true,
                    enabled: true,
                },
            ],
        }
    }

    fn row_center(layout: &PromptLayout, index: usize) -> (f32, f32) {
        let row = layout.rows[index];
        (row.x + row.w * 0.5, row.y + row.h * 0.5)
    }

    #[test]
    fn clicking_a_row_toggles_it() {
        let mut prompt = CapabilityPrompt::new();
        prompt.start_capability_pick(params());
        let display = (1280.0, 800.0);
        let (x, y) = row_center(&prompt.layout(display), 0);
        let mut out = ChromeEvents::default();
        prompt.press_at(x, y, display, &mut out);
        assert!(!prompt.groups[0].enabled);
        assert!(out.capability_pick_answered.is_none());
        prompt.press_at(x, y, display, &mut out);
        assert!(prompt.groups[0].enabled);
        assert!(prompt.capability_pick_active());
    }

    #[test]
    fn allow_returns_the_checked_keys() {
        let mut prompt = CapabilityPrompt::new();
        prompt.start_capability_pick(params());
        let display = (1280.0, 800.0);
        let (x, y) = row_center(&prompt.layout(display), 1);
        let mut out = ChromeEvents::default();
        prompt.press_at(x, y, display, &mut out);
        prompt.key_char(
            &KeyChar {
                keysym: aegis_model::input::XKB_KEY_Return,
                ch: None,
                mods: aegis_model::input::Mods::NONE,
            },
            &mut out,
        );
        assert_eq!(
            out.capability_pick_answered,
            Some(CapabilityPickResult {
                approved: Some(vec!["Focus".to_string()]),
            })
        );
        assert!(!prompt.capability_pick_active());
    }

    #[test]
    fn escape_denies() {
        let mut prompt = CapabilityPrompt::new();
        prompt.start_capability_pick(params());
        let mut out = ChromeEvents::default();
        prompt.key_char(
            &KeyChar {
                keysym: aegis_model::input::XKB_KEY_Escape,
                ch: None,
                mods: aegis_model::input::Mods::NONE,
            },
            &mut out,
        );
        assert_eq!(
            out.capability_pick_answered,
            Some(CapabilityPickResult { approved: None })
        );
        assert!(!prompt.capability_pick_active());
    }

    #[test]
    fn clicking_outside_denies() {
        let mut prompt = CapabilityPrompt::new();
        prompt.start_capability_pick(params());
        let mut out = ChromeEvents::default();
        prompt.press_at(4.0, 4.0, (1280.0, 800.0), &mut out);
        assert_eq!(
            out.capability_pick_answered,
            Some(CapabilityPickResult { approved: None })
        );
        assert!(!prompt.capability_pick_active());
    }

    #[test]
    fn deny_button_denies() {
        let mut prompt = CapabilityPrompt::new();
        prompt.start_capability_pick(params());
        let display = (1280.0, 800.0);
        let layout = prompt.layout(display);
        let mut out = ChromeEvents::default();
        prompt.press_at(layout.deny.x + 4.0, layout.deny.y + 4.0, display, &mut out);
        assert_eq!(
            out.capability_pick_answered,
            Some(CapabilityPickResult { approved: None })
        );
    }

    #[test]
    fn allow_button_allows_every_checked_group() {
        let mut prompt = CapabilityPrompt::new();
        prompt.start_capability_pick(params());
        let display = (1280.0, 800.0);
        let layout = prompt.layout(display);
        let mut out = ChromeEvents::default();
        prompt.press_at(
            layout.allow.x + 4.0,
            layout.allow.y + 4.0,
            display,
            &mut out,
        );
        assert_eq!(
            out.capability_pick_answered,
            Some(CapabilityPickResult {
                approved: Some(vec![
                    "Focus".to_string(),
                    "CaptureInteractionDomain".to_string()
                ]),
            })
        );
    }

    #[test]
    fn gated_groups_are_flagged_for_the_first_use_note() {
        let mut prompt = CapabilityPrompt::new();
        prompt.start_capability_pick(params());
        assert!(prompt.groups.iter().any(|group| group.gated));
        assert!(prompt.groups.iter().any(|group| !group.gated));
    }

    #[test]
    fn a_warning_lays_out_its_own_row() {
        let mut prompt = CapabilityPrompt::new();
        prompt.start_capability_pick(params());
        let layout = prompt.layout((1280.0, 800.0));
        assert!(layout.warning.is_some());

        let mut prompt = CapabilityPrompt::new();
        prompt.start_capability_pick(CapabilityPickParams {
            warning: None,
            ..params()
        });
        let layout = prompt.layout((1280.0, 800.0));
        assert!(layout.warning.is_none());
    }

    #[test]
    fn the_active_panel_is_one_analytic_glass_body() {
        let mut prompt = CapabilityPrompt::new();
        let display = (1280.0, 800.0);
        let workspaces = crate::WorkspaceSnapshot {
            outputs: Vec::new(),
        };
        assert!(
            prompt
                .liquid_glass_regions(display, &[], &workspaces)
                .is_empty()
        );
        assert!(!prompt.exclusive_presentation_active());

        prompt.start_capability_pick(params());
        let backdrop = prompt.backdrop_regions(display, &[], &workspaces);
        let glass = prompt.liquid_glass_regions(display, &[], &workspaces);
        assert_eq!(backdrop.len(), 1);
        assert_eq!(glass.len(), 1);
        assert_eq!(glass[0].bounds, backdrop[0]);
        assert_eq!(glass[0].corner_radius, Design::dark().radii.glass_panel);
        assert_eq!(glass[0].opacity, 1.0);
        assert!(prompt.exclusive_presentation_active());
    }
}
