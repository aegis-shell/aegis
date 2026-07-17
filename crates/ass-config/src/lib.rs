//! Declarative configuration for ass.
//!
//! A single TOML file at `$XDG_CONFIG_HOME/ass/config.toml` (defaulting to
//! `~/.config/ass/config.toml`) is the source of truth for user-tunable
//! behavior. The file carries an explicit [`SUPPORTED_SCHEMA_VERSION`]; a
//! future-incompatible change bumps it and ships a migration note in the
//! CHANGELOG. The loader never silently ignores a problem: a malformed or
//! unsupported file is reported as structured [`Diagnostic`]s.
//!
//! The crate is pure: it depends on [`ass_core`] for the shared keymap model
//! and name resolvers, and on `serde`/`toml` for the schema. It has no flux,
//! lens, or Wayland dependency, so it is unit-testable in isolation. See
//! [ADR-0026](../../docs/adr/0026-configuration-system.md).
//!
//! Live reload is mtime-based and driven by the caller's event loop: the
//! compositor polls [`ReloadWatcher::changed`] each frame (cheap; one
//! `stat`) and reloads when the file's modification time moves. A
//! filesystem-watcher thread would add threading complexity for no gain at
//! the low frequency a config file changes.

use std::path::{Path, PathBuf};

use ass_core::input::Mods;
use ass_core::keybind::{Keybind, Keymap};

/// The schema `schema_version` this build understands. A file whose
/// `schema_version` differs is rejected with a precise diagnostic rather
/// than guessed at; bumping this is a real event documented in the CHANGELOG.
pub const SUPPORTED_SCHEMA_VERSION: u32 = 1;

/// The top-level configuration object, deserialized from the TOML file.
///
/// Sections are flat top-level fields; new milestones add fields here rather
/// than spreading behavior across environment variables. Every field is
/// [`Default`]-able so a partially specified file still loads.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Schema major version. Required; must equal
    /// [`SUPPORTED_SCHEMA_VERSION`]. Missing or mismatched versions are
    /// reported as diagnostics, never silently accepted.
    pub schema_version: u32,
    /// Global key bindings, an array-of-tables written `[[keybind]]` in the
    /// file. The TOML key is `keybind` (the array-of-tables convention); the
    /// Rust field is `keybinds` for readability. Resolved against the
    /// built-in defaults with [`Config::keymap`].
    #[serde(default, rename = "keybind")]
    pub keybinds: Vec<KeybindEntry>,

    /// Window rules, an array-of-tables written `[[window_rule]]`. Evaluated
    /// on first map; the first match applies (move to workspace, force a
    /// layout role). See [`ass_core::window_rule::WindowRule`].
    #[serde(default, rename = "window_rule")]
    pub window_rules: Vec<ass_core::window_rule::WindowRule>,

    /// Tiling policy parameters (ADR-0024), written as a `[layout]` table.
    #[serde(default)]
    pub layout: LayoutConfig,

    /// Dock configuration, written as a `[dock]` table. Controls which apps
    /// are pinned to the bottom dock.
    #[serde(default)]
    pub dock: DockConfig,

    /// Shell-wide UI policy, written as a `[ui]` table.
    #[serde(default)]
    pub ui: UiConfig,

    /// Per-output scale policy (ADR-0028), written as `[[output]]`
    /// array-of-tables. Each entry overrides the backend-reported scale for
    /// one connector, for mixed-DPI setups.
    #[serde(default, rename = "output")]
    pub outputs: Vec<OutputConfig>,

    /// Agent scope declarations (ADR-0034), written as `[[agent.scope]]`
    /// array-of-tables. Each entry names a scope the compositor resolves
    /// when an IPC client presents the name at the Hello handshake.
    #[serde(default)]
    pub agent: AgentConfig,
}

/// The `[agent]` section (ADR-0034). Named scopes that bound what an agent
/// IPC client may do: which operations, which windows, which workspaces.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentConfig {
    /// One named scope per `[[agent.scope]]` entry.
    #[serde(default, rename = "scope")]
    pub scopes: Vec<AgentScopeEntry>,
}

/// One declared agent scope. `ops` lists `OpClass` names (`Focus`,
/// `Close`, …); `windows` and `workspaces` are id allowlists (empty or
/// omitted means unrestricted at that axis).
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentScopeEntry {
    /// The scope name an IPC client presents at Hello.
    pub name: String,
    /// Operation-class names this scope permits.
    #[serde(default)]
    pub ops: Vec<String>,
    /// Window-id allowlist. Empty means unrestricted.
    #[serde(default)]
    pub windows: Vec<u64>,
    /// Workspace-id allowlist. Empty means unrestricted.
    #[serde(default)]
    pub workspaces: Vec<u64>,
}

/// The `[dock]` section. `pinned` lists the apps to keep on the dock in order;
/// each value matches an enumerated `.desktop` entry by its id, desktop-file
/// stem, `StartupWMClass`, or icon name (case-insensitive). An empty list (the
/// default) lets the compositor auto-populate the dock with the first handful
/// of apps that have a usable icon, so the dock is never empty out of the box.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DockConfig {
    #[serde(default)]
    pub pinned: Vec<String>,
}

/// The `[ui]` section: shell-wide UI policy (ADR-0029).
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UiConfig {
    /// Accessibility reduced-motion switch. When true, every chrome and lens
    /// transition resolves to its end state in at most one frame — no fades,
    /// springs, or slides (ADR-0029). Individual effects do not override it.
    #[serde(default)]
    pub reduced_motion: bool,
    /// XDG cursor theme name for the software cursor on direct display.
    /// `$XCURSOR_THEME` wins when set; this is the session-independent
    /// fallback (bare TTY sessions usually have no cursor env vars).
    #[serde(default)]
    pub cursor_theme: Option<String>,
    /// Cursor size in logical pixels. `$XCURSOR_SIZE` wins when set.
    #[serde(default)]
    pub cursor_size: Option<u32>,
}

/// One `[[output]]` entry: a per-connector scale override (ADR-0028).
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputConfig {
    /// The connector name as reported by the backend (e.g. "DP-1",
    /// "HDMI-A-1", "nested"). Unmatched names are ignored with a diagnostic
    /// at load time by the caller, not by the schema.
    pub connector: String,
    /// Output scale factor. Integer scales advertise through `wl_output`;
    /// fractional scales through `wp_fractional_scale_v1`.
    pub scale: f64,
}

/// The `[layout]` section: tiling gaps and master ratio. Defaults match the
/// built-in `ass_core::layout::LayoutParams`.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LayoutConfig {
    /// Gap in logical pixels between tiles and around the work-area edge.
    #[serde(default = "default_gaps")]
    pub gaps: i32,
    /// Fraction of the work-area width for the master column (0.0..=1.0).
    #[serde(default = "default_master_ratio")]
    pub master_ratio: f32,
    /// Whether newly created workspaces start in tiled mode (ADR-0024).
    /// Window rules can still force individual windows floating or tiled.
    #[serde(default)]
    pub default_tiled: bool,
}

fn default_gaps() -> i32 {
    8
}
fn default_master_ratio() -> f32 {
    0.5
}

impl Default for LayoutConfig {
    fn default() -> LayoutConfig {
        LayoutConfig {
            gaps: default_gaps(),
            master_ratio: default_master_ratio(),
            default_tiled: false,
        }
    }
}

impl From<LayoutConfig> for ass_core::layout::LayoutParams {
    fn from(c: LayoutConfig) -> ass_core::layout::LayoutParams {
        ass_core::layout::LayoutParams {
            gaps: c.gaps,
            master_ratio: c.master_ratio,
        }
    }
}

/// One key binding: a modifier set, a key, and the action it triggers.
///
/// Field names match the [`ass_core::keybind`] name resolvers: `mods` is a
/// list so several modifiers can combine, `key` and `action` are single
/// names. Unknown names produce a per-entry diagnostic rather than aborting
/// the whole file.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeybindEntry {
    /// Modifier names: `shift`, `ctrl`/`control`, `alt`/`mod1`,
    /// `super`/`meta`/`win`/`mod4`. Combinations are OR'd together.
    #[serde(default)]
    pub mods: Vec<String>,
    /// Key name: a letter, digit, or control name (`return`, `escape`,
    /// `tab`, `f1`..`f12`, `up`/`down`/`left`/`right`, …).
    pub key: String,
    /// Action name: `launcher`, `close`, `cycle`/`next`, `prev`, `quit`.
    pub action: String,
}

/// One problem found while loading or resolving a configuration file.
///
/// Diagnostics are collected, not thrown: a file with one bad entry still
/// yields the good entries, with one diagnostic per problem. `line` is the
/// 1-based source line when the underlying error carries a byte span;
/// `field` names the offending field path (for example `keybind[2]`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub line: Option<usize>,
    pub field: Option<String>,
    pub message: String,
}

impl Diagnostic {
    fn new(field: Option<String>, message: impl Into<String>) -> Self {
        Diagnostic {
            line: None,
            field,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (self.line, &self.field) {
            (Some(line), Some(field)) => {
                write!(f, "line {line}, {field}: {}", self.message)
            }
            (Some(line), None) => write!(f, "line {line}: {}", self.message),
            (None, Some(field)) => write!(f, "{field}: {}", self.message),
            (None, None) => write!(f, "{}", self.message),
        }
    }
}

/// A filesystem-level failure while reading the config file.
#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    /// The file exists but could not be read.
    #[error("{path}: read failed")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// The file was read but failed schema or semantic validation.
    #[error("{path}: {} error(s)", diagnostics.len())]
    Invalid {
        path: PathBuf,
        diagnostics: Vec<Diagnostic>,
    },
}

impl Config {
    /// Parse a config file from its text. Returns the parsed [`Config`] or
    /// the diagnostics that prevented it. Parse errors carry a 1-based
    /// source line where the `toml` crate provides a byte span.
    pub fn parse(text: &str) -> Result<Config, Vec<Diagnostic>> {
        let cfg: Config = match toml::from_str(text) {
            Ok(c) => c,
            Err(e) => {
                let line = e.span().map(|span| byte_to_line(text, span.start));
                return Err(vec![Diagnostic {
                    line,
                    field: None,
                    message: format!("parse error: {e}"),
                }]);
            }
        };
        if cfg.schema_version != SUPPORTED_SCHEMA_VERSION {
            return Err(vec![Diagnostic {
                line: None,
                field: Some("schema_version".into()),
                message: format!(
                    "unsupported schema_version {} (this build supports {SUPPORTED_SCHEMA_VERSION})",
                    cfg.schema_version
                ),
            }]);
        }
        let mut diagnostics = Vec::new();
        if cfg.layout.gaps < 0 {
            diagnostics.push(Diagnostic::new(
                Some("layout.gaps".into()),
                "must be zero or greater",
            ));
        }
        if !cfg.layout.master_ratio.is_finite() || !(0.0..=1.0).contains(&cfg.layout.master_ratio) {
            diagnostics.push(Diagnostic::new(
                Some("layout.master_ratio".into()),
                "must be between 0.0 and 1.0",
            ));
        }
        for (index, output) in cfg.outputs.iter().enumerate() {
            if output.connector.trim().is_empty() {
                diagnostics.push(Diagnostic::new(
                    Some(format!("output.{index}.connector")),
                    "must not be empty",
                ));
            }
            if !output.scale.is_finite() || !(0.25..=4.0).contains(&output.scale) {
                diagnostics.push(Diagnostic::new(
                    Some(format!("output.{index}.scale")),
                    "must be between 0.25 and 4.0",
                ));
            }
        }
        if let Some(size) = cfg.ui.cursor_size {
            if !(8..=128).contains(&size) {
                diagnostics.push(Diagnostic::new(
                    Some("ui.cursor_size".into()),
                    "must be between 8 and 128",
                ));
            }
        }
        if diagnostics.is_empty() {
            Ok(cfg)
        } else {
            Err(diagnostics)
        }
    }

    /// Resolve the configured key bindings into [`Keybind`]s. Returns the
    /// resolved bindings plus one diagnostic per entry that could not
    /// resolve (unknown modifier, key, or action). Good entries are kept so
    /// a file with one typo still yields the rest.
    pub fn resolve_keybinds(&self) -> (Vec<Keybind>, Vec<Diagnostic>) {
        let mut binds = Vec::new();
        let mut errs = Vec::new();
        for (i, entry) in self.keybinds.iter().enumerate() {
            let field = format!("keybind[{i}]");
            let mut mods = Mods::NONE;
            let mut mods_ok = true;
            for m in &entry.mods {
                match ass_core::keybind::mod_from_name(m) {
                    Some(bit) => mods |= bit,
                    None => {
                        errs.push(Diagnostic::new(
                            Some(field.clone()),
                            format!("unknown modifier '{m}'"),
                        ));
                        mods_ok = false;
                    }
                }
            }
            let Some(keysym) = ass_core::keybind::keysym_from_name(&entry.key) else {
                errs.push(Diagnostic::new(
                    Some(field.clone()),
                    format!("unknown key '{}'", entry.key),
                ));
                continue;
            };
            let Some(action) = ass_core::keybind::action_from_name(&entry.action) else {
                errs.push(Diagnostic::new(
                    Some(field.clone()),
                    format!("unknown action '{}'", entry.action),
                ));
                continue;
            };
            if mods_ok {
                binds.push(Keybind {
                    mods,
                    keysym,
                    action,
                });
            }
        }
        (binds, errs)
    }

    /// Build the active [`Keymap`]: built-in defaults overridden by the
    /// configured entries, plus the diagnostics from resolution. Callers log
    /// the diagnostics and install the returned keymap.
    pub fn keymap(&self) -> (Keymap, Vec<Diagnostic>) {
        let (overrides, errs) = self.resolve_keybinds();
        (Keymap::defaults().with_overrides(overrides), errs)
    }
}

impl std::str::FromStr for Config {
    type Err = Vec<Diagnostic>;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Config::parse(text)
    }
}

/// Read and parse the config file at `path`. Returns `Ok(None)` when the
/// file does not exist (the caller falls back to defaults), an error for
/// read or validation failures.
pub fn load(path: &Path) -> Result<Option<Config>, LoadError> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(LoadError::Read {
                path: path.into(),
                source: e,
            })
        }
    };
    match Config::parse(&text) {
        Ok(cfg) => Ok(Some(cfg)),
        Err(diagnostics) => Err(LoadError::Invalid {
            path: path.into(),
            diagnostics,
        }),
    }
}

/// The default config path: `$XDG_CONFIG_HOME/ass/config.toml`, falling back
/// to `~/.config/ass/config.toml` per the `dirs` crate. `None` when the home
/// directory cannot be determined.
pub fn default_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("ass").join("config.toml"))
}

/// Convert a byte offset into a 1-based source line by counting newlines
/// before it. Used only to enrich parse-error diagnostics; never panics.
fn byte_to_line(text: &str, offset: usize) -> usize {
    let upto = offset.min(text.len());
    1 + text[..upto].bytes().filter(|&b| b == b'\n').count()
}

/// Mtime-based live-reload tracker, polled by the caller's event loop.
///
/// Construct with [`ReloadWatcher::at`] after the initial load; call
/// [`ReloadWatcher::changed`] each frame. It reports `true` once after a
/// create, modify, or delete transition, then stays quiet until the next
/// transition. There is no background thread: a `stat` per frame is cheap,
/// and avoiding threads keeps reload on the compositor's main loop where
/// the keymap rebuild must happen anyway.
#[derive(Debug)]
pub struct ReloadWatcher {
    last: Option<std::time::SystemTime>,
}

impl ReloadWatcher {
    /// Capture the file's current mtime as the baseline. If the file is
    /// absent, the baseline is `None`, so a later creation is reported as a
    /// change.
    pub fn at(path: &Path) -> ReloadWatcher {
        let last = std::fs::metadata(path).and_then(|m| m.modified()).ok();
        ReloadWatcher { last }
    }

    /// `true` if the file's mtime changed since the last call (or since
    /// construction). Updates the internal baseline, so it returns `true`
    /// at most once per transition.
    pub fn changed(&mut self, path: &Path) -> bool {
        match std::fs::metadata(path).and_then(|m| m.modified()) {
            Ok(m) => {
                if Some(m) != self.last {
                    self.last = Some(m);
                    true
                } else {
                    false
                }
            }
            Err(_) => {
                if self.last.is_some() {
                    self.last = None;
                    true
                } else {
                    false
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ass_core::input::Mods as M;
    use ass_core::keybind::Action;

    #[test]
    fn minimal_valid_config_loads() {
        let cfg = Config::parse("schema_version = 1\n").unwrap();
        assert_eq!(cfg.schema_version, 1);
        assert!(cfg.keybinds.is_empty());
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
        let top = Config::parse("schema_version = 1\ntheme = \"mystery\"\n").unwrap_err();
        assert!(top[0].message.contains("unknown field"), "{top:?}");

        let nested = Config::parse("schema_version = 1\n[layout]\ngaps = 8\nmaster_rato = 0.7\n")
            .unwrap_err();
        assert!(nested[0].message.contains("master_rato"), "{nested:?}");
    }

    #[test]
    fn invalid_layout_ranges_are_diagnosed() {
        let err = Config::parse("schema_version = 1\n[layout]\ngaps = -1\nmaster_ratio = 1.5\n")
            .unwrap_err();
        assert_eq!(err.len(), 2);
        assert!(err
            .iter()
            .any(|d| d.field.as_deref() == Some("layout.gaps")));
        assert!(err
            .iter()
            .any(|d| d.field.as_deref() == Some("layout.master_ratio")));
    }

    #[test]
    fn parse_error_reports_a_line() {
        // Malformed TOML: an unterminated string.
        let err = Config::parse("schema_version = 1\nkey = \"oops\n").unwrap_err();
        assert_eq!(err.len(), 1);
        assert!(err[0].line.is_some(), "parse error should map to a line");
        assert!(err[0].message.starts_with("parse error"));
    }

    #[test]
    fn keybind_entry_resolves_to_keybind() {
        let cfg = Config::parse(
            "schema_version = 1\n\
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
            "schema_version = 1\n\
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
        assert!(errs
            .iter()
            .all(|d| d.field.as_deref().unwrap_or("").starts_with("keybind[")));
    }

    #[test]
    fn good_entries_survive_alongside_bad_ones() {
        let cfg = Config::parse(
            "schema_version = 1\n\
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
            "schema_version = 1\n\
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
            km.match_key(M::SUPER, ass_core::input::XKB_KEY_Tab),
            Some(Action::CycleFocus)
        );
        assert!(km.len() >= 6);
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
            "schema_version = 1\n\
             [layout]\n\
             gaps = 16\n\
             master_ratio = 0.6\n",
        )
        .unwrap();
        assert_eq!(cfg.layout.gaps, 16);
        assert_eq!(cfg.layout.master_ratio, 0.6);
        // Absent section → defaults.
        let cfg2 = Config::parse("schema_version = 1\n").unwrap();
        assert_eq!(cfg2.layout, LayoutConfig::default());
        // Partial section fills the rest with field defaults.
        let cfg3 = Config::parse("schema_version = 1\n[layout]\ngaps = 4\n").unwrap();
        assert_eq!(cfg3.layout.gaps, 4);
        assert_eq!(cfg3.layout.master_ratio, 0.5);
        // Converts to the core layout params.
        let p = ass_core::layout::LayoutParams::from(cfg.layout.clone());
        assert_eq!(p.gaps, 16);
    }

    #[test]
    fn layout_default_tiled_parses_and_defaults_false() {
        let cfg = Config::parse("schema_version = 1\n[layout]\ndefault_tiled = true\n").unwrap();
        assert!(cfg.layout.default_tiled);
        // Absent key → false.
        let cfg2 = Config::parse("schema_version = 1\n[layout]\ngaps = 4\n").unwrap();
        assert!(!cfg2.layout.default_tiled);
        assert!(!LayoutConfig::default().default_tiled);
    }

    #[test]
    fn ui_reduced_motion_parses_and_defaults_false() {
        let cfg = Config::parse("schema_version = 1\n[ui]\nreduced_motion = true\n").unwrap();
        assert!(cfg.ui.reduced_motion);
        // Absent section → false.
        let cfg2 = Config::parse("schema_version = 1\n").unwrap();
        assert!(!cfg2.ui.reduced_motion);
        assert_eq!(cfg2.ui, UiConfig::default());
    }

    #[test]
    fn output_entries_parse_and_validate() {
        let cfg = Config::parse(
            "schema_version = 1\n\
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
        assert_eq!(cfg.outputs[0].scale, 1.5);
        assert_eq!(cfg.outputs[1].connector, "HDMI-A-1");
        // Absent section → empty.
        let cfg2 = Config::parse("schema_version = 1\n").unwrap();
        assert!(cfg2.outputs.is_empty());
        // Out-of-range scale and empty connector are diagnosed.
        let err = Config::parse(
            "schema_version = 1\n\
             [[output]]\n\
             connector = \"\"\n\
             scale = 9.0\n",
        )
        .unwrap_err();
        assert!(err
            .iter()
            .any(|d| d.field.as_deref() == Some("output.0.connector")));
        assert!(err
            .iter()
            .any(|d| d.field.as_deref() == Some("output.0.scale")));
    }

    #[test]
    fn window_rules_parse_from_toml() {
        let cfg = Config::parse(
            "schema_version = 1\n\
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
            Some(ass_core::layout::LayoutRole::Tiled)
        );
        assert_eq!(cfg.window_rules[1].title.as_deref(), Some("calculator"));
        assert_eq!(
            cfg.window_rules[1].role,
            Some(ass_core::layout::LayoutRole::Floating)
        );
        assert!(cfg.window_rules[1].matches(None, Some("GNOME Calculator")));
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
}
