//! XDG cursor themes for the software cursor on direct display.
//!
//! The DRM backend has no hardware cursor plane yet, so the compositor draws
//! the cursor itself. Load real cursor themes per the freedesktop cursor
//! specification: `$XCURSOR_THEME` / `$XCURSOR_SIZE`, `$XCURSOR_PATH` or the
//! standard icon roots, `index.theme` inheritance, and the Xcursor file
//! format. Client-provided cursor surfaces are composited by the server as
//! before; this covers the `wp_cursor_shape` protocol and compositor-owned
//! (resize/move) cursors.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// One parsed Xcursor file: every image it carries, largest first.
#[derive(Debug)]
pub struct XcursorFile {
    pub images: Vec<XcursorImage>,
}

/// One cursor image with its hotspot, in pixels.
#[derive(Debug, Clone)]
pub struct XcursorImage {
    pub size: u32,
    pub width: u32,
    pub height: u32,
    pub xhot: u32,
    pub yhot: u32,
    /// Animation frame delay (only the first frame is used today).
    #[allow(dead_code)]
    pub delay_ms: u32,
    /// BGRA premultiplied pixels, row-major (the flux/wl_shm contract).
    pub pixels: Vec<u8>,
}

/// `wp_cursor_shape_device_v1.shape` value with its XDG candidate names,
/// protocol/CSS name first and legacy Xcursor aliases afterwards. The first
/// name found in the theme wins.
fn shape_candidates(shape: u32) -> &'static [&'static str] {
    match shape {
        1 => &["default", "left_ptr", "arrow"],
        2 => &["context-menu", "left_ptr"],
        3 => &["help", "question_arrow", "left_ptr_help", "left_ptr"],
        4 => &["pointer", "hand2", "pointing_hand", "left_ptr"],
        5 => &["progress", "left_ptr_watch", "watch"],
        6 => &["wait", "watch", "left_ptr"],
        7 => &["cell", "crosshair"],
        8 => &["crosshair", "cross"],
        9 => &["text", "xterm", "ibeam"],
        10 => &["vertical-text", "xterm"],
        11 => &["alias", "dnd-link", "left_ptr"],
        12 => &["copy", "dnd-copy", "left_ptr"],
        13 => &["move", "dnd-move", "fleur", "all-scroll"],
        14 => &["no-drop", "not-allowed"],
        15 => &["not-allowed", "forbidden", "crossed_circle"],
        16 => &["grab", "hand1", "openhand", "left_ptr"],
        17 => &["grabbing", "closedhand", "hand1"],
        18 => &["e-resize", "right_side", "sb_h_double_arrow"],
        19 => &["n-resize", "top_side", "sb_v_double_arrow"],
        20 => &["ne-resize", "top_right_corner", "sb_h_double_arrow"],
        21 => &["nw-resize", "top_left_corner", "sb_h_double_arrow"],
        22 => &["s-resize", "bottom_side", "sb_v_double_arrow"],
        23 => &["se-resize", "bottom_right_corner", "sb_h_double_arrow"],
        24 => &["sw-resize", "bottom_left_corner", "sb_h_double_arrow"],
        25 => &["w-resize", "left_side", "sb_h_double_arrow"],
        26 => &["ew-resize", "sb_h_double_arrow", "h_double_arrow"],
        27 => &["ns-resize", "sb_v_double_arrow", "v_double_arrow"],
        28 => &["nesw-resize", "bd_double_arrow", "size_bdiag"],
        29 => &["nwse-resize", "fd_double_arrow", "size_fdiag"],
        30 => &["col-resize", "sb_h_double_arrow", "h_double_arrow"],
        31 => &["row-resize", "sb_v_double_arrow", "v_double_arrow"],
        32 => &["all-scroll", "fleur", "move"],
        33 => &["zoom-in", "zoom_in"],
        34 => &["zoom-out", "zoom_out"],
        35 => &["dnd-ask", "question_arrow", "help"],
        36 => &["all-resize", "all-scroll", "fleur", "move"],
        _ => &["default", "left_ptr", "arrow"],
    }
}

/// Parse an Xcursor file into its images, largest size first. Returns `None`
/// on a bad magic or truncated layout; individual malformed images are
/// skipped rather than failing the whole file.
pub fn parse_xcursor(data: &[u8]) -> Option<XcursorFile> {
    let u32at = |off: usize| -> Option<u32> {
        data.get(off..off + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    if data.len() < 16 || u32at(0)? != 0x7275_6358 {
        return None; // "Xcur" little-endian
    }
    let ntoc = u32at(12)? as usize;
    let mut images = Vec::new();
    for i in 0..ntoc {
        let toc = 16 + i * 12;
        let (kind, position) = (u32at(toc)?, u32at(toc + 8)? as usize);
        // 0xFFFD0002 = image chunk; 0xFFFE0001 = comment chunk (skipped).
        if kind != 0xFFFD_0002 {
            continue;
        }
        let (width, height) = (u32at(position + 16)?, u32at(position + 20)?);
        if width == 0 || height == 0 || width > 256 || height > 256 {
            continue;
        }
        let (xhot, yhot, delay) = (
            u32at(position + 24)?,
            u32at(position + 28)?,
            u32at(position + 32)?,
        );
        let start = position + 36;
        let len = width as usize * height as usize * 4;
        let raw = data.get(start..start + len)?;
        // Xcursor pixels are XRGB8888 little-endian (= BGRA in memory) with
        // straight alpha; flux wants BGRA premultiplied.
        let mut pixels = raw.to_vec();
        for px in pixels.chunks_exact_mut(4) {
            let a = u32::from(px[3]);
            if a > 0 && a < 255 {
                for c in &mut px[0..3] {
                    *c = ((u32::from(*c) * a + 127) / 255) as u8;
                }
            }
        }
        images.push(XcursorImage {
            size: u32at(position + 8)?,
            width,
            height,
            xhot,
            yhot,
            delay_ms: delay,
            pixels,
        });
    }
    if images.is_empty() {
        return None;
    }
    images.sort_by_key(|img| std::cmp::Reverse(img.size));
    Some(XcursorFile { images })
}

/// Pick the image for `want` pixels: exact match first, then the nearest
/// larger size (downscaling looks better than upscaling), then the largest
/// available.
pub fn best_image(file: &XcursorFile, want: u32) -> Option<&XcursorImage> {
    let mut larger: Option<&XcursorImage> = None;
    let mut largest: Option<&XcursorImage> = None;
    for img in &file.images {
        if img.size == want {
            return Some(img);
        }
        if img.size > want && larger.is_none_or(|b| img.size < b.size) {
            larger = Some(img);
        }
        if largest.is_none_or(|b| img.size > b.size) {
            largest = Some(img);
        }
    }
    larger.or(largest)
}

/// Cursor theme search roots, in XDG priority order.
fn search_roots() -> Vec<PathBuf> {
    if let Some(path) = std::env::var_os("XCURSOR_PATH") {
        let roots = std::env::split_paths(&path).collect::<Vec<_>>();
        if !roots.is_empty() {
            return roots;
        }
    }

    let mut roots = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        roots.push(PathBuf::from(&home).join(".icons"));
        let data_home = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(&home).join(".local/share"));
        roots.push(data_home.join("icons"));
    }
    let data_dirs = std::env::var("XDG_DATA_DIRS")
        .unwrap_or_else(|_| "/usr/local/share:/usr/share".to_string());
    for dir in data_dirs.split(':').filter(|d| !d.is_empty()) {
        roots.push(PathBuf::from(dir).join("icons"));
    }
    roots.push(PathBuf::from("/usr/share/pixmaps"));
    roots
}

/// Read a theme's `Inherits=` list from its `index.theme`, if present.
fn theme_inherits(theme_dir: &Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(theme_dir.join("index.theme")) else {
        return Vec::new();
    };
    let mut inherits = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if let Some(value) = line.strip_prefix("Inherits=") {
            for name in value.split([',', ':']) {
                let name = name.trim();
                if !name.is_empty() {
                    inherits.push(name.to_string());
                }
            }
        }
    }
    inherits
}

/// A resolved cursor theme: the search chain of candidate theme directories.
pub struct CursorTheme {
    chain: Vec<PathBuf>,
    size: u32,
}

impl CursorTheme {
    /// Resolve a theme by name and preferred size. The search chain follows
    /// `index.theme` inheritance, breadth-first, cycles cut.
    pub fn resolve(name: &str, size: u32) -> CursorTheme {
        let roots = search_roots();
        let mut chain = Vec::new();
        let mut queue = vec![name.to_string()];
        let mut seen = std::collections::HashSet::new();
        while let Some(theme) = queue.pop() {
            if !seen.insert(theme.clone()) {
                continue;
            }
            for root in &roots {
                let dir = root.join(&theme);
                if dir.is_dir() {
                    for inherited in theme_inherits(&dir) {
                        if !seen.contains(&inherited) {
                            queue.push(inherited);
                        }
                    }
                    chain.push(dir.join("cursors"));
                    break;
                }
            }
        }
        CursorTheme {
            chain,
            size: size.max(1),
        }
    }

    /// Preferred cursor size in pixels, before output scaling.
    #[allow(dead_code)] // public surface of the theme; used by callers to come
    pub fn size(&self) -> u32 {
        self.size
    }

    /// Load the best image for a `wp_cursor_shape` value at `scale`, trying
    /// each candidate name down the inheritance chain.
    pub fn load_shape(&self, shape: u32, scale: f32) -> Option<XcursorImage> {
        let want = ((self.size as f32 * scale.max(0.25)).round() as u32).max(1);
        for name in shape_candidates(shape) {
            for dir in &self.chain {
                let path = dir.join(name);
                let Ok(data) = std::fs::read(&path) else {
                    continue;
                };
                if let Some(file) = parse_xcursor(&data)
                    && let Some(img) = best_image(&file, want)
                {
                    return Some(img.clone());
                }
            }
        }
        None
    }
}

/// One resolved, uploaded cursor ready to draw: a texture plus its hotspot.
pub struct LoadedCursor {
    pub image: flux::Image,
    /// Original premultiplied BGRA pixels retained for cursor-inclusive
    /// screenshots on nested backends, whose host cursor is not part of the
    /// compositor framebuffer.
    pub pixels: std::sync::Arc<[u8]>,
    /// Draw size in physical pixels (the image's natural size).
    pub width: f32,
    pub height: f32,
    /// Hotspot offset within the image, in physical pixels.
    pub xhot: f32,
    pub yhot: f32,
}

/// Upload and cache theme cursors per (shape, scale). Keys are cheap; themes
/// are reloaded only when the environment says they changed.
#[derive(Default)]
pub struct CursorCache {
    theme: Option<(String, u32, CursorTheme)>,
    cursors: HashMap<(u32, u32), Option<LoadedCursor>>,
    /// Configured fallback theme/size (`[ui] cursor_theme/cursor_size`).
    /// Env vars win when set; these cover bare TTY sessions with no cursor
    /// environment at all.
    config_theme: Option<String>,
    config_size: Option<u32>,
}

impl CursorCache {
    /// Set the configured fallback theme and size (`[ui]` config section);
    /// `None` falls through to the "default" theme and 24px.
    pub fn set_config(&mut self, theme: Option<String>, size: Option<u32>) {
        if self.config_theme != theme || self.config_size != size {
            self.config_theme = theme;
            self.config_size = size;
            self.theme = None;
            self.cursors.clear();
        }
    }

    /// The effective (theme, size): `$XCURSOR_THEME`/`$XCURSOR_SIZE` win,
    /// then the configured fallback, then the freedesktop defaults.
    fn effective(&self) -> (String, u32) {
        let name = std::env::var("XCURSOR_THEME")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .or_else(|| self.config_theme.clone())
            .unwrap_or_else(|| "default".into());
        let size = std::env::var("XCURSOR_SIZE")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            .filter(|s| *s > 0)
            .or(self.config_size)
            .unwrap_or(24);
        (name, size)
    }

    /// The cursor for a `wp_cursor_shape` value at `scale`, or `None` when no
    /// theme ships it. Missing theme data deliberately does not fall back to
    /// a compositor-designed cursor: cursor appearance belongs to the theme.
    pub fn get(&mut self, device: &flux::Device, shape: u32, scale: f32) -> Option<&LoadedCursor> {
        let (name, size) = self.effective();
        if !matches!(&self.theme, Some((n, s, _)) if *n == name && *s == size) {
            let theme = CursorTheme::resolve(&name, size);
            if theme.chain.is_empty() {
                log::error!(
                    "cursor: Xcursor theme {name:?} was not found; no compositor-designed fallback will be used"
                );
            } else {
                log::info!("cursor: using Xcursor theme {name:?} at {size} logical px");
            }
            self.theme = Some((name.clone(), size, theme));
            self.cursors.clear();
        }
        let key = (shape, (scale * 4.0).round() as u32);
        let theme = &self.theme.as_ref().expect("theme just set").2;
        self.cursors
            .entry(key)
            .or_insert_with(|| {
                let img = theme.load_shape(shape, scale)?;
                let pixels: std::sync::Arc<[u8]> = img.pixels.into();
                let image = flux::Image::from_bytes(
                    device,
                    img.width,
                    img.height,
                    flux::Format::FLUX_FORMAT_BGRA8_UNORM,
                    &pixels,
                )
                .ok()?;
                Some(LoadedCursor {
                    image,
                    pixels,
                    width: img.width as f32,
                    height: img.height as f32,
                    xhot: img.xhot as f32,
                    yhot: img.yhot as f32,
                })
            })
            .as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal valid Xcursor file with `sizes` images of the given
    /// sizes, solid white pixels.
    fn synthetic_xcursor(sizes: &[u32]) -> Vec<u8> {
        let mut data = Vec::new();
        let ntoc = sizes.len() as u32;
        data.extend_from_slice(&0x7275_6358u32.to_le_bytes()); // "Xcur"
        data.extend_from_slice(&1u32.to_le_bytes()); // header size... (unused)
        data.extend_from_slice(&1u32.to_le_bytes()); // version
        data.extend_from_slice(&ntoc.to_le_bytes());
        let mut pos = 16 + sizes.len() * 12;
        let mut chunks = Vec::new();
        for &size in sizes {
            data.extend_from_slice(&0xFFFD_0002u32.to_le_bytes());
            data.extend_from_slice(&size.to_le_bytes());
            data.extend_from_slice(&(pos as u32).to_le_bytes());
            let mut chunk = Vec::new();
            chunk.extend_from_slice(&36u32.to_le_bytes());
            chunk.extend_from_slice(&0xFFFD_0002u32.to_le_bytes());
            chunk.extend_from_slice(&size.to_le_bytes());
            chunk.extend_from_slice(&1u32.to_le_bytes()); // version
            chunk.extend_from_slice(&size.to_le_bytes()); // width
            chunk.extend_from_slice(&size.to_le_bytes()); // height
            chunk.extend_from_slice(&2u32.to_le_bytes()); // xhot
            chunk.extend_from_slice(&3u32.to_le_bytes()); // yhot
            chunk.extend_from_slice(&0u32.to_le_bytes()); // delay
            chunk.resize(36 + (size * size * 4) as usize, 0xFF);
            pos += chunk.len();
            chunks.push(chunk);
        }
        for chunk in chunks {
            data.extend_from_slice(&chunk);
        }
        data
    }

    #[test]
    fn parses_images_and_picks_best_size() {
        let data = synthetic_xcursor(&[16, 24, 48]);
        let file = parse_xcursor(&data).expect("valid file");
        assert_eq!(file.images.len(), 3);
        assert_eq!(file.images[0].size, 48, "largest first");
        assert_eq!(best_image(&file, 24).unwrap().size, 24, "exact");
        assert_eq!(best_image(&file, 32).unwrap().size, 48, "larger nearest");
        assert_eq!(best_image(&file, 8).unwrap().size, 16, "smaller nearest");
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_xcursor(b"not a cursor").is_none());
        assert!(parse_xcursor(&[]).is_none());
        let mut data = synthetic_xcursor(&[24]);
        data.truncate(20);
        assert!(parse_xcursor(&data).is_none());
    }

    #[test]
    fn shape_candidates_cover_common_shapes() {
        for shape in [1, 4, 9, 13, 18, 23, 26, 32] {
            assert!(!shape_candidates(shape).is_empty(), "shape {shape}");
        }
        assert_eq!(shape_candidates(1)[0], "default");
        assert_eq!(shape_candidates(3)[0], "help");
        assert_eq!(shape_candidates(6)[0], "wait");
        assert_eq!(shape_candidates(9)[0], "text");
        assert_eq!(shape_candidates(18)[0], "e-resize");
        assert_eq!(shape_candidates(35)[0], "dnd-ask");
        assert_eq!(shape_candidates(36)[0], "all-resize");
    }

    #[test]
    fn resolves_and_loads_an_installed_theme() {
        // Host-dependent: Bibata-Modern-Ice is the development machine's
        // $XCURSOR_THEME; skip silently where it is not installed.
        let theme = CursorTheme::resolve("Bibata-Modern-Ice", 24);
        if theme.chain.is_empty() {
            eprintln!("Bibata-Modern-Ice not installed; skipping");
            return;
        }
        let img = theme
            .load_shape(1, 2.0)
            .expect("left_ptr loads from the theme");
        assert!(img.width > 0 && img.height > 0);
        assert_eq!(img.pixels.len(), (img.width * img.height * 4) as usize);
        assert!(img.xhot < img.width && img.yhot < img.height);
        // 2x scale prefers a 48px image over the 24px base when available.
        assert!(img.size >= 24);
    }
}
