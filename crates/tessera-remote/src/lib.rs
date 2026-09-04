//! Universal Interaction Protocol (UIP) network gateway and remote seat manager.
//!
//! Provides cryptographic actor authentication, multi-tenant session tracking,
//! dead-man switch fail-safes, and lightweight client SDKs for mobile, embedded,
//! and autonomous agents connecting to Tessera.

pub mod client;
pub mod crypto;
pub mod session;

pub use client::UipClient;
pub use crypto::{AuthToken, PairingChallenge};
pub use session::{RemoteSession, SessionError, SessionLifecycle};
