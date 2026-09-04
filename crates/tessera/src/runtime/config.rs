use super::*;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct PreferenceOverrides {
    pub(super) icon_theme: Option<String>,
    pub(super) cursor_theme: Option<String>,
    pub(super) cursor_size: Option<u32>,
}

impl PreferenceOverrides {
    fn from_env() -> Self {
        Self {
            icon_theme: nonempty_env("TESSERA_ICON_THEME"),
            cursor_theme: nonempty_env("XCURSOR_THEME"),
            cursor_size: std::env::var("XCURSOR_SIZE")
                .ok()
                .and_then(|value| value.parse().ok())
                .filter(|size| (8..=128).contains(size)),
        }
    }
}

fn nonempty_env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

pub(super) fn resolve_desktop_preferences(
    config: Option<&tessera_config::Config>,
    overrides: &PreferenceOverrides,
) -> tessera_model::settings::DesktopPreferences {
    let mut preferences = config
        .map(tessera_config::Config::desktop_preferences)
        .unwrap_or_default();
    if let Some(theme) = overrides.icon_theme.as_ref() {
        preferences.icon_theme.clone_from(theme);
    }
    if let Some(theme) = overrides.cursor_theme.as_ref() {
        preferences.cursor_theme.clone_from(theme);
    }
    if let Some(size) = overrides.cursor_size {
        preferences.cursor_size = size;
    }
    preferences
}

pub(super) fn preferences_for_persistence(
    config: Option<&tessera_config::Config>,
    mut requested: tessera_model::settings::DesktopPreferences,
    overrides: &PreferenceOverrides,
) -> tessera_model::settings::DesktopPreferences {
    let configured = config
        .map(tessera_config::Config::desktop_preferences)
        .unwrap_or_default();
    // A process-start override owns its field for this session. Do not let a
    // complete effective-profile transaction accidentally copy that override
    // into persistent configuration when the user edits an unrelated field.
    if overrides.icon_theme.is_some() {
        requested.icon_theme = configured.icon_theme;
    }
    if overrides.cursor_theme.is_some() {
        requested.cursor_theme = configured.cursor_theme;
    }
    if overrides.cursor_size.is_some() {
        requested.cursor_size = configured.cursor_size;
    }
    requested
}

/// Resolve Tessera config plus the small, documented set of explicit startup
/// overrides. Toolkit or foreign-desktop settings stores are never consulted.
pub(super) fn effective_desktop_preferences(
    config: Option<&tessera_config::Config>,
) -> tessera_model::settings::DesktopPreferences {
    resolve_desktop_preferences(config, &PreferenceOverrides::from_env())
}

/// Whether the legacy source override makes the persistent `[wallpaper]`
/// section inactive for this process. The optional model override is
/// field-specific and can still be layered over a configured 2D source.
pub(super) fn wallpaper_source_overridden() -> bool {
    nonempty_env("TESSERA_WALLPAPER").is_some()
}

/// Build the effective wallpaper from the persistent mode model plus the
/// backwards-compatible process environment. Relative configured paths are
/// resolved beside `config.toml`; environment paths retain their historical
/// current-working-directory semantics.
pub(super) fn load_wallpaper(
    config: Option<&tessera_config::Config>,
    config_path: Option<&std::path::Path>,
    device: &flux::Device,
    surface: &flux::Surface,
    target_size: (u32, u32),
    bundled_image: &[u8],
) -> Result<(tessera_wallpaper::Wallpaper, String), tessera_wallpaper::Error> {
    let source_override = nonempty_env("TESSERA_WALLPAPER");
    let model_override = nonempty_env("TESSERA_WALLPAPER_MODEL");
    let override_is_gltf = source_override
        .as_deref()
        .is_some_and(|path| has_extension(path, "glb"));
    let configured_is_parallax = source_override.is_none()
        && config
            .is_some_and(|config| config.wallpaper.mode == tessera_config::WallpaperMode::Parallax);

    let (mut wallpaper, mut label) = if let Some(path) = source_override.as_deref() {
        let wallpaper = if override_is_gltf {
            tessera_wallpaper::Wallpaper::from_gltf(device, surface, path)?
        } else {
            tessera_wallpaper::Wallpaper::from_path(path, target_size.0, target_size.1)?
        };
        (wallpaper, format!("environment source {path}"))
    } else {
        load_configured_wallpaper(
            config.map(|config| &config.wallpaper),
            config_path,
            device,
            surface,
            target_size,
            bundled_image,
        )?
    };

    if !override_is_gltf
        && !configured_is_parallax
        && let Some(model) = model_override.as_deref()
    {
        if model == "builtin" {
            wallpaper.set_builtin_model(device, surface)?;
        } else {
            wallpaper.set_model_from_gltf(device, surface, model)?;
        }
        label.push_str(&format!(" + environment model {model}"));
    } else if configured_is_parallax && model_override.is_some() {
        log::warn!("wallpaper: TESSERA_WALLPAPER_MODEL ignored for explicit parallax mode");
    }
    Ok((wallpaper, label))
}

fn load_configured_wallpaper(
    config: Option<&tessera_config::WallpaperConfig>,
    config_path: Option<&std::path::Path>,
    device: &flux::Device,
    surface: &flux::Surface,
    target_size: (u32, u32),
    bundled_image: &[u8],
) -> Result<(tessera_wallpaper::Wallpaper, String), tessera_wallpaper::Error> {
    let defaults = tessera_config::WallpaperConfig::default();
    let config = config.unwrap_or(&defaults);
    use tessera_config::WallpaperMode;

    match config.mode {
        WallpaperMode::Image => {
            if let Some(path) = config.source.as_deref() {
                let path = configured_asset_path(config_path, path);
                let wallpaper = tessera_wallpaper::Wallpaper::from_image_path(&path)?;
                Ok((wallpaper, format!("image {}", path.display())))
            } else {
                Ok((
                    tessera_wallpaper::Wallpaper::from_static_image_bytes(
                        bundled_image,
                        "bundled procedural-generation.png",
                    )?,
                    "bundled image".into(),
                ))
            }
        }
        WallpaperMode::Video => {
            let path = configured_asset_path(
                config_path,
                config
                    .source
                    .as_deref()
                    .expect("validated video wallpaper source"),
            );
            let wallpaper =
                tessera_wallpaper::Wallpaper::from_video_path(&path, target_size.0, target_size.1)?;
            Ok((wallpaper, format!("video {}", path.display())))
        }
        WallpaperMode::ThreeD => {
            let model = config
                .source
                .as_deref()
                .expect("validated 3D wallpaper source");
            let mut wallpaper = if let Some(background) = config.background.as_deref() {
                let background = configured_asset_path(config_path, background);
                tessera_wallpaper::Wallpaper::from_path(&background, target_size.0, target_size.1)?
            } else if model == "builtin" {
                return Ok((
                    tessera_wallpaper::Wallpaper::from_builtin_model(device, surface)?,
                    "3d builtin".into(),
                ));
            } else {
                let path = configured_asset_path(config_path, model);
                return Ok((
                    tessera_wallpaper::Wallpaper::from_gltf(device, surface, &path)?,
                    format!("3d {}", path.display()),
                ));
            };
            if model == "builtin" {
                wallpaper.set_builtin_model(device, surface)?;
            } else {
                let path = configured_asset_path(config_path, model);
                wallpaper.set_model_from_gltf(device, surface, &path)?;
            }
            Ok((wallpaper, format!("3d {model} with background")))
        }
        WallpaperMode::Parallax => {
            let layers = config
                .layers
                .iter()
                .map(|layer| {
                    tessera_wallpaper::ParallaxLayerSpec::new(
                        configured_asset_path(config_path, &layer.path),
                        layer.depth,
                    )
                })
                .collect::<Vec<_>>();
            let wallpaper = tessera_wallpaper::Wallpaper::from_parallax_layers(
                &layers,
                tessera_wallpaper::ParallaxOptions {
                    max_shift: config.max_shift,
                    transition: std::time::Duration::from_millis(config.transition_ms.into()),
                },
            )?;
            Ok((wallpaper, format!("parallax ({} layers)", layers.len())))
        }
    }
}

fn configured_asset_path(config_path: Option<&std::path::Path>, value: &str) -> std::path::PathBuf {
    let path = std::path::Path::new(value);
    if path.is_absolute() {
        return path.to_path_buf();
    }
    config_path
        .and_then(std::path::Path::parent)
        .unwrap_or_else(|| std::path::Path::new("."))
        .join(path)
}

fn has_extension(path: &str, wanted: &str) -> bool {
    std::path::Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case(wanted))
}

/// Resolve `$TESSERA_BACKEND`, defaulting to `auto`.
///
/// Backend selection is process environment because it describes the launch
/// environment, not an Tessera command. X11/XWayland are intentionally not
/// accepted backends. The informational options (`--help`/`--version`) are
/// handled in `main` before this runs; any other argument is rejected there.
pub(super) fn requested_backend() -> Result<BackendKind, Box<dyn std::error::Error>> {
    let selected = std::env::var("TESSERA_BACKEND").unwrap_or_else(|_| "auto".to_owned());
    Ok(selected.parse()?)
}

/// `[[output]]` mode requests as the backend's connector → `ModeSpec` map
/// (ADR-0028). Entries without a `mode` use the connector's highest-pixel
/// mode at its highest refresh rate.
pub(super) fn configured_output_modes(
    config: Option<&tessera_config::Config>,
) -> std::collections::HashMap<String, tessera_model::output::ModeSpec> {
    config
        .map(|c| c.output_policies())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(connector, policy)| policy.mode.map(|mode| (connector, mode)))
        .collect()
}

/// `[[output]]` color policy as the backend's connector → `ColorPolicy`
/// map: the `hdr` / `deep_color` opt-ins.
pub(super) fn configured_color_policies(
    config: Option<&tessera_config::Config>,
) -> std::collections::HashMap<String, tessera_model::output::ColorPolicy> {
    config
        .map(|c| c.output_policies())
        .unwrap_or_default()
        .into_iter()
        .map(|(connector, policy)| {
            (
                connector,
                tessera_model::output::ColorPolicy {
                    hdr: policy.hdr,
                    deep_color: policy.deep_color,
                    sdr_white_nits: policy.sdr_white_nits,
                },
            )
        })
        .collect()
}

/// `[[output]]` ICC profile paths as the backend's connector → path map.
pub(super) fn configured_icc_profiles(
    config: Option<&tessera_config::Config>,
) -> std::collections::HashMap<String, String> {
    config
        .map(|c| {
            c.outputs
                .iter()
                .filter_map(|output| {
                    output
                        .icc_profile
                        .clone()
                        .map(|path| (output.connector.clone(), path))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Generate a timestamped screenshot filename inside `dir`, creating the
/// directory if it does not exist.
pub(super) fn screenshot_path(dir: &std::path::Path) -> String {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let _ = std::fs::create_dir_all(dir);
    dir.join(format!("tessera-{ms}.png"))
        .to_string_lossy()
        .into_owned()
}

/// Load the configuration from `path`, logging diagnostics on failure.
/// `None` (no path, or a file that does not exist) means "use built-in
/// defaults" and is not an error.
pub(super) fn load_config(path: Option<&std::path::Path>) -> Option<tessera_config::Config> {
    let path = path?;
    match tessera_config::load(path) {
        Ok(Some(c)) => {
            log::info!("config: loaded {}", path.display());
            Some(c)
        }
        Ok(None) => None,
        Err(e) => {
            match &e {
                tessera_config::LoadError::Invalid { diagnostics, .. } => {
                    for d in diagnostics {
                        log::warn!("config: {d}");
                    }
                }
                _ => log::warn!("config: {e}"),
            }
            log::warn!("config: using built-in defaults");
            None
        }
    }
}

/// Re-load `path` and, on success, swap in the new config and rebuild the
/// keymap and gesture map. On failure, keep the previous config and maps.
pub(super) fn reload_config(
    path: &std::path::Path,
    config: &mut Option<tessera_config::Config>,
    keymap: &mut tessera_model::keybind::Keymap,
    gesture_map: &mut tessera_model::gesture::GestureMap,
    server: &mut tessera_compositor::Server,
    shell: &mut tessera_shell::Shell,
    cursor_cache: &mut cursor::CursorCache,
) -> bool {
    let apply = |config: &Option<tessera_config::Config>,
                 server: &mut tessera_compositor::Server,
                 shell: &mut tessera_shell::Shell,
                 cursor_cache: &mut cursor::CursorCache| {
        let preferences = effective_desktop_preferences(config.as_ref());
        server.set_window_rules(
            config
                .as_ref()
                .map(|c| c.window_rules.clone())
                .unwrap_or_default(),
        );
        if let Some(c) = config.as_ref() {
            server.set_remember_window_positions(c.layout.remember_window_positions);
            server.set_minimize_animation(c.dock.minimize_animation);
            shell.set_reduced_motion(preferences.reduced_motion);
            server.set_reduced_motion(preferences.reduced_motion);
            shell.set_color_scheme(preferences.color_scheme);
            server.set_decoration_policy(c.ui.window_decorations);
            server.set_output_policies(c.output_policies());
            server.set_allow_quit_while_locked(c.dev.allow_quit_while_locked);
            server.set_keyboard_repeat(c.input.keyboard);
        } else {
            server.set_remember_window_positions(true);
            server.set_minimize_animation(tessera_model::dock::MinimizeAnimationStyle::default());
            shell.set_reduced_motion(preferences.reduced_motion);
            server.set_reduced_motion(preferences.reduced_motion);
            shell.set_color_scheme(preferences.color_scheme);
            server.set_decoration_policy(tessera_model::window::DecorationPolicy::default());
            server.set_output_policies(std::collections::HashMap::new());
            server.set_allow_quit_while_locked(false);
            server.set_keyboard_repeat(tessera_model::input::KeyboardConfig::default());
        }
        cursor_cache.set_preferences(preferences.cursor_theme, preferences.cursor_size);
    };
    match tessera_config::load(path) {
        Ok(Some(new_cfg)) => {
            log::info!("config: reloaded {}", path.display());
            *config = Some(new_cfg);
            *keymap = build_keymap(config.as_ref());
            *gesture_map = build_gesture_map(config.as_ref());
            apply(config, server, shell, cursor_cache);
            true
        }
        Ok(None) => {
            log::warn!("config: {} removed; reverting to defaults", path.display());
            *config = None;
            *keymap = build_keymap(config.as_ref());
            *gesture_map = build_gesture_map(config.as_ref());
            apply(config, server, shell, cursor_cache);
            true
        }
        Err(e) => {
            match &e {
                tessera_config::LoadError::Invalid { diagnostics, .. } => {
                    for d in diagnostics {
                        log::warn!("config: {d}");
                    }
                }
                _ => log::warn!("config: {e}"),
            }
            log::warn!("config: reload failed; keeping previous configuration");
            false
        }
    }
}

/// Persist and apply one validated System Settings display edit through the
/// same configuration path used by startup and external file changes.
/// Explicit field borrows let this run after chrome rendering while the
/// current Flux frame still borrows the unrelated presentation surface.
#[allow(clippy::too_many_arguments)]
pub(super) fn apply_display_settings(
    settings: tessera_shell::DisplaySettings,
    config_path: Option<&std::path::Path>,
    config_writer: &ConfigWriter,
    config: &mut Option<tessera_config::Config>,
    keymap: &mut tessera_model::keybind::Keymap,
    gesture_map: &mut tessera_model::gesture::GestureMap,
    server: &mut tessera_compositor::Server,
    shell: &mut tessera_shell::Shell,
    cursor_cache: &mut cursor::CursorCache,
    host: &mut Host,
    reload: &mut Option<tessera_config::ReloadWatcher>,
    live: &std::sync::Arc<LiveState>,
    system_status: &mut tessera_shell::SystemStatus,
    input_acc: &mut InputAccumulator,
) -> Result<(), String> {
    if host.name() != "drm" {
        return Err("the outer compositor owns display settings in a nested session".into());
    }
    let path = config_path
        .map(std::path::Path::to_path_buf)
        .ok_or_else(|| "no writable configuration path is available".to_owned())?;
    // Persist through the serialized config-write worker (same queue as dock
    // pins and touchpad) and block on the receipt, so this read-modify-write
    // cannot lose a concurrent edit and the settings reply stays synchronous.
    config_writer.apply_and_wait(tessera_config::ConfigEdit::SetOutput { settings })?;

    if !reload_config(
        &path,
        config,
        keymap,
        gesture_map,
        server,
        shell,
        cursor_cache,
    ) {
        return Err("the saved display configuration could not be reloaded".into());
    }
    // Reset the watcher baseline after our own atomic replacement so it does
    // not apply the same edit again on the next frame.
    *reload = Some(tessera_config::ReloadWatcher::at(&path));
    host.set_configured_modes(configured_output_modes(config.as_ref()));
    host.set_configured_color_policies(configured_color_policies(config.as_ref()));
    host.set_configured_icc_profiles(configured_icc_profiles(config.as_ref()));
    server.set_color_pipeline(host.color_pipeline());
    server.set_outputs(host.output_infos());
    let outputs = server.output_infos();
    live.set_outputs(outputs.clone());
    if let Some(logical) = outputs.first().map(|output| output.geometry.logical_size()) {
        input_acc.display_size = (logical.w.max(1) as f32, logical.h.max(1) as f32);
    }
    system_status.display = tessera_shell::DisplayStatus {
        configurable: true,
        outputs,
        error: None,
    };
    Ok(())
}

/// Persist a complete desktop-preferences transaction and feed the resolved
/// values back through the same reload path used by external file edits.
#[allow(clippy::too_many_arguments)]
pub(super) fn apply_desktop_preferences(
    preferences: tessera_model::settings::DesktopPreferences,
    config_path: Option<&std::path::Path>,
    config_writer: &ConfigWriter,
    config: &mut Option<tessera_config::Config>,
    keymap: &mut tessera_model::keybind::Keymap,
    gesture_map: &mut tessera_model::gesture::GestureMap,
    server: &mut tessera_compositor::Server,
    shell: &mut tessera_shell::Shell,
    cursor_cache: &mut cursor::CursorCache,
    reload: &mut Option<tessera_config::ReloadWatcher>,
) -> Result<(), String> {
    let path = config_path
        .map(std::path::Path::to_path_buf)
        .ok_or_else(|| "no writable configuration path is available".to_owned())?;
    let preferences = preferences_for_persistence(
        config.as_ref(),
        preferences,
        &PreferenceOverrides::from_env(),
    );
    config_writer
        .apply_and_wait(tessera_config::ConfigEdit::SetDesktopPreferences { preferences })
        .map_err(|error| format!("failed to persist desktop preferences: {error}"))?;
    if !reload_config(
        &path,
        config,
        keymap,
        gesture_map,
        server,
        shell,
        cursor_cache,
    ) {
        return Err("the saved desktop preferences could not be reloaded".into());
    }
    *reload = Some(tessera_config::ReloadWatcher::at(&path));
    Ok(())
}

/// Build the nested output geometry from its logical surface size and the
/// host's preferred render scale. `wl_output.mode` is expressed in physical
/// pixels while xdg-output derives the original logical size by dividing by
/// `scale`; keeping both in one constructor prevents the two coordinate spaces
/// from silently drifting apart.
#[cfg(test)]
pub(super) fn output_geometry_from_host(
    logical_w: i32,
    logical_h: i32,
    scale: f32,
) -> tessera_model::output::OutputGeometry {
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    tessera_model::output::OutputGeometry {
        mode: tessera_model::output::OutputMode {
            width: (logical_w.max(1) as f32 * scale).round() as i32,
            height: (logical_h.max(1) as f32 * scale).round() as i32,
            refresh_mhz: 0,
        },
        scale: tessera_model::output::Scale(scale),
        transform: tessera_model::Transform::Normal,
        logical_origin: tessera_model::Point::default(),
    }
}

/// Build the active keymap from the config file's `[[keybind]]` entries,
/// layered over the built-in defaults.
pub(super) fn build_keymap(config: Option<&tessera_config::Config>) -> tessera_model::keybind::Keymap {
    let mut overrides: Vec<tessera_model::keybind::Keybind> = Vec::new();

    if let Some(cfg) = config {
        let (cfg_binds, errs) = cfg.resolve_keybinds();
        for e in &errs {
            log::warn!("config: {e}");
        }
        for binding in cfg_binds {
            if let Some(feature) = unavailable_key_action_feature(binding.action) {
                log::warn!(
                    "config: key binding action {:?} requires the disabled '{feature}' feature",
                    binding.action
                );
            } else {
                overrides.push(binding);
            }
        }
    }

    if !overrides.is_empty() {
        log::debug!("keybinds: {} override(s) applied", overrides.len());
    }
    tessera_model::keybind::Keymap::defaults()
        .with_overrides(overrides)
        .retain_actions(|action| unavailable_key_action_feature(action).is_none())
}

const fn unavailable_key_action_feature(
    action: tessera_model::keybind::Action,
) -> Option<&'static str> {
    match action {
        tessera_model::keybind::Action::TogglePrism if !cfg!(feature = "chrome-prism") => {
            Some("chrome-prism")
        }
        tessera_model::keybind::Action::ToggleCommandPanel
            if !cfg!(feature = "chrome-command-panel") =>
        {
            Some("chrome-command-panel")
        }
        _ => None,
    }
}

/// Build the active gesture map from the config file's `[[gesture]]`
/// entries, layered over the built-in defaults.
pub(super) fn build_gesture_map(
    config: Option<&tessera_config::Config>,
) -> tessera_model::gesture::GestureMap {
    let mut overrides: Vec<tessera_model::gesture::GestureBinding> = Vec::new();

    if let Some(cfg) = config {
        let (cfg_binds, errs) = cfg.resolve_gestures();
        for e in &errs {
            log::warn!("config: {e}");
        }
        for binding in cfg_binds {
            if let Some(feature) = unavailable_gesture_action_feature(binding.action) {
                log::warn!(
                    "config: gesture action {:?} requires the disabled '{feature}' feature",
                    binding.action
                );
            } else {
                overrides.push(binding);
            }
        }
    }

    if !overrides.is_empty() {
        log::debug!("gestures: {} override(s) applied", overrides.len());
    }
    tessera_model::gesture::GestureMap::defaults()
        .with_overrides(overrides)
        .retain_actions(|action| unavailable_gesture_action_feature(action).is_none())
}

const fn unavailable_gesture_action_feature(
    action: tessera_model::gesture::GestureAction,
) -> Option<&'static str> {
    match action {
        tessera_model::gesture::GestureAction::CommandPanel
            if !cfg!(feature = "chrome-command-panel") =>
        {
            Some("chrome-command-panel")
        }
        _ => None,
    }
}

/// Built-in trusted named IPC scopes for ordinary owner control, agent
/// administration, Interaction Domain administration, and portal consent (ADR-0090).
/// Config-declared agent scopes were removed in protocol v18 (ADR-0088);
/// agent capability ceilings now come from the compositor-held principal
/// registry.
pub(super) fn builtin_ipc_scopes() -> std::collections::HashMap<String, tessera_ipc::Scope> {
    std::collections::HashMap::from([
        (
            tessera_ipc::LOCAL_OWNER_ADMIN_SCOPE.to_string(),
            tessera_ipc::Scope {
                windows: None,
                workspaces: None,
                outputs: None,
                interaction_domains: None,
                ops: Some(vec![
                    tessera_ipc::ActorCapability::Focus,
                    tessera_ipc::ActorCapability::Minimize,
                    tessera_ipc::ActorCapability::Close,
                    tessera_ipc::ActorCapability::SetWindowGeometry,
                    tessera_ipc::ActorCapability::SwitchWorkspace,
                    tessera_ipc::ActorCapability::SwitchWorkspaceTo,
                    tessera_ipc::ActorCapability::MoveToWorkspace,
                    tessera_ipc::ActorCapability::SystemControl,
                    tessera_ipc::ActorCapability::Notify,
                    tessera_ipc::ActorCapability::DismissNotification,
                    tessera_ipc::ActorCapability::Screenshot,
                    tessera_ipc::ActorCapability::ScreenshotRegion,
                    tessera_ipc::ActorCapability::ToggleOverview,
                ]),
                ask_ops: None,
            },
        ),
        (
            tessera_ipc::LOCAL_AGENT_ADMIN_SCOPE.to_string(),
            tessera_ipc::Scope {
                // Agent administration is authorized by the dedicated scope
                // name in the IPC server, not by a reusable operation family.
                ops: Some(Vec::new()),
                ..tessera_ipc::Scope::default()
            },
        ),
        (
            tessera_ipc::LOCAL_INTERACTION_DOMAIN_ADMIN_SCOPE.to_string(),
            tessera_ipc::Scope {
                windows: None,
                workspaces: None,
                outputs: None,
                interaction_domains: None,
                ops: Some(vec![
                    tessera_ipc::ActorCapability::InjectInteractionDomainInput,
                    tessera_ipc::ActorCapability::CreateInteractionDomain,
                    tessera_ipc::ActorCapability::TransactInteractionDomain,
                    tessera_ipc::ActorCapability::RevokeInteractionDomain,
                    tessera_ipc::ActorCapability::CaptureInteractionDomain,
                    tessera_ipc::ActorCapability::ObserveInteractionDomain,
                    tessera_ipc::ActorCapability::LaunchInInteractionDomain,
                ]),
                ask_ops: None,
            },
        ),
        // The portal backend (ADR-0075; mechanisms ADR-0052/0054) serves
        // compositor-owned capture, inhibit, target/app/consent prompts, and
        // notification/wallpaper actions through explicit fail-closed ops.
        // FileChooser is intentionally absent: its prompter is a portal-owned
        // Wayland client and no filesystem path crosses compositor IPC.
        // PromptSecret is intentionally absent too (ADR-0112): the portal
        // owns its secret vault prompts in a supervised GTK prompter and no
        // production client may trigger a compositor-hosted secret prompt.
        (
            tessera_ipc::LOCAL_PORTAL_SCOPE.to_string(),
            tessera_ipc::Scope {
                windows: None,
                workspaces: None,
                outputs: None,
                interaction_domains: None,
                ops: Some(vec![
                    tessera_ipc::ActorCapability::CaptureOutput,
                    tessera_ipc::ActorCapability::StreamOutput,
                    tessera_ipc::ActorCapability::IdleInhibit,
                    tessera_ipc::ActorCapability::PickTarget,
                    tessera_ipc::ActorCapability::PickApp,
                    tessera_ipc::ActorCapability::Notify,
                    tessera_ipc::ActorCapability::DismissNotification,
                    tessera_ipc::ActorCapability::PickConfirm,
                    tessera_ipc::ActorCapability::SetWallpaper,
                ]),
                ask_ops: None,
            },
        ),
    ])
}

/// The compiled-in executable allowlist of a built-in IPC scope (ADR-0128):
/// the canonicalized `/proc/<pid>/exe` values that may claim it. `None` for
/// names that are not built-in scopes. A `[ipc.scope_executables]` entry
/// replaces these defaults for its scope.
pub(super) fn builtin_scope_executables(name: &str) -> Option<Vec<std::path::PathBuf>> {
    let executable = |dir: &str, file: &str| std::path::PathBuf::from(dir).join(file);
    let paths: Vec<std::path::PathBuf> = match name {
        tessera_ipc::LOCAL_PORTAL_SCOPE => ["/usr/bin", "/usr/libexec", "/usr/lib", "/usr/local/bin"]
            .into_iter()
            .map(|dir| executable(dir, "xdg-desktop-portal-atrium"))
            .collect(),
        tessera_ipc::LOCAL_OWNER_ADMIN_SCOPE
        | tessera_ipc::LOCAL_AGENT_ADMIN_SCOPE
        | tessera_ipc::LOCAL_INTERACTION_DOMAIN_ADMIN_SCOPE => ["/usr/bin", "/usr/local/bin"]
            .into_iter()
            .map(|dir| executable(dir, "tessera"))
            .collect(),
        _ => return None,
    };
    Some(paths)
}

/// Whether `peer_exe` — the peer's canonicalized `/proc/<pid>/exe` — appears
/// in a scope allowlist. An entry matches literally, or through its own
/// canonicalization so a distribution symlink (e.g. `/usr/bin/foo` →
/// `/usr/libexec/foo`) satisfies an entry for either spelling; an entry that
/// does not resolve (the component is not installed there) still compares
/// literally.
pub(super) fn scope_exe_permitted(
    allowlist: &[std::path::PathBuf],
    peer_exe: &std::path::Path,
) -> bool {
    allowlist.iter().any(|entry| {
        entry == peer_exe
            || std::fs::canonicalize(entry)
                .as_deref()
                .is_ok_and(|canonical| canonical == peer_exe)
    })
}

pub(super) fn authorize_interaction_domain_action_against_snapshot(
    scope: &tessera_ipc::Scope,
    action: &tessera_ipc::InteractionDomainAction,
    snapshot: &tessera_model::interaction_domain::InteractionDomainSnapshot,
) -> Result<(), String> {
    if !scope.permits_interaction_domain_action(action) {
        return Err("out of scope".into());
    }
    authorize_interaction_domain_action_groups_against_snapshot(scope, action, snapshot)
}

/// Reauthorization for an Interaction Domain action whose operation family was approved
/// by a runtime grant (ADR-0088): identical to
/// [`authorize_interaction_domain_action_against_snapshot`] except the operation
/// allowlist is already satisfied by the grant, so only the resource
/// allowlists (and the interaction-group smuggling guard) apply.
pub(super) fn authorize_interaction_domain_action_granted_against_snapshot(
    scope: &tessera_ipc::Scope,
    action: &tessera_ipc::InteractionDomainAction,
    snapshot: &tessera_model::interaction_domain::InteractionDomainSnapshot,
) -> Result<(), String> {
    if !scope.permits_interaction_domain_action_resources(action) {
        return Err("out of scope".into());
    }
    authorize_interaction_domain_action_groups_against_snapshot(scope, action, snapshot)
}

/// The interaction-group smuggling guard shared by both Interaction Domain
/// reauthorization paths: a group-level mutation expands to every affected
/// window, so an allowlisted member cannot smuggle sibling windows across
/// interaction domains.
fn authorize_interaction_domain_action_groups_against_snapshot(
    scope: &tessera_ipc::Scope,
    action: &tessera_ipc::InteractionDomainAction,
    snapshot: &tessera_model::interaction_domain::InteractionDomainSnapshot,
) -> Result<(), String> {
    let tessera_ipc::InteractionDomainAction::Transact { mutations, .. } = action else {
        return Ok(());
    };
    for mutation in mutations {
        let group = match mutation {
            tessera_model::interaction_domain::InteractionDomainMutation::TransferWindow {
                window,
                ..
            } => snapshot
                .interaction_groups
                .iter()
                .find(|group| group.windows.contains(window)),
            tessera_model::interaction_domain::InteractionDomainMutation::SetObserver {
                group,
                ..
            } => snapshot
                .interaction_groups
                .iter()
                .find(|candidate| candidate.id == *group),
            tessera_model::interaction_domain::InteractionDomainMutation::ConfigureOutput {
                ..
            }
            | tessera_model::interaction_domain::InteractionDomainMutation::SetState { .. } => None,
        };
        if group.is_some_and(|group| {
            group
                .windows
                .iter()
                .any(|window| !scope.permits_window(*window))
        }) {
            return Err(
                "out of scope: Interaction Domain mutation affects another interaction-group window"
                    .into(),
            );
        }
    }
    Ok(())
}
