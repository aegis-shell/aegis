//! The managed Agent Interaction Domain lifecycle (ADR-0125): locate,
//! create-on-first-use, and revoke, with crash recovery through the
//! per-instance state store. Ownership is proven by the authenticated
//! principal subject, never by the recovery file alone.

use std::path::{Path, PathBuf};

use aegis_ipc::{Client, InteractionDomainAction, InteractionDomainActionResult};
use aegis_model::interaction_domain::{
    HUMAN_INTERACTION_DOMAIN, InteractionDomainId, InteractionDomainKind,
    InteractionDomainSnapshot, InteractionDomainState, SeatCapabilities, VirtualOutput,
};

use crate::state::{SessionError, StateStore};

/// One agent-owned Interaction Domain and its latest model revision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ManagedInteractionDomain {
    pub id: InteractionDomainId,
    pub revision: u64,
}

/// Process-local lifecycle manager with crash-recovery metadata.
pub struct InteractionDomainSession {
    label: String,
    subject: String,
    store: StateStore,
    managed: Option<InteractionDomainId>,
}

impl InteractionDomainSession {
    /// Acquire the recovery lock for one agent instance and authenticated
    /// subject. `instance_id` partitions state between independent agent
    /// configurations sharing one data directory.
    pub fn acquire(
        label: &str,
        instance_id: &str,
        subject: &str,
        state_dir: &Path,
    ) -> Result<Self, SessionError> {
        Ok(Self {
            label: label.to_owned(),
            subject: subject.to_owned(),
            store: StateStore::acquire(state_dir, &format!("{instance_id}:{subject}"))?,
            managed: None,
        })
    }

    /// Find a previously managed live Interaction Domain without creating authority.
    pub fn locate(
        &mut self,
        client: &mut Client,
    ) -> Result<(InteractionDomainSnapshot, Option<ManagedInteractionDomain>), SessionError> {
        let snapshot = client.interaction_domains()?;

        if let Some(id) = self.managed
            && interaction_domain_is_managed(&snapshot, id, &self.label, &self.subject)
        {
            return Ok((
                snapshot.clone(),
                Some(ManagedInteractionDomain {
                    id,
                    revision: snapshot.revision,
                }),
            ));
        }
        self.managed = None;

        if let Some(record) = self.store.read()?
            && record.label == self.label
            && record.subject == self.subject
            && interaction_domain_is_managed(
                &snapshot,
                InteractionDomainId(record.interaction_domain),
                &self.label,
                &self.subject,
            )
        {
            let id = InteractionDomainId(record.interaction_domain);
            self.managed = Some(id);
            return Ok((
                snapshot.clone(),
                Some(ManagedInteractionDomain {
                    id,
                    revision: snapshot.revision,
                }),
            ));
        }

        let candidates = snapshot
            .interaction_domains
            .iter()
            .filter(|interaction_domain| {
                interaction_domain.kind == InteractionDomainKind::Agent
                    && interaction_domain.label == self.label
                    && interaction_domain.state != InteractionDomainState::Revoked
                    && snapshot
                        .principals
                        .iter()
                        .find(|principal| principal.id == interaction_domain.controller)
                        .and_then(|principal| principal.subject.as_deref())
                        == Some(self.subject.as_str())
            })
            .map(|interaction_domain| interaction_domain.id)
            .collect::<Vec<_>>();
        match candidates.as_slice() {
            [] => {
                self.store.clear()?;
                Ok((snapshot, None))
            }
            [id] => {
                self.managed = Some(*id);
                self.store.write(*id, &self.label, &self.subject)?;
                Ok((
                    snapshot.clone(),
                    Some(ManagedInteractionDomain {
                        id: *id,
                        revision: snapshot.revision,
                    }),
                ))
            }
            _ => Err(SessionError::Ambiguous {
                label: self.label.clone(),
                interaction_domains: candidates.iter().map(|id| id.0).collect(),
            }),
        }
    }

    /// Reuse the agent's live Interaction Domain or create it atomically on first use.
    pub fn ensure(&mut self, client: &mut Client) -> Result<ManagedInteractionDomain, SessionError> {
        let (_, existing) = self.locate(client)?;
        if let Some(existing) = existing {
            return Ok(existing);
        }
        let result = client.interaction_domain_action(InteractionDomainAction::Create {
            label: self.label.clone(),
            capabilities: SeatCapabilities::POINTER_KEYBOARD,
            output: Some(VirtualOutput::DEFAULT_AGENT),
        })?;
        let InteractionDomainActionResult::Created { bundle } = result else {
            return Err(SessionError::UnexpectedResponse);
        };
        self.managed = Some(bundle.interaction_domain);
        self.store
            .write(bundle.interaction_domain, &self.label, &self.subject)?;
        Ok(ManagedInteractionDomain {
            id: bundle.interaction_domain,
            revision: bundle.revision,
        })
    }

    /// Permanently revoke the managed Interaction Domain, returning all controlled groups
    /// to the human Interaction Domain in the same optimistic revision.
    pub fn revoke(&mut self, client: &mut Client) -> Result<bool, SessionError> {
        let (_, managed) = self.locate(client)?;
        let Some(managed) = managed else {
            return Ok(false);
        };
        let result = client.interaction_domain_action(InteractionDomainAction::Revoke {
            interaction_domain: managed.id,
            fallback: HUMAN_INTERACTION_DOMAIN,
            expected_revision: Some(managed.revision),
        })?;
        let InteractionDomainActionResult::Revoked { .. } = result else {
            return Err(SessionError::UnexpectedResponse);
        };
        self.managed = None;
        self.store.clear()?;
        Ok(true)
    }

    /// Atomically persist the latest directed capture for agent clients that
    /// do not forward image content into the model conversation.
    pub fn store_capture(&self, png: &[u8]) -> Result<PathBuf, SessionError> {
        self.store.write_capture(png)
    }

    /// Atomically persist the latest per-window capture under its own name
    /// so it never overwrites the directed Interaction Domain capture.
    pub fn store_window_capture(&self, png: &[u8]) -> Result<PathBuf, SessionError> {
        self.store.write_capture_named("window", png)
    }
}

fn interaction_domain_is_managed(
    snapshot: &InteractionDomainSnapshot,
    id: InteractionDomainId,
    label: &str,
    subject: &str,
) -> bool {
    snapshot
        .interaction_domains
        .iter()
        .any(|interaction_domain| {
            interaction_domain.id == id
                && interaction_domain.kind == InteractionDomainKind::Agent
                && interaction_domain.label == label
                && interaction_domain.state != InteractionDomainState::Revoked
                && snapshot
                    .principals
                    .iter()
                    .find(|principal| principal.id == interaction_domain.controller)
                    .and_then(|principal| principal.subject.as_deref())
                    == Some(subject)
        })
}
