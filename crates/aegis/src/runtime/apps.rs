/// Decoded application-icon textures shared with chrome. `_images` owns the
/// GPU textures; `map` keys raw pointers (borrowed from `_images`) by every
/// `app_id` the entry might run as. The cache must outlive the shell:
/// components hold borrowed handles from the most recently pushed
/// `aegis_shell::AppCatalog` (see `aegis_shell::IconSet`), so a refreshed cache is
/// swapped in only after the new catalog has been fanned out.
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

/// Choose the raster scale from the compositor's effective output geometry.
/// The backend scale is only a fallback: direct-display `[[output]]` policy
/// can override it without changing the physical backend's native value.
pub(super) fn effective_icon_scale(output_scale: Option<f32>, backend_scale: f32) -> u32 {
    let scale = output_scale
        .filter(|scale| scale.is_finite() && *scale > 0.0)
        .or_else(|| (backend_scale.is_finite() && backend_scale > 0.0).then_some(backend_scale))
        .unwrap_or(1.0);
    scale.ceil().max(1.0) as u32
}

/// Enumerate XDG applications and append compositor-owned virtual entries.
///
/// First-party external applications use the same installed metadata path as
/// every other client. Development tooling stages that layout under `target/`
/// instead of synthesizing compositor-only external entries.
pub(super) fn application_catalog(
    icon_theme: &str,
    icon_scale: u32,
) -> Vec<aegis_core::app::Entry> {
    let mut applications =
        aegis_desktop_entries::enumerate_with_theme_and_scale(icon_theme, icon_scale.max(1));
    let i18n = aegis_shell::Localizer::from_env();
    applications.push(aegis_core::app::Entry::ai_workspaces(
        i18n.text(aegis_shell::Message::AiWorkspaces),
        i18n.text(aegis_shell::Message::AiWorkspacesDescription),
    ));
    applications
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
    apps: &[aegis_core::app::Entry],
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

/// Maximum number of apps selected when automatic population is explicitly
/// enabled for an empty pin list.
pub(super) const AUTOPOPULATE_MAX: usize = 12;

/// Resolve the dock's pinned entries. When `pinned` names apps, each name is
/// resolved against the enumerated entries by id / desktop-stem / WM class /
/// icon name (case-insensitive), in the order given; unresolved names are
/// logged and skipped. When `pinned` is empty and `autopopulate` is set, the
/// first [`AUTOPOPULATE_MAX`] apps that have a decoded icon are pinned
/// automatically; with `autopopulate` off, an empty list stays empty (the
/// user's manual "no pins" choice).
pub(super) fn resolve_pinned(
    apps: &[aegis_core::app::Entry],
    icons: &std::collections::HashMap<String, *mut std::ffi::c_void>,
    pinned: &[String],
    autopopulate: bool,
) -> Vec<aegis_core::app::Entry> {
    if pinned.is_empty() {
        if !autopopulate {
            return Vec::new();
        }
        return apps
            .iter()
            .filter(|e| e.match_keys().iter().any(|k| icons.contains_key(k)))
            .take(AUTOPOPULATE_MAX)
            .cloned()
            .collect();
    }
    let mut out = Vec::with_capacity(pinned.len());
    for name in pinned {
        match apps
            .iter()
            .find(|entry| entry_matches_pin_name(entry, name))
        {
            Some(e) => out.push(e.clone()),
            None => log::warn!("dock: pinned app '{name}' not found among enumerated entries"),
        }
    }
    out
}

/// Match every configuration spelling accepted for a persistent pin. The full
/// desktop-file id is a configuration identity, while `Entry::match_keys`
/// covers the extensionless id and runtime aliases used by windows and icons.
fn entry_matches_pin_name(entry: &aegis_core::app::Entry, name: &str) -> bool {
    entry.id.eq_ignore_ascii_case(name) || entry.match_keys().contains(&name.to_ascii_lowercase())
}

/// Apply explicit pin mutations to a configured list. Actions are idempotent:
/// pinning an existing application or unpinning an absent one is a no-op.
/// Application identity uses the same aliases as [`resolve_pinned`].
pub(super) fn apply_pin_actions(
    apps: &[aegis_core::app::Entry],
    pinned: &[String],
    actions: &[aegis_shell::PinAction],
) -> Vec<String> {
    let mut out = pinned.to_vec();
    for action in actions {
        let id = match action {
            aegis_shell::PinAction::Pin(id) | aegis_shell::PinAction::Unpin(id) => id,
        };
        let Some(entry) = apps.iter().find(|entry| entry.id == *id) else {
            continue;
        };
        let matches = |name: &String| entry_matches_pin_name(entry, name);
        match action {
            aegis_shell::PinAction::Pin(_) if !out.iter().any(matches) => {
                out.push(entry.id.clone());
            }
            aegis_shell::PinAction::Unpin(_) => out.retain(|name| !matches(name)),
            aegis_shell::PinAction::Pin(_) => {}
        }
    }
    out
}

/// Convert an opt-in automatic selection into a concrete list before its first
/// manual edit. This preserves every other visible tile when one automatic
/// tile is removed and permanently hands control to the user.
pub(super) fn materialize_pins_for_manual_edit(
    apps: &[aegis_core::app::Entry],
    icons: &std::collections::HashMap<String, *mut std::ffi::c_void>,
    pinned: &[String],
    autopopulate: bool,
) -> Vec<String> {
    if pinned.is_empty() && autopopulate {
        return resolve_pinned(apps, icons, pinned, true)
            .into_iter()
            .map(|entry| entry.id)
            .collect();
    }
    pinned.to_vec()
}

/// One icon decoded to GPU-ready BGRA8 pixels. Produced off the frame loop
/// (icon decoding forks `rsvg-convert` per SVG — far too slow for the main
/// thread); the main loop only uploads the pixels as flux textures.
pub(super) struct DecodedIcon {
    /// Every map key the texture is inserted under. For applications these
    /// are the ids a window might report as `app_id` (StartupWMClass, the
    /// desktop-id stem, and the icon name, all lowercased); for HUD symbols
    /// the `aegis-hud:*` keys.
    pub(super) keys: Vec<String>,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) bgra: Vec<u8>,
    /// HUD symbols overwrite existing keys; application icons let the first
    /// key to claim a texture win so a texture is never double-counted.
    pub(super) overwrite: bool,
}

/// Decode every application and HUD icon into raw BGRA8 pixels. Runs on the
/// app-scan worker thread: it performs no GPU work, only file I/O and
/// (for SVG) `rsvg-convert` subprocesses.
pub(super) fn decode_icons(
    apps: &[aegis_core::app::Entry],
    icon_theme: &str,
    icon_scale: u32,
) -> Vec<DecodedIcon> {
    let mut decoded = Vec::new();
    for entry in apps {
        let Some(path) = &entry.icon_path else {
            continue;
        };
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .unwrap_or_default();
        let Some(image) = decode_icon(path, &ext, icon_scale) else {
            continue;
        };
        let rgba = image.to_rgba8();
        let (w, h) = rgba.dimensions();
        let mut bgra = rgba.into_raw();
        for chunk in bgra.chunks_exact_mut(4) {
            chunk.swap(0, 2); // RGBA8 -> BGRA8 (flux samples BGRA8_UNORM).
        }
        decoded.push(DecodedIcon {
            keys: entry.match_keys(),
            width: w,
            height: h,
            bgra,
            overwrite: false,
        });
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
    for name in symbolic_names {
        let Some(path) = aegis_desktop_entries::resolve_icon_scaled(
            &name,
            Some(icon_theme),
            &[],
            24,
            icon_scale.max(1),
        ) else {
            log::debug!("hud icon: '{name}' was not found in theme '{icon_theme}'");
            continue;
        };
        let ext = path
            .extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase)
            .unwrap_or_default();
        let Some(image) = decode_icon(&path, &ext, icon_scale) else {
            continue;
        };
        let mut rgba = image.to_rgba8();
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
        let mut keys = vec![format!("aegis-hud:{name}")];
        if name == "preferences-system-symbolic" {
            // Stable keys for the external System Settings fallback and the
            // compositor-owned AI Workspaces surface.
            keys.push("aegis-settings".into());
            keys.push(aegis_core::app::AI_WORKSPACES_ID.into());
        }
        decoded.push(DecodedIcon {
            keys,
            width: w,
            height: h,
            bgra,
            overwrite: true,
        });
    }
    decoded
}

/// Upload decoded icons as flux textures, keyed by every id the window might
/// report as `app_id`. Pure GPU work: all file I/O and decoding happened in
/// [`decode_icons`] on the worker thread.
pub(super) fn build_icon_cache(device: &flux::Device, decoded: &[DecodedIcon]) -> IconCache {
    use std::ffi::c_void;
    let mut images: Vec<flux::Image> = Vec::new();
    let mut map: std::collections::HashMap<String, *mut c_void> = std::collections::HashMap::new();
    let mut hud_count = 0usize;

    for icon in decoded {
        match flux::Image::from_bytes(
            device,
            icon.width,
            icon.height,
            flux::Format::FLUX_FORMAT_BGRA8_UNORM,
            &icon.bgra,
        ) {
            Ok(img) => {
                let ptr = img.as_raw() as *mut c_void;
                // The dock resolves both icons and running-window matches
                // through these same keys.
                if icon.overwrite {
                    for key in &icon.keys {
                        map.insert(key.clone(), ptr);
                    }
                    hud_count += 1;
                } else {
                    for key in &icon.keys {
                        map.entry(key.clone()).or_insert(ptr);
                    }
                }
                images.push(img);
            }
            Err(e) => log::warn!(
                "icon: upload failed for {:?}: {e:?}",
                icon.keys.first().map(String::as_str).unwrap_or("?")
            ),
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
    let target = aegis_desktop_entries::DEFAULT_ICON_SIZE
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

#[cfg(test)]
mod tests {
    use super::*;

    fn app(id: &str) -> aegis_core::app::Entry {
        aegis_core::app::Entry {
            id: id.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn unpinning_an_absent_app_does_not_pin_it() {
        let apps = vec![app("org.example.Editor.desktop")];
        let pinned = apply_pin_actions(
            &apps,
            &[],
            &[aegis_shell::PinAction::Unpin(
                "org.example.Editor.desktop".into(),
            )],
        );
        assert!(pinned.is_empty());
    }

    #[test]
    fn configured_full_desktop_id_resolves() {
        let apps = vec![app("org.example.Editor.desktop")];
        let resolved = resolve_pinned(
            &apps,
            &std::collections::HashMap::new(),
            &["org.example.Editor.desktop".into()],
            false,
        );
        assert_eq!(resolved, apps);
    }

    #[test]
    fn pin_actions_are_idempotent_and_match_application_aliases() {
        let mut editor = app("org.example.Editor.desktop");
        editor.startup_wm_class = Some("ExampleEditor".into());
        let apps = vec![editor];

        let pinned = apply_pin_actions(
            &apps,
            &["exampleeditor".into()],
            &[aegis_shell::PinAction::Pin(
                "org.example.Editor.desktop".into(),
            )],
        );
        assert_eq!(pinned, vec!["exampleeditor"]);

        let pinned = apply_pin_actions(
            &apps,
            &pinned,
            &[aegis_shell::PinAction::Unpin(
                "org.example.Editor.desktop".into(),
            )],
        );
        assert!(pinned.is_empty());
    }

    #[test]
    fn first_manual_edit_preserves_other_opt_in_automatic_pins() {
        let apps = vec![
            app("org.example.Editor.desktop"),
            app("org.example.Terminal.desktop"),
        ];
        let mut icons = std::collections::HashMap::new();
        icons.insert("org.example.editor".into(), std::ptr::dangling_mut());
        icons.insert("org.example.terminal".into(), std::ptr::dangling_mut());

        let materialized = materialize_pins_for_manual_edit(&apps, &icons, &[], true);
        let pinned = apply_pin_actions(
            &apps,
            &materialized,
            &[aegis_shell::PinAction::Unpin(
                "org.example.Editor.desktop".into(),
            )],
        );
        assert_eq!(pinned, vec!["org.example.Terminal.desktop"]);
    }
}
