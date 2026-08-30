//! The user-consent confirmation dialog: a modal, centered panel asking
//! one question. The yes/no style offers Cancel and an affirmative button
//! (portal consent flows: Account, Access, DynamicLauncher); the grant
//! style offers the four runtime-grant persistences (ADR-0088): Deny,
//! Allow once, This session, Always.
//!
//! The flow mirrors the other pickers: [`ChromeCommand::StartConfirmPick`] opens
//! the panel, and the user's answer travels back through
//! [`ChromeEvents::confirm_pick_answered`]. Ordinary modal chrome over the
//! live scene: no freeze, no screen-content capture.

use lens::{Frame, Input, Rect};

use crate::{
    BackdropRegion, Chrome, ChromeCommand, ChromeEvents, ChromeUpdate, CursorShape,
    LiquidGlassRegion, Localizer, Reserved, ellipsize, modal_scrim_backdrop,
};
use aegis_design::{Design, GlassRole, materials, themes};
use aegis_model::input::{KeyAction, KeyChar, key_action};
use aegis_model::window::Window;
use aegis_ui::{
    ActionButtonStyle, DEFAULT_BACKDROP_BLUR_SIGMA, DEFAULT_BUTTON_HEIGHT, DEFAULT_BUTTON_WIDTH,
    DEFAULT_MODAL_PAD, DEFAULT_MODAL_WIDTH, DEFAULT_TITLE_HEIGHT, contains, place_modal_panel,
    render_action_button, render_grant_action_buttons, stretch, stretch_top,
};

const PANEL_W: f32 = DEFAULT_MODAL_WIDTH;
const PANEL_PAD: f32 = DEFAULT_MODAL_PAD;
const TITLE_H: f32 = DEFAULT_TITLE_HEIGHT;
const BODY_LINE_H: f32 = 18.0;
const BUTTON_H: f32 = DEFAULT_BUTTON_HEIGHT;
const BUTTON_W: f32 = DEFAULT_BUTTON_WIDTH;
const BACKDROP_BLUR_SIGMA: f32 = DEFAULT_BACKDROP_BLUR_SIGMA;
const GRANT_ANSWERS: [ConfirmAnswer; 4] = [
    ConfirmAnswer::Cancelled,
    ConfirmAnswer::AllowOnce,
    ConfirmAnswer::AllowSession,
    ConfirmAnswer::AllowAlways,
];

/// The dialog style: a plain yes/no consent, or the four-option
/// runtime-grant consent (ADR-0088).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConfirmPickStyle {
    /// Cancel plus one affirmative button (portal consent flows).
    #[default]
    YesNo,
    /// Deny / Allow once / This session / Always (agent runtime grants).
    Grant,
}

/// The answer the user gave at the confirmation dialog. The yes/no style
/// only ever yields `Confirmed`/`Cancelled`; the grant style yields
/// `Cancelled` or one of the three persistence levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmAnswer {
    /// Yes/no style: the affirmative button (or Enter).
    Confirmed,
    /// Either style: Deny/Cancel, `Escape`, or the compositor panic chord.
    Cancelled,
    /// Grant style: allow this one operation only; nothing is recorded.
    AllowOnce,
    /// Grant style: allow until the compositor exits.
    AllowSession,
    /// Grant style: allow and remember durably.
    AllowAlways,
}

/// Parameters of one user-consent confirmation, mapped from the IPC
/// request by the compositor runtime.
#[derive(Debug, Clone)]
pub struct ConfirmPickParams {
    /// Dialog heading (e.g. "Share personal information?").
    pub title: String,
    /// Explanation of what is requested and by whom.
    pub body: String,
    /// Affirmative button label override ("Allow", "Share", …); the
    /// default is "OK". Only used by the yes/no style.
    pub accept_label: Option<String>,
    /// Dialog style; the default is the plain yes/no consent.
    pub style: ConfirmPickStyle,
}

/// The resolved geometry of the panel for one frame.
#[derive(Debug, Clone, Copy)]
struct PromptLayout {
    panel: Rect,
    title: Rect,
    body: Rect,
    cancel: Rect,
    accept: Rect,
    grant: [Rect; 4],
}

impl PromptLayout {
    fn for_display(display: (f32, f32), reserved: Reserved, body_lines: usize) -> PromptLayout {
        let left = reserved.left.max(0) as f32;
        let top = reserved.top.max(0) as f32;
        let usable_w = (display.0 - left - reserved.right.max(0) as f32).max(1.0);
        let usable_h = (display.1 - top - reserved.bottom.max(0) as f32).max(1.0);

        let panel_w = PANEL_W.min((usable_w - 32.0).max(240.0));
        let body_h = body_lines.max(1) as f32 * BODY_LINE_H;
        let panel_h = PANEL_PAD + TITLE_H + 4.0 + body_h + 10.0 + BUTTON_H + PANEL_PAD;
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
        let body = Rect {
            x: inner_x,
            y: title.y + title.h + 4.0,
            w: inner_w,
            h: body_h,
        };
        let buttons_y = panel.y + panel.h - PANEL_PAD - BUTTON_H;
        let accept = Rect {
            x: panel.x + panel.w - PANEL_PAD - BUTTON_W,
            y: buttons_y,
            w: BUTTON_W,
            h: BUTTON_H,
        };
        let cancel = Rect {
            x: accept.x - BUTTON_W - 8.0,
            y: buttons_y,
            w: BUTTON_W,
            h: BUTTON_H,
        };
        let grant_w = ((inner_w - 3.0 * 8.0) / 4.0).floor();
        let grant = std::array::from_fn(|index| Rect {
            x: inner_x + index as f32 * (grant_w + 8.0),
            y: buttons_y,
            w: grant_w,
            h: BUTTON_H,
        });
        PromptLayout {
            panel,
            title,
            body,
            cancel,
            accept,
            grant,
        }
    }
}

/// The confirmation chrome component. Inert until the runtime opens it
/// with [`ChromeCommand::StartConfirmPick`].
pub struct ConfirmPrompt {
    active: bool,
    title: String,
    body: String,
    accept_label: String,
    style: ConfirmPickStyle,
    modal_reserved: Reserved,
    /// The design snapshot the prompt paints from, from
    /// [`ChromeUpdate::Appearance`]. Seeded on registration by
    /// [`crate::Shell::add`] and refreshed when the desktop color scheme
    /// changes; defaults to the dark appearance until the first update arrives.
    design: Design,
}

impl ConfirmPrompt {
    pub fn new() -> ConfirmPrompt {
        ConfirmPrompt {
            active: false,
            title: String::new(),
            body: String::new(),
            accept_label: "OK".to_string(),
            style: ConfirmPickStyle::YesNo,
            modal_reserved: Reserved::default(),
            design: Design::dark(),
        }
    }

    /// Answer the dialog and close.
    fn answer(&mut self, answer: ConfirmAnswer, out: &mut ChromeEvents) {
        out.confirm_pick_answered = Some(answer);
        self.active = false;
    }

    fn start_confirm_pick(&mut self, params: ConfirmPickParams) {
        self.title = params.title;
        self.body = params.body;
        self.accept_label = params.accept_label.unwrap_or_else(|| "OK".to_string());
        self.style = params.style;
        self.active = true;
    }

    /// The body renders one label per line: lens labels do not break on
    /// `\n`, so the panel height derives from the line count here.
    fn body_lines(&self) -> Vec<&str> {
        self.body.split('\n').collect()
    }

    fn layout(&self, display: (f32, f32)) -> PromptLayout {
        PromptLayout::for_display(display, self.modal_reserved, self.body_lines().len())
    }

    /// Handle one primary-button press at output-space `(x, y)`: answers on
    /// the style's buttons. Clicks outside the panel are ignored: consent
    /// must be a deliberate choice, never an accidental click.
    fn press_at(&mut self, x: f32, y: f32, display: (f32, f32), out: &mut ChromeEvents) {
        let layout = self.layout(display);
        if !contains(layout.panel, x, y) {
            return;
        }
        match self.style {
            ConfirmPickStyle::YesNo => {
                if contains(layout.cancel, x, y) {
                    self.answer(ConfirmAnswer::Cancelled, out);
                } else if contains(layout.accept, x, y) {
                    self.answer(ConfirmAnswer::Confirmed, out);
                }
            }
            ConfirmPickStyle::Grant => {
                for (rect, answer) in layout.grant.iter().zip(GRANT_ANSWERS) {
                    if contains(*rect, x, y) {
                        self.answer(answer, out);
                        return;
                    }
                }
            }
        }
    }
}

impl Default for ConfirmPrompt {
    fn default() -> Self {
        ConfirmPrompt::new()
    }
}

impl Chrome for ConfirmPrompt {
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

        let original_theme = frame.theme();
        frame.set_theme(themes::application(&design));

        // Minimal foreground tint only. The compositor-owned analytic pass
        // supplies the body, refraction, rim light, and shadow.
        place_modal_panel(frame, "aegis-confirm-prompt-panel", layout.panel, &design);

        let title = ellipsize(
            frame,
            &self.title,
            design.typography.headline,
            layout.title.w,
        );
        frame.place(
            "aegis-confirm-prompt-title",
            &materials::chrome_place(layout.title, materials::transparent()),
            |frame| {
                frame.row_ex(&stretch(layout.title), |frame| {
                    frame.label_compact_sized(&title, design.typography.headline);
                });
            },
        );

        // One compact label per body line; empty lines stay as blank
        // spacing. Every line's left edge aligns with the title's.
        let lines = self.body_lines();
        for (index, line) in lines.iter().enumerate() {
            if line.is_empty() {
                continue;
            }
            let line_rect = Rect {
                x: layout.body.x,
                y: layout.body.y + index as f32 * BODY_LINE_H,
                w: layout.body.w,
                h: BODY_LINE_H,
            };
            let line = ellipsize(frame, line, design.typography.label, line_rect.w);
            frame.place(
                &format!("aegis-confirm-prompt-body-{index}"),
                &materials::chrome_place(line_rect, materials::transparent()),
                |frame| {
                    frame.row_ex(&stretch_top(line_rect), |frame| {
                        frame.label_compact_sized(&line, design.typography.label);
                    });
                },
            );
        }

        match self.style {
            ConfirmPickStyle::YesNo => {
                let cancel_hovered = contains(layout.cancel, cursor.x, cursor.y);
                let accept_hovered = contains(layout.accept, cursor.x, cursor.y);
                render_action_button(
                    frame,
                    "aegis-confirm-prompt-cancel",
                    layout.cancel,
                    "Cancel",
                    ActionButtonStyle::Subtle,
                    cancel_hovered,
                    &design,
                );
                render_action_button(
                    frame,
                    "aegis-confirm-prompt-accept",
                    layout.accept,
                    &self.accept_label,
                    ActionButtonStyle::Accented,
                    accept_hovered,
                    &design,
                );
            }
            ConfirmPickStyle::Grant => {
                render_grant_action_buttons(
                    frame,
                    "aegis-confirm-prompt",
                    &layout.grant,
                    (cursor.x, cursor.y),
                    &design,
                );
            }
        }

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
        let over_button = match self.style {
            ConfirmPickStyle::YesNo => {
                contains(layout.accept, x, y) || contains(layout.cancel, x, y)
            }
            ConfirmPickStyle::Grant => layout.grant.iter().any(|rect| contains(*rect, x, y)),
        };
        Some(if over_button {
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

    fn key_char(&mut self, key: &KeyChar, out: &mut ChromeEvents) {
        if !self.active {
            return;
        }
        match key_action(key.keysym, key.ch) {
            KeyAction::Enter => match self.style {
                ConfirmPickStyle::YesNo => self.answer(ConfirmAnswer::Confirmed, out),
                // The least persistent affirmative is the keyboard default.
                ConfirmPickStyle::Grant => self.answer(ConfirmAnswer::AllowOnce, out),
            },
            KeyAction::Escape => self.answer(ConfirmAnswer::Cancelled, out),
            _ => {}
        }
    }

    fn command(&mut self, command: &ChromeCommand<'_>, out: &mut ChromeEvents) {
        match command {
            ChromeCommand::StartConfirmPick(params) => self.start_confirm_pick((**params).clone()),
            ChromeCommand::CancelConfirmPick if self.active => self.active = false,
            ChromeCommand::DismissModal if self.active => {
                self.answer(ConfirmAnswer::Cancelled, out);
            }
            _ => {}
        }
    }

    fn confirm_pick_active(&self) -> bool {
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
        let _ = self.layout(display);
        // The modal's full-display dim is a wash INTO the frost (beneath the
        // panel's glass body) — it used to be painted above the glass, which
        // hid the lens's refraction and split the layer stack.
        vec![modal_scrim_backdrop(display, &self.design)]
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

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> ConfirmPickParams {
        ConfirmPickParams {
            title: "Share personal information?".to_string(),
            body: "The application 'mail' wants your name and avatar.".to_string(),
            accept_label: Some("Share".to_string()),
            style: ConfirmPickStyle::YesNo,
        }
    }

    fn grant_params() -> ConfirmPickParams {
        ConfirmPickParams {
            title: "Allow Codex to borrow a sensitive capability?".to_string(),
            body: "Close windows".to_string(),
            accept_label: None,
            style: ConfirmPickStyle::Grant,
        }
    }

    fn enter() -> KeyChar {
        KeyChar {
            keysym: aegis_model::input::XKB_KEY_Return,
            ch: None,
            mods: aegis_model::input::Mods::NONE,
        }
    }

    fn escape() -> KeyChar {
        KeyChar {
            keysym: aegis_model::input::XKB_KEY_Escape,
            ch: None,
            mods: aegis_model::input::Mods::NONE,
        }
    }

    #[test]
    fn enter_confirms() {
        let mut prompt = ConfirmPrompt::new();
        prompt.start_confirm_pick(params());
        assert_eq!(prompt.accept_label, "Share");
        let mut out = ChromeEvents::default();
        prompt.key_char(&enter(), &mut out);
        assert_eq!(out.confirm_pick_answered, Some(ConfirmAnswer::Confirmed));
        assert!(!prompt.confirm_pick_active());
    }

    #[test]
    fn escape_declines() {
        let mut prompt = ConfirmPrompt::new();
        prompt.start_confirm_pick(params());
        let mut out = ChromeEvents::default();
        prompt.key_char(&escape(), &mut out);
        assert_eq!(out.confirm_pick_answered, Some(ConfirmAnswer::Cancelled));
        assert!(!prompt.confirm_pick_active());
    }

    #[test]
    fn default_accept_label_is_ok() {
        let mut prompt = ConfirmPrompt::new();
        prompt.start_confirm_pick(ConfirmPickParams {
            title: "t".to_string(),
            body: "b".to_string(),
            accept_label: None,
            style: ConfirmPickStyle::YesNo,
        });
        assert_eq!(prompt.accept_label, "OK");
    }

    #[test]
    fn grant_enter_allows_once() {
        let mut prompt = ConfirmPrompt::new();
        prompt.start_confirm_pick(grant_params());
        let mut out = ChromeEvents::default();
        prompt.key_char(&enter(), &mut out);
        assert_eq!(out.confirm_pick_answered, Some(ConfirmAnswer::AllowOnce));
        assert!(!prompt.confirm_pick_active());
    }

    #[test]
    fn grant_buttons_cover_all_four_persistences() {
        let display = (1280.0, 800.0);
        for (index, expected) in GRANT_ANSWERS.iter().enumerate() {
            let mut prompt = ConfirmPrompt::new();
            prompt.start_confirm_pick(grant_params());
            let rect = prompt.layout(display).grant[index];
            let mut out = ChromeEvents::default();
            prompt.press_at(rect.x + 4.0, rect.y + 4.0, display, &mut out);
            assert_eq!(out.confirm_pick_answered, Some(*expected), "button {index}");
            assert!(!prompt.confirm_pick_active());
        }
    }

    #[test]
    fn each_body_line_gets_its_own_row() {
        let mut prompt = ConfirmPrompt::new();
        prompt.start_confirm_pick(grant_params());
        let one_line = prompt.layout((1280.0, 800.0));
        assert_eq!(one_line.body.h, BODY_LINE_H);

        let mut prompt = ConfirmPrompt::new();
        prompt.start_confirm_pick(ConfirmPickParams {
            body: "Close windows\n\nAllow once: just this time.\nAlways: remembered.".to_string(),
            ..grant_params()
        });
        let multi_line = prompt.layout((1280.0, 800.0));
        assert_eq!(multi_line.body.h, 4.0 * BODY_LINE_H);
        assert_eq!(multi_line.panel.h - one_line.panel.h, 3.0 * BODY_LINE_H);
    }

    #[test]
    fn grant_escape_denies() {
        let mut prompt = ConfirmPrompt::new();
        prompt.start_confirm_pick(grant_params());
        let mut out = ChromeEvents::default();
        prompt.key_char(&escape(), &mut out);
        assert_eq!(out.confirm_pick_answered, Some(ConfirmAnswer::Cancelled));
    }

    #[test]
    fn grant_click_outside_keeps_the_prompt_open() {
        let mut prompt = ConfirmPrompt::new();
        prompt.start_confirm_pick(grant_params());
        let mut out = ChromeEvents::default();
        prompt.press_at(4.0, 4.0, (1280.0, 800.0), &mut out);
        assert_eq!(out.confirm_pick_answered, None);
        assert!(prompt.confirm_pick_active());
    }

    #[test]
    fn the_panic_chord_command_cancels() {
        let mut prompt = ConfirmPrompt::new();
        prompt.start_confirm_pick(grant_params());
        let mut out = ChromeEvents::default();
        prompt.command(&ChromeCommand::DismissModal, &mut out);
        assert_eq!(out.confirm_pick_answered, Some(ConfirmAnswer::Cancelled));
        assert!(!prompt.confirm_pick_active());
    }

    #[test]
    fn the_active_panel_is_one_analytic_glass_body() {
        let mut prompt = ConfirmPrompt::new();
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

        prompt.start_confirm_pick(params());
        let backdrop = prompt.backdrop_regions(display, &[], &workspaces);
        let glass = prompt.liquid_glass_regions(display, &[], &workspaces);
        assert_eq!(backdrop.len(), 1);
        assert!(
            backdrop[0].wash.is_some(),
            "the dim is a wash into the frost"
        );
        assert_eq!(glass.len(), 1);
        assert_eq!(glass[0].bounds.w, 460.0);
        assert_eq!(glass[0].corner_radius, Design::dark().radii.glass_panel);
        assert_eq!(glass[0].opacity, 1.0);
        assert!(prompt.exclusive_presentation_active());
    }
}
