//! `org.freedesktop.Secret.Collection`: item lookup and creation within one
//! collection. The served object is a thin handle over `SecretState`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use zbus::zvariant::{OwnedObjectPath, Value};

use super::item::ItemIface;
use super::{SERVICE_PATH, generate_id, is_locked, root_path};
use crate::secret::{CollectionRt, ItemRt, SecretState};

/// Create (or find) a collection after a successful vault unlock. Runs on
/// the unlock worker thread; the bus objects appear through a later
/// `register_collections` pass.
pub(crate) fn create_collection_now(
    state: &Arc<Mutex<SecretState>>,
    alias: &str,
    label: &str,
) -> Result<OwnedObjectPath, String> {
    let mut state = state.lock().unwrap();
    if !state.is_unlocked() {
        return Err("collection creation while locked".to_string());
    }
    let col_id = if alias.is_empty() {
        format!("c{}", generate_id())
    } else {
        alias.to_string()
    };
    state
        .collections
        .entry(col_id.clone())
        .or_insert_with(|| CollectionRt {
            label: label.to_string(),
            deleted: false,
            items: HashMap::new(),
        });
    state.sync_to_vault();
    OwnedObjectPath::try_from(format!("{SERVICE_PATH}/collection/{col_id}"))
        .map_err(|e| e.to_string())
}

/// The served collection object.
pub(crate) struct CollectionIface {
    pub(crate) id: String,
    pub(crate) state: Arc<Mutex<SecretState>>,
}

#[zbus::interface(name = "org.freedesktop.Secret.Collection")]
impl CollectionIface {
    async fn delete(
        &self,
        #[zbus(object_server)] server: &zbus::ObjectServer,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        log::info!("portal: secrets Delete collection {}", self.id);
        let item_ids = {
            let mut state = self.state.lock().unwrap();
            if !state.is_unlocked() {
                return Err(is_locked());
            }
            let ids = state
                .collections
                .get(&self.id)
                .map(|col| col.items.keys().cloned().collect::<Vec<_>>())
                .unwrap_or_default();
            if let Some(col) = state.collections.get_mut(&self.id) {
                col.deleted = true;
            }
            state.sync_to_vault();
            ids
        };
        // A deleted collection and everything under it must leave the bus,
        // so later calls on the stale paths fail instead of reading
        // tombstones.
        for item_id in item_ids {
            let path = format!("{SERVICE_PATH}/collection/{}/{item_id}", self.id);
            if let Err(error) = server.remove::<ItemIface, _>(path).await {
                log::warn!(
                    "portal: could not unregister item {item_id} of deleted collection {}: {error}",
                    self.id
                );
            }
        }
        let col_path = format!("{SERVICE_PATH}/collection/{}", self.id);
        if let Err(error) = server.remove::<CollectionIface, _>(col_path).await {
            log::warn!(
                "portal: could not unregister deleted collection {}: {error}",
                self.id
            );
        }
        if self.id == "login"
            && let Err(error) = server
                .remove::<CollectionIface, _>(super::ALIAS_DEFAULT_PATH)
                .await
        {
            log::warn!("portal: could not unregister the default alias: {error}");
        }
        Ok(root_path())
    }

    async fn search_items(
        &self,
        attributes: HashMap<&str, &str>,
    ) -> zbus::fdo::Result<Vec<OwnedObjectPath>> {
        log::debug!(
            "portal: secrets Collection::SearchItems {} with {} attribute(s)",
            self.id,
            attributes.len()
        );
        let state = self.state.lock().unwrap();
        let mut matched = Vec::new();
        if let Some(col) = state.collections.get(&self.id) {
            for (item_id, item) in &col.items {
                if item.deleted {
                    continue;
                }
                let matches = attributes
                    .iter()
                    .all(|(k, v)| item.attributes.get(*k).map(String::as_str) == Some(*v));
                if matches
                    && let Ok(p) = OwnedObjectPath::try_from(format!(
                        "{SERVICE_PATH}/collection/{}/{item_id}",
                        self.id
                    ))
                {
                    matched.push(p);
                }
            }
        }
        Ok(matched)
    }

    async fn create_item(
        &self,
        #[zbus(object_server)] server: &zbus::ObjectServer,
        properties: HashMap<&str, Value<'_>>,
        secret: (OwnedObjectPath, Vec<u8>, Vec<u8>, String),
        replace: bool,
    ) -> zbus::fdo::Result<(OwnedObjectPath, OwnedObjectPath)> {
        log::info!("portal: secrets CreateItem in collection {}", self.id);

        let label = match properties.get("org.freedesktop.Secret.Item.Label") {
            Some(Value::Str(label)) => label.as_str().to_string(),
            _ => "New Item".to_string(),
        };
        let mut attributes = HashMap::new();
        if let Some(Value::Dict(dict)) = properties.get("org.freedesktop.Secret.Item.Attributes") {
            for (key, value) in dict.iter() {
                if let (Value::Str(key), Value::Str(value)) = (key, value) {
                    attributes.insert(key.as_str().to_string(), value.as_str().to_string());
                }
            }
        }

        // Mutate and persist in one critical section; only the D-Bus
        // registration happens after the guard is dropped.
        let item_id = generate_id();
        let replaced_ids = {
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

            let Some(col) = state.collections.get_mut(&self.id) else {
                return Err(zbus::fdo::Error::Failed(format!(
                    "unknown collection {}",
                    self.id
                )));
            };
            let mut replaced = Vec::new();
            if replace {
                for (old_id, item) in col.items.iter_mut() {
                    if !item.deleted && item.attributes == attributes {
                        item.deleted = true;
                        replaced.push(old_id.clone());
                    }
                }
            }
            col.items.insert(
                item_id.clone(),
                ItemRt {
                    label,
                    attributes,
                    secret: decrypted,
                    deleted: false,
                },
            );
            state.sync_to_vault();
            replaced
        };

        let item_path =
            OwnedObjectPath::try_from(format!("{SERVICE_PATH}/collection/{}/{item_id}", self.id))
                .map_err(|e| zbus::fdo::Error::InvalidArgs(e.to_string()))?;
        if server
            .at(
                item_path.clone(),
                ItemIface {
                    collection_id: self.id.clone(),
                    item_id,
                    state: Arc::clone(&self.state),
                },
            )
            .await
            .is_err()
        {
            return Err(zbus::fdo::Error::Failed(
                "D-Bus registration failed".to_string(),
            ));
        }
        // Replaced items are deleted; drop their bus objects too.
        for old_id in replaced_ids {
            let old_path = format!("{SERVICE_PATH}/collection/{}/{old_id}", self.id);
            if let Err(error) = server.remove::<ItemIface, _>(old_path).await {
                log::warn!("portal: could not unregister replaced item {old_id}: {error}");
            }
        }

        Ok((item_path, root_path()))
    }

    #[zbus(property)]
    fn locked(&self) -> bool {
        !self.state.lock().unwrap().is_unlocked()
    }

    #[zbus(property)]
    fn items(&self) -> Vec<OwnedObjectPath> {
        let state = self.state.lock().unwrap();
        let mut paths = Vec::new();
        if let Some(col) = state.collections.get(&self.id) {
            for (item_id, item) in &col.items {
                if item.deleted {
                    continue;
                }
                if let Ok(p) = OwnedObjectPath::try_from(format!(
                    "{SERVICE_PATH}/collection/{}/{item_id}",
                    self.id
                )) {
                    paths.push(p);
                }
            }
        }
        paths
    }

    #[zbus(property)]
    fn label(&self) -> String {
        let state = self.state.lock().unwrap();
        state
            .collections
            .get(&self.id)
            .map(|col| col.label.clone())
            .unwrap_or_default()
    }

    #[zbus(property)]
    fn set_label(&self, label: String) {
        let mut state = self.state.lock().unwrap();
        let changed = if let Some(col) = state.collections.get_mut(&self.id) {
            col.label = label;
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
