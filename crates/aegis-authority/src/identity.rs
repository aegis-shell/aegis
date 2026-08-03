use crate::ActorCapability;

/// Stable, compositor-issued Actor principal identifier.
///
/// Display labels and credentials are never accepted as principal ids. The
/// bounded ASCII form is safe for logs and persistence keys without making
/// the identifier itself an authentication secret.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize)]
#[serde(transparent)]
pub struct ActorPrincipal(String);

impl ActorPrincipal {
    pub fn new(value: impl Into<String>) -> Result<Self, &'static str> {
        let value = value.into();
        if value.is_empty()
            || value.len() > 128
            || !value.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b':')
            })
        {
            return Err("Actor principal is empty, oversized, or contains unsafe characters");
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ActorPrincipal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::ops::Deref for ActorPrincipal {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}

impl AsRef<str> for ActorPrincipal {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl<'de> serde::Deserialize<'de> for ActorPrincipal {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

/// Durable authorization profile resolved for an authenticated Agent.
///
/// This is distinct from [`crate::ActorBinding`], which binds that principal
/// to one live transport connection for observation and action leases.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentIdentity {
    pub principal: ActorPrincipal,
    pub pregranted: Vec<ActorCapability>,
    pub gated: Vec<ActorCapability>,
}

/// Result of a user-approved Agent pairing.
#[derive(Clone, PartialEq, Eq)]
pub struct PairedAgent {
    pub principal: ActorPrincipal,
    pub credential: String,
    pub pregranted: Vec<ActorCapability>,
    pub gated: Vec<ActorCapability>,
}

impl std::fmt::Debug for PairedAgent {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PairedAgent")
            .field("principal", &self.principal)
            .field("credential", &"[REDACTED]")
            .field("pregranted", &self.pregranted)
            .field("gated", &self.gated)
            .finish()
    }
}

impl Drop for PairedAgent {
    fn drop(&mut self) {
        use zeroize::Zeroize as _;
        self.credential.zeroize();
    }
}
