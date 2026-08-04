use super::*;
use aegis_model::input::Mods as M;
use aegis_model::keybind::Action;

#[test]
fn minimal_valid_config_loads() {
    let cfg = Config::parse("schema_version = 2\n").unwrap();
    assert_eq!(cfg.schema_version, 2);
    assert!(cfg.keybinds.is_empty());
    assert!(cfg.agent.lockdown, "agent IPC must fail closed by default");
}

#[test]
fn dev_escape_hatches_default_off_and_parse() {
    let defaults = Config::parse("schema_version = 2\n").unwrap();
    assert!(!defaults.dev.allow_quit_while_locked);

    let configured = Config::parse(
        "schema_version = 2\n\
         [dev]\n\
         allow_quit_while_locked = true\n",
    )
    .unwrap();
    assert!(configured.dev.allow_quit_while_locked);
}

#[test]
fn hud_defaults_to_enabled() {
    let cfg = Config::parse("schema_version = 2\n").unwrap();
    assert!(cfg.hud.enabled);
}

#[test]
fn appearance_defaults_to_no_preference() {
    let cfg = Config::parse("schema_version = 2\n").unwrap();
    assert_eq!(cfg.appearance.color_scheme, ColorScheme::System);
}

#[test]
fn idle_policy_defaults_parses_and_enforces_security_order() {
    let defaults = Config::parse("schema_version = 2\n").unwrap();
    assert_eq!(defaults.idle, IdleSettings::default());

    let configured = Config::parse(
        "schema_version = 2\n\
         [idle]\n\
         enabled = true\n\
         dim_after_seconds = 120\n\
         lock_after_seconds = 300\n\
         display_off_after_seconds = 360\n\
         suspend_after_seconds = 900\n\
         dim_percent = 25\n",
    )
    .unwrap();
    assert_eq!(configured.idle.lock_after_seconds, 300);
    assert_eq!(configured.idle.dim_percent, 25);

    let invalid = Config::parse(
        "schema_version = 2\n\
         [idle]\n\
         lock_after_seconds = 0\n\
         display_off_after_seconds = 60\n",
    )
    .unwrap_err();
    assert!(invalid.iter().any(|diagnostic| {
        diagnostic.field.as_deref() == Some("idle")
            && diagnostic.message.contains("locking must be enabled")
    }));
}

#[test]
fn config_store_persists_idle_policy_without_losing_other_sections() {
    let path = temp_config_path("idle-policy");
    std::fs::write(
        &path,
        "schema_version = 2\n\n# keep this\n[ui]\nreduced_motion = true\n",
    )
    .unwrap();
    let settings = IdleSettings {
        enabled: false,
        dim_after_seconds: 60,
        lock_after_seconds: 120,
        display_off_after_seconds: 180,
        suspend_after_seconds: 600,
        dim_percent: 20,
    };
    ConfigStore::new(&path)
        .apply(ConfigEdit::SetIdle { settings })
        .unwrap();
    let contents = std::fs::read_to_string(&path).unwrap();
    assert!(contents.contains("# keep this"));
    let config = load(&path).unwrap().expect("updated config");
    assert_eq!(config.idle, settings);
    assert!(config.ui.reduced_motion);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn appearance_color_scheme_parses_portal_vocabulary() {
    let cfg = Config::parse("schema_version = 2\n[appearance]\ncolor_scheme = \"dark\"\n").unwrap();
    assert_eq!(cfg.appearance.color_scheme, ColorScheme::Dark);
    let cfg =
        Config::parse("schema_version = 2\n[appearance]\ncolor_scheme = \"light\"\n").unwrap();
    assert_eq!(cfg.appearance.color_scheme, ColorScheme::Light);
    assert!(Config::parse("schema_version = 2\n[appearance]\ncolor_scheme = \"neon\"\n").is_err());
}

#[test]
fn ui_icon_theme_is_a_declared_configuration_field() {
    let cfg = Config::parse("schema_version = 2\n[ui]\nicon_theme = \"Papirus-Dark\"\n").unwrap();
    assert_eq!(cfg.ui.icon_theme.as_deref(), Some("Papirus-Dark"));
}

#[test]
fn desktop_preferences_resolve_the_complete_config_profile() {
    let cfg = Config::parse(
        "schema_version = 2\n\
         [appearance]\n\
         color_scheme = \"dark\"\n\
         accent_color = \"#3366FF\"\n\
         contrast = \"high\"\n\
         font_name = \"Inter 11\"\n\
         monospace_font_name = \"Iosevka 11\"\n\
         text_scale = 1.25\n\
         [ui]\n\
         reduced_motion = true\n\
         icon_theme = \"Papirus\"\n\
         cursor_theme = \"Bibata\"\n\
         cursor_size = 32\n",
    )
    .unwrap();
    let preferences = cfg.desktop_preferences();
    assert_eq!(preferences.color_scheme, ColorScheme::Dark);
    assert_eq!(preferences.accent_color.unwrap().to_hex(), "#3366FF");
    assert_eq!(preferences.contrast, Contrast::High);
    assert!(preferences.reduced_motion);
    assert_eq!(preferences.font_name, "Inter 11");
    assert_eq!(preferences.monospace_font_name, "Iosevka 11");
    assert_eq!(preferences.text_scale, 1.25);
    assert_eq!(preferences.icon_theme, "Papirus");
    assert_eq!(preferences.cursor_theme, "Bibata");
    assert_eq!(preferences.cursor_size, 32);
}

#[test]
fn desktop_preferences_reject_invalid_values_at_parse_time() {
    for body in [
        "[appearance]\naccent_color = \"blue\"\n",
        "[appearance]\ntext_scale = 8.0\n",
        "[appearance]\nfont_name = \"\"\n",
        "[ui]\ncursor_size = 4\n",
        "[ui]\nicon_theme = \"\"\n",
    ] {
        assert!(Config::parse(&format!("schema_version = 2\n{body}")).is_err());
    }
}

#[test]
fn config_store_persists_desktop_preferences_without_losing_ui_policy() {
    let path = temp_config_path("desktop-preferences");
    std::fs::write(
        &path,
        "schema_version = 2\n\
         # keep this policy and comment\n\
         [ui]\n\
         window_decorations = \"client-side\"\n",
    )
    .unwrap();
    let preferences = DesktopPreferences {
        color_scheme: ColorScheme::Light,
        accent_color: Some(AccentColor {
            red: 51,
            green: 102,
            blue: 255,
        }),
        contrast: Contrast::High,
        reduced_motion: true,
        font_name: "Inter 11".into(),
        monospace_font_name: "Iosevka 11".into(),
        text_scale: 1.2,
        icon_theme: "Papirus".into(),
        cursor_theme: "Bibata".into(),
        cursor_size: 36,
    };
    ConfigStore::new(&path)
        .apply(ConfigEdit::SetDesktopPreferences {
            preferences: preferences.clone(),
        })
        .unwrap();
    let contents = std::fs::read_to_string(&path).unwrap();
    assert!(contents.contains("# keep this policy and comment"));
    assert!(contents.contains("window_decorations = \"client-side\""));
    assert_eq!(
        load(&path)
            .unwrap()
            .expect("updated config")
            .desktop_preferences(),
        preferences
    );
    std::fs::remove_file(path).unwrap();
}

#[test]
fn dock_defaults_to_an_empty_user_owned_strip() {
    let cfg = Config::parse("schema_version = 2\n").unwrap();
    assert!(cfg.dock.pinned.is_empty());
    assert!(!cfg.dock.autopopulate);
}

#[test]
fn dock_autopopulation_remains_an_explicit_opt_in() {
    let cfg = Config::parse(
        "schema_version = 2\n\
             [dock]\n\
             autopopulate = true\n",
    )
    .unwrap();
    assert!(cfg.dock.autopopulate);
}

#[test]
fn hud_can_be_disabled() {
    let cfg = Config::parse("schema_version = 2\n[hud]\nenabled = false\n").unwrap();
    assert!(!cfg.hud.enabled);
}

fn temp_config_path(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "aegis-config-test-{}-{tag}.toml",
        std::process::id()
    ))
}

#[test]
fn config_store_creates_a_loadable_config_for_dock_pins() {
    let path = temp_config_path("create");
    let _ = std::fs::remove_file(&path);
    ConfigStore::new(&path)
        .apply(ConfigEdit::SetDockPinned {
            pinned: vec!["foot.desktop".to_string(), "firefox".to_string()],
        })
        .unwrap();
    let cfg = load(&path).unwrap().expect("file written");
    assert_eq!(cfg.dock.pinned, vec!["foot.desktop", "firefox"]);
    assert!(
        !cfg.dock.autopopulate,
        "manual control disables the fallback"
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn config_store_preserves_other_content_and_comments() {
    let path = temp_config_path("preserve");
    let original = "schema_version = 2\n\n# my apps\n[dock]\npinned = [\"a.desktop\"]\n\n[ui]\nreduced_motion = true\n";
    std::fs::write(&path, original).unwrap();
    ConfigStore::new(&path)
        .apply(ConfigEdit::SetDockPinned {
            pinned: vec!["b.desktop".to_string()],
        })
        .unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("# my apps"), "comment survives: {text}");
    let cfg = load(&path).unwrap().expect("file still valid");
    assert_eq!(cfg.dock.pinned, vec!["b.desktop"]);
    assert!(cfg.ui.reduced_motion, "untouched section survives");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn config_store_does_not_overwrite_invalid_toml() {
    let path = temp_config_path("invalid");
    std::fs::write(&path, "schema_version = [unterminated\n").unwrap();
    let err = ConfigStore::new(&path)
        .apply(ConfigEdit::SetDockPinned {
            pinned: vec!["a.desktop".to_string()],
        })
        .unwrap_err();
    assert!(matches!(err, LoadError::Invalid { .. }), "{err}");
    // The invalid file must not be overwritten.
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "schema_version = [unterminated\n"
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn config_store_validates_the_complete_document_before_replacing_it() {
    let path = temp_config_path("invalid-schema");
    let original = "schema_version = 99\n";
    std::fs::write(&path, original).unwrap();
    let err = ConfigStore::new(&path)
        .apply(ConfigEdit::SetDockPinned {
            pinned: vec!["a.desktop".to_string()],
        })
        .unwrap_err();
    assert!(matches!(err, LoadError::Invalid { .. }), "{err}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn config_store_rejects_an_invalid_edit_without_replacing_a_valid_document() {
    let path = temp_config_path("invalid-edit");
    let original = "schema_version = 2\n\n# keep this\n[ui]\nreduced_motion = true\n";
    std::fs::write(&path, original).unwrap();
    let err = ConfigStore::new(&path)
        .apply(ConfigEdit::SetOutput {
            settings: aegis_model::settings::DisplaySettings {
                connector: String::new(),
                mode: aegis_model::output::ModeSpec {
                    width: 1920,
                    height: 1080,
                    refresh_hz: Some(60),
                },
                scale: 5.0,
                position: aegis_model::Point::default(),
                primary: true,
            },
        })
        .unwrap_err();
    assert!(matches!(err, LoadError::Invalid { .. }), "{err}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn config_store_composes_typed_edits_without_losing_fields() {
    let path = temp_config_path("composed-edits");
    let _ = std::fs::remove_file(&path);
    let store = ConfigStore::new(&path);
    store
        .apply(ConfigEdit::SetDockPinned {
            pinned: vec!["foot.desktop".to_string()],
        })
        .unwrap();
    let touchpad = TouchpadConfig {
        natural_scroll: true,
        pointer_speed: 0.25,
        ..TouchpadConfig::default()
    };
    store
        .apply(ConfigEdit::SetTouchpad { config: touchpad })
        .unwrap();
    store
        .apply(ConfigEdit::SetOutput {
            settings: aegis_model::settings::DisplaySettings {
                connector: "DP-1".into(),
                mode: aegis_model::output::ModeSpec {
                    width: 1920,
                    height: 1080,
                    refresh_hz: Some(60),
                },
                scale: 1.0,
                position: aegis_model::Point::default(),
                primary: true,
            },
        })
        .unwrap();

    let config = store.load().unwrap().unwrap();
    assert_eq!(store.path(), path);
    assert_eq!(config.dock.pinned, vec!["foot.desktop"]);
    assert_eq!(config.input.touchpad, touchpad);
    assert_eq!(config.outputs.len(), 1);
    assert_eq!(config.outputs[0].connector, "DP-1");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn touchpad_config_parses_defaults_and_rejects_bad_speed() {
    let defaults = Config::parse("schema_version = 2\n").unwrap();
    assert_eq!(defaults.input.touchpad, TouchpadConfig::default());
    assert!(defaults.input.touchpad.natural_scroll);

    let cfg = Config::parse(
        "schema_version = 2\n\
             [input.touchpad]\n\
             natural_scroll = true\n\
             tap_to_click = false\n\
             tap_and_drag = false\n\
             drag_lock = true\n\
             disable_while_typing = false\n\
             pointer_speed = 0.35\n\
             scroll_method = \"edge\"\n",
    )
    .unwrap();
    assert!(cfg.input.touchpad.natural_scroll);
    assert!(!cfg.input.touchpad.tap_to_click);
    assert_eq!(cfg.input.touchpad.pointer_speed, 0.35);
    assert_eq!(cfg.input.touchpad.scroll_method, TouchpadScrollMethod::Edge);

    let err =
        Config::parse("schema_version = 2\n[input.touchpad]\npointer_speed = 1.5\n").unwrap_err();
    assert!(
        err.iter()
            .any(|d| d.field.as_deref() == Some("input.touchpad.pointer_speed"))
    );
}

#[test]
fn config_store_sets_touchpad_profile_and_preserves_other_content() {
    let path = temp_config_path("touchpad");
    let original = "schema_version = 2\n\n# keep this\n[ui]\nreduced_motion = true\n";
    std::fs::write(&path, original).unwrap();
    let profile = TouchpadConfig {
        natural_scroll: true,
        tap_to_click: false,
        tap_and_drag: false,
        drag_lock: true,
        disable_while_typing: false,
        pointer_speed: -0.4,
        scroll_method: TouchpadScrollMethod::Edge,
    };
    ConfigStore::new(&path)
        .apply(ConfigEdit::SetTouchpad { config: profile })
        .unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("# keep this"), "comment survives: {text}");
    let cfg = load(&path).unwrap().expect("file remains loadable");
    assert_eq!(cfg.input.touchpad, profile);
    assert!(cfg.ui.reduced_motion);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn missing_schema_version_is_rejected() {
    let err = Config::parse("").unwrap_err();
    assert_eq!(err.len(), 1);
    assert!(
        err[0].message.contains("schema_version"),
        "{}",
        err[0].message
    );
}

#[test]
fn future_schema_version_is_rejected() {
    let err = Config::parse("schema_version = 99\n").unwrap_err();
    assert_eq!(err.len(), 1);
    assert!(err[0].field.as_deref() == Some("schema_version"));
    assert!(err[0].message.contains("99"));
}

#[test]
fn unknown_fields_are_rejected_instead_of_silently_ignored() {
    let top = Config::parse("schema_version = 2\ntheme = \"mystery\"\n").unwrap_err();
    assert!(top[0].message.contains("unknown field"), "{top:?}");

    let nested =
        Config::parse("schema_version = 2\n[layout]\ngaps = 8\nmaster_rato = 0.7\n").unwrap_err();
    assert!(nested[0].message.contains("master_rato"), "{nested:?}");
}

#[test]
fn invalid_layout_ranges_are_diagnosed() {
    let err =
        Config::parse("schema_version = 2\n[layout]\ngaps = -1\nmaster_ratio = 1.5\n").unwrap_err();
    assert_eq!(err.len(), 2);
    assert!(
        err.iter()
            .any(|d| d.field.as_deref() == Some("layout.gaps"))
    );
    assert!(
        err.iter()
            .any(|d| d.field.as_deref() == Some("layout.master_ratio"))
    );
}

#[test]
fn parse_error_reports_a_line() {
    // Malformed TOML: an unterminated string.
    let err = Config::parse("schema_version = 2\nkey = \"oops\n").unwrap_err();
    assert_eq!(err.len(), 1);
    assert!(err[0].line.is_some(), "parse error should map to a line");
    assert!(err[0].message.starts_with("parse error"));
}

#[test]
fn keybind_entry_resolves_to_keybind() {
    let cfg = Config::parse(
        "schema_version = 2\n\
             [[keybind]]\n\
             mods = [\"super\", \"shift\"]\n\
             key = \"q\"\n\
             action = \"close\"\n",
    )
    .unwrap();
    let (binds, errs) = cfg.resolve_keybinds();
    assert!(errs.is_empty(), "{errs:?}");
    assert_eq!(binds.len(), 1);
    assert_eq!(
        binds[0].mods,
        M::SUPER | M::SHIFT,
        "mods should OR together"
    );
    assert_eq!(binds[0].action, Action::CloseFocused);
}

#[test]
fn unknown_modifier_key_and_action_each_diagnose_without_aborting() {
    let cfg = Config::parse(
        "schema_version = 2\n\
             [[keybind]]\n\
             mods = [\"super\", \"caps\"]\n\
             key = \"q\"\n\
             action = \"close\"\n\
             [[keybind]]\n\
             mods = [\"super\"]\n\
             key = \"nonsense\"\n\
             action = \"cycle\"\n\
             [[keybind]]\n\
             mods = [\"super\"]\n\
             key = \"w\"\n\
             action = \"fly-away\"\n",
    )
    .unwrap();
    let (binds, errs) = cfg.resolve_keybinds();
    // Entry 0 dropped (bad mod), entry 1 dropped (bad key), entry 2
    // dropped (bad action): no survivors, three diagnostics.
    assert!(binds.is_empty());
    assert_eq!(errs.len(), 3);
    assert!(errs.iter().any(|d| d.message.contains("caps")));
    assert!(errs.iter().any(|d| d.message.contains("nonsense")));
    assert!(errs.iter().any(|d| d.message.contains("fly-away")));
    assert!(
        errs.iter()
            .all(|d| d.field.as_deref().unwrap_or("").starts_with("keybind["))
    );
}

#[test]
fn good_entries_survive_alongside_bad_ones() {
    let cfg = Config::parse(
        "schema_version = 2\n\
             [[keybind]]\n\
             mods = [\"super\"]\n\
             key = \"q\"\n\
             action = \"close\"\n\
             [[keybind]]\n\
             mods = [\"super\"]\n\
             key = \"bad\"\n\
             action = \"close\"\n",
    )
    .unwrap();
    let (binds, errs) = cfg.resolve_keybinds();
    assert_eq!(binds.len(), 1, "the good entry survives");
    assert_eq!(errs.len(), 1);
}

#[test]
fn keymap_layers_overrides_on_defaults() {
    let cfg = Config::parse(
        "schema_version = 2\n\
             [[keybind]]\n\
             mods = [\"super\"]\n\
             key = \"space\"\n\
             action = \"launcher\"\n",
    )
    .unwrap();
    let (km, errs) = cfg.keymap();
    assert!(errs.is_empty());
    // Override present.
    assert_eq!(km.match_key(M::SUPER, 0x20), Some(Action::ToggleLauncher));
    // Defaults still present.
    assert_eq!(
        km.match_key(M::SUPER, aegis_model::input::XKB_KEY_Tab),
        Some(Action::CycleFocus)
    );
    assert!(km.len() >= 6);
}

#[test]
fn prism_keybind_action_resolves() {
    let cfg = Config::parse(
        "schema_version = 2\n\
             [[keybind]]\n\
             mods = [\"super\"]\n\
             key = \"space\"\n\
             action = \"prism\"\n",
    )
    .unwrap();
    let (binds, errs) = cfg.resolve_keybinds();
    assert!(errs.is_empty());
    assert_eq!(binds.len(), 1);
    assert_eq!(binds[0].mods, M::SUPER);
    assert_eq!(binds[0].keysym, 0x20);
    assert_eq!(binds[0].action, Action::TogglePrism);
}

#[test]
fn lock_keybind_action_resolves() {
    let cfg = Config::parse(
        "schema_version = 2\n\
             [[keybind]]\n\
             mods = [\"super\"]\n\
             key = \"l\"\n\
             action = \"lockscreen\"\n",
    )
    .unwrap();
    let (binds, errs) = cfg.resolve_keybinds();
    assert!(errs.is_empty());
    assert_eq!(binds[0].action, Action::Lock);
}

#[test]
fn gesture_entry_resolves_to_gesture_binding() {
    let cfg = Config::parse(
        "schema_version = 2\n\
             [[gesture]]\n\
             fingers = 4\n\
             axis = \"horizontal\"\n\
             action = \"workspace_switch\"\n",
    )
    .unwrap();
    let (binds, errs) = cfg.resolve_gestures();
    assert!(errs.is_empty(), "{errs:?}");
    assert_eq!(binds.len(), 1);
    assert_eq!(binds[0].fingers, 4);
    assert_eq!(binds[0].axis, aegis_model::gesture::GestureAxis::Horizontal);
    assert_eq!(
        binds[0].action,
        aegis_model::gesture::GestureAction::WorkspaceSwitch
    );
}

#[test]
fn bad_gesture_entries_diagnose_without_aborting() {
    let cfg = Config::parse(
        "schema_version = 2\n\
             [[gesture]]\n\
             fingers = 2\n\
             axis = \"vertical\"\n\
             action = \"command_panel\"\n\
             [[gesture]]\n\
             fingers = 3\n\
             axis = \"diagonal\"\n\
             action = \"window_cycle\"\n\
             [[gesture]]\n\
             fingers = 3\n\
             axis = \"vertical\"\n\
             action = \"fly-away\"\n",
    )
    .unwrap();
    let (binds, errs) = cfg.resolve_gestures();
    // Entry 0 dropped (too few fingers), entry 1 dropped (bad axis),
    // entry 2 dropped (bad action): no survivors, three diagnostics.
    assert!(binds.is_empty());
    assert_eq!(errs.len(), 3);
    assert!(errs.iter().any(|d| d.message.contains("at least 3")));
    assert!(errs.iter().any(|d| d.message.contains("diagonal")));
    assert!(errs.iter().any(|d| d.message.contains("fly-away")));
    assert!(
        errs.iter()
            .all(|d| d.field.as_deref().unwrap_or("").starts_with("gesture["))
    );
}

#[test]
fn gesture_map_layers_overrides_on_defaults() {
    let cfg = Config::parse(
        "schema_version = 2\n\
             [[gesture]]\n\
             fingers = 3\n\
             axis = \"vertical\"\n\
             action = \"command_panel\"\n",
    )
    .unwrap();
    let (gm, errs) = cfg.gesture_map();
    assert!(errs.is_empty());
    // Override present.
    assert_eq!(
        gm.lookup(3, aegis_model::gesture::GestureAxis::Vertical),
        Some(aegis_model::gesture::GestureAction::CommandPanel)
    );
    // Defaults still present.
    assert_eq!(
        gm.lookup(3, aegis_model::gesture::GestureAxis::Horizontal),
        Some(aegis_model::gesture::GestureAction::WorkspaceSwitch)
    );
    assert!(gm.len() >= 4);
}

#[test]
fn diagnostic_display_formats_line_and_field() {
    let d = Diagnostic {
        line: Some(4),
        field: Some("keybind[1]".into()),
        message: "unknown key 'bad'".into(),
    };
    assert_eq!(d.to_string(), "line 4, keybind[1]: unknown key 'bad'");
}

#[test]
fn layout_section_overrides_defaults() {
    let cfg = Config::parse(
        "schema_version = 2\n\
             [layout]\n\
             gaps = 16\n\
             master_ratio = 0.6\n",
    )
    .unwrap();
    assert_eq!(cfg.layout.gaps, 16);
    assert_eq!(cfg.layout.master_ratio, 0.6);
    // Absent section → defaults.
    let cfg2 = Config::parse("schema_version = 2\n").unwrap();
    assert_eq!(cfg2.layout, LayoutConfig::default());
    // Partial section fills the rest with field defaults.
    let cfg3 = Config::parse("schema_version = 2\n[layout]\ngaps = 4\n").unwrap();
    assert_eq!(cfg3.layout.gaps, 4);
    assert_eq!(cfg3.layout.master_ratio, 0.5);
    // Converts to the core layout params.
    let p = aegis_model::layout::LayoutParams::from(cfg.layout.clone());
    assert_eq!(p.gaps, 16);
}

#[test]
fn layout_default_tiled_parses_and_defaults_false() {
    let cfg = Config::parse("schema_version = 2\n[layout]\ndefault_tiled = true\n").unwrap();
    assert!(cfg.layout.default_tiled);
    // Absent key → false.
    let cfg2 = Config::parse("schema_version = 2\n[layout]\ngaps = 4\n").unwrap();
    assert!(!cfg2.layout.default_tiled);
    assert!(!LayoutConfig::default().default_tiled);
}

#[test]
fn ui_reduced_motion_parses_and_defaults_false() {
    let cfg = Config::parse("schema_version = 2\n[ui]\nreduced_motion = true\n").unwrap();
    assert!(cfg.ui.reduced_motion);
    // Absent section → false.
    let cfg2 = Config::parse("schema_version = 2\n").unwrap();
    assert!(!cfg2.ui.reduced_motion);
    assert_eq!(cfg2.ui, UiConfig::default());
}

#[test]
fn wallpaper_modes_parse_with_safe_defaults() {
    let default = Config::parse("schema_version = 2\n").unwrap();
    assert_eq!(default.wallpaper, WallpaperConfig::default());

    let parallax = Config::parse(
        "schema_version = 2\n\
         [wallpaper]\n\
         mode = \"parallax\"\n\
         max_shift = 40.0\n\
         transition_ms = 300\n\
         [[wallpaper.layer]]\n\
         path = \"wallpapers/sky.png\"\n\
         depth = 0.0\n\
         [[wallpaper.layer]]\n\
         path = \"wallpapers/trees.png\"\n\
         depth = 1.0\n",
    )
    .unwrap();
    assert_eq!(parallax.wallpaper.mode, WallpaperMode::Parallax);
    assert_eq!(parallax.wallpaper.layers.len(), 2);
    assert_eq!(parallax.wallpaper.max_shift, 40.0);

    let model = Config::parse(
        "schema_version = 2\n\
         [wallpaper]\n\
         mode = \"3d\"\n\
         source = \"builtin\"\n\
         background = \"wallpapers/sky.png\"\n",
    )
    .unwrap();
    assert_eq!(model.wallpaper.mode, WallpaperMode::ThreeD);
}

#[test]
fn lock_screen_defaults_to_cinematic_with_an_independent_builtin_background() {
    let config = Config::parse("schema_version = 2\n").unwrap();
    assert_eq!(config.lock_screen.style, LockScreenStyle::Cinematic);
    assert_eq!(
        config.lock_screen.background.mode,
        LockScreenBackgroundMode::Builtin
    );
    assert!(config.lock_screen.background.source.is_none());

    let config = Config::parse(
        "schema_version = 2\n\
         [lock_screen]\n\
         style = \"centered\"\n\
         [lock_screen.background]\n\
         mode = \"image\"\n\
         source = \"wallpapers/night-lock.png\"\n\
         dim = 0.4\n",
    )
    .unwrap();
    assert_eq!(config.lock_screen.style, LockScreenStyle::Centered);
    assert_eq!(
        config.lock_screen.background.mode,
        LockScreenBackgroundMode::Image
    );
    assert_eq!(
        config.lock_screen.background.source.as_deref(),
        Some("wallpapers/night-lock.png")
    );
    assert_eq!(config.lock_screen.background.dim, 0.4);
}

#[test]
fn lock_screen_background_rejects_ambiguous_or_unsafe_fields() {
    for text in [
        "schema_version = 2\n[lock_screen.background]\nmode = \"image\"\n",
        "schema_version = 2\n[lock_screen.background]\nmode = \"solid\"\nsource = \"lock.png\"\n",
        "schema_version = 2\n[lock_screen.background]\nmode = \"solid\"\ncolor = \"navy\"\n",
        "schema_version = 2\n[lock_screen.background]\ndim = 1.0\n",
    ] {
        assert!(
            Config::parse(text).is_err(),
            "accepted invalid config: {text}"
        );
    }
}

#[test]
fn wallpaper_parallax_rejects_discrete_or_ambiguous_configs() {
    for text in [
        "schema_version = 2\n[wallpaper]\nmode = \"parallax\"\n",
        "schema_version = 2\n[wallpaper]\nmode = \"video\"\n",
        "schema_version = 2\n[wallpaper]\nmode = \"image\"\n[[wallpaper.layer]]\npath = \"a.png\"\ndepth = 0.0\n",
        "schema_version = 2\n[wallpaper]\nmode = \"parallax\"\ntransition_ms = 0\n[[wallpaper.layer]]\npath = \"near.png\"\ndepth = 1.0\n[[wallpaper.layer]]\npath = \"far.png\"\ndepth = 0.0\n",
    ] {
        assert!(Config::parse(text).is_err(), "unexpectedly accepted {text}");
    }
}

#[test]
fn bundled_parallax_example_and_its_layers_stay_valid() {
    let text = include_str!("../../../examples/parallax-wallpaper/aegis/config.toml");
    let config = Config::parse(text).expect("example config parses");
    assert_eq!(config.wallpaper.mode, WallpaperMode::Parallax);
    let config_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/parallax-wallpaper/aegis");
    for layer in &config.wallpaper.layers {
        assert!(
            config_dir.join(&layer.path).is_file(),
            "missing example layer {}",
            layer.path
        );
    }
}

#[test]
fn ui_window_decorations_default_to_borderless_and_accept_client_side() {
    let default = Config::parse("schema_version = 2\n").unwrap();
    assert_eq!(
        default.ui.window_decorations,
        aegis_model::window::DecorationPolicy::Borderless
    );

    let client = Config::parse(
        "schema_version = 2\n\
             [ui]\n\
             window_decorations = \"client-side\"\n",
    )
    .unwrap();
    assert_eq!(
        client.ui.window_decorations,
        aegis_model::window::DecorationPolicy::ClientSide
    );

    assert!(
        Config::parse(
            "schema_version = 2\n\
                 [ui]\n\
                 window_decorations = \"server-side\"\n",
        )
        .is_err()
    );
}

#[test]
fn output_entries_parse_and_validate() {
    let cfg = Config::parse(
        "schema_version = 2\n\
             [[output]]\n\
             connector = \"DP-1\"\n\
             scale = 1.5\n\
             [[output]]\n\
             connector = \"HDMI-A-1\"\n\
             scale = 2.0\n",
    )
    .unwrap();
    assert_eq!(cfg.outputs.len(), 2);
    assert_eq!(cfg.outputs[0].connector, "DP-1");
    assert_eq!(cfg.outputs[0].scale, Some(1.5));
    assert_eq!(cfg.outputs[1].connector, "HDMI-A-1");
    // Absent section → empty.
    let cfg2 = Config::parse("schema_version = 2\n").unwrap();
    assert!(cfg2.outputs.is_empty());
    // Out-of-range scale and empty connector are diagnosed.
    let err = Config::parse(
        "schema_version = 2\n\
             [[output]]\n\
             connector = \"\"\n\
             scale = 9.0\n",
    )
    .unwrap_err();
    assert!(
        err.iter()
            .any(|d| d.field.as_deref() == Some("output.0.connector"))
    );
    assert!(
        err.iter()
            .any(|d| d.field.as_deref() == Some("output.0.scale"))
    );
}

#[test]
fn output_mode_position_transform_and_primary_parse() {
    let cfg = Config::parse(
        "schema_version = 2\n\
             [[output]]\n\
             connector = \"DP-1\"\n\
             mode = \"2560x1440@144\"\n\
             position = { x = 1920, y = 0 }\n\
             transform = \"flipped-90\"\n\
             [[output]]\n\
             connector = \"HDMI-A-1\"\n\
             mode = \"1920x1080\"\n\
             primary = true\n",
    )
    .unwrap();
    assert_eq!(cfg.outputs[0].mode.as_deref(), Some("2560x1440@144"));
    assert_eq!(
        cfg.outputs[0].position,
        Some(OutputPosition { x: 1920, y: 0 })
    );
    assert_eq!(cfg.outputs[0].transform.as_deref(), Some("flipped-90"));
    assert!(!cfg.outputs[0].primary);
    assert_eq!(cfg.outputs[1].mode.as_deref(), Some("1920x1080"));
    assert!(cfg.outputs[1].primary);
}

#[test]
fn output_mode_and_transform_errors_are_diagnosed() {
    let err = Config::parse(
        "schema_version = 2\n\
             [[output]]\n\
             connector = \"DP-1\"\n\
             mode = \"1080p\"\n\
             transform = \"upside-down\"\n\
             [[output]]\n\
             connector = \"HDMI-A-1\"\n\
             mode = \"99999x99999@2000\"\n",
    )
    .unwrap_err();
    assert_eq!(err.len(), 4, "{err:?}");
    assert!(
        err.iter()
            .any(|d| d.field.as_deref() == Some("output.0.mode") && d.message.contains("1080p"))
    );
    assert!(
        err.iter()
            .any(|d| d.field.as_deref() == Some("output.0.transform")
                && d.message.contains("upside-down"))
    );
    assert!(
        err.iter()
            .any(|d| d.field.as_deref() == Some("output.1.mode") && d.message.contains("16384"))
    );
    assert!(
        err.iter()
            .any(|d| d.field.as_deref() == Some("output.1.mode") && d.message.contains("1000"))
    );
}

#[test]
fn output_entry_with_no_effect_is_diagnosed() {
    let err = Config::parse("schema_version = 2\n[[output]]\nconnector = \"DP-1\"\n").unwrap_err();
    assert_eq!(err.len(), 1);
    assert_eq!(err[0].field.as_deref(), Some("output.0"));
    assert!(err[0].message.contains("no effect"), "{err:?}");
    // Any single field is enough to make the entry meaningful.
    let cfg =
        Config::parse("schema_version = 2\n[[output]]\nconnector = \"DP-1\"\nprimary = true\n")
            .unwrap();
    assert!(cfg.outputs[0].primary);
}

#[test]
fn output_policies_resolve_and_later_duplicate_wins() {
    let cfg = Config::parse(
        "schema_version = 2\n\
             [[output]]\n\
             connector = \"DP-1\"\n\
             scale = 1.5\n\
             mode = \"2560x1440@144\"\n\
             position = { x = 1920, y = 0 }\n\
             transform = \"180\"\n\
             primary = true\n\
             [[output]]\n\
             connector = \"HDMI-A-1\"\n\
             scale = 1.0\n\
             [[output]]\n\
             connector = \"HDMI-A-1\"\n\
             scale = 2.0\n",
    )
    .unwrap();
    let policies = cfg.output_policies();
    assert_eq!(policies.len(), 2, "duplicate connector collapses");
    let dp = &policies["DP-1"];
    assert_eq!(dp.scale, Some(1.5));
    assert_eq!(dp.mode, "2560x1440@144".parse().ok());
    assert_eq!(dp.position, Some(aegis_model::Point { x: 1920, y: 0 }));
    assert_eq!(dp.transform, Some(aegis_model::Transform::Rotate180));
    assert!(dp.primary);
    // The later HDMI-A-1 entry replaces the earlier one wholesale.
    let hdmi = &policies["HDMI-A-1"];
    assert_eq!(hdmi.scale, Some(2.0));
    assert!(!hdmi.primary);
}

#[test]
fn window_rules_parse_from_toml() {
    let cfg = Config::parse(
        "schema_version = 2\n\
             [[window_rule]]\n\
             app_id = \"firefox\"\n\
             workspace = 2\n\
             role = \"tiled\"\n\
             [[window_rule]]\n\
             title = \"calculator\"\n\
             role = \"floating\"\n",
    )
    .unwrap();
    assert_eq!(cfg.window_rules.len(), 2);
    assert_eq!(cfg.window_rules[0].app_id.as_deref(), Some("firefox"));
    assert_eq!(cfg.window_rules[0].workspace, Some(2));
    assert_eq!(
        cfg.window_rules[0].role,
        Some(aegis_model::layout::LayoutRole::Tiled)
    );
    assert_eq!(cfg.window_rules[1].title.as_deref(), Some("calculator"));
    assert_eq!(
        cfg.window_rules[1].role,
        Some(aegis_model::layout::LayoutRole::Floating)
    );
    assert!(cfg.window_rules[1].matches(None, Some("GNOME Calculator")));
}

#[test]
fn screenshot_config_defaults_and_parses() {
    let cfg = Config::parse("schema_version = 2\n").unwrap();
    assert!(!cfg.screenshot.save_dir.is_empty());
    assert!(cfg.screenshot.save_dir.ends_with("screenshots"));
    assert!(cfg.screenshot.include_cursor);

    let cfg2 = Config::parse(
        "schema_version = 2\n\
             [screenshot]\n\
             save_dir = \"/tmp/shots\"\n\
             include_cursor = false\n",
    )
    .unwrap();
    assert_eq!(cfg2.screenshot.save_dir, "/tmp/shots");
    assert!(!cfg2.screenshot.include_cursor);

    let err = Config::parse("schema_version = 2\n[screenshot]\nsave_dir = \"\"\n").unwrap_err();
    assert!(
        err.iter()
            .any(|d| d.field.as_deref() == Some("screenshot.save_dir"))
    );
}

#[test]
fn interaction_domain_sandbox_policy_is_default_deny_and_app_overrides_are_last_wins() {
    let cfg = Config::parse(
        "schema_version = 2\n\
             [interaction_domain_sandbox]\n\
             memory_max_mib = 4096\n\
             [[interaction_domain_sandbox.app]]\n\
             desktop_id = \"browser.desktop\"\n\
             [[interaction_domain_sandbox.app]]\n\
             desktop_id = \"browser.desktop\"\n\
             memory_max_mib = 2048\n",
    )
    .unwrap();
    let default = cfg.interaction_domain_sandbox.policy_for("editor.desktop");
    assert_eq!(default.memory_max_bytes, 4096 * 1024 * 1024);

    let browser = cfg.interaction_domain_sandbox.policy_for("browser.desktop");
    assert_eq!(browser.memory_max_bytes, 2048 * 1024 * 1024);
}

#[test]
fn ambient_network_paths_and_legacy_realm_sandbox_are_rejected() {
    for text in [
        "schema_version = 2\n[interaction_domain_sandbox]\nnetwork = true\n",
        "schema_version = 2\n[interaction_domain_sandbox]\nreadable_paths = [\"/srv\"]\n",
        "schema_version = 2\n[interaction_domain_sandbox]\nwritable_paths = [\"/srv\"]\n",
        "schema_version = 2\n[realm_sandbox]\nmemory_max_mib = 2048\n",
    ] {
        assert!(
            Config::parse(text).is_err(),
            "accepted ambient authority: {text}"
        );
    }
}

#[test]
fn interaction_domain_sandbox_policy_rejects_unbounded_limits() {
    let diagnostics = Config::parse(
        "schema_version = 2\n\
             [interaction_domain_sandbox]\n\
             memory_max_mib = 1\n\
             pids_max = 1\n\
             cpu_weight = 0\n",
    )
    .unwrap_err();
    for field in [
        "interaction_domain_sandbox.memory_max_mib",
        "interaction_domain_sandbox.pids_max",
        "interaction_domain_sandbox.cpu_weight",
    ] {
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.field.as_deref() == Some(field)),
            "missing diagnostic for {field}: {diagnostics:?}"
        );
    }
}

#[test]
fn config_store_persists_output_atomically_and_keeps_unrelated_fields() {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let directory = std::env::temp_dir().join(format!(
        "aegis-config-output-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("config.toml");
    std::fs::write(
        &path,
        "schema_version = 2 # keep this comment\n\
             [[output]]\nconnector = \"DP-1\"\nprimary = true\n\
             [[output]]\nconnector = \"HDMI-A-1\"\ntransform = \"180\"\n",
    )
    .unwrap();

    ConfigStore::new(&path)
        .apply(ConfigEdit::SetOutput {
            settings: aegis_model::settings::DisplaySettings {
                connector: "HDMI-A-1".into(),
                mode: aegis_model::output::ModeSpec {
                    width: 2560,
                    height: 1440,
                    refresh_hz: Some(144),
                },
                scale: 1.5,
                position: aegis_model::Point { x: 120, y: -40 },
                primary: true,
            },
        })
        .unwrap();

    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("# keep this comment"));
    assert!(text.contains("transform = \"180\""));
    assert!(!text.contains("connector = \"DP-1\""));
    let config = load(&path).unwrap().unwrap();
    assert_eq!(config.outputs.len(), 1);
    let policy = config.output_policies()["HDMI-A-1"];
    assert_eq!(policy.scale, Some(1.5));
    assert_eq!(policy.mode.unwrap().refresh_hz, Some(144));
    assert_eq!(policy.position, Some(aegis_model::Point { x: 120, y: -40 }));
    assert!(policy.primary);
    assert_eq!(policy.transform, Some(aegis_model::Transform::Rotate180));
    assert!(
        std::fs::read_dir(&directory)
            .unwrap()
            .flatten()
            .all(|entry| !entry.file_name().to_string_lossy().ends_with(".tmp"))
    );
    std::fs::remove_dir_all(directory).ok();
}

#[test]
fn byte_to_line_is_one_based_and_clamps() {
    let text = "a\nb\nc\n"; // indices: 0='a' 1='\n' 2='b' 3='\n' 4='c' 5='\n'
    assert_eq!(byte_to_line(text, 0), 1); // first line
    assert_eq!(byte_to_line(text, 2), 2); // after the first newline
    assert_eq!(byte_to_line(text, 4), 3); // after the second newline
    // An offset past the final newline is the line that follows it; a
    // huge offset clamps to the text end rather than indexing past it.
    assert_eq!(byte_to_line(text, usize::MAX), 4);
}
