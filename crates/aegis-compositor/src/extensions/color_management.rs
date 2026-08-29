//! `wp_color_management_v1` (staging, advertised at version 1): clients tag
//! their buffers with an image description (parametric primaries+transfer,
//! or an ICC profile); the compositor decodes tagged content into the
//! working space at sample time via flux image tags (ADR-0001 boundary:
//! conversion lives in flux, this module owns protocol + per-surface state).
//!
//! The compositor renders one shared framebuffer, so the output image
//! description is uniform across outputs and follows the session color
//! pipeline (`State::color_pipeline`): sRGB for SDR, BT.2020 PQ for HDR.

use super::*;

use aegis_model::color::{
    ContentColor, ContentPrimaries, ContentTransfer, CustomPrimaries, Luminances, NamedPrimaries,
    NamedTransfer, ParametricColor,
};
use aegis_model::output::ColorPipeline;

// ----- protocol enum values (color-management-v1) ---------------------------

// wp_color_manager_v1.error
const MANAGER_ERROR_UNSUPPORTED_FEATURE: u32 = 0;
const MANAGER_ERROR_SURFACE_EXISTS: u32 = 1;
// wp_color_management_surface_v1.error
const SURFACE_ERROR_IMAGE_DESCRIPTION: u32 = 1;
const SURFACE_ERROR_INERT: u32 = 2;
// wp_image_description_creator_*_v1.error (shared leading values)
const CREATOR_ERROR_INCOMPLETE_SET: u32 = 0;
const CREATOR_ERROR_ALREADY_SET: u32 = 1;
// icc creator
const ICC_ERROR_BAD_FD: u32 = 2;
const ICC_ERROR_BAD_SIZE: u32 = 3;
const ICC_ERROR_OUT_OF_FILE: u32 = 4;
// params creator
const PARAMS_ERROR_UNSUPPORTED_FEATURE: u32 = 2;
const PARAMS_ERROR_INVALID_TF: u32 = 3;
const PARAMS_ERROR_INVALID_PRIMARIES_NAMED: u32 = 4;
const PARAMS_ERROR_INVALID_LUMINANCE: u32 = 5;
// wp_image_description_v1.error
const DESCRIPTION_ERROR_NOT_READY: u32 = 0;
const DESCRIPTION_ERROR_NO_INFORMATION: u32 = 1;
// wp_image_description_v1.cause
const CAUSE_UNSUPPORTED: u32 = 1;
const CAUSE_OPERATING_SYSTEM: u32 = 2;

// render intents we advertise
const INTENT_PERCEPTUAL: u32 = 0;
const INTENT_RELATIVE: u32 = 1;
// features we advertise
const FEATURE_ICC_V2_V4: u32 = 0;
const FEATURE_PARAMETRIC: u32 = 1;
const FEATURE_SET_PRIMARIES: u32 = 2;
const FEATURE_SET_TF_POWER: u32 = 3;
const FEATURE_SET_LUMINANCES: u32 = 4;
// named transfer functions we advertise (protocol ids)
const TF_EXT_LINEAR: u32 = 5;
const TF_GAMMA22: u32 = 2;
/// `srgb`: the version-1 spelling of the sRGB curve (deprecated since 2).
const TF_SRGB: u32 = 9;
/// `compound_power_2_4`: the since-2 spelling of the same sRGB curve.
const TF_COMPOUND_POWER_2_4: u32 = 14;
const TF_ST2084_PQ: u32 = 11;
const TF_HLG: u32 = 13;
// named primaries we advertise (protocol ids)
const PRIMARIES_SRGB: u32 = 1;
const PRIMARIES_BT2020: u32 = 6;
const PRIMARIES_DISPLAY_P3: u32 = 9;
const PRIMARIES_ADOBE_RGB: u32 = 10;

/// Largest ICC profile accepted from a client fd (16 MiB; real display
/// profiles are a few KiB).
const ICC_MAX_BYTES: u32 = 16 * 1024 * 1024;

// Minimal fd I/O without pulling the libc crate (see crate::libc_close).
unsafe extern "C" {
    fn pread(fd: c_int, buf: *mut c_void, count: usize, offset: i64) -> isize;
    fn fstat(fd: c_int, stat: *mut u8) -> c_int;
    fn memfd_create(name: *const std::ffi::c_char, flags: u32) -> c_int;
    fn write(fd: c_int, buf: *const c_void, count: usize) -> isize;
    fn lseek(fd: c_int, offset: i64, whence: c_int) -> i64;
}
const MFD_CLOEXEC: u32 = 1;
const SEEK_SET: c_int = 0;

/// The record behind a `wp_image_description_v1` resource.
struct ImageDescriptionRec {
    value: ContentColor,
    /// Set when creation-time validation rejected the payload; the client
    /// got `failed` instead of `ready` and `set_image_description` with it
    /// is a protocol error.
    failed: bool,
}

struct ColorSurfaceRec {
    surface: *mut SurfaceRec,
}

struct ColorFeedbackRec {
    surface: *mut SurfaceRec,
}

struct IccCreatorRec {
    state: *mut State,
    bytes: Option<Vec<u8>>,
}

#[derive(Default)]
struct ParamsCreatorRec {
    state: *mut State,
    primaries: Option<ContentPrimaries>,
    transfer: Option<ContentTransfer>,
    luminances: Option<Luminances>,
    max_cll: Option<u32>,
    max_fall: Option<u32>,
}

fn named_transfer(tf: u32) -> Option<ContentTransfer> {
    Some(ContentTransfer::Named(match tf {
        TF_EXT_LINEAR => NamedTransfer::Linear,
        TF_GAMMA22 => NamedTransfer::Gamma22,
        // Both spellings of the sRGB curve: `srgb` (v1 clients) and
        // `compound_power_2_4` (clients built against protocol version 2
        // headers, which may still send it on a v1 binding).
        TF_SRGB | TF_COMPOUND_POWER_2_4 => NamedTransfer::Srgb,
        TF_ST2084_PQ => NamedTransfer::Pq,
        TF_HLG => NamedTransfer::Hlg,
        _ => return None,
    }))
}

fn named_primaries(p: u32) -> Option<NamedPrimaries> {
    Some(match p {
        PRIMARIES_SRGB => NamedPrimaries::Srgb,
        PRIMARIES_BT2020 => NamedPrimaries::Bt2020,
        PRIMARIES_DISPLAY_P3 => NamedPrimaries::DisplayP3,
        PRIMARIES_ADOBE_RGB => NamedPrimaries::AdobeRgb,
        _ => return None,
    })
}

/// The wire id of a named transfer, for the info events. `version` is the
/// bound interface version of the receiving resource: the sRGB curve is
/// spelled `srgb` (9) for version-1 peers and `compound_power_2_4` (14)
/// from version 2 on — a peer must not be sent an enum value its version
/// does not define.
fn transfer_wire_id(tf: NamedTransfer, version: u32) -> u32 {
    match tf {
        NamedTransfer::Linear => TF_EXT_LINEAR,
        NamedTransfer::Gamma22 => TF_GAMMA22,
        NamedTransfer::Srgb if version >= 2 => TF_COMPOUND_POWER_2_4,
        NamedTransfer::Srgb => TF_SRGB,
        NamedTransfer::Pq => TF_ST2084_PQ,
        NamedTransfer::Hlg => TF_HLG,
    }
}

fn primaries_wire_id(p: NamedPrimaries) -> u32 {
    match p {
        NamedPrimaries::Srgb => PRIMARIES_SRGB,
        NamedPrimaries::Bt2020 => PRIMARIES_BT2020,
        NamedPrimaries::DisplayP3 => PRIMARIES_DISPLAY_P3,
        NamedPrimaries::AdobeRgb => PRIMARIES_ADOBE_RGB,
    }
}

/// The image description the outputs currently present (uniform by design:
/// one shared framebuffer, one content encoding).
fn pipeline_description(state: *mut State) -> ContentColor {
    unsafe {
        let pipeline = if state.is_null() {
            ColorPipeline::Sdr
        } else {
            (*state).color_pipeline
        };
        ContentColor::Parametric(pipeline.output_color())
    }
}

/// The identity naming the current pipeline output description record
/// (minted by `Server::set_color_pipeline`; the null-state fallback matches
/// the initial record minted at state construction).
fn pipeline_identity(state: *mut State) -> u32 {
    unsafe {
        if state.is_null() {
            1
        } else {
            (*state).color_pipeline_identity
        }
    }
}

/// A fresh identity for a client-created description record.
fn fresh_identity(state: *mut State) -> u32 {
    unsafe {
        if state.is_null() {
            static FALLBACK: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);
            return FALLBACK
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                .max(1);
        }
        (*state).alloc_color_identity()
    }
}

// ----- wp_color_manager_v1 ---------------------------------------------------

static COLOR_MANAGER_IMPL: ffi::wp_color_manager_v1_interface_impl =
    ffi::wp_color_manager_v1_interface_impl {
        destroy: crate::res_destroy,
        get_output: color_manager_get_output,
        get_surface: color_manager_get_surface,
        get_surface_feedback: color_manager_get_surface_feedback,
        create_icc_creator: color_manager_create_icc_creator,
        create_parametric_creator: color_manager_create_parametric_creator,
        create_windows_scrgb: color_manager_unsupported,
        get_image_description: color_manager_unsupported_with_output,
        create_windows_bt2100: color_manager_unsupported,
    };

pub(crate) unsafe extern "C" fn color_manager_bind(
    client: *mut ffi::wl_client,
    data: *mut c_void,
    version: u32,
    id: u32,
) {
    unsafe {
        // Advertised at v1: the later-version vtable slots post
        // unsupported_feature errors (see the stubs).
        let res = ffi::wl_resource_create(
            client,
            &ffi::wp_color_manager_v1_interface,
            version.min(1) as c_int,
            id,
        );
        if res.is_null() {
            return;
        }
        ffi::wl_resource_set_implementation(
            res,
            &COLOR_MANAGER_IMPL as *const _ as *const c_void,
            data,
            None,
        );
        // The capability burst: intents, features, named TFs/primaries, done.
        for intent in [INTENT_PERCEPTUAL, INTENT_RELATIVE] {
            ffi::wl_resource_post_event(res, ffi::WP_COLOR_MANAGER_V1_SUPPORTED_INTENT, intent);
        }
        for feature in [
            FEATURE_ICC_V2_V4,
            FEATURE_PARAMETRIC,
            FEATURE_SET_PRIMARIES,
            FEATURE_SET_TF_POWER,
            FEATURE_SET_LUMINANCES,
        ] {
            ffi::wl_resource_post_event(res, ffi::WP_COLOR_MANAGER_V1_SUPPORTED_FEATURE, feature);
        }
        // The sRGB curve's wire id moved in interface version 2; advertise
        // only the spelling the bound version defines (the protocol forbids
        // advertising ids the peer's version does not know).
        let srgb_tf = if ffi::wl_resource_get_version(res) >= 2 {
            TF_COMPOUND_POWER_2_4
        } else {
            TF_SRGB
        };
        for tf in [TF_EXT_LINEAR, TF_GAMMA22, srgb_tf, TF_ST2084_PQ, TF_HLG] {
            ffi::wl_resource_post_event(res, ffi::WP_COLOR_MANAGER_V1_SUPPORTED_TF_NAMED, tf);
        }
        for primaries in [
            PRIMARIES_SRGB,
            PRIMARIES_BT2020,
            PRIMARIES_DISPLAY_P3,
            PRIMARIES_ADOBE_RGB,
        ] {
            ffi::wl_resource_post_event(
                res,
                ffi::WP_COLOR_MANAGER_V1_SUPPORTED_PRIMARIES_NAMED,
                primaries,
            );
        }
        ffi::wl_resource_post_event(res, ffi::WP_COLOR_MANAGER_V1_DONE);
    }
}

/// Stubs for requests whose features the bind burst does not advertise
/// (windows_scrgb / windows_bt2100 / the v2 get_image_description).
unsafe extern "C" fn color_manager_unsupported(
    _client: *mut ffi::wl_client,
    mgr: *mut ffi::wl_resource,
    _id: u32,
) {
    unsafe {
        ffi::wl_resource_post_error(
            mgr,
            MANAGER_ERROR_UNSUPPORTED_FEATURE,
            c"wp_color_manager_v1: feature not supported by this compositor".as_ptr(),
        );
    }
}

unsafe extern "C" fn color_manager_unsupported_with_output(
    _client: *mut ffi::wl_client,
    mgr: *mut ffi::wl_resource,
    _id: u32,
    _output: *mut ffi::wl_resource,
) {
    unsafe {
        ffi::wl_resource_post_error(
            mgr,
            MANAGER_ERROR_UNSUPPORTED_FEATURE,
            c"wp_color_manager_v1: get_image_description needs interface version 2".as_ptr(),
        );
    }
}

// ----- wp_color_management_output_v1 ------------------------------------------

static COLOR_OUTPUT_IMPL: ffi::wp_color_management_output_v1_interface_impl =
    ffi::wp_color_management_output_v1_interface_impl {
        destroy: color_output_destroy,
        get_image_description: color_output_get_image_description,
    };

unsafe extern "C" fn color_manager_get_output(
    client: *mut ffi::wl_client,
    mgr: *mut ffi::wl_resource,
    id: u32,
    _output: *mut ffi::wl_resource,
) {
    unsafe {
        let state = ffi::wl_resource_get_user_data(mgr) as *mut State;
        let ver = ffi::wl_resource_get_version(mgr);
        let res = ffi::wl_resource_create(
            client,
            &ffi::wp_color_management_output_v1_interface,
            ver,
            id,
        );
        if res.is_null() {
            return;
        }
        ffi::wl_resource_set_implementation(
            res,
            &COLOR_OUTPUT_IMPL as *const _ as *const c_void,
            state as *mut c_void,
            Some(color_output_resource_destroy),
        );
        if !state.is_null() {
            (*state).color_management_outputs.push(res);
        }
    }
}

unsafe extern "C" fn color_output_destroy(
    _client: *mut ffi::wl_client,
    res: *mut ffi::wl_resource,
) {
    unsafe {
        ffi::wl_resource_destroy(res);
    }
}

unsafe extern "C" fn color_output_resource_destroy(res: *mut ffi::wl_resource) {
    unsafe {
        let state = ffi::wl_resource_get_user_data(res) as *mut State;
        if !state.is_null() {
            (*state)
                .color_management_outputs
                .retain(|&entry| entry != res);
        }
    }
}

unsafe extern "C" fn color_output_get_image_description(
    client: *mut ffi::wl_client,
    res: *mut ffi::wl_resource,
    id: u32,
) {
    unsafe {
        let state = ffi::wl_resource_get_user_data(res) as *mut State;
        create_image_description_resource(
            client,
            ffi::wl_resource_get_version(res) as u32,
            id,
            pipeline_description(state),
            true,
            pipeline_identity(state),
        );
    }
}

// ----- wp_color_management_surface_v1 -----------------------------------------

static COLOR_SURFACE_IMPL: ffi::wp_color_management_surface_v1_interface_impl =
    ffi::wp_color_management_surface_v1_interface_impl {
        destroy: color_surface_destroy,
        set_image_description: color_surface_set_image_description,
        unset_image_description: color_surface_unset_image_description,
    };

unsafe extern "C" fn color_manager_get_surface(
    client: *mut ffi::wl_client,
    mgr: *mut ffi::wl_resource,
    id: u32,
    surface: *mut ffi::wl_resource,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(surface) as *mut SurfaceRec;
        if rec.is_null() {
            return;
        }
        if !(*rec).color_management.is_null() {
            ffi::wl_resource_post_error(
                mgr,
                MANAGER_ERROR_SURFACE_EXISTS,
                c"wl_surface already has a color-management object".as_ptr(),
            );
            return;
        }
        let ver = ffi::wl_resource_get_version(mgr);
        let res = ffi::wl_resource_create(
            client,
            &ffi::wp_color_management_surface_v1_interface,
            ver,
            id,
        );
        if res.is_null() {
            return;
        }
        let color_rec = Box::into_raw(Box::new(ColorSurfaceRec { surface: rec }));
        ffi::wl_resource_set_implementation(
            res,
            &COLOR_SURFACE_IMPL as *const _ as *const c_void,
            color_rec as *mut c_void,
            Some(color_surface_resource_destroy),
        );
        (*rec).color_management = res;
    }
}

unsafe extern "C" fn color_surface_destroy(
    _client: *mut ffi::wl_client,
    res: *mut ffi::wl_resource,
) {
    unsafe {
        // Destroying the object unsets the description (spec).
        let color = ffi::wl_resource_get_user_data(res) as *mut ColorSurfaceRec;
        if !color.is_null() && !(*color).surface.is_null() {
            (*(*color).surface).pending_image_description = Some(None);
        }
        ffi::wl_resource_destroy(res);
    }
}

unsafe extern "C" fn color_surface_resource_destroy(res: *mut ffi::wl_resource) {
    unsafe {
        let color = ffi::wl_resource_get_user_data(res) as *mut ColorSurfaceRec;
        if color.is_null() {
            return;
        }
        if !(*color).surface.is_null() && (*(*color).surface).color_management == res {
            (*(*color).surface).color_management = std::ptr::null_mut();
        }
        drop(Box::from_raw(color));
    }
}

/// Called from the surface destroy path: the protocol object goes inert.
pub(crate) unsafe fn color_management_surface_destroyed(surface: *mut SurfaceRec) {
    unsafe {
        for res in [(*surface).color_management, (*surface).color_feedback] {
            if res.is_null() {
                continue;
            }
            let rec = ffi::wl_resource_get_user_data(res) as *mut ColorSurfaceRec;
            if !rec.is_null() {
                (*rec).surface = std::ptr::null_mut();
            }
        }
        (*surface).color_management = std::ptr::null_mut();
        (*surface).color_feedback = std::ptr::null_mut();
    }
}

unsafe extern "C" fn color_surface_set_image_description(
    _client: *mut ffi::wl_client,
    res: *mut ffi::wl_resource,
    image_description: *mut ffi::wl_resource,
    render_intent: u32,
) {
    unsafe {
        let color = ffi::wl_resource_get_user_data(res) as *mut ColorSurfaceRec;
        if color.is_null() || (*color).surface.is_null() {
            ffi::wl_resource_post_error(
                res,
                SURFACE_ERROR_INERT,
                c"wp_color_management_surface_v1: surface is gone".as_ptr(),
            );
            return;
        }
        if render_intent != INTENT_PERCEPTUAL && render_intent != INTENT_RELATIVE {
            ffi::wl_resource_post_error(
                res,
                0, // surface.error.render_intent
                c"wp_color_management_surface_v1: unsupported render intent".as_ptr(),
            );
            return;
        }
        let desc = ffi::wl_resource_get_user_data(image_description) as *mut ImageDescriptionRec;
        if desc.is_null() || (*desc).failed {
            ffi::wl_resource_post_error(
                res,
                SURFACE_ERROR_IMAGE_DESCRIPTION,
                c"wp_color_management_surface_v1: invalid image description".as_ptr(),
            );
            return;
        }
        (*(*color).surface).pending_image_description = Some(Some((*desc).value.clone()));
    }
}

unsafe extern "C" fn color_surface_unset_image_description(
    _client: *mut ffi::wl_client,
    res: *mut ffi::wl_resource,
) {
    unsafe {
        let color = ffi::wl_resource_get_user_data(res) as *mut ColorSurfaceRec;
        if !color.is_null() && !(*color).surface.is_null() {
            (*(*color).surface).pending_image_description = Some(None);
        }
    }
}

// ----- wp_color_management_surface_feedback_v1 ---------------------------------

static COLOR_FEEDBACK_IMPL: ffi::wp_color_management_surface_feedback_v1_interface_impl =
    ffi::wp_color_management_surface_feedback_v1_interface_impl {
        destroy: color_feedback_destroy,
        get_preferred: color_feedback_get_preferred,
        get_preferred_parametric: color_feedback_get_preferred,
    };

unsafe extern "C" fn color_manager_get_surface_feedback(
    client: *mut ffi::wl_client,
    mgr: *mut ffi::wl_resource,
    id: u32,
    surface: *mut ffi::wl_resource,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(surface) as *mut SurfaceRec;
        if rec.is_null() {
            return;
        }
        if !(*rec).color_feedback.is_null() {
            ffi::wl_resource_post_error(
                mgr,
                MANAGER_ERROR_SURFACE_EXISTS,
                c"wl_surface already has a color-management feedback object".as_ptr(),
            );
            return;
        }
        let ver = ffi::wl_resource_get_version(mgr);
        let res = ffi::wl_resource_create(
            client,
            &ffi::wp_color_management_surface_feedback_v1_interface,
            ver,
            id,
        );
        if res.is_null() {
            return;
        }
        let feedback_rec = Box::into_raw(Box::new(ColorFeedbackRec { surface: rec }));
        ffi::wl_resource_set_implementation(
            res,
            &COLOR_FEEDBACK_IMPL as *const _ as *const c_void,
            feedback_rec as *mut c_void,
            Some(color_feedback_resource_destroy),
        );
        (*rec).color_feedback = res;
        // The initial hint names the current pipeline description record so
        // the client can de-duplicate it against a description it already
        // holds (0 is the reserved invalid id and must not be sent).
        let state = ffi::wl_resource_get_user_data(mgr) as *mut State;
        ffi::wl_resource_post_event(
            res,
            ffi::WP_COLOR_MANAGEMENT_SURFACE_FEEDBACK_V1_PREFERRED_CHANGED,
            pipeline_identity(state),
        );
    }
}

unsafe extern "C" fn color_feedback_destroy(
    _client: *mut ffi::wl_client,
    res: *mut ffi::wl_resource,
) {
    unsafe {
        ffi::wl_resource_destroy(res);
    }
}

unsafe extern "C" fn color_feedback_resource_destroy(res: *mut ffi::wl_resource) {
    unsafe {
        let feedback = ffi::wl_resource_get_user_data(res) as *mut ColorFeedbackRec;
        if feedback.is_null() {
            return;
        }
        if !(*feedback).surface.is_null() && (*(*feedback).surface).color_feedback == res {
            (*(*feedback).surface).color_feedback = std::ptr::null_mut();
        }
        drop(Box::from_raw(feedback));
    }
}

unsafe extern "C" fn color_feedback_get_preferred(
    client: *mut ffi::wl_client,
    res: *mut ffi::wl_resource,
    id: u32,
) {
    unsafe {
        let feedback = ffi::wl_resource_get_user_data(res) as *mut ColorFeedbackRec;
        if feedback.is_null() || (*feedback).surface.is_null() {
            ffi::wl_resource_post_error(
                res,
                0, // feedback.error.inert
                c"wp_color_management_surface_feedback_v1: surface is gone".as_ptr(),
            );
            return;
        }
        let state = (*(*feedback).surface).state;
        // The preferred description is what the framebuffer is written in:
        // matching it exactly skips every conversion.
        create_image_description_resource(
            client,
            ffi::wl_resource_get_version(res) as u32,
            id,
            pipeline_description(state),
            true,
            pipeline_identity(state),
        );
    }
}

/// Broadcast `preferred_changed` to every live feedback resource and
/// `image_description_changed` to every color-output resource. Called when
/// the session color pipeline changes (HDR toggles, hotplug renegotiation).
pub(crate) unsafe fn resend_color_pipeline(state: *mut State) {
    unsafe {
        if state.is_null() {
            return;
        }
        for &res in &(*state).color_management_outputs {
            ffi::wl_resource_post_event(
                res,
                ffi::WP_COLOR_MANAGEMENT_OUTPUT_V1_IMAGE_DESCRIPTION_CHANGED,
            );
        }
        for p in (*state).live_surfaces_pub() {
            let feedback = (*p).color_feedback;
            if !feedback.is_null() {
                ffi::wl_resource_post_event(
                    feedback,
                    ffi::WP_COLOR_MANAGEMENT_SURFACE_FEEDBACK_V1_PREFERRED_CHANGED,
                    (*state).color_pipeline_identity,
                );
            }
        }
    }
}

// ----- wp_image_description_v1 -------------------------------------------------

static IMAGE_DESCRIPTION_IMPL: ffi::wp_image_description_v1_interface_impl =
    ffi::wp_image_description_v1_interface_impl {
        destroy: image_description_destroy,
        get_information: image_description_get_information,
    };

/// Create an image-description resource holding `value`, then immediately
/// deliver `ready` with `identity` (or `failed` when `valid` is false).
unsafe fn create_image_description_resource(
    client: *mut ffi::wl_client,
    version: u32,
    id: u32,
    value: ContentColor,
    valid: bool,
    identity: u32,
) {
    unsafe {
        let res = ffi::wl_resource_create(
            client,
            &ffi::wp_image_description_v1_interface,
            version as c_int,
            id,
        );
        if res.is_null() {
            return;
        }
        let rec = Box::into_raw(Box::new(ImageDescriptionRec {
            value,
            failed: !valid,
        }));
        ffi::wl_resource_set_implementation(
            res,
            &IMAGE_DESCRIPTION_IMPL as *const _ as *const c_void,
            rec as *mut c_void,
            Some(image_description_resource_destroy),
        );
        if valid {
            // `ready` carries the record identity; 0 is the reserved
            // invalid id and a garbage vararg here is UB.
            ffi::wl_resource_post_event(res, ffi::WP_IMAGE_DESCRIPTION_V1_READY, identity);
        } else {
            // `failed` is (cause, msg): both arguments are mandatory.
            ffi::wl_resource_post_event(
                res,
                ffi::WP_IMAGE_DESCRIPTION_V1_FAILED,
                CAUSE_UNSUPPORTED,
                c"image description validation failed".as_ptr(),
            );
        }
    }
}

unsafe extern "C" fn image_description_destroy(
    _client: *mut ffi::wl_client,
    res: *mut ffi::wl_resource,
) {
    unsafe {
        ffi::wl_resource_destroy(res);
    }
}

unsafe extern "C" fn image_description_resource_destroy(res: *mut ffi::wl_resource) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(res) as *mut ImageDescriptionRec;
        if !rec.is_null() {
            drop(Box::from_raw(rec));
        }
    }
}

unsafe extern "C" fn image_description_get_information(
    client: *mut ffi::wl_client,
    res: *mut ffi::wl_resource,
    id: u32,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(res) as *mut ImageDescriptionRec;
        if rec.is_null() {
            return;
        }
        if (*rec).failed {
            ffi::wl_resource_post_error(
                res,
                DESCRIPTION_ERROR_NOT_READY,
                c"wp_image_description_v1: description is not ready".as_ptr(),
            );
            return;
        }
        let info = ffi::wl_resource_create(
            client,
            &ffi::wp_image_description_info_v1_interface,
            ffi::wl_resource_get_version(res),
            id,
        );
        if info.is_null() {
            return;
        }
        send_image_description_info(info, &(*rec).value);
    }
}

/// Stream the description's fields as info events, then `done` (which
/// destroys the info object per the protocol).
unsafe fn send_image_description_info(info: *mut ffi::wl_resource, value: &ContentColor) {
    unsafe {
        match value {
            ContentColor::Parametric(parametric) => {
                send_parametric_info(info, parametric);
            }
            ContentColor::Icc(bytes) => {
                send_icc_info_fd(info, bytes);
            }
        }
        ffi::wl_resource_post_event(info, ffi::WP_IMAGE_DESCRIPTION_INFO_V1_DONE);
        ffi::wl_resource_destroy(info);
    }
}

fn send_parametric_info(info: *mut ffi::wl_resource, value: &ParametricColor) {
    unsafe {
        // The info resource inherits the bound interface version; pick the
        // transfer-function spelling that version defines.
        let version = ffi::wl_resource_get_version(info) as u32;
        match &value.primaries {
            ContentPrimaries::Named(named) => {
                ffi::wl_resource_post_event(
                    info,
                    ffi::WP_IMAGE_DESCRIPTION_INFO_V1_PRIMARIES_NAMED,
                    primaries_wire_id(*named),
                );
            }
            ContentPrimaries::Custom(xy) => {
                let scale = |v: f32| (v * 1_000_000.0).round() as i32;
                ffi::wl_resource_post_event(
                    info,
                    ffi::WP_IMAGE_DESCRIPTION_INFO_V1_PRIMARIES,
                    scale(xy.rx),
                    scale(xy.ry),
                    scale(xy.gx),
                    scale(xy.gy),
                    scale(xy.bx),
                    scale(xy.by),
                    scale(xy.wx),
                    scale(xy.wy),
                );
            }
        }
        match &value.transfer {
            ContentTransfer::Named(named) => {
                ffi::wl_resource_post_event(
                    info,
                    ffi::WP_IMAGE_DESCRIPTION_INFO_V1_TF_NAMED,
                    transfer_wire_id(*named, version),
                );
            }
            ContentTransfer::Gamma(gamma) => {
                ffi::wl_resource_post_event(
                    info,
                    ffi::WP_IMAGE_DESCRIPTION_INFO_V1_TF_POWER,
                    (gamma * 10_000.0).round() as u32,
                );
            }
        }
        // The luminances event is what lets clients anchor absolute levels
        // (SDR vs HDR detection); omitting it leaves them with a 0/0
        // reference and garbage headroom math.
        if let Some(lum) = value.luminances {
            ffi::wl_resource_post_event(
                info,
                ffi::WP_IMAGE_DESCRIPTION_INFO_V1_LUMINANCES,
                (lum.min * 10_000.0).round() as u32,
                lum.max.round() as u32,
                lum.reference.round() as u32,
            );
            ffi::wl_resource_post_event(
                info,
                ffi::WP_IMAGE_DESCRIPTION_INFO_V1_TARGET_LUMINANCE,
                (lum.min * 10_000.0).round() as u32,
                lum.max.round() as u32,
            );
        }
        let (rx, ry, gx, gy, bx, by, wx, wy) = match &value.primaries {
            ContentPrimaries::Named(NamedPrimaries::Srgb) => {
                (0.640, 0.330, 0.300, 0.600, 0.150, 0.060, 0.3127, 0.3290)
            }
            ContentPrimaries::Named(NamedPrimaries::Bt2020) => {
                (0.708, 0.292, 0.170, 0.797, 0.131, 0.046, 0.3127, 0.3290)
            }
            ContentPrimaries::Named(NamedPrimaries::DisplayP3) => {
                (0.680, 0.320, 0.265, 0.690, 0.150, 0.060, 0.3127, 0.3290)
            }
            ContentPrimaries::Named(NamedPrimaries::AdobeRgb) => {
                (0.640, 0.330, 0.210, 0.710, 0.150, 0.060, 0.3127, 0.3290)
            }
            ContentPrimaries::Custom(xy) => {
                (xy.rx, xy.ry, xy.gx, xy.gy, xy.bx, xy.by, xy.wx, xy.wy)
            }
        };
        let scale = |v: f32| (v * 1_000_000.0).round() as i32;
        ffi::wl_resource_post_event(
            info,
            ffi::WP_IMAGE_DESCRIPTION_INFO_V1_TARGET_PRIMARIES,
            scale(rx),
            scale(ry),
            scale(gx),
            scale(gy),
            scale(bx),
            scale(by),
            scale(wx),
            scale(wy),
        );
        if let Some(max_cll) = value.max_cll {
            ffi::wl_resource_post_event(
                info,
                ffi::WP_IMAGE_DESCRIPTION_INFO_V1_TARGET_MAX_CLL,
                max_cll,
            );
        }
        if let Some(max_fall) = value.max_fall {
            ffi::wl_resource_post_event(
                info,
                ffi::WP_IMAGE_DESCRIPTION_INFO_V1_TARGET_MAX_FALL,
                max_fall,
            );
        }
    }
}

/// The `icc_file` info event carries the profile as a memfd.
unsafe fn send_icc_info_fd(info: *mut ffi::wl_resource, bytes: &[u8]) {
    unsafe {
        let fd = memfd_create(c"aegis-icc".as_ptr(), MFD_CLOEXEC);
        if fd < 0 {
            return;
        }
        let mut written = 0usize;
        while written < bytes.len() {
            let n = write(
                fd,
                bytes[written..].as_ptr() as *const c_void,
                bytes.len() - written,
            );
            if n <= 0 {
                crate::libc_close(fd);
                return;
            }
            written += n as usize;
        }
        lseek(fd, 0, SEEK_SET);
        ffi::wl_resource_post_event(info, ffi::WP_IMAGE_DESCRIPTION_INFO_V1_ICC_FILE, fd);
        crate::libc_close(fd); // the receiver owns its copy via the socket
    }
}

// ----- wp_image_description_creator_icc_v1 --------------------------------------

static ICC_CREATOR_IMPL: ffi::wp_image_description_creator_icc_v1_interface_impl =
    ffi::wp_image_description_creator_icc_v1_interface_impl {
        create: icc_creator_create,
        set_icc_file: icc_creator_set_icc_file,
    };

unsafe extern "C" fn color_manager_create_icc_creator(
    client: *mut ffi::wl_client,
    mgr: *mut ffi::wl_resource,
    id: u32,
) {
    unsafe {
        let ver = ffi::wl_resource_get_version(mgr);
        let res = ffi::wl_resource_create(
            client,
            &ffi::wp_image_description_creator_icc_v1_interface,
            ver,
            id,
        );
        if res.is_null() {
            return;
        }
        let rec = Box::into_raw(Box::new(IccCreatorRec {
            state: ffi::wl_resource_get_user_data(mgr) as *mut State,
            bytes: None,
        }));
        ffi::wl_resource_set_implementation(
            res,
            &ICC_CREATOR_IMPL as *const _ as *const c_void,
            rec as *mut c_void,
            Some(icc_creator_resource_destroy),
        );
    }
}

unsafe extern "C" fn icc_creator_resource_destroy(res: *mut ffi::wl_resource) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(res) as *mut IccCreatorRec;
        if !rec.is_null() {
            drop(Box::from_raw(rec));
        }
    }
}

unsafe extern "C" fn icc_creator_set_icc_file(
    _client: *mut ffi::wl_client,
    res: *mut ffi::wl_resource,
    fd: i32,
    offset: u32,
    length: u32,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(res) as *mut IccCreatorRec;
        if rec.is_null() {
            return;
        }
        if (*rec).bytes.is_some() {
            ffi::wl_resource_post_error(
                res,
                CREATOR_ERROR_ALREADY_SET,
                c"icc creator: ICC data already set".as_ptr(),
            );
            return;
        }
        if !(132..=ICC_MAX_BYTES).contains(&length) {
            ffi::wl_resource_post_error(
                res,
                ICC_ERROR_BAD_SIZE,
                c"icc creator: bad ICC data length".as_ptr(),
            );
            return;
        }
        // Bound offset+length against the file size.
        let mut stat = [0u8; 144]; // struct stat is 144 bytes on x86_64/most targets
        if fstat(fd, stat.as_mut_ptr()) != 0 {
            ffi::wl_resource_post_error(res, ICC_ERROR_BAD_FD, c"icc creator: bad fd".as_ptr());
            return;
        }
        let file_size = i64::from_ne_bytes(stat[48..56].try_into().unwrap_or([0; 8])); // st_size
        if (offset as i64) < 0 || (offset as i64) + (length as i64) > file_size {
            ffi::wl_resource_post_error(
                res,
                ICC_ERROR_OUT_OF_FILE,
                c"icc creator: offset + length exceeds file size".as_ptr(),
            );
            return;
        }
        let mut bytes = vec![0u8; length as usize];
        let mut read = 0usize;
        while read < bytes.len() {
            let n = pread(
                fd,
                bytes[read..].as_mut_ptr() as *mut c_void,
                bytes.len() - read,
                (offset as i64) + read as i64,
            );
            if n <= 0 {
                ffi::wl_resource_post_error(
                    res,
                    ICC_ERROR_BAD_FD,
                    c"icc creator: fd not seekable and readable".as_ptr(),
                );
                return;
            }
            read += n as usize;
        }
        (*rec).bytes = Some(bytes);
    }
}

unsafe extern "C" fn icc_creator_create(
    client: *mut ffi::wl_client,
    res: *mut ffi::wl_resource,
    id: u32,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(res) as *mut IccCreatorRec;
        if rec.is_null() {
            return;
        }
        let Some(bytes) = (*rec).bytes.take() else {
            ffi::wl_resource_post_error(
                res,
                CREATOR_ERROR_INCOMPLETE_SET,
                c"icc creator: no ICC data set".as_ptr(),
            );
            return;
        };
        // Header-level validation (acsp signature, RGB display/scanner
        // class); the renderer's flux ICC parser is the final authority.
        let valid = bytes.len() >= 132
            && &bytes[36..40] == b"acsp"
            && matches!(&bytes[12..16], b"mntr" | b"scnr")
            && &bytes[16..20] == b"RGB ";
        create_image_description_resource(
            client,
            ffi::wl_resource_get_version(res) as u32,
            id,
            ContentColor::Icc(bytes.into()),
            valid,
            fresh_identity((*rec).state),
        );
        ffi::wl_resource_destroy(res);
    }
}

// ----- wp_image_description_creator_params_v1 -----------------------------------

static PARAMS_CREATOR_IMPL: ffi::wp_image_description_creator_params_v1_interface_impl =
    ffi::wp_image_description_creator_params_v1_interface_impl {
        create: params_creator_create,
        set_tf_named: params_creator_set_tf_named,
        set_tf_power: params_creator_set_tf_power,
        set_primaries_named: params_creator_set_primaries_named,
        set_primaries: params_creator_set_primaries,
        set_luminances: params_creator_set_luminances,
        set_mastering_display_primaries: params_creator_unsupported8,
        set_mastering_luminance: params_creator_unsupported2,
        set_max_cll: params_creator_set_max_cll,
        set_max_fall: params_creator_set_max_fall,
    };

unsafe extern "C" fn color_manager_create_parametric_creator(
    client: *mut ffi::wl_client,
    mgr: *mut ffi::wl_resource,
    id: u32,
) {
    unsafe {
        let ver = ffi::wl_resource_get_version(mgr);
        let res = ffi::wl_resource_create(
            client,
            &ffi::wp_image_description_creator_params_v1_interface,
            ver,
            id,
        );
        if res.is_null() {
            return;
        }
        let rec = Box::into_raw(Box::new(ParamsCreatorRec {
            state: ffi::wl_resource_get_user_data(mgr) as *mut State,
            ..ParamsCreatorRec::default()
        }));
        ffi::wl_resource_set_implementation(
            res,
            &PARAMS_CREATOR_IMPL as *const _ as *const c_void,
            rec as *mut c_void,
            Some(params_creator_resource_destroy),
        );
    }
}

unsafe extern "C" fn params_creator_resource_destroy(res: *mut ffi::wl_resource) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(res) as *mut ParamsCreatorRec;
        if !rec.is_null() {
            drop(Box::from_raw(rec));
        }
    }
}

// `set_max_cll` / `set_max_fall` carry optional CTA-861 content metadata.
// They are *not* gated on any feature — the spec makes them always
// accepted — so a protocol error here would kill every HDR client (mpv
// calls them unconditionally) for no reason. Remember the values; the
// renderer does not consume them yet.
unsafe extern "C" fn params_creator_set_max_cll(
    _c: *mut ffi::wl_client,
    r: *mut ffi::wl_resource,
    max_cll: u32,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(r) as *mut ParamsCreatorRec;
        if !rec.is_null() {
            (*rec).max_cll = Some(max_cll);
        }
    }
}
unsafe extern "C" fn params_creator_set_max_fall(
    _c: *mut ffi::wl_client,
    r: *mut ffi::wl_resource,
    max_fall: u32,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(r) as *mut ParamsCreatorRec;
        if !rec.is_null() {
            (*rec).max_fall = Some(max_fall);
        }
    }
}

/// `set_luminances` (advertised feature `set_luminances`): wire units are
/// min x 10000, max/reference in cd/m².
unsafe extern "C" fn params_creator_set_luminances(
    _c: *mut ffi::wl_client,
    r: *mut ffi::wl_resource,
    min_lum: u32,
    max_lum: u32,
    reference_lum: u32,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(r) as *mut ParamsCreatorRec;
        if rec.is_null() {
            return;
        }
        if (*rec).luminances.is_some() {
            ffi::wl_resource_post_error(
                r,
                CREATOR_ERROR_ALREADY_SET,
                c"params creator: luminances already set".as_ptr(),
            );
            return;
        }
        let luminances = Luminances {
            min: min_lum as f32 / 10_000.0,
            max: max_lum as f32,
            reference: reference_lum as f32,
        };
        if !(luminances.max > 0.0 && luminances.reference > 0.0 && luminances.min <= luminances.max)
        {
            ffi::wl_resource_post_error(
                r,
                PARAMS_ERROR_INVALID_LUMINANCE,
                c"params creator: implausible luminances".as_ptr(),
            );
            return;
        }
        (*rec).luminances = Some(luminances);
    }
}

// The mastering-display setters stay outside the advertised feature set
// (the renderer cannot consume mastering metadata yet); the spec answer
// for those is unsupported_feature.
unsafe extern "C" fn params_creator_unsupported2(
    _c: *mut ffi::wl_client,
    r: *mut ffi::wl_resource,
    _a: u32,
    _b: u32,
) {
    unsafe {
        ffi::wl_resource_post_error(
            r,
            PARAMS_ERROR_UNSUPPORTED_FEATURE,
            c"params creator: mastering-display metadata is not supported".as_ptr(),
        );
    }
}
#[allow(clippy::too_many_arguments)]
unsafe extern "C" fn params_creator_unsupported8(
    _c: *mut ffi::wl_client,
    r: *mut ffi::wl_resource,
    _a: i32,
    _b: i32,
    _d: i32,
    _e: i32,
    _f: i32,
    _g: i32,
    _h: i32,
    _i: i32,
) {
    unsafe {
        ffi::wl_resource_post_error(
            r,
            PARAMS_ERROR_UNSUPPORTED_FEATURE,
            c"params creator: mastering-display metadata is not supported".as_ptr(),
        );
    }
}

unsafe extern "C" fn params_creator_set_tf_named(
    _client: *mut ffi::wl_client,
    res: *mut ffi::wl_resource,
    tf: u32,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(res) as *mut ParamsCreatorRec;
        if rec.is_null() {
            return;
        }
        if (*rec).transfer.is_some() {
            ffi::wl_resource_post_error(
                res,
                CREATOR_ERROR_ALREADY_SET,
                c"params creator: transfer already set".as_ptr(),
            );
            return;
        }
        let Some(tf) = named_transfer(tf) else {
            ffi::wl_resource_post_error(
                res,
                PARAMS_ERROR_INVALID_TF,
                c"params creator: unsupported transfer function".as_ptr(),
            );
            return;
        };
        (*rec).transfer = Some(tf);
    }
}

unsafe extern "C" fn params_creator_set_tf_power(
    _client: *mut ffi::wl_client,
    res: *mut ffi::wl_resource,
    eexp: u32,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(res) as *mut ParamsCreatorRec;
        if rec.is_null() {
            return;
        }
        if (*rec).transfer.is_some() {
            ffi::wl_resource_post_error(
                res,
                CREATOR_ERROR_ALREADY_SET,
                c"params creator: transfer already set".as_ptr(),
            );
            return;
        }
        let gamma = eexp as f32 / 10_000.0;
        if !(gamma.is_finite() && (0.5..=8.0).contains(&gamma)) {
            ffi::wl_resource_post_error(
                res,
                PARAMS_ERROR_INVALID_TF,
                c"params creator: implausible gamma exponent".as_ptr(),
            );
            return;
        }
        (*rec).transfer = Some(ContentTransfer::Gamma(gamma));
    }
}

unsafe extern "C" fn params_creator_set_primaries_named(
    _client: *mut ffi::wl_client,
    res: *mut ffi::wl_resource,
    primaries: u32,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(res) as *mut ParamsCreatorRec;
        if rec.is_null() {
            return;
        }
        if (*rec).primaries.is_some() {
            ffi::wl_resource_post_error(
                res,
                CREATOR_ERROR_ALREADY_SET,
                c"params creator: primaries already set".as_ptr(),
            );
            return;
        }
        let Some(named) = named_primaries(primaries) else {
            ffi::wl_resource_post_error(
                res,
                PARAMS_ERROR_INVALID_PRIMARIES_NAMED,
                c"params creator: unsupported named primaries".as_ptr(),
            );
            return;
        };
        (*rec).primaries = Some(ContentPrimaries::Named(named));
    }
}

#[allow(clippy::too_many_arguments)]
unsafe extern "C" fn params_creator_set_primaries(
    _client: *mut ffi::wl_client,
    res: *mut ffi::wl_resource,
    r_x: i32,
    r_y: i32,
    g_x: i32,
    g_y: i32,
    b_x: i32,
    b_y: i32,
    w_x: i32,
    w_y: i32,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(res) as *mut ParamsCreatorRec;
        if rec.is_null() {
            return;
        }
        if (*rec).primaries.is_some() {
            ffi::wl_resource_post_error(
                res,
                CREATOR_ERROR_ALREADY_SET,
                c"params creator: primaries already set".as_ptr(),
            );
            return;
        }
        // Wire unit: xy x 1,000,000.
        let xy = CustomPrimaries {
            rx: r_x as f32 / 1e6,
            ry: r_y as f32 / 1e6,
            gx: g_x as f32 / 1e6,
            gy: g_y as f32 / 1e6,
            bx: b_x as f32 / 1e6,
            by: b_y as f32 / 1e6,
            wx: w_x as f32 / 1e6,
            wy: w_y as f32 / 1e6,
        };
        let plausible = |x: f32, y: f32| {
            x.is_finite()
                && y.is_finite()
                && (0.0..1.0).contains(&x)
                && (0.0..1.0).contains(&y)
                && x + y <= 1.0
        };
        if !(plausible(xy.rx, xy.ry)
            && plausible(xy.gx, xy.gy)
            && plausible(xy.bx, xy.by)
            && plausible(xy.wx, xy.wy))
        {
            ffi::wl_resource_post_error(
                res,
                PARAMS_ERROR_INVALID_PRIMARIES_NAMED,
                c"params creator: chromaticities outside the CIE diagram".as_ptr(),
            );
            return;
        }
        (*rec).primaries = Some(ContentPrimaries::Custom(xy));
    }
}

unsafe extern "C" fn params_creator_create(
    client: *mut ffi::wl_client,
    res: *mut ffi::wl_resource,
    id: u32,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(res) as *mut ParamsCreatorRec;
        if rec.is_null() {
            return;
        }
        let (Some(primaries), Some(transfer)) = ((*rec).primaries, (*rec).transfer) else {
            ffi::wl_resource_post_error(
                res,
                CREATOR_ERROR_INCOMPLETE_SET,
                c"params creator: primaries and transfer are required".as_ptr(),
            );
            return;
        };
        create_image_description_resource(
            client,
            ffi::wl_resource_get_version(res) as u32,
            id,
            ContentColor::Parametric(ParametricColor {
                primaries,
                transfer,
                luminances: (*rec).luminances,
                max_cll: (*rec).max_cll,
                max_fall: (*rec).max_fall,
            }),
            true,
            fresh_identity((*rec).state),
        );
        ffi::wl_resource_destroy(res);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_transfer_mapping_round_trips_through_wire_ids() {
        for (wire, expected) in [
            (TF_EXT_LINEAR, NamedTransfer::Linear),
            (TF_GAMMA22, NamedTransfer::Gamma22),
            (TF_SRGB, NamedTransfer::Srgb),
            (TF_COMPOUND_POWER_2_4, NamedTransfer::Srgb),
            (TF_ST2084_PQ, NamedTransfer::Pq),
            (TF_HLG, NamedTransfer::Hlg),
        ] {
            let Some(ContentTransfer::Named(named)) = named_transfer(wire) else {
                panic!("wire id {wire} must map");
            };
            assert_eq!(named, expected);
        }
        // The sRGB curve is spelled per the receiver's interface version:
        // `srgb` for v1 peers, `compound_power_2_4` from v2 on.
        assert_eq!(transfer_wire_id(NamedTransfer::Srgb, 1), TF_SRGB);
        assert_eq!(
            transfer_wire_id(NamedTransfer::Srgb, 2),
            TF_COMPOUND_POWER_2_4
        );
        for (named, wire) in [
            (NamedTransfer::Linear, TF_EXT_LINEAR),
            (NamedTransfer::Gamma22, TF_GAMMA22),
            (NamedTransfer::Pq, TF_ST2084_PQ),
            (NamedTransfer::Hlg, TF_HLG),
        ] {
            assert_eq!(transfer_wire_id(named, 1), wire);
            assert_eq!(transfer_wire_id(named, 2), wire);
        }
        assert!(named_transfer(1).is_none()); // bt1886 (not advertised)
    }

    #[test]
    fn named_primaries_mapping_round_trips_through_wire_ids() {
        for (wire, expected) in [
            (PRIMARIES_SRGB, NamedPrimaries::Srgb),
            (PRIMARIES_BT2020, NamedPrimaries::Bt2020),
            (PRIMARIES_DISPLAY_P3, NamedPrimaries::DisplayP3),
            (PRIMARIES_ADOBE_RGB, NamedPrimaries::AdobeRgb),
        ] {
            assert_eq!(named_primaries(wire), Some(expected));
            assert_eq!(primaries_wire_id(expected), wire);
        }
        assert_eq!(named_primaries(7), None); // cie1931_xyz: not advertised
    }

    #[test]
    fn pipeline_description_follows_the_session_mode() {
        // SDR pipelines present sRGB anchored at the BT.2408 reference
        // white; HDR presents BT.2020 PQ with the backend's HDR10 peak.
        let sdr = ColorPipeline::Sdr.output_color();
        assert_eq!(sdr.primaries, ContentColor::SRGB.primaries);
        assert_eq!(sdr.transfer, ContentColor::SRGB.transfer);
        assert_eq!(sdr.luminances, Some(aegis_model::color::Luminances::SDR));
        let deep = ColorPipeline::SdrDeepColor.output_color();
        assert_eq!(deep, sdr);
        let hdr = ColorPipeline::Hdr.output_color();
        assert_eq!(
            hdr.primaries,
            ContentPrimaries::Named(NamedPrimaries::Bt2020)
        );
        assert_eq!(hdr.transfer, ContentTransfer::Named(NamedTransfer::Pq));
        assert_eq!(hdr.luminances, Some(aegis_model::color::Luminances::HDR));
    }
}
