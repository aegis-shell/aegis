//! `org.freedesktop.Secret.Item`: one stored secret plus its metadata, and
//! the `Secret` struct used to transport secrets inside a session.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use zbus::zvariant::{ObjectPath, OwnedObjectPath};

use super::{is_locked, root_path};
use crate::secret::SecretState;

/// The wire struct transporting a secret inside a session:
/// `(session, parameters, value, content_type)`.
#[derive(serde::Serialize, serde::Deserialize, zbus::zvariant::Type)]
pub(crate) struct SecretStruct {
    pub(crate) session: OwnedObjectPath,
    pub(crate) parameters: Vec<u8>,
    pub(crate) value: Vec<u8>,
    pub(crate) content_type: String,
}

/// The served item object: a thin handle over `SecretState`.
pub(crate) struct ItemIface {
    pub(crate) collection_id: String,
    pub(crate) item_id: String,
    pub(crate) state: Arc<Mutex<SecretState>>,
}

#[zbus::interface(name = "org.freedesktop.Secret.Item")]
impl ItemIface {
    async fn delete(
        &self,
        #[zbus(object_server)] server: &zbus::ObjectServer,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        log::info!("portal: secrets Delete item {}", self.item_id);
        {
            let mut state = self.state.lock().unwrap();
            if !state.is_unlocked() {
                return Err(is_locked());
            }
            if let Some(col) = state.collections.get_mut(&self.collection_id)
                && let Some(item) = col.items.get_mut(&self.item_id)
            {
                item.deleted = true;
            }
            state.sync_to_vault();
        }
        // A deleted item must leave the bus, so later calls on the stale
        // path fail instead of reading the tombstone.
        let path = format!(
            "{}/collection/{}/{}",
            super::SERVICE_PATH, self.collection_id, self.item_id
        );
        if let Err(error) = server.remove::<ItemIface, _>(path).await {
            log::warn!(
                "portal: could not unregister deleted item {}: {error}",
                self.item_id
            );
        }
        Ok(root_path())
    }

    async fn get_secret(&self, session_path: ObjectPath<'_>) -> zbus::fdo::Result<(SecretStruct,)> {
        log::info!("portal: secrets GetSecret {}", self.item_id);
        let state = self.state.lock().unwrap();
        if !state.is_unlocked() {
            return Err(is_locked());
        }
        let session = state
            .sessions
            .get(&OwnedObjectPath::from(session_path.clone()))
            .ok_or_else(|| zbus::fdo::Error::InvalidArgs("invalid session".to_string()))?;
        let item = state
            .collections
            .get(&self.collection_id)
            .and_then(|col| col.items.get(&self.item_id))
            .ok_or_else(|| zbus::fdo::Error::Failed(format!("unknown item {}", self.item_id)))?;

        let (params, value) = session
            .encrypt(&item.secret)
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;

        Ok((SecretStruct {
            session: session_path.into(),
            parameters: params,
            value,
            content_type: "text/plain".into(),
        },))
    }

    async fn set_secret(
        &self,
        secret: (OwnedObjectPath, Vec<u8>, Vec<u8>, String),
    ) -> zbus::fdo::Result<()> {
        log::info!("portal: secrets SetSecret {}", self.item_id);
        let mut state = self.state.lock().unwrap();
        if !state.is_unlocked() {
            return Err(is_locked());
        }
        let session = state
            .sessions
            .get(&secret.0)
            .ok_or_else(|| zbus::fdo::Error::InvalidArgs("invalid session".to_string()))?;
        let decrypted = session
            .decrypt(&secret.1, &secret.2)
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;

        let item = state
            .collections
            .get_mut(&self.collection_id)
            .and_then(|col| col.items.get_mut(&self.item_id))
            .ok_or_else(|| zbus::fdo::Error::Failed(format!("unknown item {}", self.item_id)))?;
        item.secret = decrypted;
        state.sync_to_vault();
        Ok(())
    }

    #[zbus(property)]
    fn locked(&self) -> bool {
        !self.state.lock().unwrap().is_unlocked()
    }

    #[zbus(property)]
    fn label(&self) -> String {
        let state = self.state.lock().unwrap();
        state
            .collections
            .get(&self.collection_id)
            .and_then(|col| col.items.get(&self.item_id))
            .map(|item| item.label.clone())
            .unwrap_or_default()
    }

    #[zbus(property)]
    fn set_label(&self, label: String) {
        let mut state = self.state.lock().unwrap();
        let changed = if let Some(item) = state
            .collections
            .get_mut(&self.collection_id)
            .and_then(|col| col.items.get_mut(&self.item_id))
        {
            item.label = label;
            true
        } else {
            false
        };
        if changed {
            state.sync_to_vault();
        }
    }

    #[zbus(property)]
    fn attributes(&self) -> HashMap<String, String> {
        let state = self.state.lock().unwrap();
        state
            .collections
            .get(&self.collection_id)
            .and_then(|col| col.items.get(&self.item_id))
            .map(|item| item.attributes.clone())
            .unwrap_or_default()
    }

    #[zbus(property)]
    fn set_attributes(&self, attributes: HashMap<String, String>) {
        let mut state = self.state.lock().unwrap();
        let changed = if let Some(item) = state
            .collections
            .get_mut(&self.collection_id)
            .and_then(|col| col.items.get_mut(&self.item_id))
        {
            item.attributes = attributes;
            true
        } else {
            false
        };
        if changed {
            state.sync_to_vault();
        }
    }

    #[zbus(property)]
    fn created(&self) -> u64 {
        0
    }

    #[zbus(property)]
    fn modified(&self) -> u64 {
        0
    }
}
