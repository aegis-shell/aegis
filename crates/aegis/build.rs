//! Re-emit the rpaths published by the flux / lens `-sys` crates (via their
//! `links` metadata) so the final `aegis` binary resolves libflux.so and
//! liblens.so from the sibling meson build trees at runtime without
//! `LD_LIBRARY_PATH`. `rustc-link-arg` does not propagate across crates, so the
//! terminal binary must re-emit them itself.

fn main() {
    // A stale system libflux may sit in /usr/lib (pulled in as a link-search by
    // system deps such as vulkan/freetype). Search the build-tree flux first so
    // `-lflux` binds to the current library, not the system one. Emitted from
    // the terminal binary because link-search order follows crate order and the
    // binary's own paths come first.
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
