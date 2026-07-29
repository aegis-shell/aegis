//! XDG cursor themes for the software cursor on direct display.
//!
//! The DRM backend has no hardware cursor plane yet, so the compositor draws
//! the cursor itself. Cursor themes are resolved per the freedesktop cursor
//! specification — `$XCURSOR_THEME` / `$XCURSOR_SIZE`, `XCURSOR_PATH` or the
//! standard icon roots, `index.theme` inheritance — but aegis draws cursors
//! from **SVG** rather than the legacy Xcursor binary format: SVG stays crisp
//! at any scale and is rasterized on demand with the pure-Rust `resvg`.
//!
//! A full Bibata-Modern-Ice theme is embedded in the binary
//! (`assets/cursors/Bibata-Modern-Ice`) so a sane default cursor always
//! exists, even on a bare TTY with no installed icon theme and no
//! `XCURSOR_THEME`. Client-provided cursor surfaces are composited by the
//! server as before; this covers the `wp_cursor_shape` protocol and
//! compositor-owned (resize/move) cursors.
//!
//! Hotspot convention: the bundled art stamps each cursor's hotspot onto the
//! `<svg>` root as `data-hotspot-x` / `data-hotspot-y`, in the SVG's native
//! (viewBox) coordinate space (see `scripts/prepare-bibata-cursors.py`).
//! `resvg`/`usvg` drop unknown attributes, so the hotspot is read from the
//! raw SVG text before rasterization and scaled by the requested size.

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use include_dir::{Dir, include_dir};

/// The cursor art shipped with the binary, used as the universal fallback
/// when no filesystem theme resolves a shape. GPL-3.0 art from Bibata Cursor;
/// see `LICENSE`/`NOTICE` under the directory.
static BUNDLED_THEME: Dir<'static> =
    include_dir!("$CARGO_MANIFEST_DIR/../../assets/cursors/Bibata-Modern-Ice");

/// `wp_cursor_shape_device_v1.shape` value with its XDG candidate names,
/// protocol/CSS name first and legacy cursor aliases afterwards. The first
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

/// Hotspot and native size read from the raw SVG text. `native` is the
/// viewBox (or width/height) the hotspot is expressed in; both dimensions are
/// assumed square for cursor art.
struct SvgMeta {
    hotspot: (f32, f32),
    native: (f32, f32),
}

/// Scan the `<svg ...>` opening tag for `data-hotspot-x/y` and the viewBox or
/// width/height. Unknown themes without a stamped hotspot fall back to the
/// top-left corner (0, 0). A malformed tag simply yields defaults.
fn svg_meta(svg: &[u8]) -> SvgMeta {
    let text = std::str::from_utf8(svg).unwrap_or("");
    let tag = svg_open_tag(text).unwrap_or("");
    let hotspot = (
        attr(tag, "data-hotspot-x").and_then(num).unwrap_or(0.0),
        attr(tag, "data-hotspot-y").and_then(num).unwrap_or(0.0),
    );
    let native = viewbox(tag)
        .or_else(|| width_height(tag))
        .unwrap_or((256.0, 256.0));
    SvgMeta { hotspot, native }
}

/// Slice the first `<svg ...>` opening tag (the cursor's root), so attribute
/// scanning never matches nested elements.
fn svg_open_tag(text: &str) -> Option<&str> {
    let start = text.find("<svg")?;
    let end = text[start..].find('>').map(|p| start + p + 1)?;
    Some(&text[start..end])
}

/// Extract the value of attribute `name` (`name="..."` or `name='...'`) from
/// `tag`. Byte-level and therefore UTF-8-safe: quotes are ASCII and cannot be
/// part of a multibyte continuation, so the returned slice lands on char
/// boundaries.
fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let bytes = tag.as_bytes();
    let nb = name.as_bytes();
    let mut from = 0;
    while let Some(rel) = tag[from..].find(name) {
        let start = from + rel;
        from = start + nb.len();
        // The name must be its own token: preceded by whitespace (it is never
        // at the very start of the slice because "<svg" leads it).
        let prev = bytes.get(start.wrapping_sub(1)).copied().unwrap_or(b' ');
        if !(prev == b' ' || prev == b'\t' || prev == b'\n' || prev == b'\r') {
            continue;
        }
        let mut j = start + nb.len();
        while matches!(bytes.get(j), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            j += 1;
        }
        if bytes.get(j) != Some(&b'=') {
            continue;
        }
        j += 1;
        while matches!(bytes.get(j), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            j += 1;
        }
        let quote = match bytes.get(j) {
            Some(&b'"') => b'"',
            Some(&b'\'') => b'\'',
            _ => continue,
        };
        let value_start = j + 1;
        let mut k = value_start;
        while bytes.get(k).is_some_and(|&b| b != quote) {
            k += 1;
        }
        if bytes.get(k) == Some(&quote) {
            return Some(&tag[value_start..k]);
        }
    }
    None
}

/// Parse a leading floating-point number out of a value like `55` or `55.5px`.
fn num(value: &str) -> Option<f32> {
    let token: String = value
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
        .collect();
    token.parse().ok()
}

/// `viewBox="0 0 W H"` → `(W, H)`.
fn viewbox(tag: &str) -> Option<(f32, f32)> {
    let v = attr(tag, "viewBox")?;
    let parts: Vec<&str> = v.split_ascii_whitespace().collect();
    let (w, h) = (parts.get(2)?.parse().ok()?, parts.get(3)?.parse().ok()?);
    (w > 0.0 && h > 0.0).then_some((w, h))
}

/// `width="W"` / `height="H"` fallback when there is no viewBox.
fn width_height(tag: &str) -> Option<(f32, f32)> {
    let w = num(attr(tag, "width")?)?;
    let h = num(attr(tag, "height")?)?;
    (w > 0.0 && h > 0.0).then_some((w, h))
}

/// One rasterized cursor image: premultiplied BGRA8 pixels with its hotspot.
struct RasterCursor {
    width: u32,
    height: u32,
    xhot: u32,
    yhot: u32,
    pixels: Vec<u8>,
}

/// Render `svg` to a square `out`×`out` premultiplied BGRA8 sprite, applying
/// the stamped hotspot scaled from the native viewBox space. Returns `None`
/// on any parse/render failure so the caller can try the next candidate.
fn rasterize(svg: &[u8], out: u32) -> Option<RasterCursor> {
    let out = out.clamp(1, 256);
    let meta = svg_meta(svg);
    let tree = usvg::Tree::from_data(svg, &usvg::Options::default()).ok()?;
    let (nw, nh) = meta.native;
    let mut pixmap = tiny_skia::Pixmap::new(out, out)?;
    let transform = tiny_skia::Transform::from_scale(out as f32 / nw, out as f32 / nh);
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    let mut pixels = pixmap.data().to_vec();
    // tiny-skia emits premultiplied RGBA8; flux samples premultiplied BGRA8.
    for chunk in pixels.chunks_exact_mut(4) {
        chunk.swap(0, 2);
    }
    let xhot = (meta.hotspot.0 * out as f32 / nw)
        .round()
        .clamp(0.0, out as f32 - 1.0) as u32;
    let yhot = (meta.hotspot.1 * out as f32 / nh)
        .round()
        .clamp(0.0, out as f32 - 1.0) as u32;
    Some(RasterCursor {
        width: out,
        height: out,
        xhot,
        yhot,
        pixels,
    })
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

/// One link in a theme's search chain: either a filesystem `cursors/`
/// directory or the in-binary bundled theme.
enum Source {
    Dir(PathBuf),
    Bundled,
}

impl Source {
    /// Resolve `name` to raw SVG bytes from this source, if present.
    fn svg(&self, name: &str) -> Option<Cow<'_, [u8]>> {
        match self {
            Source::Dir(dir) => std::fs::read(dir.join(format!("{name}.svg")))
                .ok()
                .map(Cow::Owned),
            Source::Bundled => BUNDLED_THEME
                .get_file(format!("cursors/{name}.svg"))
                .map(|file| Cow::Borrowed(file.contents())),
        }
    }
}

/// A resolved cursor theme: the search chain of candidate sources.
pub struct CursorTheme {
    chain: Vec<Source>,
    size: u32,
}

impl CursorTheme {
    /// Resolve a theme by name and preferred size. The search chain follows
    /// `index.theme` inheritance, breadth-first (cycles cut), and always ends
    /// with the bundled Bibata-Modern-Ice theme so a cursor exists even when
    /// nothing is installed.
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
                    chain.push(Source::Dir(dir.join("cursors")));
                    break;
                }
            }
        }
        chain.push(Source::Bundled);
        CursorTheme {
            chain,
            size: size.max(1),
        }
    }

    /// Whether any filesystem theme directory matched (i.e. the bundled
    /// fallback is not the only source in the chain).
    fn has_filesystem(&self) -> bool {
        self.chain.iter().any(|s| matches!(s, Source::Dir(_)))
    }

    /// Rasterize the best SVG for a `wp_cursor_shape` value at `scale`,
    /// trying each candidate name down the inheritance chain.
    fn load_shape(&self, shape: u32, scale: f32) -> Option<RasterCursor> {
        let out = ((self.size as f32 * scale.max(0.25)).round() as u32).max(1);
        for name in shape_candidates(shape) {
            for source in &self.chain {
                if let Some(bytes) = source.svg(name)
                    && let Some(raster) = rasterize(&bytes, out)
                {
                    return Some(raster);
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
/// are reloaded only when the runtime's effective preference changes.
pub struct CursorCache {
    theme: Option<(String, u32, CursorTheme)>,
    cursors: HashMap<(u32, u32), Option<LoadedCursor>>,
    preference_theme: String,
    preference_size: u32,
}

impl Default for CursorCache {
    fn default() -> Self {
        Self {
            theme: None,
            cursors: HashMap::new(),
            preference_theme: "default".into(),
            preference_size: 24,
        }
    }
}

impl CursorCache {
    /// Install the runtime's already-resolved cursor preferences. Environment
    /// overrides are resolved once in the compositor settings pipeline, so
    /// this rendering cache has no independent source of truth.
    pub fn set_preferences(&mut self, theme: String, size: u32) {
        if self.preference_theme != theme || self.preference_size != size {
            self.preference_theme = theme;
            self.preference_size = size;
            self.theme = None;
            self.cursors.clear();
        }
    }

    /// The effective pair supplied by the compositor preference resolver.
    fn effective(&self) -> (String, u32) {
        (self.preference_theme.clone(), self.preference_size)
    }

    /// The cursor for a `wp_cursor_shape` value at `scale`. The bundled
    /// Bibata theme guarantees a result for every standard shape, so this
    /// returns `None` only if rasterization itself fails.
    pub fn get(&mut self, device: &flux::Device, shape: u32, scale: f32) -> Option<&LoadedCursor> {
        let (name, size) = self.effective();
        if !matches!(&self.theme, Some((n, s, _)) if *n == name && *s == size) {
            let theme = CursorTheme::resolve(&name, size);
            if theme.has_filesystem() {
                log::info!(
                    "cursor: using SVG theme {name:?} at {size} logical px; missing shapes fall back to bundled Bibata-Modern-Ice"
                );
            } else {
                log::info!(
                    "cursor: theme {name:?} not installed; using bundled Bibata-Modern-Ice at {size} logical px"
                );
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
    fn svg_meta_reads_hotspot_and_viewbox() {
        let svg = b"<svg width=\"256\" height=\"256\" viewBox=\"0 0 256 256\" \
                   data-hotspot-x=\"55\" data-hotspot-y=\"17\"><path/></svg>";
        let meta = svg_meta(svg);
        assert_eq!(meta.hotspot, (55.0, 17.0));
        assert_eq!(meta.native, (256.0, 256.0));
    }

    #[test]
    fn svg_meta_defaults_when_unstamped() {
        let svg = b"<svg viewBox=\"0 0 32 32\"><circle/></svg>";
        let meta = svg_meta(svg);
        assert_eq!(meta.hotspot, (0.0, 0.0), "unstamped -> top-left");
        assert_eq!(meta.native, (32.0, 32.0));
    }

    #[test]
    fn resolution_always_appends_the_bundled_fallback() {
        // No filesystem theme by this name exists on the test host, yet the
        // chain must still resolve a cursor via the bundled theme.
        let theme = CursorTheme::resolve("aegis-nonexistent-theme-xyz", 24);
        assert!(
            theme.chain.iter().any(|s| matches!(s, Source::Bundled)),
            "bundled fallback is always present"
        );
    }

    #[test]
    fn bundled_theme_loads_default_cursor() {
        let theme = CursorTheme::resolve("aegis-nonexistent-theme-xyz", 24);
        let img = theme
            .load_shape(1, 1.0)
            .expect("bundled default (left_ptr) cursor rasterizes");
        assert_eq!(img.width, 24);
        assert_eq!(img.height, 24);
        assert_eq!(img.pixels.len(), (img.width * img.height * 4) as usize);
        assert!(img.xhot < img.width && img.yhot < img.height);
        // The rasterizer must produce visible art, not a correctly-sized
        // blank buffer (guards against a silent transform/render bug). BGRA:
        // alpha is every 4th byte.
        let opaque = img.pixels.chunks_exact(4).any(|px| px[3] > 0);
        assert!(opaque, "rasterized cursor has at least one opaque pixel");
    }

    #[test]
    fn hotspot_scales_with_render_size() {
        let theme = CursorTheme::resolve("aegis-nonexistent-theme-xyz", 32);
        let img = theme.load_shape(1, 1.0).expect("bundled left_ptr");
        // left_ptr hotspot (55, 17) in 256-space -> ~6.9, ~2.1 at 32px.
        assert_eq!(img.xhot, 7, "x hot scales: 55 * 32 / 256 = 6.875 -> 7");
        assert!(img.yhot <= 3, "y hot scales: 17 * 32 / 256 = 2.125");
    }

    #[test]
    fn rasterize_clamps_oversized_requests() {
        // The source art is 256px; requesting far more must not allocate a
        // giant sprite, it is clamped to the native cap.
        let theme = CursorTheme::resolve("aegis-nonexistent-theme-xyz", 4096);
        let img = theme.load_shape(9, 4.0).expect("bundled text cursor");
        assert!(img.width <= 256);
    }
}
