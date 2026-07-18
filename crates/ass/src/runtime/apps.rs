/// Decoded application-icon textures for the dock. `_images` owns the GPU
/// textures; `map` keys raw pointers (borrowed from `_images`) by every
/// `app_id` the entry might run as. The cache must outlive the shell, which
/// holds clones of the pointers in its dock component.
pub(super) struct IconCache {
    pub(super) _images: Vec<flux::Image>,
    pub(super) map: std::collections::HashMap<String, *mut std::ffi::c_void>,
}

/// Raster extensions the `image` crate decodes directly. SVG/SVGZ uses the
/// standard librsvg command-line rasterizer when installed and otherwise
/// falls back to the dock glyph without failing startup.
pub(super) const RASTER_ICON_EXTS: &[&str] =
    &["png", "jpg", "jpeg", "webp", "bmp", "gif", "tiff", "ico"];
pub(super) const SVG_ICON_EXTS: &[&str] = &["svg", "svgz"];
pub(super) const HUD_SYMBOLIC_ICON_NAMES: &[&str] = &[
    "audio-volume-muted-symbolic",
    "audio-volume-low-symbolic",
    "audio-volume-medium-symbolic",
    "audio-volume-high-symbolic",
    "network-wireless-signal-excellent-symbolic",
    "network-wired-symbolic",
    "network-offline-symbolic",
    "preferences-system-notifications-symbolic",
    "preferences-system-symbolic",
    "window-close-symbolic",
    "application-x-executable-symbolic",
];

/// Resolve the host's selected application icon theme. An explicit ass
/// override wins; otherwise query the GTK/GSettings desktop preference used
/// by niri and other toolkit-neutral Wayland sessions. `hicolor` remains the
/// portable fallback when GSettings is unavailable.
pub(super) fn selected_icon_theme() -> String {
    if let Some(theme) = std::env::var("ASS_ICON_THEME")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    {
        return theme;
    }

    let output = std::process::Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", "icon-theme"])
        .output();
    output
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .and_then(|value| parse_gsettings_string(&value))
        .unwrap_or_else(|| ass_apps::DEFAULT_ICON_THEME.to_string())
}

/// Merge XDG applications with compositor-owned system applications. Built-in
/// entries deliberately use the same `Entry` model so launcher search,
/// context menus, pinning, and icon lookup have one catalog contract.
pub(super) fn application_catalog(icon_theme: &str, icon_scale: u32) -> Vec<ass_core::app::Entry> {
    let mut applications = ass_apps::enumerate_with_theme_and_scale(icon_theme, icon_scale.max(1));
    let i18n = ass_shell::Localizer::from_env();
    applications.push(ass_core::app::Entry::control_center(
        i18n.text(ass_shell::Message::ControlCenter),
        i18n.text(ass_shell::Message::BuiltInSystemApp),
    ));
    applications
}

pub(super) fn parse_gsettings_string(value: &str) -> Option<String> {
    let value = value.trim();
    let unquoted = value
        .strip_prefix('\'')
        .and_then(|value| value.strip_suffix('\''))
        .or_else(|| {
            value
                .strip_prefix('"')
                .and_then(|value| value.strip_suffix('"'))
        })
        .unwrap_or(value)
        .trim();
    (!unquoted.is_empty()).then(|| unquoted.to_string())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct IconFileStamp {
    len: u64,
    modified_seconds: i64,
    modified_nanoseconds: i64,
    device: u64,
    inode: u64,
}

/// Snapshot only icons the catalog actually uses. Metadata follows symlinks,
/// so a Flatpak `current/active` update is noticed even when the exported icon
/// path itself remains unchanged.
pub(super) fn snapshot_icons(
    apps: &[ass_core::app::Entry],
) -> std::collections::BTreeMap<std::path::PathBuf, Option<IconFileStamp>> {
    use std::os::unix::fs::MetadataExt;

    let mut snapshot = std::collections::BTreeMap::new();
    for path in apps.iter().filter_map(|entry| entry.icon_path.as_ref()) {
        snapshot.entry(path.clone()).or_insert_with(|| {
            std::fs::metadata(path).ok().map(|metadata| IconFileStamp {
                len: metadata.len(),
                modified_seconds: metadata.mtime(),
                modified_nanoseconds: metadata.mtime_nsec(),
                device: metadata.dev(),
                inode: metadata.ino(),
            })
        });
    }
    snapshot
}

/// The lowercased ids an entry might be matched by: its `StartupWMClass`, the
/// desktop-file stem, and the declared icon name. These are the same keys
/// [`build_icon_cache`] files icons under, so a dock tile can both find its
/// icon and fold a running toplevel (matched by `app_id`) into itself.
pub(super) fn app_keys(entry: &ass_core::app::Entry) -> Vec<String> {
    let mut keys = Vec::new();
    let mut push = |s: &str| {
        let s = s.to_ascii_lowercase();
        if !s.is_empty() && !keys.contains(&s) {
            keys.push(s);
        }
    };
    if let Some(wm) = &entry.startup_wm_class {
        push(wm);
    }
    push(entry.id.strip_suffix(".desktop").unwrap_or(&entry.id));
    if let Some(ic) = &entry.icon {
        push(ic);
    }
    keys
}

/// How many apps to auto-pin to the dock when the config pins none, so the bar
/// is populated with real XDG icons out of the box rather than empty.
pub(super) const DEFAULT_PINNED_MAX: usize = 12;

/// Build the dock's pinned app list. When `pinned` names apps, each name is
/// resolved against the enumerated entries by id / desktop-stem / WM class /
/// icon name (case-insensitive), in the order given; unresolved names are
/// logged and skipped. When `pinned` is empty and `autopopulate` is set, the
/// first [`DEFAULT_PINNED_MAX`] apps that have a decoded icon are pinned
/// automatically; with `autopopulate` off, an empty list stays empty (the
/// user's manual "no pins" choice).
pub(super) fn build_dock_apps(
    apps: &[ass_core::app::Entry],
    icons: &std::collections::HashMap<String, *mut std::ffi::c_void>,
    pinned: &[String],
    autopopulate: bool,
) -> Vec<ass_shell::DockApp> {
    let make = |entry: &ass_core::app::Entry| ass_shell::DockApp {
        entry: entry.clone(),
        keys: app_keys(entry),
    };
    if pinned.is_empty() {
        if !autopopulate {
            return Vec::new();
        }
        return apps
            .iter()
            .filter(|e| app_keys(e).iter().any(|k| icons.contains_key(k)))
            .take(DEFAULT_PINNED_MAX)
            .map(make)
            .collect();
    }
    let mut out = Vec::with_capacity(pinned.len());
    for name in pinned {
        let want = name.to_ascii_lowercase();
        match apps.iter().find(|e| app_keys(e).contains(&want)) {
            Some(e) => out.push(make(e)),
            None => log::warn!("dock: pinned app '{name}' not found among enumerated entries"),
        }
    }
    out
}

/// Decode each app entry's icon into a flux texture, keyed by every id the
/// window might report as `app_id` (StartupWMClass, the desktop-id stem, and
/// the icon name, all lowercased). The first key to claim a texture wins per
/// entry, so a texture is never double-counted.
pub(super) fn build_icon_cache(
    device: &flux::Device,
    apps: &[ass_core::app::Entry],
    icon_theme: &str,
    icon_scale: u32,
) -> IconCache {
    use std::ffi::c_void;
    let mut images: Vec<flux::Image> = Vec::new();
    let mut map: std::collections::HashMap<String, *mut c_void> = std::collections::HashMap::new();

    for entry in apps {
        let Some(path) = &entry.icon_path else {
            continue;
        };
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .unwrap_or_default();
        let Some(decoded) = decode_icon(path, &ext, icon_scale) else {
            continue;
        };
        let rgba = decoded.to_rgba8();
        let (w, h) = rgba.dimensions();
        let mut bgra = rgba.into_raw();
        for chunk in bgra.chunks_exact_mut(4) {
            chunk.swap(0, 2); // RGBA8 -> BGRA8 (flux samples BGRA8_UNORM).
        }
        match flux::Image::from_bytes(device, w, h, flux::Format::FLUX_FORMAT_BGRA8_UNORM, &bgra) {
            Ok(img) => {
                let ptr = img.as_raw() as *mut c_void;
                // Key the texture under every id a window might report as its
                // `app_id`; the dock resolves both icons and running-window
                // matches through these same keys.
                for key in app_keys(entry) {
                    map.entry(key).or_insert(ptr);
                }
                images.push(img);
            }
            Err(e) => log::warn!("icon: upload failed for {}: {e:?}", path.display()),
        }
    }

    // HUD status assets come from the same icon theme as applications. SVGs
    // are rasterized at output scale (and subsequently sampled down by lens),
    // avoiding the coarse single-pixel strokes of compositor glyphs while
    // retaining the host theme's silhouettes and proportions.
    let mut symbolic_names: Vec<String> = HUD_SYMBOLIC_ICON_NAMES
        .iter()
        .map(|name| (*name).to_string())
        .collect();
    for level in (0..=100).step_by(10) {
        symbolic_names.push(format!("battery-level-{level}-symbolic"));
        symbolic_names.push(format!("battery-level-{level}-charging-symbolic"));
    }
    let mut hud_count = 0usize;
    for name in symbolic_names {
        let Some(path) =
            ass_apps::resolve_icon_scaled(&name, Some(icon_theme), &[], 24, icon_scale.max(1))
        else {
            log::debug!("hud icon: '{name}' was not found in theme '{icon_theme}'");
            continue;
        };
        let ext = path
            .extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase)
            .unwrap_or_default();
        let Some(decoded) = decode_icon(&path, &ext, icon_scale) else {
            continue;
        };
        let mut rgba = decoded.to_rgba8();
        // Symbolic themes commonly encode a dark CSS foreground intended for
        // toolkit recolouring. The compositor has no GTK style context, so
        // apply the HUD's light foreground while preserving every coverage
        // value produced by SVG antialiasing.
        for pixel in rgba.pixels_mut() {
            if pixel[3] != 0 {
                pixel[0] = 246;
                pixel[1] = 246;
                pixel[2] = 248;
            }
        }
        let (w, h) = rgba.dimensions();
        let mut bgra = rgba.into_raw();
        for chunk in bgra.chunks_exact_mut(4) {
            chunk.swap(0, 2);
        }
        match flux::Image::from_bytes(device, w, h, flux::Format::FLUX_FORMAT_BGRA8_UNORM, &bgra) {
            Ok(image) => {
                let ptr = image.as_raw() as *mut c_void;
                map.insert(format!("ass-hud:{name}"), ptr);
                if name == "preferences-system-symbolic" {
                    // Stable application-icon key for the compositor-owned
                    // control center entry and component header.
                    map.insert("ass-control-center".into(), ptr);
                }
                images.push(image);
                hud_count += 1;
            }
            Err(error) => log::warn!("hud icon: upload failed for {}: {error:?}", path.display()),
        }
    }

    log::info!(
        "icons: {} application texture(s), {hud_count} themed HUD symbol(s)",
        images.len().saturating_sub(hud_count)
    );
    IconCache {
        _images: images,
        map,
    }
}

/// Decode a desktop icon. Raster formats stay in-process; SVG is converted to
/// a bounded PNG on stdout so malformed or enormous vector sources cannot
/// dictate an unbounded GPU texture. Every failure is a normal glyph fallback.
pub(super) fn decode_icon(
    path: &std::path::Path,
    ext: &str,
    icon_scale: u32,
) -> Option<image::DynamicImage> {
    if RASTER_ICON_EXTS.contains(&ext) {
        return image::open(path).ok();
    }
    if !SVG_ICON_EXTS.contains(&ext) {
        return None;
    }
    let target = ass_apps::DEFAULT_ICON_SIZE
        .saturating_mul(icon_scale.max(1))
        .min(512)
        .to_string();
    let output = std::process::Command::new("rsvg-convert")
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
        log::debug!("icon: SVG rasterization failed for {}", path.display());
        return None;
    }
    image::load_from_memory(&output.stdout).ok()
}
