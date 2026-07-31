//! At-rest vault crypto for the secret store.
//!
//! The vault file is `serde_json` serialized [`VaultData`] encrypted with
//! XChaCha20-Poly1305 under the 32-byte master key; on disk it is a 24-byte
//! nonce prefix followed by the ciphertext. The format is byte-compatible
//! with the wssp vault so an existing `vault.enc` keeps working.
//!
//! Startup uses keyfile mode (`vault.key` holding the master key as hex).
//! The Argon2id password KDF (`derive_key`/`generate_salt`) is kept for the
//! future password-unlock slice that lands together with the native
//! prompter.

use std::fs;
use std::path::PathBuf;

use argon2::Argon2;
use argon2::password_hash::SaltString;
use chacha20poly1305::aead::{Aead, AeadCore, KeyInit};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use rand::RngCore;
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

use super::SecretError;

/// The decrypted vault contents: every persisted collection and item.
#[derive(Serialize, Deserialize, Zeroize)]
#[zeroize(drop)]
pub struct VaultData {
    pub collections: Vec<CollectionData>,
}

#[derive(Serialize, Deserialize, Zeroize)]
#[zeroize(drop)]
pub struct CollectionData {
    pub id: String,
    pub label: String,
    pub items: Vec<ItemData>,
}

#[derive(Serialize, Deserialize, Zeroize)]
#[zeroize(drop)]
pub struct ItemData {
    pub id: String,
    pub label: String,
    #[zeroize(skip)]
    pub attributes: std::collections::HashMap<String, String>,
    pub secret: Vec<u8>,
}

/// An open vault: its file location plus the master key in memory.
pub struct Vault {
    path: PathBuf,
    master_key: [u8; 32],
}

impl Vault {
    pub fn new(path: PathBuf, master_key: [u8; 32]) -> Self {
        Self { path, master_key }
    }

    /// The in-memory master key. Callers must derive purpose-separated keys
    /// from it (HKDF) instead of handing it out directly.
    pub fn get_master_key(&self) -> &[u8; 32] {
        &self.master_key
    }

    /// Argon2id password KDF for password-mode vaults. Unused while startup
    /// only supports keyfile mode; kept for the password-unlock slice.
    pub fn derive_key(password: &str, salt_str: &str) -> Result<[u8; 32], SecretError> {
        let salt = SaltString::from_b64(salt_str)
            .map_err(|e| SecretError::Crypto(format!("invalid salt: {e}")))?;
        let argon2 = Argon2::default();

        let mut key = [0u8; 32];
        argon2
            .hash_password_into(password.as_bytes(), salt.as_str().as_bytes(), &mut key)
            .map_err(|e| SecretError::Crypto(format!("hash failed: {e}")))?;

        Ok(key)
    }

    /// Fresh Argon2id salt for password mode; see `derive_key`.
    #[allow(dead_code)]
    pub fn generate_salt() -> String {
        SaltString::generate(&mut OsRng).as_str().to_string()
    }

    /// Fresh random master key for keyfile mode.
    pub fn generate_key() -> [u8; 32] {
        let mut key = [0u8; 32];
        OsRng.fill_bytes(&mut key);
        key
    }

    pub fn key_to_hex(key: &[u8; 32]) -> String {
        key.iter().map(|b| format!("{b:02x}")).collect()
    }

    pub fn key_from_hex(hex: &str) -> Result<[u8; 32], SecretError> {
        let hex = hex.trim();
        if hex.len() != 64 {
            return Err(SecretError::Crypto(
                "vault.key must be 64 hex chars (32 bytes)".into(),
            ));
        }
        let mut arr = [0u8; 32];
        for (i, chunk) in hex.as_bytes().chunks(2).enumerate() {
            let s = std::str::from_utf8(chunk)
                .map_err(|_| SecretError::Crypto("invalid hex in vault.key".into()))?;
            arr[i] = u8::from_str_radix(s, 16)
                .map_err(|_| SecretError::Crypto(format!("invalid hex byte: {s}")))?;
        }
        Ok(arr)
    }

    /// Encrypt and persist the vault contents (24-byte nonce prefix +
    /// ciphertext).
    pub fn save(&self, data: &VaultData) -> Result<(), SecretError> {
        let serialized = serde_json::to_vec(data)?;
        let cipher = XChaCha20Poly1305::new(&self.master_key.into());
        let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
        let ciphertext = cipher
            .encrypt(&nonce, serialized.as_ref())
            .map_err(|e| SecretError::Crypto(format!("encryption failure: {e}")))?;

        let mut final_data = nonce.to_vec();
        final_data.extend_from_slice(&ciphertext);

        fs::write(&self.path, final_data).map_err(SecretError::Io)
    }

    /// Load and decrypt the vault. A missing file reads as an empty vault; a
    /// truncated or undecryptable file is an error.
    pub fn load(&self) -> Result<VaultData, SecretError> {
        if !self.path.exists() {
            return Ok(VaultData {
                collections: vec![],
            });
        }

        let file_data = fs::read(&self.path).map_err(SecretError::Io)?;
        if file_data.len() < 24 {
            return Err(SecretError::Vault("vault file corrupted".to_string()));
        }

        let (nonce_bytes, ciphertext) = file_data.split_at(24);
        let nonce = XNonce::from_slice(nonce_bytes);

        let cipher = XChaCha20Poly1305::new(&self.master_key.into());
        let plaintext = cipher
            .decrypt(nonce, ciphertext)
            .map_err(|e| SecretError::Crypto(format!("decryption failure: {e}")))?;

        let data: VaultData = serde_json::from_slice(&plaintext)?;
        Ok(data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vault_crypto_roundtrip() {
        let mut suffix = [0u8; 8];
        OsRng.fill_bytes(&mut suffix);
        let suffix: String = suffix.iter().map(|b| format!("{b:02x}")).collect();
        let vault_path = std::env::temp_dir().join(format!("aegis-vault-test-{suffix}.enc"));

        let password = "super-secret-password";
        let salt = Vault::generate_salt();
        let key = Vault::derive_key(password, &salt).expect("key derivation failed");

        let vault = Vault::new(vault_path.clone(), key);
        let data = VaultData {
            collections: vec![CollectionData {
                id: "login".into(),
                label: "Login".into(),
                items: vec![ItemData {
                    id: "i1".into(),
                    label: "Item".into(),
                    attributes: [("app".to_string(), "aegis".to_string())]
                        .into_iter()
                        .collect(),
                    secret: b"hunter2".to_vec(),
                }],
            }],
        };

        vault.save(&data).expect("save failed");
        let loaded = vault.load().expect("load failed");
        assert_eq!(loaded.collections.len(), 1);
        assert_eq!(loaded.collections[0].id, "login");
        assert_eq!(loaded.collections[0].items[0].secret, b"hunter2");

        let _ = std::fs::remove_file(vault_path);
    }
}
