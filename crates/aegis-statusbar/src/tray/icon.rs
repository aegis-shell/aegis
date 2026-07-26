//! Theme-icon resolution for SNI items, run on the worker threads.
//!
//! An `IconName` arrives as a theme lookup plus a decode (raster in-process,
//! SVG through `rsvg-convert`) — far too slow for the render thread. The
//! watcher resolves names into BGRA8 pixmaps before publishing them in the
//! shared snapshot, so the bar only ever uploads ready pixels. Mirrors the
//! compositor's app-icon path (`ass::runtime::apps`).

use std::path::Path;
use std::process::Command;

use super::TrayPixmap;

/// Scale theme-resolved SNI icons are looked up and rasterized at; the
/// texture is sampled down to the 18px cell glyph, so 2x keeps HiDPI crisp.
const TRAY_ICON_SCALE: u32 = 2;

/// Raster extensions the `image` crate decodes directly. SVG/SVGZ uses the
/// standard librsvg command-line rasterizer when installed and otherwise
/// falls back to the generic glyph.
const RASTER_ICON_EXTS: &[&str] = &["png", "jpg", "jpeg", "webp", "bmp", "gif", "tiff", "ico"];
const SVG_ICON_EXTS: &[&str] = &["svg", "svgz"];

/// Resolve an SNI `IconName` through the freedesktop icon theme and decode it
/// into unpremultiplied BGRA8 (the format flux samples), ready for GPU
/// upload. Returns `None` when the name is unknown or the file undecodable;
/// the caller memoizes the failure so the theme is not rescanned.
pub(super) fn resolve_theme_icon(name: &str) -> Option<TrayPixmap> {
    let path = aegis_desktop_entries::resolve_icon_scaled(name, None, &[], 24, TRAY_ICON_SCALE)?;
    let ext = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    let decoded = decode_icon(&path, &ext, TRAY_ICON_SCALE)?;
    let mut rgba = decoded.to_rgba8();
    if is_symbolic_icon(&path) {
        // Symbolic themes commonly encode a dark CSS foreground intended for
        // toolkit recolouring. Apply the bar's light foreground while
        // preserving every coverage value from SVG antialiasing (same
        // treatment as the compositor's HUD symbols).
        for pixel in rgba.pixels_mut() {
            if pixel[3] != 0 {
                pixel[0] = 246;
                pixel[1] = 246;
                pixel[2] = 248;
            }
        }
    }
    let (width, height) = rgba.dimensions();
    let mut bgra = rgba.into_raw();
    for chunk in bgra.chunks_exact_mut(4) {
        chunk.swap(0, 2);
    }
    Some(TrayPixmap {
        width,
        height,
        bgra,
    })
}

/// Decode a resolved theme icon. Raster formats stay in-process; SVG is
/// converted to a bounded PNG on stdout so malformed or enormous vector
/// sources cannot dictate an unbounded GPU texture. Copied from the
/// compositor's app icon cache (`ass::runtime::apps::decode_icon`).
fn decode_icon(path: &Path, ext: &str, scale: u32) -> Option<image::DynamicImage> {
    if RASTER_ICON_EXTS.contains(&ext) {
        return image::open(path).ok();
    }
    if !SVG_ICON_EXTS.contains(&ext) {
        return None;
    }
    let target = aegis_desktop_entries::DEFAULT_ICON_SIZE
        .saturating_mul(scale.max(1))
        .min(512)
        .to_string();
    let output = Command::new("rsvg-convert")
        .args([
            "--width",
            &target,
            "--height",
            &target,
            "--keep-aspect-ratio",
        ])
        .arg(path)
        .output()
        .ok()?;
    if !output.status.success() {
        log::debug!("tray: SVG rasterization failed for {}", path.display());
        return None;
    }
    image::load_from_memory(&output.stdout).ok()
}

/// Symbolic icons (name ends in `-symbolic`) are the only theme icons the bar
/// recolours; regular SNI icons are full-color application artwork.
fn is_symbolic_icon(path: &Path) -> bool {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .is_some_and(|stem| stem.ends_with("-symbolic"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symbolic_icon_detection_uses_the_file_stem() {
        assert!(is_symbolic_icon(Path::new(
            "/usr/share/icons/Adwaita/symbolic/apps/foo-symbolic.svg"
        )));
        assert!(!is_symbolic_icon(Path::new(
            "/usr/share/icons/hicolor/48x48/apps/foo.png"
        )));
        // "symbolic" appearing elsewhere in the name does not count.
        assert!(!is_symbolic_icon(Path::new("symbolic.png")));
        assert!(!is_symbolic_icon(Path::new("foo-symbolicx.svg")));
    }
}
