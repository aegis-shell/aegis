//! Compositor-owned Agent Workspace lifecycle and authority presentation.

use aegis_core::app::BuiltInApplication;
use aegis_core::input::{KeyAction, KeyChar, key_action};
use aegis_core::realm::{RealmId, RealmKind, RealmSnapshot, RealmState};
use aegis_core::window::Window;
use aegis_core::workspace::WorkspaceSnapshot;
use aegis_design::{Design, materials};
use aegis_shell::{
    BackdropRegion, Chrome, ChromeEvents, CursorShape, Localizer, Message, ModalApplicationSpec,
    RealmIntent, Reserved,
};
use lens::{Align, Frame, Icon, Input, LayoutOpts};

const SURFACE: ModalApplicationSpec = ModalApplicationSpec {
    scrim_id: "aegis-ai-workspaces-scrim",
    panel_id: "aegis-ai-workspaces-app",
    scroll_id: "aegis-ai-workspaces-page",
    title: Message::AiWorkspaces,
    icon: Icon::Grid,
    max_width: 760.0,
    max_height: 590.0,
};

/// Trusted modal application for Agent Realm lifecycle management.
pub struct AiWorkspaces {
    open: bool,
    modal_reserved: Reserved,
    snapshot: RealmSnapshot,
    pending_revoke: Option<RealmId>,
}

impl AiWorkspaces {
    pub fn new() -> Self {
        Self {
            open: false,
            modal_reserved: Reserved::default(),
            snapshot: aegis_core::realm::RealmModel::new().snapshot(),
            pending_revoke: None,
        }
    }

    fn update_snapshot(&mut self, snapshot: &RealmSnapshot) {
        self.snapshot = snapshot.clone();
        if self.pending_revoke.is_some_and(|id| {
            !self
                .snapshot
                .realms
                .iter()
                .any(|realm| realm.id == id && realm.state != RealmState::Revoked)
        }) {
            self.pending_revoke = None;
        }
    }

    fn render_content(&mut self, frame: &mut Frame, i18n: &Localizer, out: &mut ChromeEvents) {
        frame.label_wrapped_sized(i18n.text(Message::AiWorkspacesDescription), 12.0, 640.0);
        frame.size_next(0.0, 32.0);
        if frame.button(i18n.text(Message::CreateEmptyAgentWorkspace)) {
            let ordinal = self
                .snapshot
                .realms
                .iter()
                .filter(|realm| realm.kind == RealmKind::Agent)
                .count()
                + 1;
            out.realm_intents.push(RealmIntent::Create {
                label: i18n.default_agent_workspace_label(ordinal),
            });
        }

        let realms = self
            .snapshot
            .realms
            .iter()
            .filter(|realm| realm.kind == RealmKind::Agent)
            .cloned()
            .collect::<Vec<_>>();
        for realm in realms {
            let controlled_windows = self
                .snapshot
                .interaction_groups
                .iter()
                .filter(|group| group.control_realm == realm.id)
                .map(|group| group.windows.len())
                .sum::<usize>();
            let seat = self
                .snapshot
                .seats
                .iter()
                .find(|seat| seat.realm == realm.id);
            frame.column_ex(&workspace_card_layout(), |frame| {
                frame.row_ex(&section_heading_layout(), |frame| {
                    frame.heading(&realm.label, 3);
                    frame.flex(1.0);
                    frame.spacer(0.0);
                    let state = match realm.state {
                        RealmState::Active => i18n.text(Message::RealmActive),
                        RealmState::Paused => i18n.text(Message::RealmPaused),
                        RealmState::Revoked => i18n.text(Message::RealmRevoked),
                    };
                    frame.label_sized(state, 11.0);
                });
                frame.label_sized(
                    &format!(
                        "Realm {} · {}: {controlled_windows}",
                        realm.id.0,
                        i18n.text(Message::ControlledWindows)
                    ),
                    11.0,
                );
                if let Some(seat) = seat {
                    let mut capabilities = Vec::new();
                    if seat.capabilities.pointer {
                        capabilities.push(i18n.text(Message::AgentPointerCapability));
                    }
                    if seat.capabilities.keyboard {
                        capabilities.push(i18n.text(Message::AgentKeyboardCapability));
                    }
                    if seat.capabilities.touch {
                        capabilities.push(i18n.text(Message::AgentTouchCapability));
                    }
                    if !capabilities.is_empty() {
                        frame.label_wrapped_sized(
                            &format!(
                                "{} · {}",
                                i18n.text(Message::SeatCapabilities),
                                capabilities.join(" · ")
                            ),
                            11.0,
                            600.0,
                        );
                    }
                }
                if realm.state != RealmState::Revoked {
                    frame.row_ex(
                        &LayoutOpts {
                            height: 30.0,
                            gap: 8.0,
                            cross: Align::Center,
                            ..Default::default()
                        },
                        |frame| {
                            frame.flex(1.0);
                            frame.size_next(116.0, 28.0);
                            let next = if realm.state == RealmState::Active {
                                RealmState::Paused
                            } else {
                                RealmState::Active
                            };
                            let label = if next == RealmState::Paused {
                                i18n.text(Message::PauseRealm)
                            } else {
                                i18n.text(Message::ResumeRealm)
                            };
                            if frame.button(label) {
                                out.realm_intents.push(RealmIntent::SetState {
                                    realm: realm.id,
                                    state: next,
                                    expected_revision: self.snapshot.revision,
                                });
                            }
                            frame.size_next(132.0, 28.0);
                            let confirming = self.pending_revoke == Some(realm.id);
                            let label = if confirming {
                                i18n.text(Message::ConfirmRevokeRealm)
                            } else {
                                i18n.text(Message::RevokeRealm)
                            };
                            if frame.button(label) {
                                if confirming {
                                    out.realm_intents.push(RealmIntent::Revoke {
                                        realm: realm.id,
                                        expected_revision: self.snapshot.revision,
                                    });
                                    self.pending_revoke = None;
                                } else {
                                    self.pending_revoke = Some(realm.id);
                                }
                            }
                        },
                    );
                }
            });
        }
    }
}

impl Default for AiWorkspaces {
    fn default() -> Self {
        Self::new()
    }
}

impl Chrome for AiWorkspaces {
    fn render(
        &mut self,
        frame: &mut Frame,
        input: &Input,
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
        i18n: &Localizer,
        out: &mut ChromeEvents,
    ) {
        if !self.open {
            return;
        }
        let reserved = self.modal_reserved;
        if SURFACE.render(frame, input, reserved, i18n, |frame| {
            self.render_content(frame, i18n, out);
        }) {
            self.open = false;
        }
    }

    fn captures_keyboard(&self) -> bool {
        self.open
    }

    fn key_char(&mut self, key: &KeyChar, _out: &mut ChromeEvents) {
        if self.open && matches!(key_action(key.keysym, key.ch), KeyAction::Escape) {
            self.open = false;
        }
    }

    fn open_builtin(&mut self, app: BuiltInApplication) {
        self.open = app == BuiltInApplication::AiWorkspaces;
    }

    fn update_realms(&mut self, snapshot: &RealmSnapshot) {
        self.update_snapshot(snapshot);
    }

    fn captures_pointer(
        &self,
        _x: f32,
        _y: f32,
        _display: (f32, f32),
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) -> bool {
        self.open
    }

    fn cursor_shape_at(
        &self,
        _x: f32,
        _y: f32,
        _display: (f32, f32),
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) -> Option<CursorShape> {
        Some(CursorShape::Pointer)
    }

    fn modal_active(&self) -> bool {
        self.open
    }

    fn requires_composition(&self) -> bool {
        self.open
    }

    fn visible_during_modal(&self) -> bool {
        true
    }

    fn set_modal_reserved(&mut self, reserved: Reserved) {
        self.modal_reserved = reserved;
    }

    fn backdrop_blur_sigma(&self) -> f32 {
        if self.open {
            SURFACE.backdrop_blur_sigma()
        } else {
            0.0
        }
    }

    fn backdrop_regions(
        &self,
        display: (f32, f32),
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) -> Vec<BackdropRegion> {
        if self.open {
            SURFACE.backdrop_regions(display, self.modal_reserved)
        } else {
            Vec::new()
        }
    }
}

fn section_heading_layout() -> LayoutOpts {
    LayoutOpts {
        height: 24.0,
        gap: 8.0,
        cross: Align::Center,
        ..Default::default()
    }
}

fn workspace_card_layout() -> LayoutOpts {
    LayoutOpts {
        min_height: 96.0,
        gap: 8.0,
        pad: 15.0,
        cross: Align::Stretch,
        ..materials::card(&Design::dark())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_ai_workspaces_identity_opens_the_surface() {
        let mut workspaces = AiWorkspaces::new();
        workspaces.open_builtin(BuiltInApplication::AiWorkspaces);
        assert!(workspaces.open);
        workspaces.open_builtin(BuiltInApplication::ScreenshotSelector);
        assert!(!workspaces.open);
    }

    #[test]
    fn snapshot_update_clears_a_completed_revoke_confirmation() {
        let mut workspaces = AiWorkspaces::new();
        workspaces.pending_revoke = Some(RealmId(42));
        workspaces.update_snapshot(&aegis_core::realm::RealmModel::new().snapshot());
        assert_eq!(workspaces.pending_revoke, None);
    }
}
