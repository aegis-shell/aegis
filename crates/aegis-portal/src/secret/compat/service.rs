//! `org.freedesktop.Secret.Service`: session negotiation, collection
//! creation, item search, secret retrieval, aliases, and the unlock/lock
//! entry points.
//!
//! `Unlock` and `CreateCollection` on a locked (password-mode) vault queue
//! behind the shared unlock coordinator (`secret` module): the compositor's
//! masked secret prompt asks for the vault password once, and every queued
//! caller's spec Prompt object completes from that single interaction.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use zbus::zvariant::{ObjectPath, OwnedObjectPath, Value};

use super::session::{SessionIface, calculate_dh_shared_secret};
use super::{SERVICE_PATH, generate_id, root_path};
use crate::secret::{CollectionRt, SecretState, SessionCrypto};

/// The served service object.
pub(crate) struct ServiceIface {
    /// Blocking handle used by the prompt-completion thread.
    pub(crate) conn: zbus::blocking::Connection,
    pub(crate) state: Arc<Mutex<SecretState>>,
    /// The compositor IPC socket the unlock prompt is served over.
    pub(crate) socket: std::path::PathBuf,
}

#[zbus::interface(name = "org.freedesktop.Secret.Service")]
impl ServiceIface {
    async fn open_session(
        &self,
        #[zbus(object_server)] server: &zbus::ObjectServer,
        algorithm: &str,
        input: Value<'_>,
    ) -> zbus::fdo::Result<(Value<'static>, OwnedObjectPath)> {
        log::debug!("portal: secrets OpenSession algorithm={algorithm}");

        let (result, crypto) = match algorithm {
            "plain" => (Value::from(""), SessionCrypto::Plain),
            "dh-ietf1024-sha256-aes128-cbc-pkcs7" => {
                let client_pub: Vec<u8> = input
                    .try_into()
                    .map_err(|_| zbus::fdo::Error::InvalidArgs("invalid DH input".to_string()))?;
                let (server_pub, sym_key) = calculate_dh_shared_secret(&client_pub)
                    .map_err(|e| zbus::fdo::Error::InvalidArgs(e.to_string()))?;
                (Value::from(server_pub), SessionCrypto::Dh(sym_key))
            }
            _ => {
                return Err(zbus::fdo::Error::InvalidArgs(format!(
                    "unsupported session algorithm '{algorithm}'"
                )));
            }
        };

        let session_path =
            OwnedObjectPath::try_from(format!("{SERVICE_PATH}/session/s{}", generate_id()))
                .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;

        // Insert before registering so a racing `Close` cannot remove a
        // session that is not in the map yet; roll back on failure.
        self.state
            .lock()
            .unwrap()
            .sessions
            .insert(session_path.clone(), crypto);
        if let Err(error) = server
            .at(
                session_path.clone(),
                SessionIface {
                    path: session_path.clone(),
                    state: Arc::clone(&self.state),
                },
            )
            .await
        {
            self.state.lock().unwrap().sessions.remove(&session_path);
            return Err(zbus::fdo::Error::Failed(error.to_string()));
        }
        Ok((result, session_path))
    }

    async fn create_collection(
        &self,
        #[zbus(object_server)] server: &zbus::ObjectServer,
        properties: HashMap<&str, Value<'_>>,
        alias: &str,
    ) -> zbus::fdo::Result<(OwnedObjectPath, OwnedObjectPath)> {
        log::info!("portal: secrets CreateCollection alias='{alias}'");

        let label = match properties.get("org.freedesktop.Secret.Collection.Label") {
            Some(Value::Str(label)) => label.as_str().to_string(),
            _ => "New Collection".to_string(),
        };

        // Locked vault: queue behind the shared unlock prompt; the created
        // collection's path arrives through the prompt's Completed signal
        // (this is how sandboxed clients such as Chrome create their
        // default collection on a locked vault).
        if !self.state.lock().unwrap().is_unlocked() {
            log::info!(
                "portal: CreateCollection alias='{alias}' while locked; prompting via compositor chrome"
            );
            let prompt = super::queue_prompt(
                server,
                &self.conn,
                &self.state,
                &self.socket,
                crate::secret::CompatUnlockKind::CreateCollection {
                    alias: alias.to_string(),
                    label,
                },
            )
            .await?;
            return Ok((root_path(), prompt));
        }

        let col_id = if alias.is_empty() {
            format!("c{}", generate_id())
        } else {
            alias.to_string()
        };
        let col_path_str = format!("{SERVICE_PATH}/collection/{col_id}");

        let created = {
            let mut state = self.state.lock().unwrap();
            if state.collections.contains_key(&col_id) {
                false
            } else {
                state.collections.insert(
                    col_id.clone(),
                    CollectionRt {
                        label,
                        deleted: false,
                        items: HashMap::new(),
                    },
                );
                state.sync_to_vault();
                true
            }
        };

        if created
            && let Err(error) = server
                .at(
                    col_path_str.clone(),
                    super::collection::CollectionIface {
                        id: col_id.clone(),
                        state: Arc::clone(&self.state),
                    },
                )
                .await
        {
            log::error!("portal: could not register collection {col_id} on the bus: {error}");
        }

        let col_path = OwnedObjectPath::try_from(col_path_str)
            .map_err(|e| zbus::fdo::Error::InvalidArgs(e.to_string()))?;
        Ok((col_path, root_path()))
    }

    async fn search_items(
        &self,
        attributes: HashMap<&str, &str>,
    ) -> zbus::fdo::Result<(Vec<OwnedObjectPath>, Vec<OwnedObjectPath>)> {
        log::debug!(
            "portal: secrets SearchItems with {} attribute(s)",
            attributes.len()
        );
        let state = self.state.lock().unwrap();
        let mut matched = Vec::new();

        for (col_id, col) in &state.collections {
            if col.deleted {
                continue;
            }
            for (item_id, item) in &col.items {
                if item.deleted {
                    continue;
                }
                let matches = attributes
                    .iter()
                    .all(|(k, v)| item.attributes.get(*k).map(String::as_str) == Some(*v));
                if matches
                    && let Ok(p) = OwnedObjectPath::try_from(format!(
                        "{SERVICE_PATH}/collection/{col_id}/{item_id}"
                    ))
                {
                    matched.push(p);
                }
            }
        }

        Ok((matched, Vec::new()))
    }

    async fn unlock(
        &self,
        #[zbus(object_server)] server: &zbus::ObjectServer,
        objects: Vec<ObjectPath<'_>>,
    ) -> zbus::fdo::Result<(Vec<OwnedObjectPath>, OwnedObjectPath)> {
        log::debug!("portal: secrets Unlock for {} object(s)", objects.len());

        {
            let state = self.state.lock().unwrap();
            if state.is_unlocked() {
                let unlocked = objects
                    .iter()
                    .map(|o| OwnedObjectPath::from(o.clone()))
                    .collect();
                return Ok((unlocked, root_path()));
            }
        }

        // The vault is locked (password mode, no PAM token yet): queue
        // behind the shared unlock coordinator, which asks the user through
        // the compositor's masked secret prompt (`PromptSecret` IPC).
        log::info!("portal: secrets Unlock while locked; prompting via compositor chrome");
        let objects: Vec<OwnedObjectPath> = objects
            .iter()
            .map(|o| OwnedObjectPath::from(o.clone()))
            .collect();
        let prompt = super::queue_prompt(
            server,
            &self.conn,
            &self.state,
            &self.socket,
            crate::secret::CompatUnlockKind::Unlock(objects),
        )
        .await?;
        Ok((vec![], prompt))
    }

    /// Locking stays a no-op stub (as in wssp): keyfile mode unlocks at
    /// startup and there is no re-lock trigger yet.
    async fn lock(
        &self,
        _objects: Vec<ObjectPath<'_>>,
    ) -> zbus::fdo::Result<(Vec<OwnedObjectPath>, OwnedObjectPath)> {
        Ok((vec![], root_path()))
    }

    async fn get_secrets(
        &self,
        items: Vec<ObjectPath<'_>>,
        session_path: ObjectPath<'_>,
    ) -> zbus::fdo::Result<HashMap<OwnedObjectPath, super::item::SecretStruct>> {
        log::debug!("portal: secrets GetSecrets for {} item(s)", items.len());
        let state = self.state.lock().unwrap();
        let session = state
            .sessions
            .get(&OwnedObjectPath::from(session_path.clone()))
            .ok_or_else(|| zbus::fdo::Error::InvalidArgs("invalid session".to_string()))?;

        let mut result = HashMap::new();

        for item_path in &items {
            let path_str = item_path.as_str();
            let parts: Vec<&str> = path_str.split('/').collect();
            if parts.len() < 7 || parts[4] != "collection" {
                continue;
            }
            let (col_id, item_id) = (parts[5], parts[6]);

            let Some(col) = state.collections.get(col_id) else {
                continue;
            };
            if col.deleted {
                continue;
            }
            let Some(item) = col.items.get(item_id) else {
                continue;
            };
            if item.deleted {
                continue;
            }

            match session.encrypt(&item.secret) {
                Ok((params, value)) => {
                    if let Ok(p) = OwnedObjectPath::try_from(path_str.to_string()) {
                        result.insert(
                            p,
                            super::item::SecretStruct {
                                session: session_path.clone().into(),
                                parameters: params,
                                value,
                                content_type: "text/plain".into(),
                            },
                        );
                    }
                }
                Err(error) => {
                    log::error!("portal: could not encrypt secret {path_str}: {error}");
                }
            }
        }

        Ok(result)
    }

    async fn read_alias(&self, alias: &str) -> zbus::fdo::Result<OwnedObjectPath> {
        log::debug!("portal: secrets ReadAlias '{alias}'");
        let state = self.state.lock().unwrap();
        let target = if alias == "default" && !state.collections.contains_key("default") {
            "login"
        } else {
            alias
        };
        if state.collections.contains_key(target) {
            OwnedObjectPath::try_from(format!("{SERVICE_PATH}/collection/{target}"))
                .map_err(|e| zbus::fdo::Error::InvalidArgs(e.to_string()))
        } else {
            Ok(root_path())
        }
    }

    async fn set_alias(&self, _alias: &str, _collection: ObjectPath<'_>) -> zbus::fdo::Result<()> {
        Ok(())
    }

    #[zbus(property)]
    fn collections(&self) -> Vec<OwnedObjectPath> {
        let state = self.state.lock().unwrap();
        state
            .collections
            .iter()
            .filter(|(_, col)| !col.deleted)
            .filter_map(|(col_id, _)| {
                OwnedObjectPath::try_from(format!("{SERVICE_PATH}/collection/{col_id}")).ok()
            })
            .collect()
    }
}
