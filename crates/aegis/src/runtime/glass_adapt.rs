//! Region-level liquid-glass backdrop adaptation — the policy half of the
//! material. prism measures each glass body's backdrop (mean luminance,
//! high-frequency energy) with a frame-slot lag and exposes the numbers;
//! this module owns everything prism deliberately does not: temporal
//! smoothing keyed by each region's stable id, quantization with hysteresis
//! (an emitted value change re-runs the glass composite, so the shipped
//! values must not dither), and the mapping from backdrop friendliness to
//! tint-strength recovery.
//!
//! The legibility budget behind the policy: white menu text (L ≈ 0.95)
//! needs a plate at L ≤ 0.18 for WCAG AA (4.5:1). With the Menu role's
//! recipe — interior frost ≈ 40% (frost_strength 5.0), pinned smoke
//! polarity, tint_strength 3.6, shader energy boost up to 2× — a bright
//! text glyph (L ≈ 0.55 over a local mean ≈ 0.2) lands at plate L ≈ 0.16
//! (≈ 4.9:1), and even a uniform white backdrop (L = 1) lands at ≈ 0.17
//! (≈ 4.6:1). The recovery curve below then hands translucency back when
//! the measured backdrop is friendly, which is where the liquid look lives.

use std::collections::HashMap;

/// Exponential smoothing rate (per second). Fast enough to settle within a
/// menu's opening beat, slow enough that video playback behind glass cannot
/// pump the tint.
const SMOOTHING_RATE: f32 = 2.5;

/// Quantization step for values shipped to the shader. 1/32 steps are far
/// below a just-noticeable material difference, and each step boundary has
/// hysteresis so a hovering value cannot oscillate the composite.
const QUANTUM: f32 = 1.0 / 32.0;

/// Tint-strength floor on friendly backdrops: a calm backdrop in the
/// plate's own tone needs little tint, so the body recovers translucency
/// down to this fraction of the role's strength.
const RECOVERY_FLOOR: f32 = 0.55;

#[derive(Clone, Copy)]
struct Smoothed {
    luminance: f32,
    energy: f32,
    emitted: aegis_shell::LiquidGlassAdaptation,
}

/// Per-region temporal smoother for backdrop statistics. Regions identify
/// themselves with a stable id; ids are never reused across bodies, so a
/// transient surface can never inherit another body's backdrop.
#[derive(Default)]
pub(crate) struct GlassAdaptation {
    regions: HashMap<u64, Smoothed>,
}

impl GlassAdaptation {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Fold one fresh sample for region `id` into its smoothed state. The
    /// first sample after a region appears snaps immediately — a freshly
    /// opened menu adapts within the stats pipeline's frame-slot lag, not
    /// the smoothing time constant.
    pub(crate) fn observe(&mut self, id: u64, luminance: f32, energy: f32, dt_seconds: f32) {
        let luminance = luminance.clamp(0.0, 1.0);
        let energy = energy.clamp(0.0, 1.0);
        match self.regions.get_mut(&id) {
            Some(state) => {
                let blend = 1.0 - (-SMOOTHING_RATE * dt_seconds.max(0.0)).exp();
                state.luminance += (luminance - state.luminance) * blend;
                state.energy += (energy - state.energy) * blend;
                state.emitted.plate_luminance =
                    quantize_hysteresis(state.luminance, state.emitted.plate_luminance);
                state.emitted.backdrop_energy =
                    quantize_hysteresis(state.energy, state.emitted.backdrop_energy);
            }
            None => {
                self.regions.insert(
                    id,
                    Smoothed {
                        luminance,
                        energy,
                        emitted: aegis_shell::LiquidGlassAdaptation {
                            plate_luminance: quantize(luminance),
                            backdrop_energy: quantize(energy),
                        },
                    },
                );
            }
        }
    }

    /// Write this region's adaptation back before the regions are mapped to
    /// prism groups: the smoothed statistics, plus the tint-strength
    /// recovery for friendly backdrops. Anonymous regions (id 0) and regions
    /// without a first sample keep their declared material.
    pub(crate) fn apply_to(&self, region: &mut aegis_shell::LiquidGlassRegion) {
        if region.id == 0 {
            return;
        }
        let Some(state) = self.regions.get(&region.id) else {
            return;
        };
        region.adaptation = Some(state.emitted);
        // Polarity-aware friendliness: a smoke plate (polarity 0) finds dark
        // backdrops friendly, a pearl plate (1) bright ones. Strength eases
        // toward RECOVERY_FLOOR as the backdrop approaches the friendliest
        // calm state; unpinned bodies (polarity < 0) get no recovery.
        let polarity = region.plate_polarity;
        if (0.0..=1.0).contains(&polarity) {
            let friendly = if polarity <= 0.0 {
                1.0 - state.emitted.plate_luminance
            } else {
                state.emitted.plate_luminance
            };
            let ease = smoothstep(0.55, 0.9, friendly);
            region.tint_strength *= 1.0 + (RECOVERY_FLOOR - 1.0) * ease;
        }
    }

    /// Drop smoother state for regions no longer declared, so a reopened
    /// surface starts from a fresh snap rather than a stale backdrop.
    pub(crate) fn retain(&mut self, live_ids: &[u64]) {
        self.regions.retain(|id, _| live_ids.contains(id));
    }
}

fn quantize(value: f32) -> f32 {
    ((value / QUANTUM).round() * QUANTUM).clamp(0.0, 1.0)
}

/// Move the shipped value only when the smoothed value has travelled at
/// least one full step from it; a value hovering at a step boundary must
/// not oscillate the composite-triggering output.
fn quantize_hysteresis(value: f32, current: f32) -> f32 {
    let candidate = quantize(value);
    if candidate != current && (value - current).abs() >= QUANTUM {
        candidate
    } else {
        current
    }
}

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests;
