//! Link libwayland-server. The server drives libwayland's C API directly; the
//! core `wl_*` interface tables and the shm implementation are provided by the
//! library itself, so no protocol code is generated for the core globals.
//! Extension protocols (xdg-shell) are added in a later stage through a shared
//! protocol crate.

fn main() {
    pkg_config::Config::new()
        .probe("wayland-server")
        .expect("pkg-config: wayland-server not found");
    println!("cargo:rerun-if-changed=build.rs");
}
