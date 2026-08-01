//! Render the ignored source-tree VRM fixture once and optionally dump the
//! circle-masked portrait through `AEGIS_AVATAR_DEBUG_DUMP`.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var_os("AEGIS_AVATAR_DEBUG_ASSETS").is_none() {
        return Err("set AEGIS_AVATAR_DEBUG_ASSETS=1 to select debug-assets/avatar.vrm".into());
    }
    let device = flux::Device::new(true, &[], &[], 1)?;
    let mut avatar = aegis_avatar::Avatar::load(&device)?
        .ok_or("no debug avatar found; copy avatar.vrm into debug-assets")?;
    // Sample a non-zero pose so a still dump proves the VRMA path is active.
    let sample_time = std::env::var("AEGIS_AVATAR_DEBUG_TIME")
        .ok()
        .and_then(|value| value.parse::<f32>().ok())
        .unwrap_or(1.0);
    avatar.advance(sample_time)?;
    device.wait_idle();
    println!("debug avatar loaded: {:?}", avatar.kind());
    println!("debug sample time: {sample_time:.3}s");
    if let Some(path) = std::env::var_os("AEGIS_AVATAR_DEBUG_DUMP") {
        println!("debug portrait written to {:?}", path);
    }
    Ok(())
}
