//! Compositor-owned Agent Workspaces lifecycle and authority presentation.

use aegis_design::{Design, materials};
use aegis_model::app::BuiltInApplication;
use aegis_model::input::{KeyAction, KeyChar, key_action};
use aegis_model::interaction_domain::{
    InteractionDomainId, InteractionDomainKind, InteractionDomainSnapshot, InteractionDomainState,
};
use aegis_model::window::Window;
use aegis_model::workspace::WorkspaceSnapshot;
use aegis_shell::{
    BackdropRegion, Chrome, ChromeCommand, ChromeEvents, ChromeUpdate, CursorShape,
    InteractionDomainIntent, Localizer, Message, ModalApplicationSpec, Reserved,
};
use lens::{Align, Frame, Icon, Input, LayoutOpts};

const SURFACE: ModalApplicationSpec = ModalApplicationSpec {
    scrim_id: "aegis-agent-workspaces-scrim",
    panel_id: "aegis-agent-workspaces-app",
    scroll_id: "aegis-agent-workspaces-page",
    title: Message::AgentWorkspaces,
    icon: Icon::Grid,
    max_width: 760.0,
    max_height: 590.0,
};

/// Trusted modal application for Agent Workspace lifecycle presentation.
pub struct AgentWorkspaces {
    open: bool,
    modal_reserved: Reserved,
    snapshot: InteractionDomainSnapshot,
    pending_revoke: Option<InteractionDomainId>,
    /// The design snapshot the surface paints from, from
    /// [`ChromeUpdate::Appearance`]. Seeded on registration by
    /// [`aegis_shell::Shell::add`] and refreshed when the desktop color scheme
    /// changes; defaults to the dark appearance until the first update arrives.
    design: Design,
}

impl AgentWorkspaces {
    pub fn new() -> Self {
        Self {
            open: false,
            modal_reserved: Reserved::default(),
            snapshot: aegis_model::interaction_domain::InteractionDomainModel::new().snapshot(),
            pending_revoke: None,
            design: Design::dark(),
        }
    }

    #[cfg(test)]
    fn open_builtin(&mut self, app: BuiltInApplication) {
        <Self as Chrome>::command(
            self,
            &ChromeCommand::OpenBuiltIn(app),
            &mut ChromeEvents::default(),
        );
    }

    fn update_snapshot(&mut self, snapshot: &InteractionDomainSnapshot) {
        self.snapshot = snapshot.clone();
        if self.pending_revoke.is_some_and(|id| {
            !self
                .snapshot
                .interaction_domains
                .iter()
                .any(|interaction_domain| {
                    interaction_domain.id == id
                        && interaction_domain.state != InteractionDomainState::Revoked
                })
        }) {
            self.pending_revoke = None;
        }
    }

    fn render_content(&mut self, frame: &mut Frame, i18n: &Localizer, out: &mut ChromeEvents) {
        frame.label_wrapped_sized(i18n.text(Message::AgentWorkspacesDescription), 12.0, 640.0);
        frame.size_next(0.0, 32.0);
        if frame.button(i18n.text(Message::CreateEmptyAgentWorkspace)) {
            let ordinal = self
                .snapshot
                .interaction_domains
                .iter()
                .filter(|interaction_domain| {
                    interaction_domain.kind == InteractionDomainKind::Agent
                })
                .count()
                + 1;
            out.interaction_domain_intents
                .push(InteractionDomainIntent::Create {
                    label: i18n.default_agent_workspace_label(ordinal),
                });
        }

        let interaction_domains = self
            .snapshot
            .interaction_domains
            .iter()
            .filter(|interaction_domain| interaction_domain.kind == InteractionDomainKind::Agent)
            .cloned()
            .collect::<Vec<_>>();
        for interaction_domain in interaction_domains {
            let controlled_windows = self
                .snapshot
                .interaction_groups
                .iter()
                .filter(|group| group.control_interaction_domain == interaction_domain.id)
                .map(|group| group.windows.len())
                .sum::<usize>();
            let seat = self
                .snapshot
                .seats
                .iter()
                .find(|seat| seat.interaction_domain == interaction_domain.id);
            frame.column_ex(&workspace_card_layout(&self.design), |frame| {
                frame.row_ex(&section_heading_layout(), |frame| {
                    frame.heading(&interaction_domain.label, 3);
                    frame.flex(1.0);
                    frame.spacer(0.0);
                    let state = match interaction_domain.state {
                        InteractionDomainState::Active => {
                            i18n.text(Message::InteractionDomainActive)
                        }
                        InteractionDomainState::Paused => {
                            i18n.text(Message::InteractionDomainPaused)
                        }
                        InteractionDomainState::Revoked => {
                            i18n.text(Message::InteractionDomainRevoked)
                        }
                    };
                    frame.label_sized(state, 11.0);
                });
                frame.label_sized(
                    &format!(
                        "Interaction Domain {} · {}: {controlled_windows}",
                        interaction_domain.id.0,
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
                if interaction_domain.state != InteractionDomainState::Revoked {
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
                            let next = if interaction_domain.state == InteractionDomainState::Active
                            {
                                InteractionDomainState::Paused
                            } else {
                                InteractionDomainState::Active
                            };
                            let label = if next == InteractionDomainState::Paused {
                                i18n.text(Message::PauseInteractionDomain)
                            } else {
                                i18n.text(Message::ResumeInteractionDomain)
                            };
                            if frame.button(label) {
                                out.interaction_domain_intents.push(
                                    InteractionDomainIntent::SetState {
                                        interaction_domain: interaction_domain.id,
                                        state: next,
                                        expected_revision: self.snapshot.revision,
                                    },
                                );
                            }
                            frame.size_next(132.0, 28.0);
                            let confirming = self.pending_revoke == Some(interaction_domain.id);
                            let label = if confirming {
                                i18n.text(Message::ConfirmRevokeInteractionDomain)
                            } else {
                                i18n.text(Message::RevokeInteractionDomain)
                            };
                            if frame.button(label) {
                                if confirming {
                                    out.interaction_domain_intents.push(
                                        InteractionDomainIntent::Revoke {
                                            interaction_domain: interaction_domain.id,
                                            expected_revision: self.snapshot.revision,
                                        },
                                    );
                                    self.pending_revoke = None;
                                } else {
                                    self.pending_revoke = Some(interaction_domain.id);
                                }
                            }
                        },
                    );
                }
            });
        }
    }
}

impl Default for AgentWorkspaces {
    fn default() -> Self {
        Self::new()
    }
}

impl Chrome for AgentWorkspaces {
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
        let design = self.design;
        if SURFACE.render(frame, input, reserved, i18n, &design, |frame| {
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

    fn command(&mut self, command: &ChromeCommand<'_>, _out: &mut ChromeEvents) {
        if let ChromeCommand::OpenBuiltIn(app) = command {
            self.open = *app == BuiltInApplication::AgentWorkspaces;
        }
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

    fn update(&mut self, update: ChromeUpdate<'_>) {
        match update {
            ChromeUpdate::InteractionDomains(snapshot) => self.update_snapshot(snapshot),
            ChromeUpdate::ModalReserved(reserved) => self.modal_reserved = reserved,
            ChromeUpdate::Appearance(design) => self.design = *design,
            _ => {}
        }
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

fn workspace_card_layout(design: &Design) -> LayoutOpts {
    LayoutOpts {
        min_height: 96.0,
        gap: 8.0,
        pad: 15.0,
        cross: Align::Stretch,
        ..materials::card(design)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_agent_workspaces_identity_opens_the_surface() {
        let mut workspaces = AgentWorkspaces::new();
        workspaces.open_builtin(BuiltInApplication::AgentWorkspaces);
        assert!(workspaces.open);
        workspaces.open_builtin(BuiltInApplication::ScreenshotSelector);
        assert!(!workspaces.open);
    }

    #[test]
    fn snapshot_update_clears_a_completed_revoke_confirmation() {
        let mut workspaces = AgentWorkspaces::new();
        workspaces.pending_revoke = Some(InteractionDomainId(42));
        workspaces.update_snapshot(
            &aegis_model::interaction_domain::InteractionDomainModel::new().snapshot(),
        );
        assert_eq!(workspaces.pending_revoke, None);
    }
}
