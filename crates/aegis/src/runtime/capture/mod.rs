//! Frame capture orchestration for the compositor runtime.
//!
//! Capture remains a runtime mechanism rather than an AI-specific crate:
//! geometry, encoding, filesystem publication, and bounded background work
//! have separate internal owners, while IPC policy stays in `ass-ipc`.

mod encoding;
mod geometry;
mod output;
mod worker;

#[cfg(test)]
pub(super) use encoding::encode_rgba_capture;
pub(super) use encoding::{PendingReadback, flux_last_error_detail, read_captured_pixels};
pub(super) use geometry::{clamp_logical_region, logical_rect_to_physical};
pub(super) use output::screenshot_uri_list;
pub(super) use worker::{
    CaptureCompletion, CaptureTarget, CaptureWorker, PendingCapture, StreamPixels,
    queue_captured_pixels, refuse_capture_target,
};
