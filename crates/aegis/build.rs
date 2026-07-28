//! Re-emit the native-library paths published by the Optics `-sys` crates via
//! their `links` metadata. This keeps the final binary correct for both
//! system-installed libraries and the opt-in local Optics override.
//! `rustc-link-arg` does not propagate across crates, so the terminal binary
//! must re-emit the paths itself.

fn main() {
    // Keep the flux path selected by flux-sys ahead of unrelated search paths
    // introduced by dependencies such as Vulkan or FreeType. Emitting it from
    // the terminal binary gives it the intended link-search precedence.
    if let Ok(rpaths) = std::env::var("DEP_FLUX_RPATHS") {
        for dir in rpaths.split(';').filter(|s| !s.is_empty()) {
            println!("cargo:rustc-link-search=native={dir}");
        }
    }

    let mut emitted_dtags = false;
    for var in [
        "DEP_FLUX_RPATHS",
        "DEP_FLUX_SCENE_GRAPH_RPATHS",
        "DEP_LENS_RPATHS",
    ] {
        if let Ok(rpaths) = std::env::var(var) {
            if !emitted_dtags {
                // DT_RPATH (not DT_RUNPATH) so the search also covers transitive
                // NEEDED libs (libflux is liblens's NEEDED, not ours).
                println!("cargo:rustc-link-arg=-Wl,--disable-new-dtags");
                emitted_dtags = true;
            }
            for dir in rpaths.split(';').filter(|s| !s.is_empty()) {
                println!("cargo:rustc-link-arg=-Wl,-rpath,{dir}");
            }
        }
    }
}
