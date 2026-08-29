//! Cryptographic primitives and pairing tokens for UIP remote actors.

use sha2::{Digest, Sha256};

/// A 32-byte cryptographic token used for actor pairing or one-time transaction authorizations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct AuthToken(pub [u8; 32]);

impl AuthToken {
    /// Generate a cryptographically randomized token using `fastrand`.
    pub fn random() -> Self {
        let mut bytes = [0u8; 32];
        for b in &mut bytes {
            *b = fastrand::u8(..);
        }
        Self(bytes)
    }

    /// Derive an authorization digest from a shared secret and nonce.
    pub fn derive(secret: &[u8], nonce: &[u8]) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(secret);
        hasher.update(b":uip:token:");
        hasher.update(nonce);
        let result = hasher.finalize();
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(&result);
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

/// Pairing challenge exchanged during initial device registration.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PairingChallenge {
    pub server_nonce: [u8; 16],
    pub pin_code: String,
}

impl PairingChallenge {
    pub fn generate() -> Self {
        let mut server_nonce = [0u8; 16];
        for b in &mut server_nonce {
            *b = fastrand::u8(..);
        }
        let pin = format!("{:06}", fastrand::u32(0..1_000_000));
        Self {
            server_nonce,
            pin_code: pin,
        }
    }
}
