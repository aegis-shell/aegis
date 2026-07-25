//! Runtime Realm management, intentionally separate from persistent settings.

use aegis_core::realm::{RealmId, RealmKind, RealmSnapshot, RealmState};
use aegis_shell::{ChromeEvents, Localizer, Message, RealmIntent};
use lens::{Align, Frame, LayoutOpts};

use crate::ui::{section_heading_layout, settings_card_layout};

pub(crate) struct RealmManager {
    snapshot: RealmSnapshot,
    pending_revoke: Option<RealmId>,
}

impl RealmManager {
    pub(crate) fn new() -> Self {
        Self {
            snapshot: aegis_core::realm::RealmModel::new().snapshot(),
            pending_revoke: None,
        }
    }

    pub(crate) fn update(&mut self, snapshot: &RealmSnapshot) {
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

    pub(crate) fn render(&mut self, frame: &mut Frame, i18n: &Localizer, out: &mut ChromeEvents) {
        frame.column_ex(
            &LayoutOpts {
                gap: 5.0,
                cross: Align::Stretch,
                ..Default::default()
            },
            |frame| {
                frame.heading(i18n.text(Message::AiWorkspaces), 2);
                frame.label_wrapped_sized(i18n.text(Message::AiWorkspacesDescription), 12.0, 560.0);
            },
        );
        frame.size_next(0.0, 32.0);
        if frame.button(i18n.text(Message::NewAiWorkspace)) {
            let ordinal = self
                .snapshot
                .realms
                .iter()
                .filter(|realm| realm.kind == RealmKind::Agent)
                .count()
                + 1;
            out.realm_intents.push(RealmIntent::Create {
                label: format!("AI Workspace {ordinal}"),
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
            frame.column_ex(&settings_card_layout(), |frame| {
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
                            500.0,
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

impl Default for RealmManager {
    fn default() -> Self {
        Self::new()
    }
}
