//! Wayland extension protocol globals and request handlers.
//!
//! Each extension advertised by the compositor lives here: a bind callback
//! creating the resource, and request vtables. Several are fully functional
//! (xdg-output, fractional-scale, relative-pointer,
//! pointer-constraints, cursor-shape, idle-inhibit, ext-idle-notify,
//! ext-foreign-toplevel-list, xdg-foreign-v2, text-input-v3, input-method-v2, and
//! virtual-keyboard-v1), while others accept requests but defer some
//! compositor-side behavior (ext-session-lock surfaces). Every advertised
//! protocol still owns a complete object lifecycle so clients do not fail
//! with protocol errors. Presentation-time remains unadvertised until the
//! backend can return commit-correlated timestamps.
//!
//! Every global stores `State*` in its resource user-data (or derives it
//! from a bound object), matching the core protocol handlers in lib.rs.

#![allow(non_upper_case_globals, dead_code)]

use std::ffi::{CStr, CString, c_void};
use std::os::raw::c_int;

use crate::{State, SurfaceRec, ffi};

mod activation;
mod cursor_shape;
mod decoration;
mod explicit_sync;
mod foreign_toplevel;
mod fractional_scale;
mod idle;
mod input_method;
mod keyboard_shortcuts;
mod output;
mod pointer_constraints;
mod pointer_gestures;
mod presentation;
mod relative_pointer;
mod session_lock;
mod tablet;
mod text_input;
mod xdg_foreign;

pub(crate) use activation::*;
pub(crate) use cursor_shape::*;
pub(crate) use decoration::*;
pub(crate) use explicit_sync::*;
pub(crate) use foreign_toplevel::*;
pub(crate) use fractional_scale::*;
pub(crate) use idle::*;
pub(crate) use input_method::*;
pub(crate) use keyboard_shortcuts::*;
pub(crate) use output::*;
pub(crate) use pointer_constraints::*;
pub(crate) use pointer_gestures::*;
pub(crate) use relative_pointer::*;
pub(crate) use session_lock::*;
pub(crate) use tablet::*;
pub(crate) use text_input::*;
pub(crate) use xdg_foreign::*;
