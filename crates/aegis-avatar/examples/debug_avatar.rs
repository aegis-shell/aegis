//! Render either the ignored source-tree VRM fixture or the current XDG avatar
//! once and optionally dump the circle-masked portrait.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var_os("AEGIS_AVATAR_DEBUG_ASSETS").is_none()
        && std::env::var_os("AEGIS_AVATAR_DEBUG_XDG").is_none()
    {
        return Err("set AEGIS_AVATAR_DEBUG_ASSETS=1 for debug-assets or \
             AEGIS_AVATAR_DEBUG_XDG=1 for the current XDG avatar"
            .into());
    }
    let device = flux::Device::new(true, &[], &[], 1)?;
    let mut avatar = aegis_avatar::Avatar::load(&device)?
        .ok_or("no avatar found for the selected debug source")?;
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
    println!("debug avatar loaded: {:?}", avatar.kind());
    println!("debug motions: {:#?}", avatar.motions());
    println!("debug motion: {:?}", avatar.current_motion());
    println!("debug sample time: {sample_time:.3}s");
    if let Some(path) = std::env::var_os("AEGIS_AVATAR_DEBUG_DUMP") {
        println!("debug portrait written to {:?}", path);
    }
    Ok(())
}
