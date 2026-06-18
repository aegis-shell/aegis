//! Link libwayland-client for the nested host window. The xdg-shell interface
//! tables come from the shared `ass-protocols` crate, so they are not generated
//! here.

fn main() {
    pkg_config::Config::new()
        .probe("wayland-client")
        .expect("pkg-config: wayland-client not found");
    println!("cargo:rerun-if-changed=build.rs");
}
