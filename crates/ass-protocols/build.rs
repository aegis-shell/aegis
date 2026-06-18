//! Generate and compile the xdg-shell `wl_interface` tables once, here, so both
//! the client-side nested backend and the server share a single definition of
//! `xdg_*_interface` (compiling them in two crates would duplicate the symbols
//! at the final link). The tables reference core `wl_*_interface` symbols, which
//! the consumer resolves from the libwayland it links (client or server).

use std::path::PathBuf;
use std::process::Command;

fn main() {
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());

    let pkgdatadir = Command::new("pkg-config")
        .args(["--variable=pkgdatadir", "wayland-protocols"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .expect("pkg-config: wayland-protocols not found");
    // Each (subdir, basename) protocol is scanned to its interface tables and
    // compiled into one static lib shared by client and server.
    let protocols = [
        ("stable/xdg-shell", "xdg-shell"),
        ("unstable/linux-dmabuf", "linux-dmabuf-unstable-v1"),
        ("stable/viewporter", "viewporter"),
    ];

    let mut build = cc::Build::new();
    build.warnings(false);
    for (subdir, base) in protocols {
        let xml = PathBuf::from(&pkgdatadir)
            .join(subdir)
            .join(format!("{base}.xml"));
        assert!(xml.exists(), "missing protocol xml: {}", xml.display());
        let cfile = out.join(format!("{base}-protocol.c"));
        let status = Command::new("wayland-scanner")
            .arg("private-code")
            .arg(&xml)
            .arg(&cfile)
            .status()
            .expect("failed to run wayland-scanner");
        assert!(
            status.success(),
            "wayland-scanner private-code failed for {base}"
        );
        build.file(&cfile);
        println!("cargo:rerun-if-changed={}", xml.display());
    }
    build.compile("ass_protocols");

    println!("cargo:rerun-if-changed=build.rs");
}
