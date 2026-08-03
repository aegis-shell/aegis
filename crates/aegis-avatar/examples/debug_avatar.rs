//! Render the ignored source-tree VRM fixture with an explicit caller camera.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var_os("AEGIS_AVATAR_DEBUG_ASSETS").is_none() {
        return Err("set AEGIS_AVATAR_DEBUG_ASSETS=1 for debug-assets".into());
    }
    let device = flux::Device::new(true, &[], &[], 1)?;
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("debug-assets");
    let motion = directory.join("avatar.vrma");
    let mut avatar = aegis_avatar::Avatar::load(
        &device,
        &directory.join("avatar.vrm"),
        motion.is_file().then_some(motion.as_path()),
        aegis_avatar::VrmCamera::new(28.0, 0.25, 0.48, 0.0),
    )?;
    if let Ok(name) = std::env::var("AEGIS_AVATAR_DEBUG_MOTION")
        && !avatar.play_motion(&name)
    {
        return Err(format!("debug avatar has no motion named {name:?}").into());
    }
    // Sample a non-zero pose so a still dump proves the VRMA path is active.
    let sample_time = std::env::var("AEGIS_AVATAR_DEBUG_TIME")
        .ok()
        .and_then(|value| value.parse::<f32>().ok())
        .unwrap_or(1.0);
    avatar.advance(sample_time)?;
    device.wait_idle();
    println!("debug avatar loaded: {:?}", avatar.animation_support());
    println!("debug motions: {:#?}", avatar.motions());
    println!("debug motion: {:?}", avatar.current_motion());
    println!("debug sample time: {sample_time:.3}s");
    if let Some(path) = std::env::var_os("AEGIS_AVATAR_DEBUG_DUMP") {
        println!("debug portrait written to {path:?}");
    }
    Ok(())
}
