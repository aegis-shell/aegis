//! Product logic shared by the secure locker host and headless tests.

#![forbid(unsafe_code)]

mod secret;
mod state;
mod ui;

pub use secret::Secret;
pub use state::{AuthResult, LockAction, LockState, PresentationMode};
pub use ui::{
    BsodLayout, BsodTypeScale, LockLayout, LockScreenStyle, bsod_layout, bsod_type_scale,
    lock_layout, lock_layout_for,
};
