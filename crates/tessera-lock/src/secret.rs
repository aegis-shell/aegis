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

    /// Compare two credentials without exposing either backing buffer.
    ///
    /// The full length of both values is visited before returning. Production
    /// authentication still delegates comparison to PAM; this helper exists
    /// for deterministic development-preview credentials.
    #[must_use]
    pub fn content_eq(&self, other: &Secret) -> bool {
        let compared_len = self.bytes.len().max(other.bytes.len());
        let mut difference = self.bytes.len() ^ other.bytes.len();
        for index in 0..compared_len {
            let left = self.bytes.get(index).copied().unwrap_or_default();
            let right = other.bytes.get(index).copied().unwrap_or_default();
            difference |= usize::from(left ^ right);
        }
        difference == 0
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

    #[test]
    fn content_comparison_requires_exact_bytes_and_length() {
        let mut expected = Secret::default();
        let mut same = Secret::default();
        let mut prefix = Secret::default();
        let mut other = Secret::default();
        assert!(expected.push_str("00密🙂"));
        assert!(same.push_str("00密🙂"));
        assert!(prefix.push_str("00密"));
        assert!(other.push_str("00密🙃"));

        assert!(expected.content_eq(&same));
        assert!(!expected.content_eq(&prefix));
        assert!(!expected.content_eq(&other));
    }
}
