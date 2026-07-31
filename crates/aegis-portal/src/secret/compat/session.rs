//! `org.freedesktop.Secret.Session` objects and the
//! `dh-ietf1024-sha256-aes128-cbc-pkcs7` key exchange.
//!
//! The served session object is a thin handle; the derived key material
//! lives in `SecretState::sessions` as `SessionCrypto`. The exchange is the
//! RFC 2409 1024-bit MODP group 2 Diffie-Hellman followed by HKDF-SHA256
//! (null salt, empty info) down to a 16-byte AES key — byte-for-byte what
//! libsecret implements.

use std::sync::{Arc, Mutex};

use hkdf::Hkdf;
use num_bigint::{BigUint, RandBigInt};
use sha2::Sha256;
use zbus::zvariant::OwnedObjectPath;
use zeroize::Zeroize;

use crate::secret::{SecretError, SecretState};

// RFC 2409 IETF 1024-bit MODP Group 2.
const DH_P: &str = "FFFFFFFFFFFFFFFFC90FDAA22168C234C4C6628B80DC1CD129024E088A67CC74020BBEA63B139B22514A08798E3404DDEF9519B3CD3A431B302B0A6DF25F14374FE1356D6D51C245E485B576625E7EC6F44C42E9A637ED6B0BFF5CB6F406B7EDEE386BFB5A899FA5AE9F24117C4B1FE649286651ECE65381FFFFFFFFFFFFFFFF";
const DH_G: u32 = 2;

/// The served session object.
pub(crate) struct SessionIface {
    pub(crate) path: OwnedObjectPath,
    pub(crate) state: Arc<Mutex<SecretState>>,
}

#[zbus::interface(name = "org.freedesktop.Secret.Session")]
impl SessionIface {
    async fn close(
        &self,
        #[zbus(object_server)] server: &zbus::ObjectServer,
    ) -> zbus::fdo::Result<()> {
        log::info!("portal: secrets session {} closed", self.path);
        self.state.lock().unwrap().sessions.remove(&self.path);
        if let Err(error) = server.remove::<SessionIface, _>(self.path.clone()).await {
            log::warn!(
                "portal: could not unregister secrets session {}: {error}",
                self.path
            );
        }
        Ok(())
    }
}

/// Perform the server side of the DH exchange: returns the server public
/// key (padded to 128 bytes) and the derived 16-byte AES session key.
///
/// Returns `Err` if the client's public key fails RFC 2409 range validation
/// (must be in `(1, p-1)`).
pub(super) fn calculate_dh_shared_secret(
    client_pub: &[u8],
) -> Result<(Vec<u8>, Vec<u8>), SecretError> {
    let p = BigUint::parse_bytes(DH_P.as_bytes(), 16)
        .ok_or_else(|| SecretError::Crypto("failed to parse DH prime".into()))?;
    let g = BigUint::from(DH_G);

    let mut rng = rand::thread_rng();
    let priv_key = rng.gen_biguint_range(&BigUint::from(2u32), &(&p - BigUint::from(2u32)));

    let pub_key = g.modpow(&priv_key, &p);

    // RFC 2409: the client public key must be in (1, p-1).
    let client_pub_bn = BigUint::from_bytes_be(client_pub);
    let one = BigUint::from(1u32);
    if client_pub_bn <= one || client_pub_bn >= &p - &one {
        return Err(SecretError::Crypto(
            "client DH public key out of valid range".into(),
        ));
    }

    let shared_secret = client_pub_bn.modpow(&priv_key, &p);

    // Pad the shared secret to 128 bytes (spec: hash the full prime-sized
    // value).
    let mut shared_bytes = shared_secret.to_bytes_be();
    if shared_bytes.len() < 128 {
        let mut padded = vec![0u8; 128 - shared_bytes.len()];
        padded.extend_from_slice(&shared_bytes);
        shared_bytes = padded;
    }

    // HKDF-SHA256, null salt (32 zero bytes per RFC 5869 §2.2), empty info —
    // matches libsecret.
    let hk = Hkdf::<Sha256>::new(None, &shared_bytes);
    shared_bytes.zeroize();
    let mut sym_key = vec![0u8; 16];
    hk.expand(&[], &mut sym_key)
        .expect("a 16-byte HKDF-SHA256 output is always valid");
    log::debug!("portal: secrets DH session key derived");

    // Pad the server public key to 128 bytes.
    let mut pub_bytes = pub_key.to_bytes_be();
    if pub_bytes.len() < 128 {
        let mut padded = vec![0u8; 128 - pub_bytes.len()];
        padded.extend_from_slice(&pub_bytes);
        pub_bytes = padded;
    }

    Ok((pub_bytes, sym_key))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn python_hkdf(shared_secret_128: &[u8]) -> [u8; 16] {
        use hmac::{Hmac, Mac};
        type HmacSha256 = Hmac<Sha256>;

        // Matches secretstorage/libsecret exactly:
        // PRK = HMAC-SHA256(key=b'\x00'*32, data=shared_secret)
        let mut mac = HmacSha256::new_from_slice(&[0u8; 32]).unwrap();
        mac.update(shared_secret_128);
        let prk = mac.finalize().into_bytes();

        // T(1) = HMAC-SHA256(key=PRK, data=b'\x01')
        let mut mac2 = HmacSha256::new_from_slice(&prk).unwrap();
        mac2.update(&[0x01]);
        let t1 = mac2.finalize().into_bytes();

        let mut key = [0u8; 16];
        key.copy_from_slice(&t1[..16]);
        key
    }

    #[test]
    fn hkdf_matches_libsecret_reference() {
        let shared = [0u8; 128]; // trivial test vector

        // Our HKDF (via the hkdf crate).
        let hk = Hkdf::<Sha256>::new(None, &shared);
        let mut ours = [0u8; 16];
        hk.expand(&[], &mut ours).unwrap();

        // Python/libsecret manual HKDF.
        let reference = python_hkdf(&shared);

        assert_eq!(
            ours, reference,
            "HKDF output must match libsecret reference"
        );
    }

    #[test]
    fn full_dh_roundtrip() {
        // Simulate the client side (libsecret).
        let p = BigUint::parse_bytes(DH_P.as_bytes(), 16).unwrap();
        let g = BigUint::from(2u32);

        let client_priv = BigUint::from(12345678901234u64);
        let client_pub = g.modpow(&client_priv, &p);
        let mut client_pub_bytes = client_pub.to_bytes_be();
        if client_pub_bytes.len() < 128 {
            let mut padded = vec![0u8; 128 - client_pub_bytes.len()];
            padded.extend_from_slice(&client_pub_bytes);
            client_pub_bytes = padded;
        }

        // Server processes the client's public key.
        let (server_pub_bytes, server_key) = calculate_dh_shared_secret(&client_pub_bytes).unwrap();

        // Client computes the key from the server's public key.
        let server_pub_bn = BigUint::from_bytes_be(&server_pub_bytes);
        let shared = server_pub_bn.modpow(&client_priv, &p);
        let mut shared_bytes = shared.to_bytes_be();
        if shared_bytes.len() < 128 {
            let mut padded = vec![0u8; 128 - shared_bytes.len()];
            padded.extend_from_slice(&shared_bytes);
            shared_bytes = padded;
        }
        let client_key = python_hkdf(&shared_bytes);

        assert_eq!(
            server_key.as_slice(),
            &client_key,
            "DH keys must match on both sides"
        );
    }
}
