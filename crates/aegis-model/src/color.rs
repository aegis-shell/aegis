//! Content color-space descriptions (the compositor's half of
//! `wp_color_management_v1`).
//!
//! Pure, backend- and renderer-agnostic model of the color space a
//! client says its buffer is in. The compositor server stores these per
//! surface; the renderer maps them onto flux image tags
//! (`flux_image_color_space_desc`). The numeric enums mirror the
//! protocol's named primaries / transfer functions one to one.

use std::sync::Arc;

/// Named primaries, mirroring `wp_color_manager_v1.primaries`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum NamedPrimaries {
    Srgb,
    Bt2020,
    DisplayP3,
    AdobeRgb,
}

/// Custom primaries as CIE 1931 xy chromaticities (protocol `set_primaries`).
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CustomPrimaries {
    pub rx: f32,
    pub ry: f32,
    pub gx: f32,
    pub gy: f32,
    pub bx: f32,
    pub by: f32,
    pub wx: f32,
    pub wy: f32,
}

/// The primary set of a content color space.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ContentPrimaries {
    Named(NamedPrimaries),
    Custom(CustomPrimaries),
}

/// Named transfer functions, mirroring the supported subset of
/// `wp_color_manager_v1.transfer_function`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum NamedTransfer {
    /// Extended linear (`ext_linear`).
    Linear,
    /// Display gamma 2.2 (`gamma22`).
    Gamma22,
    /// The IEC 61966-2-1 piecewise encoding (`compound_power_2_4`).
    Srgb,
    /// ST 2084 perceptual quantizer (`st2084_pq`).
    Pq,
    /// Hybrid Log-Gamma (`hlg`).
    Hlg,
}

/// The transfer function of a content color space: a named curve or a
/// pure gamma power (protocol `set_tf_power`).
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ContentTransfer {
    Named(NamedTransfer),
    Gamma(f32),
}

/// A luminance range with a reference white level, in cd/m² (the protocol's
/// `set_luminances` request and `luminances` info event).
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Luminances {
    /// Minimum display emission (black level), including ambient flare.
    pub min: f32,
    /// Maximum primary color volume luminance.
    pub max: f32,
    /// Reference white luminance.
    pub reference: f32,
}

impl Luminances {
    /// SDR output anchoring: sRGB white at the BT.2408 reference level.
    pub const SDR: Luminances = Luminances {
        min: 0.2,
        max: 203.0,
        reference: 203.0,
    };
    /// HDR output anchoring: matches the HDR10 metadata the DRM backend
    /// emits (1000 cd/m² peak, BT.2408 reference white).
    pub const HDR: Luminances = Luminances {
        min: 0.0,
        max: 1000.0,
        reference: 203.0,
    };
}

/// A parametric image description: primaries plus transfer function.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ParametricColor {
    pub primaries: ContentPrimaries,
    pub transfer: ContentTransfer,
    /// Primary color volume luminances; `None` means the protocol default
    /// (the transfer function's implicit anchoring).
    pub luminances: Option<Luminances>,
    /// CTA-861-H MaxCLL content metadata (cd/m²); accepted and remembered,
    /// not yet consumed by the renderer.
    pub max_cll: Option<u32>,
    /// CTA-861-H MaxFALL content metadata (cd/m²); same standing as
    /// `max_cll`.
    pub max_fall: Option<u32>,
}

/// The color space a surface's buffer contents are in (its "image
/// description"). `None` anywhere means the protocol default: sRGB.
/// Not serialized: this is a runtime value, never on the IPC wire.
#[derive(Debug, Clone, PartialEq)]
pub enum ContentColor {
    /// Parametric description (wp_image_description_creator_params_v1).
    Parametric(ParametricColor),
    /// ICC profile bytes (wp_image_description_creator_icc_v1). Shared
    /// behind an `Arc`: buffers are cloned per frame, the profile is not.
    Icc(Arc<[u8]>),
}

impl ContentColor {
    /// The implicit tag of an untagged buffer: sRGB.
    pub const SRGB: ParametricColor = ParametricColor {
        primaries: ContentPrimaries::Named(NamedPrimaries::Srgb),
        transfer: ContentTransfer::Named(NamedTransfer::Srgb),
        luminances: None,
        max_cll: None,
        max_fall: None,
    };
}
