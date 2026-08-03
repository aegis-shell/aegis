//! Link the native session dependencies and preserve local Optics rpaths.

fn main() {
    let mut local_optics_dirs = Vec::new();
    for var in [
        "DEP_LENS_RPATHS",
        "DEP_FLUX_RPATHS",
        "DEP_FLUX_SCENE_GRAPH_RPATHS",
    ] {
        if let Ok(rpaths) = std::env::var(var) {
            for dir in rpaths.split(';').filter(|path| !path.is_empty()) {
                if !local_optics_dirs.iter().any(|existing| existing == dir) {
                    local_optics_dirs.push(dir.to_owned());
                }
            }
        }
    }

    // In local Optics mode an older system libflux may still live in
    // `/usr/lib`. Put the binding-published Meson directories first so the
    // linker and the runtime loader select the same native build.
    for dir in &local_optics_dirs {
        println!("cargo:rustc-link-search=native={dir}");
    }

    pkg_config::Config::new()
        .atleast_version("1.20")
        .probe("wayland-client")
        .expect("aegis-lock requires wayland-client");
    pkg_config::Config::new()
        .probe("pam")
        .expect("aegis-lock requires PAM");

    if !local_optics_dirs.is_empty() {
        println!("cargo:rustc-link-arg=-Wl,--disable-new-dtags");
    }
    for dir in local_optics_dirs {
        println!("cargo:rustc-link-arg=-Wl,-rpath,{dir}");
    }
}
