use std::collections::BTreeMap;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::os::unix::ffi::OsStrExt as _;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use super::{ActorCapability, ActorPrincipal, ActorSessionId};

const MAX_RESOURCE_GRANTS: usize = 4_096;
const MAX_RESOURCE_TTL: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilesystemAccess {
    Read,
    Write,
}

/// Exact resource governed by a dynamic Actor grant.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ActorResource {
    FilesystemPath {
        path: PathBuf,
        access: FilesystemAccess,
    },
    NetworkOrigin {
        scheme: String,
        host: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        port: Option<u16>,
    },
    SecretPrompt {
        purpose: String,
    },
    PaymentRequest {
        payee: String,
        currency: String,
        maximum_minor_units: u64,
    },
}

impl ActorResource {
    pub fn secret_prompt(title: &str, reason: Option<&str>) -> Self {
        let purpose = reason
            .filter(|reason| !reason.trim().is_empty())
            .unwrap_or(title)
            .to_owned();
        Self::SecretPrompt { purpose }
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        match self {
            Self::FilesystemPath { path, .. } => {
                if !path.is_absolute()
                    || path.as_os_str().len() > 4_096
                    || path.as_os_str().as_bytes().contains(&0)
                {
                    return Err("filesystem resource must be a bounded absolute path");
                }
                if path.components().any(|component| {
                    matches!(
                        component,
                        std::path::Component::CurDir | std::path::Component::ParentDir
                    )
                }) {
                    return Err("filesystem resource must be lexically normalized");
                }
                let normalized = path.components().collect::<PathBuf>();
                if normalized.as_os_str().as_bytes() != path.as_os_str().as_bytes() {
                    return Err("filesystem resource must have one canonical lexical spelling");
                }
            }
            Self::NetworkOrigin { scheme, host, .. } => {
                if !matches!(scheme.as_str(), "http" | "https" | "ws" | "wss")
                    || !valid_exact_host(host)
                {
                    return Err("network resource origin is invalid");
                }
            }
            Self::SecretPrompt { purpose } => {
                validate_label(purpose, 256, "secret purpose")?;
            }
            Self::PaymentRequest {
                payee,
                currency,
                maximum_minor_units,
            } => {
                validate_label(payee, 256, "payment payee")?;
                if currency.len() != 3
                    || !currency.bytes().all(|byte| byte.is_ascii_uppercase())
                    || *maximum_minor_units == 0
                {
                    return Err(
                        "payment resource requires a three-letter uppercase currency code and positive limit",
                    );
                }
            }
        }
        Ok(())
    }

    pub fn required_capability(&self) -> ActorCapability {
        match self {
            Self::FilesystemPath {
                access: FilesystemAccess::Read,
                ..
            } => ActorCapability::ReadFile,
            Self::FilesystemPath {
                access: FilesystemAccess::Write,
                ..
            } => ActorCapability::WriteFile,
            Self::NetworkOrigin { .. } => ActorCapability::AccessNetworkOrigin,
            Self::SecretPrompt { .. } => ActorCapability::PromptSecret,
            Self::PaymentRequest { .. } => ActorCapability::RequestPayment,
        }
    }
}

/// Unguessable id of one dynamic, Actor-bound resource grant.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(transparent)]
pub struct ResourceGrantId(pub String);

/// Client-visible resource grant without its authoritative monotonic clock.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ResourceGrant {
    pub id: ResourceGrantId,
    pub session: ActorSessionId,
    pub principal: Option<ActorPrincipal>,
    pub capability: ActorCapability,
    pub resource: ActorResource,
    pub ttl_ms: u64,
    pub uses_remaining: u32,
}

struct ResourceGrantRecord {
    grant: ResourceGrant,
    expires_at: Instant,
}

/// Bounded dynamic resource grants. Static path or network configuration is
/// not a grant; a caller must receive one of these Actor-bound handles before
/// the resource becomes usable.
#[derive(Default)]
pub struct ResourceGrantRegistry {
    grants: BTreeMap<ResourceGrantId, ResourceGrantRecord>,
}

impl ResourceGrantRegistry {
    pub fn issue(
        &mut self,
        session: ActorSessionId,
        principal: Option<ActorPrincipal>,
        capability: ActorCapability,
        resource: ActorResource,
        ttl: Duration,
        uses: u32,
    ) -> Result<ResourceGrant, String> {
        if !session.is_valid() {
            return Err("resource grant session is invalid".into());
        }
        resource.validate().map_err(str::to_owned)?;
        if capability != resource.required_capability() {
            return Err("resource does not match the granted capability".into());
        }
        if ttl < Duration::from_secs(1) || ttl > MAX_RESOURCE_TTL {
            return Err("resource grant ttl is out of range".into());
        }
        if uses == 0 || uses > 1_024 {
            return Err("resource grant use count is out of range".into());
        }
        let now = Instant::now();
        if self.grants.len() >= MAX_RESOURCE_GRANTS {
            return Err("resource grant capacity exhausted".into());
        }
        let id = loop {
            let mut bytes = [0u8; 32];
            getrandom::fill(&mut bytes)
                .map_err(|error| format!("generate resource grant id: {error}"))?;
            let id = ResourceGrantId(hex(&bytes));
            if !self.grants.contains_key(&id) {
                break id;
            }
        };
        let grant = ResourceGrant {
            id: id.clone(),
            session,
            principal,
            capability,
            resource,
            ttl_ms: ttl.as_millis() as u64,
            uses_remaining: uses,
        };
        self.grants.insert(
            id,
            ResourceGrantRecord {
                grant: grant.clone(),
                expires_at: now + ttl,
            },
        );
        Ok(grant)
    }

    pub fn consume(
        &mut self,
        session: ActorSessionId,
        principal: Option<&ActorPrincipal>,
        id: &ResourceGrantId,
        expected: &ActorResource,
    ) -> Result<ResourceGrant, String> {
        expected.validate().map_err(str::to_owned)?;
        let now = Instant::now();
        let record = self
            .grants
            .get_mut(id)
            .ok_or_else(|| "unknown or already consumed resource grant".to_owned())?;
        if record.grant.session != session || record.grant.principal.as_ref() != principal {
            return Err("resource grant belongs to a different Actor session".into());
        }
        if record.expires_at <= now {
            // Expiry is collected explicitly by `expire_due` so the runtime
            // can emit the lifecycle event before forgetting the grant.
            return Err("resource grant expired".into());
        }
        if &record.grant.resource != expected {
            return Err("resource grant does not match the requested resource".into());
        }
        record.grant.uses_remaining -= 1;
        let consumed = record.grant.clone();
        if record.grant.uses_remaining == 0 {
            self.grants.remove(id);
        }
        Ok(consumed)
    }

    pub fn revoke_session(&mut self, session: ActorSessionId) -> Vec<ResourceGrant> {
        let revoked = self
            .grants
            .values()
            .filter_map(|record| (record.grant.session == session).then_some(record.grant.clone()))
            .collect::<Vec<_>>();
        self.grants
            .retain(|_, record| record.grant.session != session);
        revoked
    }

    pub fn revoke(
        &mut self,
        session: ActorSessionId,
        principal: Option<&ActorPrincipal>,
        id: &ResourceGrantId,
    ) -> Result<ResourceGrant, String> {
        let record = self
            .grants
            .get(id)
            .ok_or_else(|| "unknown resource grant".to_owned())?;
        if record.grant.session != session || record.grant.principal.as_ref() != principal {
            return Err("resource grant belongs to a different Actor session".into());
        }
        self.grants
            .remove(id)
            .map(|record| record.grant)
            .ok_or_else(|| "unknown resource grant".to_owned())
    }

    pub fn revoke_principal(&mut self, principal: &ActorPrincipal) -> Vec<ResourceGrant> {
        let revoked = self
            .grants
            .values()
            .filter_map(|record| {
                (record.grant.principal.as_ref() == Some(principal)).then_some(record.grant.clone())
            })
            .collect::<Vec<_>>();
        self.grants
            .retain(|_, record| record.grant.principal.as_ref() != Some(principal));
        revoked
    }

    /// Expire and forget every due grant, returning privacy-safe snapshots
    /// to the runtime so expiry is never a silent lifecycle transition.
    pub fn expire_due(&mut self) -> Vec<ResourceGrant> {
        let now = Instant::now();
        let expired = self
            .grants
            .values()
            .filter_map(|record| (record.expires_at <= now).then_some(record.grant.clone()))
            .collect::<Vec<_>>();
        for grant in &expired {
            self.grants.remove(&grant.id);
        }
        expired
    }
}

fn valid_exact_host(host: &str) -> bool {
    if host.is_empty()
        || host.len() > 253
        || host.bytes().any(|byte| byte.is_ascii_uppercase())
        || host.starts_with('[')
        || host.ends_with(']')
    {
        return false;
    }
    if host.contains(':') {
        return host.parse::<Ipv6Addr>().is_ok();
    }
    if host
        .bytes()
        .all(|byte| byte.is_ascii_digit() || byte == b'.')
    {
        return host.parse::<Ipv4Addr>().is_ok();
    }
    host.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && label
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
            && label
                .as_bytes()
                .first()
                .is_some_and(u8::is_ascii_alphanumeric)
            && label
                .as_bytes()
                .last()
                .is_some_and(u8::is_ascii_alphanumeric)
    })
}

fn validate_label(value: &str, max: usize, _name: &str) -> Result<(), &'static str> {
    if value.trim().is_empty() || value.len() > max || value.chars().any(char::is_control) {
        return Err("resource label is empty, oversized, or contains control characters");
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    let mut value = String::with_capacity(bytes.len() * 2);
    use std::fmt::Write as _;
    for byte in bytes {
        write!(&mut value, "{byte:02x}").expect("writing to String cannot fail");
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resource_grants_are_actor_bound_exact_and_bounded() {
        let mut grants = ResourceGrantRegistry::default();
        let principal = ActorPrincipal::new("prin_a").unwrap();
        let resource = ActorResource::NetworkOrigin {
            scheme: "https".into(),
            host: "amazon.com".into(),
            port: None,
        };
        let grant = grants
            .issue(
                ActorSessionId(7),
                Some(principal.clone()),
                ActorCapability::AccessNetworkOrigin,
                resource.clone(),
                Duration::from_secs(60),
                1,
            )
            .unwrap();
        assert!(
            grants
                .consume(ActorSessionId(8), Some(&principal), &grant.id, &resource)
                .is_err()
        );
        grants
            .consume(ActorSessionId(7), Some(&principal), &grant.id, &resource)
            .unwrap();
        assert!(
            grants
                .consume(ActorSessionId(7), Some(&principal), &grant.id, &resource)
                .is_err()
        );
    }

    #[test]
    fn payment_and_filesystem_resources_validate_fail_closed() {
        assert!(
            ActorResource::FilesystemPath {
                path: "relative".into(),
                access: FilesystemAccess::Read,
            }
            .validate()
            .is_err()
        );
        assert!(
            ActorResource::PaymentRequest {
                payee: "Shop".into(),
                currency: "usd".into(),
                maximum_minor_units: 100,
            }
            .validate()
            .is_err()
        );
        assert!(
            ActorResource::FilesystemPath {
                path: "/tmp//ambiguous".into(),
                access: FilesystemAccess::Read,
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn network_origins_require_unambiguous_dns_or_ip_hosts() {
        for host in ["example.com", "localhost", "127.0.0.1", "2001:db8::1"] {
            ActorResource::NetworkOrigin {
                scheme: "https".into(),
                host: host.into(),
                port: None,
            }
            .validate()
            .unwrap();
        }
        for host in [
            "Example.com",
            ".example.com",
            "example.com.",
            "bad..example",
            "-bad.example",
            "bad-.example",
            "999.1.1.1",
            "[2001:db8::1]",
            "user@example.com",
        ] {
            assert!(
                ActorResource::NetworkOrigin {
                    scheme: "https".into(),
                    host: host.into(),
                    port: None,
                }
                .validate()
                .is_err(),
                "accepted ambiguous host {host}"
            );
        }
    }

    #[test]
    fn due_resource_grants_are_collected_explicitly() {
        let mut grants = ResourceGrantRegistry::default();
        let grant = grants
            .issue(
                ActorSessionId(7),
                None,
                ActorCapability::PromptSecret,
                ActorResource::SecretPrompt {
                    purpose: "sign in".into(),
                },
                Duration::from_secs(60),
                1,
            )
            .unwrap();
        grants.grants.get_mut(&grant.id).unwrap().expires_at = Instant::now();

        assert_eq!(grants.expire_due(), vec![grant.clone()]);
        assert!(
            grants
                .consume(grant.session, None, &grant.id, &grant.resource,)
                .is_err()
        );
    }
}
