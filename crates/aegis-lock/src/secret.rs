//! Short-lived credential storage with explicit memory clearing.

use zeroize::Zeroize;

/// A bounded UTF-8 credential buffer.
///
/// The buffer is never formatted or logged. Clearing uses volatile stores so
/// the compiler cannot discard the overwrite before deallocation.
#[derive(Default)]
pub struct Secret {
    bytes: Vec<u8>,
}

impl Secret {
    pub const MAX_BYTES: usize = 1024;

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    #[must_use]
    pub fn char_count(&self) -> usize {
        std::str::from_utf8(&self.bytes)
            .map(str::chars)
            .map(Iterator::count)
            .unwrap_or_default()
    }

    pub fn push_str(&mut self, text: &str) -> bool {
        if self.bytes.len().saturating_add(text.len()) > Self::MAX_BYTES {
            return false;
        }
        self.bytes.extend_from_slice(text.as_bytes());
        true
    }

    pub fn backspace(&mut self) -> bool {
        let Ok(text) = std::str::from_utf8(&self.bytes) else {
            self.clear();
            return false;
        };
        let Some((index, _)) = text.char_indices().next_back() else {
            return false;
        };
        self.bytes[index..].zeroize();
        self.bytes.truncate(index);
        true
    }

    pub fn clear(&mut self) {
        self.bytes.zeroize();
        self.bytes.clear();
    }

    /// Move the credential into the authentication worker without cloning it.
    #[must_use]
    pub fn take(&mut self) -> Secret {
        std::mem::take(self)
    }

    #[cfg(test)]
    pub(crate) fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Transfer the credential to a C authentication boundary without
    /// cloning it. The caller must overwrite the returned allocation after
    /// the authentication library has finished with it.
    #[must_use]
    pub fn into_nul_terminated(mut self) -> Option<Vec<u8>> {
        if self.bytes.contains(&0) {
            return None;
        }
        self.bytes.push(0);
        Some(std::mem::take(&mut self.bytes))
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        self.clear();
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Secret")
            .field("bytes", &"<redacted>")
            .field("len", &self.bytes.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backspace_removes_one_unicode_scalar() {
        let mut secret = Secret::default();
        assert!(secret.push_str("a密🙂"));
        assert!(secret.backspace());
        assert_eq!(std::str::from_utf8(secret.as_bytes()).unwrap(), "a密");
        assert!(secret.backspace());
        assert_eq!(std::str::from_utf8(secret.as_bytes()).unwrap(), "a");
    }

    #[test]
    fn capacity_is_fail_closed() {
        let mut secret = Secret::default();
        assert!(secret.push_str(&"x".repeat(Secret::MAX_BYTES)));
        assert!(!secret.push_str("y"));
        assert_eq!(secret.len(), Secret::MAX_BYTES);
    }

    #[test]
    fn display_count_uses_unicode_scalars_not_utf8_bytes() {
        let mut secret = Secret::default();
        assert!(secret.push_str("a密🙂"));
        assert_eq!(secret.char_count(), 3);
    }
}
