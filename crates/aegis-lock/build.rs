//! Link the native session dependencies and preserve local Optics rpaths.

fn main() {
    pkg_config::Config::new()
        .atleast_version("1.20")
        .probe("wayland-client")
        .expect("aegis-lock requires wayland-client");
    pkg_config::Config::new()
        .probe("pam")
        .expect("aegis-lock requires PAM");

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
