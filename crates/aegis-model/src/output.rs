//! Output and monitor geometry (ADR-0028).
//!
//! Pure, backend- and renderer-agnostic model of one output's physical
//! mode, scale, and transform, plus the derivation of the logical size the
//! chrome and clients see. This is the foundation the multi-output milestone
//! (M7) and the tiling work-area build on; the workspace model's
//! [`Output`](crate::workspace::Output) gains a geometry reference when M7
//! wires real hotplug.

use crate::{Point, Rect, Size, Transform};

const MILLIMETERS_PER_INCH: f32 = 25.4;
const INTERNAL_TARGET_PPI: f32 = 125.0;
const EXTERNAL_TARGET_PPI: f32 = 110.0;
const AUTO_SCALE_STEP: f32 = 0.25;
const MAX_AUTO_SCALE: f32 = 4.0;

/// Physical placement class used to select a comfortable logical density.
/// Internal panels are normally viewed closer than external displays, so
/// they retain a slightly higher effective PPI after scaling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputKind {
    Internal,
    External,
}

/// A display mode: physical resolution and refresh rate.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OutputMode {
    /// Physical width in device pixels.
    pub width: i32,
    /// Physical height in device pixels.
    pub height: i32,
    /// Refresh rate in millihertz (e.g. 60000 for 60 Hz), matching the
    /// DRM/KMS and `wl_output.mode` convention.
    pub refresh_mhz: u32,
}

/// A configured display-mode request (ADR-0028): an exact resolution with an
/// optional whole-Hertz refresh, parsed from the `mode` string of a
/// `[[output]]` config entry (`"1920x1080"` or `"2560x1440@144"`). The
/// backend matches it against the modes a connector advertises with
/// [`ModeSpec::matches`].
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModeSpec {
    /// Requested physical width in device pixels.
    pub width: i32,
    /// Requested physical height in device pixels.
    pub height: i32,
    /// Requested refresh in whole Hertz. `None` leaves the rate open: the
    /// backend picks the highest-refresh mode of that size.
    pub refresh_hz: Option<u32>,
}

impl ModeSpec {
    /// Whether an advertised mode satisfies this request. Resolution must
    /// match exactly; a named refresh matches when the mode's millihertz
    /// rate rounds to it, and `None` matches any rate.
    pub fn matches(&self, mode: &OutputMode) -> bool {
        if mode.width != self.width || mode.height != self.height {
            return false;
        }
        match self.refresh_hz {
            Some(hz) => mode.refresh_mhz.saturating_add(500) / 1_000 == hz,
            None => true,
        }
    }
}

impl std::str::FromStr for ModeSpec {
    type Err = ();

    /// Parse `"WxH"` or `"WxH@Hz"`. Only positive decimal integers are
    /// accepted — no whitespace, signs, or trailing garbage.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        fn positive_i32(s: &str) -> Result<i32, ()> {
            if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
                return Err(());
            }
            let value = s.parse::<i32>().map_err(|_| ())?;
            if value > 0 { Ok(value) } else { Err(()) }
        }
        let (size, refresh) = match s.split_once('@') {
            Some((size, hz)) => (size, Some(hz)),
            None => (s, None),
        };
        let (w, h) = size.split_once('x').ok_or(())?;
        Ok(ModeSpec {
            width: positive_i32(w)?,
            height: positive_i32(h)?,
            refresh_hz: refresh.map(positive_i32).transpose()?.map(|hz| hz as u32),
        })
    }
}

/// Per-connector output configuration policy (ADR-0028): the resolved form
/// of one `[[output]]` config entry. `scale`, `position`, and `primary` are
/// applied by the server as outputs appear; `mode` is consumed by the
/// backend at modeset time. `transform` is parsed and validated but its
/// application is deferred until renderer output-transform support lands.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct OutputPolicy {
    /// Scale override; `None` keeps the backend-reported scale.
    pub scale: Option<f64>,
    /// Requested display mode; `None` selects the connector's highest-pixel
    /// mode at its highest refresh rate.
    pub mode: Option<ModeSpec>,
    /// Top-left in the global logical layout; `None` keeps the
    /// backend-assigned position.
    pub position: Option<Point>,
    /// Output transform. Parsed but not yet applied (see the type docs).
    pub transform: Option<Transform>,
    /// Whether this output is the primary (focused) one.
    pub primary: bool,
    /// Allow HDR (BT.2020 PQ) output on this connector. The compositor's
    /// shared framebuffer enters HDR mode only when *every* active output
    /// both allows it here and advertises ST 2084 support through EDID;
    /// anything less stays SDR.
    pub hdr: bool,
    /// Allow a 10-bit deep-color framebuffer (RGB10A2-class scanout) for
    /// reduced banding in SDR. Also gated on every active output allowing
    /// it and every primary plane supporting the format.
    pub deep_color: bool,
    /// Target SDR white luminance in cd/m² (nits) in HDR mode (ITU-R BT.2408
    /// default: 203.0). Valid range is 80.0 through 500.0.
    pub sdr_white_nits: Option<f32>,
}

/// Per-connector color policy (the `hdr` / `deep_color` / `sdr_white_nits` fields of a
/// `[[output]]` config entry), consumed by the backend at modeset time.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ColorPolicy {
    /// Allow HDR (BT.2020 PQ) on this connector.
    pub hdr: bool,
    /// Allow a 10-bit deep-color SDR framebuffer on this connector.
    pub deep_color: bool,
    /// Target SDR white luminance in cd/m² (nits) for HDR mode.
    pub sdr_white_nits: Option<f32>,
}

/// The session-wide color pipeline actually in effect (the compositor
/// renders one shared framebuffer, so the pixel encoding is uniform).
/// Reported by the backend; the compositor server derives the per-output
/// image description for `wp_color_management_v1` from it.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ColorPipeline {
    /// 8-bit sRGB (the default).
    #[default]
    Sdr,
    /// 10-bit sRGB in an RGB10A2-class container (deep color).
    SdrDeepColor,
    /// BT.2020 PQ (HDR10-class) in an RGB10A2-class container.
    Hdr,
}

impl ColorPipeline {
    /// The output image description this pipeline presents to clients:
    /// the space the framebuffer is written in. The luminances anchor the
    /// description to absolute levels so clients can tell SDR from HDR
    /// (and scale headroom correctly) instead of guessing.
    pub fn output_color(self) -> crate::color::ParametricColor {
        use crate::color::{
            ContentPrimaries, ContentTransfer, Luminances, NamedPrimaries, NamedTransfer,
            ParametricColor,
        };
        match self {
            ColorPipeline::Sdr | ColorPipeline::SdrDeepColor => ParametricColor {
                luminances: Some(Luminances::SDR),
                ..crate::color::ContentColor::SRGB
            },
            ColorPipeline::Hdr => ParametricColor {
                primaries: ContentPrimaries::Named(NamedPrimaries::Bt2020),
                transfer: ContentTransfer::Named(NamedTransfer::Pq),
                luminances: Some(Luminances::HDR),
                max_cll: None,
                max_fall: None,
            },
        }
    }
}

/// A scale factor. Carries a fractional value so HiDPI hardware that prefers
/// a non-integer scale (1.5, 1.25) is representable, beyond the integer-only
/// `wl_surface.set_buffer_scale`. Maps to `wp_fractional_scale_v1`.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Scale(pub f32);

impl Scale {
    /// No scaling — logical and physical pixels coincide.
    pub const IDENTITY: Scale = Scale(1.0);

    /// The scale as an `f32`.
    pub fn as_f32(self) -> f32 {
        self.0
    }
}

impl Default for Scale {
    fn default() -> Scale {
        Scale::IDENTITY
    }
}

/// Calculate the diagonal pixel density from a mode and the display's
/// physical dimensions. Dimensions with an implausible size, density, or
/// pixel-to-millimeter aspect ratio are rejected because EDID data is not
/// universally reliable.
pub fn physical_ppi(mode: OutputMode, physical_size_mm: (u32, u32)) -> Option<f32> {
    let (pixel_width, pixel_height) = (mode.width as f32, mode.height as f32);
    let (mm_width, mm_height) = (physical_size_mm.0 as f32, physical_size_mm.1 as f32);
    if pixel_width <= 0.0
        || pixel_height <= 0.0
        || !(40.0..=3_000.0).contains(&mm_width)
        || !(40.0..=3_000.0).contains(&mm_height)
    {
        return None;
    }

    let ppi_x = pixel_width * MILLIMETERS_PER_INCH / mm_width;
    let ppi_y = pixel_height * MILLIMETERS_PER_INCH / mm_height;
    let axis_ratio = ppi_x.max(ppi_y) / ppi_x.min(ppi_y);
    if !axis_ratio.is_finite() || axis_ratio > 1.1 {
        return None;
    }

    let diagonal_pixels = pixel_width.hypot(pixel_height);
    let diagonal_inches = mm_width.hypot(mm_height) / MILLIMETERS_PER_INCH;
    let ppi = diagonal_pixels / diagonal_inches;
    (ppi.is_finite() && (40.0..=1_000.0).contains(&ppi)).then_some(ppi)
}

/// Recommend an automatic output scale from physical pixel density.
/// Recommendations use quarter-step fractional scales and never shrink
/// below 100%. Callers retain an explicit per-output scale as the final
/// authority and use this only as the hardware-derived default.
pub fn automatic_scale(
    mode: OutputMode,
    physical_size_mm: (u32, u32),
    kind: OutputKind,
) -> Option<Scale> {
    let ppi = physical_ppi(mode, physical_size_mm)?;
    let target_ppi = match kind {
        OutputKind::Internal => INTERNAL_TARGET_PPI,
        OutputKind::External => EXTERNAL_TARGET_PPI,
    };
    let raw = (ppi / target_ppi).clamp(1.0, MAX_AUTO_SCALE);
    let quantized = (raw / AUTO_SCALE_STEP).round() * AUTO_SCALE_STEP;
    Some(Scale(quantized.clamp(1.0, MAX_AUTO_SCALE)))
}

/// One output's identity + geometry — the wire shape the IPC exposes for an
/// output. The connector is the stable identity; the geometry is its current
/// mode, scale, transform, and position.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq)]
pub struct OutputInfo {
    pub connector: String,
    pub geometry: OutputGeometry,
    /// Modes the connector advertises (deduplicated, highest resolution
    /// first), so `aegis display` and agents can see what `mode` requests
    /// are valid. Empty where the backend cannot enumerate (nested).
    /// `serde(default)` keeps pre-field IPC peers compatible.
    #[cfg_attr(feature = "serde", serde(default))]
    pub available_modes: Vec<OutputMode>,
    /// Color capabilities parsed from the display's EDID (HDR transfer
    /// support, BT.2020 colorimetry). All-false where the backend cannot
    /// read EDID (nested). `serde(default)` keeps pre-field IPC peers
    /// compatible.
    #[cfg_attr(feature = "serde", serde(default))]
    pub color_caps: crate::edid::EdidColorCapabilities,
}

/// Per-output geometry (ADR-0028): the physical mode, scale, transform, and
/// the output's top-left in the global logical layout. From these the
/// [`logical_size`](Self::logical_size) — the size the chrome and clients
/// operate in — is derived.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OutputGeometry {
    pub mode: OutputMode,
    pub scale: Scale,
    pub transform: Transform,
    /// Top-left of this output in the global logical coordinate space. The
    /// primary output sits at (0, 0); others are placed relative to it.
    pub logical_origin: Point,
}

impl Default for OutputGeometry {
    fn default() -> OutputGeometry {
        OutputGeometry {
            mode: OutputMode {
                width: 0,
                height: 0,
                refresh_mhz: 0,
            },
            scale: Scale::IDENTITY,
            transform: Transform::Normal,
            logical_origin: Point::default(),
        }
    }
}

impl OutputGeometry {
    /// The logical size the chrome and clients see: the physical mode, axes
    /// swapped for a 90°/270° transform, divided by the scale. Rounded to the
    /// nearest logical pixel.
    pub fn logical_size(&self) -> Size {
        let (w, h) = if self.transform.swap_axes() {
            (self.mode.height, self.mode.width)
        } else {
            (self.mode.width, self.mode.height)
        };
        let s = self.scale.0;
        if s <= 0.0 {
            // A non-positive scale is nonsensical; avoid divide-by-zero.
            return Size { w, h };
        }
        Size {
            w: ((w as f32) / s).round() as i32,
            h: ((h as f32) / s).round() as i32,
        }
    }

    /// The output's rect in the global logical layout.
    pub fn logical_rect(&self) -> Rect {
        Rect {
            origin: self.logical_origin,
            size: self.logical_size(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mode(w: i32, h: i32) -> OutputMode {
        OutputMode {
            width: w,
            height: h,
            refresh_mhz: 60000,
        }
    }

    #[test]
    fn identity_scale_and_transform_keeps_physical_size() {
        let g = OutputGeometry {
            mode: mode(1920, 1080),
            ..Default::default()
        };
        assert_eq!(g.logical_size(), Size { w: 1920, h: 1080 });
    }

    #[test]
    fn integer_scale_halves_a_hidpi_mode() {
        let g = OutputGeometry {
            mode: mode(3840, 2160),
            scale: Scale(2.0),
            ..Default::default()
        };
        assert_eq!(g.logical_size(), Size { w: 1920, h: 1080 });
    }

    #[test]
    fn fractional_scale_supports_non_integer() {
        let g = OutputGeometry {
            mode: mode(3000, 2000),
            scale: Scale(1.5),
            ..Default::default()
        };
        // 3000/1.5 = 2000, 2000/1.5 ≈ 1333.33 → 1333.
        assert_eq!(g.logical_size(), Size { w: 2000, h: 1333 });
    }

    #[test]
    fn rotate_90_swaps_axes() {
        let g = OutputGeometry {
            mode: mode(1920, 1080), // landscape panel
            transform: Transform::Rotate90,
            ..Default::default()
        };
        // Rotated portrait: logical width = physical height and vice versa.
        assert_eq!(g.logical_size(), Size { w: 1080, h: 1920 });
    }

    #[test]
    fn rotate_90_and_scale_compose() {
        let g = OutputGeometry {
            mode: mode(3840, 2160),
            scale: Scale(2.0),
            transform: Transform::Rotate90,
            ..Default::default()
        };
        // Swap → (2160, 3840); /2 → (1080, 1920).
        assert_eq!(g.logical_size(), Size { w: 1080, h: 1920 });
    }

    #[test]
    fn flip_variants_swap_axes_only_for_90_270() {
        let normal = OutputGeometry {
            mode: mode(1920, 1080),
            ..Default::default()
        };
        let flip_h = OutputGeometry {
            mode: mode(1920, 1080),
            transform: Transform::FlipHorizontal,
            ..normal
        };
        // Pure flips (no rotation) do not swap axes.
        assert_eq!(flip_h.logical_size(), normal.logical_size());
    }

    #[test]
    fn logical_rect_combines_origin_and_size() {
        let g = OutputGeometry {
            mode: mode(2560, 1440),
            scale: Scale(2.0),
            logical_origin: Point { x: 960, y: 0 },
            ..Default::default()
        };
        assert_eq!(
            g.logical_rect(),
            Rect {
                origin: Point { x: 960, y: 0 },
                size: Size { w: 1280, h: 720 },
            }
        );
    }

    #[test]
    fn non_positive_scale_falls_back_to_physical() {
        let g = OutputGeometry {
            mode: mode(100, 50),
            scale: Scale(0.0),
            ..Default::default()
        };
        assert_eq!(g.logical_size(), Size { w: 100, h: 50 });
    }

    #[test]
    fn physical_ppi_uses_mode_and_millimeter_dimensions() {
        let ppi = physical_ppi(mode(3072, 1920), (312, 195)).unwrap();
        assert!((ppi - 250.09).abs() < 0.01, "{ppi}");
    }

    #[test]
    fn automatic_scale_selects_two_for_hidpi_laptop_panel() {
        assert_eq!(
            automatic_scale(mode(3072, 1920), (312, 195), OutputKind::Internal),
            Some(Scale(2.0))
        );
    }

    #[test]
    fn automatic_scale_uses_output_kind_and_quarter_steps() {
        // A typical 15.6-inch 1080p laptop panel is about 141 PPI.
        assert_eq!(
            automatic_scale(mode(1920, 1080), (344, 194), OutputKind::Internal),
            Some(Scale(1.25))
        );
        // A 27-inch 4K external display is about 163 PPI and is normally
        // viewed farther away, yielding a lower target logical density.
        assert_eq!(
            automatic_scale(mode(3840, 2160), (598, 336), OutputKind::External),
            Some(Scale(1.5))
        );
        assert_eq!(
            automatic_scale(mode(1920, 1080), (509, 286), OutputKind::External),
            Some(Scale::IDENTITY)
        );
    }

    #[test]
    fn automatic_scale_rejects_missing_or_inconsistent_physical_size() {
        assert_eq!(
            automatic_scale(mode(3072, 1920), (0, 0), OutputKind::Internal),
            None
        );
        assert_eq!(
            automatic_scale(mode(1920, 1080), (300, 100), OutputKind::Internal),
            None
        );
    }

    #[test]
    fn mode_spec_parses_resolution_with_optional_refresh() {
        assert_eq!(
            "1920x1080".parse::<ModeSpec>(),
            Ok(ModeSpec {
                width: 1920,
                height: 1080,
                refresh_hz: None,
            })
        );
        assert_eq!(
            "2560x1440@144".parse::<ModeSpec>(),
            Ok(ModeSpec {
                width: 2560,
                height: 1440,
                refresh_hz: Some(144),
            })
        );
    }

    #[test]
    fn mode_spec_rejects_garbage() {
        for bad in [
            "",
            "1920",
            "1920x",
            "x1080",
            "1920X1080",
            "1920 x 1080",
            "1920x1080x2",
            "1920x1080@",
            "1920x1080@60@2",
            "0x1080",
            "1920x-1080",
            "+1920x1080",
            "1920x1080@0",
            "99999999999x1080",
        ] {
            assert!(bad.parse::<ModeSpec>().is_err(), "{bad:?} should fail");
        }
    }

    #[test]
    fn mode_spec_matches_exact_size_and_rounded_refresh() {
        let spec: ModeSpec = "1920x1080@144".parse().unwrap();
        assert!(spec.matches(&OutputMode {
            width: 1920,
            height: 1080,
            refresh_mhz: 144_000,
        }));
        // 143.856 Hz rounds to 144.
        assert!(spec.matches(&OutputMode {
            width: 1920,
            height: 1080,
            refresh_mhz: 143_856,
        }));
        // Wrong refresh, wrong size, and 144.5+ Hz (rounds to 145) all miss.
        assert!(!spec.matches(&OutputMode {
            width: 1920,
            height: 1080,
            refresh_mhz: 60_000,
        }));
        assert!(!spec.matches(&OutputMode {
            width: 2560,
            height: 1080,
            refresh_mhz: 144_000,
        }));
        assert!(!spec.matches(&OutputMode {
            width: 1920,
            height: 1080,
            refresh_mhz: 144_501,
        }));
        // No refresh in the spec matches any rate of the right size.
        let any_rate: ModeSpec = "1920x1080".parse().unwrap();
        assert!(any_rate.matches(&OutputMode {
            width: 1920,
            height: 1080,
            refresh_mhz: 60_000,
        }));
    }
}
