//! The user-consent secret prompt: a modal, centered panel asking for a
//! password or PIN with a masked edit field (the secret vault's password
//! unlock, and any future credential prompt).
//!
//! The flow mirrors the other pickers: [`ChromeCommand::StartSecretPrompt`] opens
//! the panel, and the user's confirm or cancel travels back through
//! [`ChromeEvents::secret_prompt_confirmed`] /
//! [`ChromeEvents::secret_prompt_cancelled`]. Ordinary modal chrome over the
//! live scene: no freeze, no screen-content capture. The edit buffer is
//! zeroized when the panel closes; the confirmed value is handed to the
//! compositor's IPC answer path exactly once (further zeroization is the
//! caller's job — see the portal's vault KDF).

use lens::{Align, Frame, Input, LayoutOpts, Rect};

use crate::{
    BackdropRegion, Chrome, ChromeCommand, ChromeEvents, ChromeUpdate, CursorShape,
    LiquidGlassRegion, Localizer, Reserved, ellipsize,
};
use aegis_design::{Design, GlassRole, materials, themes};
use aegis_model::input::{KeyAction, KeyChar, key_action};
use aegis_model::window::Window;
use zeroize::Zeroize;

const PANEL_W: f32 = 440.0;
const PANEL_PAD: f32 = 16.0;
const TITLE_H: f32 = 24.0;
const REASON_H: f32 = 18.0;
const FIELD_H: f32 = 34.0;
const BUTTON_H: f32 = 30.0;
const BUTTON_W: f32 = 88.0;
const BACKDROP_BLUR_SIGMA: f32 = 18.0;
/// The mask glyph drawn per typed character.
const MASK: &str = "•";

/// Parameters of one user-consent secret prompt, mapped from the IPC
/// request by the compositor runtime.
#[derive(Debug, Clone)]
pub struct SecretPromptParams {
    /// Prompt heading (e.g. "Unlock Keyring").
    pub title: String,
    /// Optional context line under the title.
    pub reason: Option<String>,
}

/// The resolved geometry of the panel for one frame.
#[derive(Debug, Clone, Copy)]
struct PromptLayout {
    panel: Rect,
    title: Rect,
    reason: Option<Rect>,
    field: Rect,
    cancel: Rect,
    accept: Rect,
}

impl PromptLayout {
    fn for_display(display: (f32, f32), reserved: Reserved, has_reason: bool) -> PromptLayout {
        let left = reserved.left.max(0) as f32;
        let top = reserved.top.max(0) as f32;
        let usable_w = (display.0 - left - reserved.right.max(0) as f32).max(1.0);
        let usable_h = (display.1 - top - reserved.bottom.max(0) as f32).max(1.0);

        let panel_w = PANEL_W.min((usable_w - 32.0).max(240.0));
        let reason_block = if has_reason { REASON_H + 4.0 } else { 0.0 };
        let panel_h =
            PANEL_PAD + TITLE_H + reason_block + 10.0 + FIELD_H + 10.0 + BUTTON_H + PANEL_PAD;
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
        let reason = has_reason.then_some(Rect {
            x: inner_x,
            y: title.y + title.h + 4.0,
            w: inner_w,
            h: REASON_H,
        });
        let field_y = reason.map(|r| r.y + r.h).unwrap_or(title.y + title.h) + 10.0;
        let field = Rect {
            x: inner_x,
            y: field_y,
            w: inner_w,
            h: FIELD_H,
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
        PromptLayout {
            panel,
            title,
            reason,
            field,
            cancel,
            accept,
        }
    }
}

/// The secret-prompt chrome component. Inert until the runtime opens it
/// with [`ChromeCommand::StartSecretPrompt`].
pub struct SecretPrompt {
    active: bool,
    title: String,
    reason: Option<String>,
    /// The edit buffer; zeroized on close.
    buffer: String,
    modal_reserved: Reserved,
    /// The design snapshot the prompt paints from, from
    /// [`ChromeUpdate::Appearance`]. Seeded on registration by
    /// [`crate::Shell::add`] and refreshed when the desktop color scheme
    /// changes; defaults to the dark appearance until the first update arrives.
    design: Design,
}

impl SecretPrompt {
    pub fn new() -> SecretPrompt {
        SecretPrompt {
            active: false,
            title: String::new(),
            reason: None,
            buffer: String::new(),
            modal_reserved: Reserved::default(),
            design: Design::dark(),
        }
    }

    /// Confirm the typed secret and close.
    fn confirm(&mut self, out: &mut ChromeEvents) {
        out.secret_prompt_confirmed = Some(std::mem::take(&mut self.buffer));
        self.close();
    }

    /// Emit a cancellation and close.
    fn cancel(&mut self, out: &mut ChromeEvents) {
        out.secret_prompt_cancelled = true;
        self.close();
    }

    fn close(&mut self) {
        self.active = false;
        self.buffer.zeroize();
    }

    fn start_secret_prompt(&mut self, params: SecretPromptParams) {
        self.title = params.title;
        self.reason = params.reason;
        self.buffer.zeroize();
        self.buffer.clear();
        self.active = true;
    }
}

impl Default for SecretPrompt {
    fn default() -> Self {
        SecretPrompt::new()
    }
}

impl Chrome for SecretPrompt {
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
        let layout = PromptLayout::for_display(display, self.modal_reserved, self.reason.is_some());

        frame.place(
            "aegis-secret-prompt-scrim",
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
            "aegis-secret-prompt-panel",
            &materials::chrome_place(layout.panel, materials::glass_panel(&design)),
            |_| {},
        );

        let title = ellipsize(
            frame,
            &self.title,
            design.typography.headline,
            (layout.title.w - frame.theme().padding() * 2.0).max(0.0),
        );
        frame.place(
            "aegis-secret-prompt-title",
            &materials::chrome_place(layout.title, materials::transparent()),
            |frame| {
                frame.row_ex(&stretch(layout.title), |frame| {
                    frame.label_sized(&title, design.typography.headline);
                });
            },
        );

        if let Some(reason_rect) = layout.reason {
            let reason = ellipsize(
                frame,
                self.reason.as_deref().unwrap_or_default(),
                design.typography.label,
                reason_rect.w,
            );
            frame.place(
                "aegis-secret-prompt-reason",
                &materials::chrome_place(reason_rect, materials::transparent()),
                |frame| {
                    frame.row_ex(&stretch(reason_rect), |frame| {
                        frame.label_compact_sized(&reason, design.typography.label);
                    });
                },
            );
        }

        // The masked field: one glyph per character, caret after the last
        // (compositor-owned, like the launcher's search field).
        let masked = MASK.repeat(self.buffer.chars().count());
        let metrics = frame.measure_text(&masked, design.typography.body);
        let font_metrics = frame.measure_text("Ag", design.typography.body);
        frame.place(
            "aegis-secret-prompt-field",
            &materials::chrome_place(
                layout.field,
                LayoutOpts {
                    bg: design.colors.card_surface,
                    border: design.colors.application_border,
                    border_width: design.strokes.hairline,
                    radius: design.radii.control,
                    pad: 0.0,
                    ..materials::surface_layout()
                },
            ),
            |frame| {
                frame.row_ex(
                    &LayoutOpts {
                        width: layout.field.w,
                        height: layout.field.h,
                        cross: Align::Center,
                        ..Default::default()
                    },
                    |frame| {
                        frame.spacer(12.0);
                        frame.label_compact_sized(&masked, design.typography.body);
                    },
                );
            },
        );
        frame.place(
            "aegis-secret-prompt-caret",
            &materials::chrome_place(
                Rect {
                    x: layout.field.x + 12.0 + metrics.width,
                    y: layout.field.y + (layout.field.h - font_metrics.height) * 0.5,
                    w: 2.0,
                    h: font_metrics.height,
                },
                LayoutOpts {
                    bg: design.colors.application_text,
                    pad: 0.0,
                    ..materials::surface_layout()
                },
            ),
            |_| {},
        );

        let cancel_hovered = contains(layout.cancel, cursor.x, cursor.y);
        let accept_hovered = contains(layout.accept, cursor.x, cursor.y);
        let clicked_cancel = pressed && cancel_hovered;
        let clicked_accept = pressed && accept_hovered;
        frame.place(
            "aegis-secret-prompt-cancel",
            &materials::chrome_place(
                layout.cancel,
                LayoutOpts {
                    bg: if cancel_hovered {
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
                frame.column_ex(&stretch(layout.cancel), |frame| {
                    frame.label_sized("Cancel", design.typography.body);
                });
            },
        );
        frame.place(
            "aegis-secret-prompt-accept",
            &materials::chrome_place(
                layout.accept,
                LayoutOpts {
                    bg: design.colors.application_accent,
                    radius: design.radii.control,
                    pad: 0.0,
                    cross: Align::Center,
                    ..materials::surface_layout()
                },
            ),
            |frame| {
                frame.column_ex(&stretch(layout.accept), |frame| {
                    frame.label_sized("Unlock", design.typography.body);
                });
            },
        );

        frame.set_theme(original_theme);

        // Clicks outside the panel are ignored: a secret handover must be a
        // deliberate choice, never an accidental click.
        if clicked_cancel {
            self.cancel(out);
            return;
        }
        if clicked_accept {
            self.confirm(out);
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
        let layout = PromptLayout::for_display(display, self.modal_reserved, self.reason.is_some());
        Some(if contains(layout.field, x, y) {
            CursorShape::Text
        } else if contains(layout.accept, x, y) || contains(layout.cancel, x, y) {
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
            KeyAction::Enter => self.confirm(out),
            KeyAction::Escape => self.cancel(out),
            KeyAction::Backspace => {
                self.buffer.pop();
            }
            KeyAction::Char(c) if !c.is_control() => self.buffer.push(c),
            _ => {}
        }
    }

    fn command(&mut self, command: &ChromeCommand<'_>, out: &mut ChromeEvents) {
        match command {
            ChromeCommand::StartSecretPrompt(params) => {
                self.start_secret_prompt((**params).clone());
            }
            ChromeCommand::CancelSecretPrompt if self.active => self.close(),
            ChromeCommand::DismissModal if self.active => self.cancel(out),
            _ => {}
        }
    }

    fn secret_prompt_active(&self) -> bool {
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
        let layout = PromptLayout::for_display(display, self.modal_reserved, self.reason.is_some());
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
        let layout = PromptLayout::for_display(display, self.modal_reserved, self.reason.is_some());
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

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> SecretPromptParams {
        SecretPromptParams {
            title: "Unlock Keyring".to_string(),
            reason: Some("The application 'mail' wants access".to_string()),
        }
    }

    fn char_key(c: char) -> KeyChar {
        KeyChar {
            keysym: c as u32,
            ch: Some(c),
            mods: aegis_model::input::Mods::NONE,
        }
    }

    #[test]
    fn typed_characters_fill_the_masked_buffer() {
        let mut prompt = SecretPrompt::new();
        prompt.start_secret_prompt(params());
        let mut out = ChromeEvents::default();
        for c in ['s', '3', 'c'] {
            prompt.key_char(&char_key(c), &mut out);
        }
        assert_eq!(prompt.buffer, "s3c");
        prompt.key_char(
            &KeyChar {
                keysym: aegis_model::input::XKB_KEY_BackSpace,
                ch: None,
                mods: aegis_model::input::Mods::NONE,
            },
            &mut out,
        );
        assert_eq!(prompt.buffer, "s3");
    }

    #[test]
    fn enter_confirms_and_zeroizes_the_buffer() {
        let mut prompt = SecretPrompt::new();
        prompt.start_secret_prompt(params());
        let mut out = ChromeEvents::default();
        for c in ['p', 'w'] {
            prompt.key_char(&char_key(c), &mut out);
        }
        prompt.key_char(
            &KeyChar {
                keysym: aegis_model::input::XKB_KEY_Return,
                ch: None,
                mods: aegis_model::input::Mods::NONE,
            },
            &mut out,
        );
        assert_eq!(out.secret_prompt_confirmed.as_deref(), Some("pw"));
        assert!(!prompt.secret_prompt_active());
        assert!(prompt.buffer.is_empty());
    }

    #[test]
    fn escape_cancels_and_zeroizes_the_buffer() {
        let mut prompt = SecretPrompt::new();
        prompt.start_secret_prompt(params());
        let mut out = ChromeEvents::default();
        prompt.key_char(&char_key('x'), &mut out);
        prompt.key_char(
            &KeyChar {
                keysym: aegis_model::input::XKB_KEY_Escape,
                ch: None,
                mods: aegis_model::input::Mods::NONE,
            },
            &mut out,
        );
        assert!(out.secret_prompt_cancelled);
        assert!(!prompt.secret_prompt_active());
        assert!(prompt.buffer.is_empty());
    }

    #[test]
    fn the_panic_chord_command_cancels_and_zeroizes_the_buffer() {
        let mut prompt = SecretPrompt::new();
        prompt.start_secret_prompt(params());
        let mut out = ChromeEvents::default();
        prompt.key_char(&char_key('x'), &mut out);
        prompt.command(&ChromeCommand::DismissModal, &mut out);
        assert!(out.secret_prompt_cancelled);
        assert!(!prompt.secret_prompt_active());
        assert!(prompt.buffer.is_empty());
    }

    #[test]
    fn the_active_panel_is_one_analytic_glass_body() {
        let mut prompt = SecretPrompt::new();
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

        prompt.start_secret_prompt(params());
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
