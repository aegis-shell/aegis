//! Propagate the Optics native-library paths to this crate's test harness.

fn main() {
    let manifest_dir = std::path::PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR").expect("Cargo sets CARGO_MANIFEST_DIR"),
    );
    let debug_assets = manifest_dir.join("persona-debug-assets");
    let debug_vrm = debug_assets.join("avatar.vrm");
    let debug_vrma = debug_assets.join("avatar.vrma");
    let debug_motions = debug_assets.join("motions");
    println!("cargo:rerun-if-changed={}", debug_vrm.display());
    println!("cargo:rerun-if-changed={}", debug_vrma.display());
    println!("cargo:rerun-if-changed={}", debug_motions.display());
    if std::env::var_os("CARGO_FEATURE_PERSONA").is_some() && debug_vrm.is_file() {
        let motion = if debug_motions.is_dir() {
            " with motion library"
        } else if debug_vrma.is_file() {
            " with companion avatar.vrma"
        } else {
            ""
        };
        println!(
            "cargo:warning=tessera-shell: local persona avatar detected{motion}; preview with \
             TESSERA_AVATAR_DEBUG_ASSETS=1 TESSERA_AVATAR_DEBUG_DUMP=/tmp/tessera-avatar.png \
             TESSERA_AVATAR_DEBUG_TIME=1 cargo run -p tessera-shell --features persona --example \
             debug_avatar"
        );
    }

    let mut emitted_dtags = false;
    for var in [
        "DEP_LENS_RPATHS",
        "DEP_FLUX_RPATHS",
        "DEP_FLUX_SCENE_GRAPH_RPATHS",
    ] {
        if let Ok(rpaths) = std::env::var(var) {
            if !emitted_dtags {
                println!("cargo:rustc-link-arg=-Wl,--disable-new-dtags");
                emitted_dtags = true;
            }
            for dir in rpaths.split(';').filter(|path| !path.is_empty()) {
                println!("cargo:rustc-link-arg=-Wl,-rpath,{dir}");
            }
        }
    }
}
