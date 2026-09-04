//! Generate and compile the Wayland extension `wl_interface` tables once, here,
//! so the client-side nested backend and server share each symbol definition.
//! Compiling them in both consumers would duplicate symbols at the final link.
//! The tables reference core `wl_*_interface` symbols, which each consumer
//! resolves from the libwayland implementation it links.

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
    // Each protocol is a `(category, basename)` pair. These are compiled into
    // one static lib shared by client and server.
    let protocols: &[(&str, &str)] = &[
        // stable
        ("stable/xdg-shell", "xdg-shell"),
        ("unstable/linux-dmabuf", "linux-dmabuf-unstable-v1"),
        (
            "unstable/linux-explicit-synchronization",
            "linux-explicit-synchronization-unstable-v1",
        ),
        ("stable/viewporter", "viewporter"),
        ("stable/presentation-time", "presentation-time"),
        ("stable/tablet", "tablet-v2"),
        // unstable
        ("unstable/xdg-output", "xdg-output-unstable-v1"),
        ("unstable/xdg-decoration", "xdg-decoration-unstable-v1"),
        ("unstable/xdg-foreign", "xdg-foreign-unstable-v2"),
        ("unstable/idle-inhibit", "idle-inhibit-unstable-v1"),
        ("unstable/relative-pointer", "relative-pointer-unstable-v1"),
        ("unstable/pointer-gestures", "pointer-gestures-unstable-v1"),
        (
            "unstable/keyboard-shortcuts-inhibit",
            "keyboard-shortcuts-inhibit-unstable-v1",
        ),
        (
            "unstable/pointer-constraints",
            "pointer-constraints-unstable-v1",
        ),
        ("unstable/text-input", "text-input-unstable-v3"),
        // staging / ext
        ("staging/fractional-scale", "fractional-scale-v1"),
        ("staging/ext-session-lock", "ext-session-lock-v1"),
        ("staging/ext-idle-notify", "ext-idle-notify-v1"),
        (
            "staging/ext-foreign-toplevel-list",
            "ext-foreign-toplevel-list-v1",
        ),
        // ext-data-control-v1: manager-managed clipboard read/write without a
        // focused surface. wl-clipboard & co. prefer it over wl_data_device,
        // which removes their need for the invisible focus-stealing helper
        // window entirely (ADR-0133).
        ("staging/ext-data-control", "ext-data-control-v1"),
        ("staging/cursor-shape", "cursor-shape-v1"),
        ("staging/xdg-activation", "xdg-activation-v1"),
        ("staging/color-management", "color-management-v1"),
    ];

    let mut build = cc::Build::new();
    build.warnings(false);
    // The generated protocol sources `#include "wayland-util.h"`. On
    // sysroot-based distributions (e.g. theseus/wright) the wayland headers
    // are not in the compiler's default include path, so ask pkg-config for
    // libwayland-server's include dir instead of assuming /usr/include.
    if let Some(includes) = pkg_include_dirs("wayland-server") {
        for dir in includes {
            build.include(dir);
        }
    }
    for &(category, base) in protocols {
        let xml = PathBuf::from(&pkgdatadir)
            .join(category)
            .join(format!("{base}.xml"));
        compile_protocol(&mut build, &out, &xml, base);
    }
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    for base in ["input-method-unstable-v2", "virtual-keyboard-unstable-v1"] {
        let xml = manifest.join("protocols").join(format!("{base}.xml"));
        compile_protocol(&mut build, &out, &xml, base);
    }
    build.compile("tessera_wayland_protocols");

    println!("cargo:rerun-if-changed=build.rs");
}

fn compile_protocol(
    build: &mut cc::Build,
    out: &std::path::Path,
    xml: &std::path::Path,
    base: &str,
) {
    assert!(xml.exists(), "missing protocol xml: {}", xml.display());
    let cfile = out.join(format!("{base}-protocol.c"));
    let status = Command::new("wayland-scanner")
        .arg("private-code")
        .arg(xml)
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

/// Parse `-I<dir>` flags out of `pkg-config --cflags-only-I <pkg>`. Returns
/// `None` when pkg-config cannot find the package or reports no include dirs
/// (the caller then falls back to the compiler's default search path).
fn pkg_include_dirs(pkg: &str) -> Option<Vec<String>> {
    let out = Command::new("pkg-config")
        .args(["--cflags-only-I", pkg])
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    let dirs: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .filter_map(|f| f.strip_prefix("-I").map(|d| d.to_string()))
        .collect();
    if dirs.is_empty() { None } else { Some(dirs) }
}
