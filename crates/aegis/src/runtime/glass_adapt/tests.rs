use super::*;

const ID: u64 = 0xdead_beef;

fn region(id: u64, polarity: f32, tint_strength: f32) -> aegis_shell::LiquidGlassRegion {
    aegis_shell::LiquidGlassRegion {
        id,
        plate_polarity: polarity,
        tint_strength,
        ..Default::default()
    }
}

#[test]
fn first_sample_snaps_without_smoothing() {
    let mut adapt = GlassAdaptation::new();
    adapt.observe(ID, 0.8, 0.4, 1.0 / 60.0);
    let mut target = region(ID, 0.0, 3.6);
    adapt.apply_to(&mut target);
    let emitted = target.adaptation.expect("first sample adapts immediately");
    assert_eq!(emitted.plate_luminance, quantize(0.8));
    assert_eq!(emitted.backdrop_energy, quantize(0.4));
}

#[test]
fn anonymous_regions_are_never_adapted() {
    let mut adapt = GlassAdaptation::new();
    adapt.observe(ID, 0.8, 0.4, 1.0 / 60.0);
    let mut anonymous = region(0, 0.0, 3.6);
    adapt.apply_to(&mut anonymous);
    assert_eq!(anonymous.adaptation, None);
    assert_eq!(anonymous.tint_strength, 3.6);
}

#[test]
fn smoothing_converges_and_hysteresis_holds_boundaries() {
    let mut adapt = GlassAdaptation::new();
    adapt.observe(ID, 0.0, 0.0, 1.0 / 60.0);
    // Drive the luminance target to 1.0 over two seconds of frames.
    for _ in 0..120 {
        adapt.observe(ID, 1.0, 0.0, 1.0 / 60.0);
    }
    let mut target = region(ID, 0.0, 3.6);
    adapt.apply_to(&mut target);
    let emitted = target.adaptation.expect("adaptation");
    assert!((emitted.plate_luminance - 1.0).abs() <= QUANTUM);

    // A value hovering half a step away must not move the shipped value.
    let before = emitted.plate_luminance;
    adapt.observe(ID, 1.0 - 0.5 * QUANTUM, 0.0, 10.0);
    let mut target = region(ID, 0.0, 3.6);
    adapt.apply_to(&mut target);
    assert_eq!(
        target.adaptation.expect("adaptation").plate_luminance,
        before
    );

    // A full-step move does ship.
    adapt.observe(ID, 1.0 - 2.0 * QUANTUM, 0.0, 10.0);
    let mut target = region(ID, 0.0, 3.6);
    adapt.apply_to(&mut target);
    assert_ne!(
        target.adaptation.expect("adaptation").plate_luminance,
        before
    );
}

#[test]
fn tint_recovery_eases_off_only_on_friendly_backdrops() {
    let mut adapt = GlassAdaptation::new();
    // Smoke-polarity body over a calm dark backdrop: full recovery.
    adapt.observe(ID, 0.02, 0.0, 1.0 / 60.0);
    let mut calm_dark = region(ID, 0.0, 3.6);
    adapt.apply_to(&mut calm_dark);
    assert!((calm_dark.tint_strength - 3.6 * RECOVERY_FLOOR).abs() < 0.05);

    // The same body over a bright backdrop keeps full strength.
    adapt.observe(ID, 0.95, 0.0, 10.0);
    let mut bright = region(ID, 0.0, 3.6);
    adapt.apply_to(&mut bright);
    assert!((bright.tint_strength - 3.6).abs() < 0.05);

    // Pearl-polarity body: bright is friendly instead.
    const OTHER: u64 = 0x1234_5678;
    adapt.observe(OTHER, 0.97, 0.0, 1.0 / 60.0);
    let mut pearl = region(OTHER, 1.0, 4.5);
    adapt.apply_to(&mut pearl);
    assert!((pearl.tint_strength - 4.5 * RECOVERY_FLOOR).abs() < 0.05);

    // Unpinned bodies never modulate.
    const LEGACY: u64 = 0x5555_aaaa;
    adapt.observe(LEGACY, 0.02, 0.0, 1.0 / 60.0);
    let mut unpinned = region(LEGACY, -1.0, 1.0);
    adapt.apply_to(&mut unpinned);
    assert_eq!(unpinned.tint_strength, 1.0);
}

#[test]
fn samples_clamp_to_unit_range() {
    let mut adapt = GlassAdaptation::new();
    adapt.observe(ID, 4.0, -1.0, 1.0 / 60.0);
    let mut target = region(ID, 0.0, 3.6);
    adapt.apply_to(&mut target);
    let emitted = target.adaptation.expect("adaptation");
    assert_eq!(emitted.plate_luminance, 1.0);
    assert_eq!(emitted.backdrop_energy, 0.0);
}

#[test]
fn retain_drops_stale_regions_and_they_snap_on_return() {
    let mut adapt = GlassAdaptation::new();
    adapt.observe(ID, 0.5, 0.2, 1.0 / 60.0);
    adapt.retain(&[]);
    // After eviction the next observation is a fresh snap, not a blend.
    adapt.observe(ID, 0.9, 0.1, 1.0 / 60.0);
    let mut target = region(ID, 0.0, 3.6);
    adapt.apply_to(&mut target);
    let emitted = target.adaptation.expect("adaptation");
    assert_eq!(emitted.plate_luminance, quantize(0.9));
    assert_eq!(emitted.backdrop_energy, quantize(0.1));
}
