//! Personalized profile and portrait content for Tessera shell surfaces.
//!
//! Security principals and authentication remain in `tessera-security` and the
//! lock-screen authentication boundary. This module resolves the local account
//! defaults presented by shell surfaces. With the `persona` feature it also
//! owns the shared still/VRM portrait, motion, and live-reload pipeline.

#[cfg(feature = "persona")]
pub mod portrait;
mod profile;

#[cfg(feature = "persona")]
pub use portrait::{
    AnimationSupport, CameraError, Error, MotionInfo, MotionKind, Portrait, PortraitCandidate,
    PortraitConfig, PortraitKind, PortraitWatcher, VrmCamera, VrmError, WatchError,
};
pub use profile::Profile;
