//! Declarative configuration for ass.
//!
//! A single TOML file at `$XDG_CONFIG_HOME/ass/config.toml` (defaulting to
//! `~/.config/aegis/config.toml`) is the source of truth for user-tunable
//! behavior. The file carries an explicit [`SUPPORTED_SCHEMA_VERSION`]; a
//! future-incompatible change bumps it and ships a migration note in the
//! CHANGELOG. The loader never silently ignores a problem: a malformed or
//! unsupported file is reported as structured [`Diagnostic`]s.
//!
//! The crate is pure: it depends on [`aegis_core`] for the shared keymap model
//! and name resolvers, and on `serde`/`toml` for the schema. It has no flux,
//! lens, or Wayland dependency, so it is unit-testable in isolation. See
//! [ADR-0026](../../docs/adr/0026-configuration-system.md).
//!
//! Live reload is mtime-based and driven by the caller's event loop: the
//! compositor polls [`ReloadWatcher::changed`] each frame (cheap; one
//! `stat`) and reloads when the file's modification time moves. A
//! filesystem-watcher thread would add threading complexity for no gain at
//! the low frequency a config file changes.

use std::io::Write;
use std::path::{Path, PathBuf};

use aegis_core::input::{Mods, TouchpadConfig, TouchpadScrollMethod};
use aegis_core::keybind::{Keybind, Keymap};
use toml_edit::DocumentMut;

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
    /// layout role). See [`aegis_core::window_rule::WindowRule`].
    #[serde(default, rename = "window_rule")]
    pub window_rules: Vec<aegis_core::window_rule::WindowRule>,

    /// Tiling policy parameters (ADR-0024), written as a `[layout]` table.
    #[serde(default)]
    pub layout: LayoutConfig,

    /// Dock configuration, written as a `[dock]` table. Controls which apps
    /// are pinned to the bottom dock.
    #[serde(default)]
    pub dock: DockConfig,

    /// Status bar configuration, written as a `[statusbar]` table. Controls
    /// whether the top status bar is registered.
    #[serde(default)]
    pub statusbar: StatusBarConfig,

    /// Desktop-wide UI and window-presentation policy, written as a `[ui]`
    /// table.
    #[serde(default)]
    pub ui: UiConfig,

    /// Physical input-device policy, written as an `[input]` table.
    #[serde(default)]
    pub input: InputConfig,

    /// Per-output display policy (ADR-0028), written as `[[output]]`
    /// array-of-tables. Each entry overrides the backend-reported mode,
    /// scale, position, transform, or primary-output selection for one connector.
    #[serde(default, rename = "output")]
    pub outputs: Vec<OutputConfig>,

    /// Agent scope declarations (ADR-0034), written as `[[agent.scope]]`
    /// array-of-tables. Each entry names a scope the compositor resolves
    /// when an IPC client presents the name at the Hello handshake.
    #[serde(default)]
    pub agent: AgentConfig,

    /// Default-deny process sandbox policy for applications launched inside
    /// Realms. Per-desktop-entry overrides are applied only to new launches.
    #[serde(default)]
    pub realm_sandbox: RealmSandboxConfig,

    /// Screenshot tool configuration, written as `[screenshot]`.
    #[serde(default)]
    pub screenshot: ScreenshotConfig,

    /// Desktop-wide appearance preference, written as `[appearance]`. The
    /// compositor itself does not theme from it yet; the portal backend
    /// (`aegis-portal`) reads it to answer the freedesktop
    /// `org.freedesktop.appearance` settings namespace (ADR-0051).
    #[serde(default)]
    pub appearance: AppearanceConfig,
}

/// The `[appearance]` section: desktop-wide appearance preference consumed
/// by the portal Settings backend (ADR-0051).
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppearanceConfig {
    /// Preferred color scheme. `system` (the default) advertises no
    /// preference; `dark` and `light` map to the portal `color-scheme`
    /// values 1 and 2.
    #[serde(default)]
    pub color_scheme: ColorScheme,
}

/// A desktop color-scheme preference. Names match the freedesktop Settings
/// portal vocabulary.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ColorScheme {
    /// No preference; applications pick their own default.
    #[default]
    System,
    /// Prefer a dark appearance.
    Dark,
    /// Prefer a light appearance.
    Light,
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

/// The `[realm_sandbox]` policy and `[[realm_sandbox.app]]` overrides.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RealmSandboxConfig {
    #[serde(default)]
    pub network: bool,
    #[serde(default)]
    pub readable_paths: Vec<String>,
    #[serde(default)]
    pub writable_paths: Vec<String>,
    #[serde(default = "default_realm_memory_mib")]
    pub memory_max_mib: u64,
    #[serde(default = "default_realm_pids")]
    pub pids_max: u32,
    #[serde(default = "default_realm_cpu_weight")]
    pub cpu_weight: u16,
    #[serde(default, rename = "app")]
    pub apps: Vec<RealmSandboxAppConfig>,
}

impl Default for RealmSandboxConfig {
    fn default() -> Self {
        Self {
            network: false,
            readable_paths: Vec::new(),
            writable_paths: Vec::new(),
            memory_max_mib: default_realm_memory_mib(),
            pids_max: default_realm_pids(),
            cpu_weight: default_realm_cpu_weight(),
            apps: Vec::new(),
        }
    }
}

impl RealmSandboxConfig {
    pub fn policy_for(&self, desktop_id: &str) -> RealmSandboxPolicy {
        let mut network = self.network;
        let mut readable_paths = self.readable_paths.clone();
        let mut writable_paths = self.writable_paths.clone();
        let mut memory_max_mib = self.memory_max_mib;
        let mut pids_max = self.pids_max;
        let mut cpu_weight = self.cpu_weight;

        for app in self.apps.iter().filter(|app| app.desktop_id == desktop_id) {
            if let Some(value) = app.network {
                network = value;
            }
            if let Some(value) = &app.readable_paths {
                readable_paths.clone_from(value);
            }
            if let Some(value) = &app.writable_paths {
                writable_paths.clone_from(value);
            }
            if let Some(value) = app.memory_max_mib {
                memory_max_mib = value;
            }
            if let Some(value) = app.pids_max {
                pids_max = value;
            }
            if let Some(value) = app.cpu_weight {
                cpu_weight = value;
            }
        }

        RealmSandboxPolicy {
            network,
            readable_paths: readable_paths.into_iter().map(PathBuf::from).collect(),
            writable_paths: writable_paths.into_iter().map(PathBuf::from).collect(),
            memory_max_bytes: memory_max_mib * 1024 * 1024,
            pids_max,
            cpu_weight,
        }
    }
}

/// One exact desktop-entry override. Omitted fields inherit the enclosing
/// `[realm_sandbox]` value; explicit empty path arrays remove inherited paths.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RealmSandboxAppConfig {
    pub desktop_id: String,
    #[serde(default)]
    pub network: Option<bool>,
    #[serde(default)]
    pub readable_paths: Option<Vec<String>>,
    #[serde(default)]
    pub writable_paths: Option<Vec<String>>,
    #[serde(default)]
    pub memory_max_mib: Option<u64>,
    #[serde(default)]
    pub pids_max: Option<u32>,
    #[serde(default)]
    pub cpu_weight: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealmSandboxPolicy {
    pub network: bool,
    pub readable_paths: Vec<PathBuf>,
    pub writable_paths: Vec<PathBuf>,
    pub memory_max_bytes: u64,
    pub pids_max: u32,
    pub cpu_weight: u16,
}

fn default_realm_memory_mib() -> u64 {
    8192
}

fn default_realm_pids() -> u32 {
    1024
}

fn default_realm_cpu_weight() -> u16 {
    100
}

/// The `[screenshot]` section. Controls where interactive and IPC screenshots
/// are saved when no explicit path is supplied.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScreenshotConfig {
    /// Directory to write screenshots into. Defaults to
    /// `$XDG_PICTURES_DIR/screenshots`.
    #[serde(default = "default_screenshot_save_dir")]
    pub save_dir: String,
}

/// Default screenshot directory from the XDG user Pictures directory,
/// falling back to `~/Pictures/screenshots` and then
/// `<current-directory>/screenshots`.
pub fn default_screenshot_dir() -> PathBuf {
    dirs::picture_dir()
        .or_else(|| dirs::home_dir().map(|home| home.join("Pictures")))
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
        .join("screenshots")
}

fn default_screenshot_save_dir() -> String {
    default_screenshot_dir().to_string_lossy().into_owned()
}

impl Default for ScreenshotConfig {
    fn default() -> Self {
        Self {
            save_dir: default_screenshot_save_dir(),
        }
    }
}

/// One declared agent scope. `ops` lists `OpClass` names (`Focus`,
/// `Close`, …); resource lists are id allowlists (empty or omitted means
/// unrestricted at that axis).
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
    /// Realm-id allowlist. Empty means unrestricted.
    #[serde(default)]
    pub realms: Vec<u64>,
}

/// The `[dock]` section. `pinned` lists the apps to keep on the dock in order;
/// each value matches an enumerated `.desktop` entry by its id, desktop-file
/// stem, `StartupWMClass`, or icon name (case-insensitive). An empty list keeps
/// the dock free of persistent application tiles by default. `autopopulate`
/// remains available as an explicit opt-in for selecting the first handful of
/// apps that have usable icons. Once the user pins or unpins an app from the
/// dock's context menu, `autopopulate` is written as `false` so the persisted
/// list is the sole source of truth.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DockConfig {
    #[serde(default)]
    pub pinned: Vec<String>,
    #[serde(default)]
    pub autopopulate: bool,
    #[serde(default)]
    pub autohide: bool,
    #[serde(default = "default_dock_autohide_timeout")]
    pub autohide_timeout: f32,
}

fn default_dock_autohide_timeout() -> f32 {
    2.5
}

/// The `[statusbar]` section. `enabled` controls whether the top status bar
/// chrome component is registered at startup; it defaults to `true` so an
/// unconfigured session keeps the bar.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StatusBarConfig {
    #[serde(default = "default_statusbar_enabled")]
    pub enabled: bool,
}

fn default_statusbar_enabled() -> bool {
    true
}

impl Default for StatusBarConfig {
    fn default() -> StatusBarConfig {
        StatusBarConfig {
            enabled: default_statusbar_enabled(),
        }
    }
}

/// The `[ui]` section: desktop-wide UI and window-presentation policy
/// (ADR-0029 and ADR-0063).
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
    /// Window-decoration ownership. `borderless` keeps controls in
    /// compositor gestures and shell surfaces without per-window title bars;
    /// `client-side` asks applications to draw their own frames.
    #[serde(default)]
    pub window_decorations: aegis_core::window::DecorationPolicy,
}

/// The `[input]` section.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputConfig {
    /// Touchpad policy, written as `[input.touchpad]`.
    #[serde(default)]
    pub touchpad: TouchpadConfig,
}

/// One `[[output]]` entry: per-connector display policy (ADR-0028). Only
/// `connector` is required; every other field overrides one aspect of the
/// backend-reported geometry.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputConfig {
    /// The connector name as reported by the backend (e.g. "DP-1",
    /// "HDMI-A-1", "nested"). Unmatched names are ignored with a diagnostic
    /// at load time by the caller, not by the schema.
    pub connector: String,
    /// Output scale factor. Integer scales advertise through `wl_output`;
    /// fractional scales through `wp_fractional_scale_v1`.
    #[serde(default)]
    pub scale: Option<f64>,
    /// Requested display mode: `"WxH"` or `"WxH@Hz"` (e.g.
    /// `"2560x1440@144"`). Applied at modeset time (startup and hotplug).
    #[serde(default)]
    pub mode: Option<String>,
    /// Position of the output's top-left corner in the global logical
    /// layout, in logical pixels.
    #[serde(default)]
    pub position: Option<OutputPosition>,
    /// Output transform name (`normal`, `90`, `180`, `270`, `flipped`,
    /// `flipped-90`, `flipped-180`, `flipped-270`; see
    /// [`aegis_core::Transform::from_name`]). Parsed and validated now;
    /// applied once the renderer supports output transforms.
    #[serde(default)]
    pub transform: Option<String>,
    /// Whether this output is the primary (focused) one.
    #[serde(default)]
    pub primary: bool,
}

/// The `position` table of a `[[output]]` entry: logical-pixel coordinates
/// in the global layout, written `position = { x = 1920, y = 0 }`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputPosition {
    pub x: i32,
    pub y: i32,
}

/// The `[layout]` section: tiling gaps and master ratio. Defaults match the
/// built-in `aegis_core::layout::LayoutParams`.
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
    /// Whether window positions, sizes, and workspaces are persisted across
    /// restarts (default true). Window rules can override for specific apps.
    #[serde(default = "default_remember_window_positions")]
    pub remember_window_positions: bool,
}

fn default_gaps() -> i32 {
    8
}
fn default_master_ratio() -> f32 {
    0.5
}
fn default_remember_window_positions() -> bool {
    true
}

impl Default for LayoutConfig {
    fn default() -> LayoutConfig {
        LayoutConfig {
            gaps: default_gaps(),
            master_ratio: default_master_ratio(),
            default_tiled: false,
            remember_window_positions: default_remember_window_positions(),
        }
    }
}

impl From<LayoutConfig> for aegis_core::layout::LayoutParams {
    fn from(c: LayoutConfig) -> aegis_core::layout::LayoutParams {
        aegis_core::layout::LayoutParams {
            gaps: c.gaps,
            master_ratio: c.master_ratio,
        }
    }
}

/// One key binding: a modifier set, a key, and the action it triggers.
///
/// Field names match the [`aegis_core::keybind`] name resolvers: `mods` is a
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

fn validate_realm_sandbox_limits(
    prefix: &str,
    memory_max_mib: u64,
    pids_max: u32,
    cpu_weight: u16,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if !(256..=1_048_576).contains(&memory_max_mib) {
        diagnostics.push(Diagnostic::new(
            Some(format!("{prefix}.memory_max_mib")),
            "must be between 256 and 1048576",
        ));
    }
    if !(16..=65_536).contains(&pids_max) {
        diagnostics.push(Diagnostic::new(
            Some(format!("{prefix}.pids_max")),
            "must be between 16 and 65536",
        ));
    }
    if !(1..=10_000).contains(&cpu_weight) {
        diagnostics.push(Diagnostic::new(
            Some(format!("{prefix}.cpu_weight")),
            "must be between 1 and 10000",
        ));
    }
}

fn validate_realm_sandbox_paths(field: &str, paths: &[String], diagnostics: &mut Vec<Diagnostic>) {
    for (index, path) in paths.iter().enumerate() {
        if !Path::new(path).is_absolute() {
            diagnostics.push(Diagnostic::new(
                Some(format!("{field}.{index}")),
                "must be an absolute path",
            ));
        }
    }
}

/// A filesystem-level failure while reading or writing the config file.
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
    /// The file could not be written.
    #[error("{path}: write failed")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
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
            if let Some(scale) = output.scale
                && (!scale.is_finite() || !(0.25..=4.0).contains(&scale))
            {
                diagnostics.push(Diagnostic::new(
                    Some(format!("output.{index}.scale")),
                    "must be between 0.25 and 4.0",
                ));
            }
            if let Some(mode) = &output.mode {
                match mode.parse::<aegis_core::output::ModeSpec>() {
                    Ok(spec) => {
                        // Sanity caps, not hardware limits: they catch typos
                        // (a stray digit) that would otherwise fall back to the
                        // best available mode with a confusing log line.
                        if spec.width > 16384 || spec.height > 16384 {
                            diagnostics.push(Diagnostic::new(
                                Some(format!("output.{index}.mode")),
                                "resolution must not exceed 16384",
                            ));
                        }
                        if spec.refresh_hz.is_some_and(|hz| hz > 1000) {
                            diagnostics.push(Diagnostic::new(
                                Some(format!("output.{index}.mode")),
                                "refresh rate must not exceed 1000 Hz",
                            ));
                        }
                    }
                    Err(()) => diagnostics.push(Diagnostic::new(
                        Some(format!("output.{index}.mode")),
                        format!("invalid mode '{mode}'; expected \"WxH\" or \"WxH@Hz\""),
                    )),
                }
            }
            if let Some(transform) = &output.transform
                && aegis_core::Transform::from_name(transform).is_none()
            {
                diagnostics.push(Diagnostic::new(
                    Some(format!("output.{index}.transform")),
                    format!("unknown transform '{transform}'"),
                ));
            }
            if output.scale.is_none()
                && output.mode.is_none()
                && output.position.is_none()
                && output.transform.is_none()
                && !output.primary
            {
                diagnostics.push(Diagnostic::new(
                    Some(format!("output.{index}")),
                    "has no effect; set at least one of scale, mode, position, transform, primary",
                ));
            }
        }
        if let Some(size) = cfg.ui.cursor_size
            && !(8..=128).contains(&size)
        {
            diagnostics.push(Diagnostic::new(
                Some("ui.cursor_size".into()),
                "must be between 8 and 128",
            ));
        }
        if !cfg.input.touchpad.pointer_speed.is_finite()
            || !(-1.0..=1.0).contains(&cfg.input.touchpad.pointer_speed)
        {
            diagnostics.push(Diagnostic::new(
                Some("input.touchpad.pointer_speed".into()),
                "must be between -1.0 and 1.0",
            ));
        }
        validate_realm_sandbox_limits(
            "realm_sandbox",
            cfg.realm_sandbox.memory_max_mib,
            cfg.realm_sandbox.pids_max,
            cfg.realm_sandbox.cpu_weight,
            &mut diagnostics,
        );
        validate_realm_sandbox_paths(
            "realm_sandbox.readable_paths",
            &cfg.realm_sandbox.readable_paths,
            &mut diagnostics,
        );
        validate_realm_sandbox_paths(
            "realm_sandbox.writable_paths",
            &cfg.realm_sandbox.writable_paths,
            &mut diagnostics,
        );
        for (index, app) in cfg.realm_sandbox.apps.iter().enumerate() {
            let prefix = format!("realm_sandbox.app.{index}");
            if app.desktop_id.trim().is_empty() {
                diagnostics.push(Diagnostic::new(
                    Some(format!("{prefix}.desktop_id")),
                    "must not be empty",
                ));
            }
            validate_realm_sandbox_limits(
                &prefix,
                app.memory_max_mib
                    .unwrap_or(cfg.realm_sandbox.memory_max_mib),
                app.pids_max.unwrap_or(cfg.realm_sandbox.pids_max),
                app.cpu_weight.unwrap_or(cfg.realm_sandbox.cpu_weight),
                &mut diagnostics,
            );
            if let Some(paths) = &app.readable_paths {
                validate_realm_sandbox_paths(
                    &format!("{prefix}.readable_paths"),
                    paths,
                    &mut diagnostics,
                );
            }
            if let Some(paths) = &app.writable_paths {
                validate_realm_sandbox_paths(
                    &format!("{prefix}.writable_paths"),
                    paths,
                    &mut diagnostics,
                );
            }
        }
        if cfg.screenshot.save_dir.trim().is_empty() {
            diagnostics.push(Diagnostic::new(
                Some("screenshot.save_dir".into()),
                "must not be empty",
            ));
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
                match aegis_core::keybind::mod_from_name(m) {
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
            let Some(keysym) = aegis_core::keybind::keysym_from_name(&entry.key) else {
                errs.push(Diagnostic::new(
                    Some(field.clone()),
                    format!("unknown key '{}'", entry.key),
                ));
                continue;
            };
            let Some(action) = aegis_core::keybind::action_from_name(&entry.action) else {
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

    /// Resolve the `[[output]]` entries into per-connector
    /// [`aegis_core::output::OutputPolicy`]s (ADR-0028). Mode and transform
    /// strings were validated at parse time, so unresolvable ones degrade to
    /// `None` rather than failing here. When several entries name the same
    /// connector, the later entry wins.
    pub fn output_policies(
        &self,
    ) -> std::collections::HashMap<String, aegis_core::output::OutputPolicy> {
        let mut policies = std::collections::HashMap::new();
        for output in &self.outputs {
            policies.insert(
                output.connector.clone(),
                aegis_core::output::OutputPolicy {
                    scale: output.scale,
                    mode: output.mode.as_deref().and_then(|m| m.parse().ok()),
                    position: output
                        .position
                        .map(|p| aegis_core::Point { x: p.x, y: p.y }),
                    transform: output
                        .transform
                        .as_deref()
                        .and_then(aegis_core::Transform::from_name),
                    primary: output.primary,
                },
            );
        }
        policies
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
            });
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

/// One typed edit to the user-owned configuration document.
///
/// Edits are persistence operations, not application commands: the
/// compositor validates authority and applies live state before or after
/// submitting them. Keeping this enum in `aegis-config` centralizes the TOML
/// representation without exposing it to the System Settings application.
#[derive(Debug, Clone, PartialEq)]
pub enum ConfigEdit {
    /// Replace the complete manually managed dock pin list and disable
    /// automatic population.
    SetDockPinned { pinned: Vec<String> },
    /// Replace the complete `[input.touchpad]` profile.
    SetTouchpad { config: TouchpadConfig },
    /// Replace the user-editable fields for one `[[output]]` entry.
    SetOutput {
        settings: aegis_core::settings::DisplaySettings,
    },
}

/// Path-bound access to the versioned configuration document.
///
/// [`ConfigStore::apply`] performs one complete read-modify-validate-write
/// transaction and publishes the result with an atomic same-directory
/// replacement. Callers that submit edits concurrently must serialize calls;
/// the compositor does so on its config-write worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigStore {
    path: PathBuf,
}

impl ConfigStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load(&self) -> Result<Option<Config>, LoadError> {
        load(&self.path)
    }

    /// Apply one typed edit while preserving comments and unrelated keys.
    ///
    /// A missing document is initialized with the current schema version. An
    /// existing document must remain valid after the edit; malformed TOML,
    /// unknown fields, unsupported schema versions, and invalid values are
    /// reported without replacing the original file.
    pub fn apply(&self, edit: ConfigEdit) -> Result<(), LoadError> {
        let mut document = editable_document(&self.path)?;
        match edit {
            ConfigEdit::SetDockPinned { pinned } => apply_dock_pinned(&mut document, &pinned),
            ConfigEdit::SetTouchpad { config } => apply_touchpad(&mut document, &config),
            ConfigEdit::SetOutput { settings } => apply_output(&mut document, settings),
        }
        let contents = document.to_string();
        Config::parse(&contents).map_err(|diagnostics| LoadError::Invalid {
            path: self.path.clone(),
            diagnostics,
        })?;
        write_document_atomic(&self.path, &contents)
    }
}

fn apply_dock_pinned(document: &mut DocumentMut, pinned: &[String]) {
    if !document.get("dock").is_some_and(toml_edit::Item::is_table) {
        document["dock"] = toml_edit::Item::Table(toml_edit::Table::new());
    }

    let mut values = toml_edit::Array::new();
    for id in pinned {
        values.push(id.as_str());
    }
    document["dock"]["pinned"] = toml_edit::Item::Value(toml_edit::Value::Array(values));
    document["dock"]["autopopulate"] = toml_edit::value(false);
}

fn apply_touchpad(document: &mut DocumentMut, config: &TouchpadConfig) {
    if !document.get("input").is_some_and(toml_edit::Item::is_table) {
        document["input"] = toml_edit::Item::Table(toml_edit::Table::new());
    }
    if !document["input"]
        .get("touchpad")
        .is_some_and(toml_edit::Item::is_table)
    {
        document["input"]["touchpad"] = toml_edit::Item::Table(toml_edit::Table::new());
    }

    let touchpad = &mut document["input"]["touchpad"];
    touchpad["natural_scroll"] = toml_edit::value(config.natural_scroll);
    touchpad["tap_to_click"] = toml_edit::value(config.tap_to_click);
    touchpad["tap_and_drag"] = toml_edit::value(config.tap_and_drag);
    touchpad["drag_lock"] = toml_edit::value(config.drag_lock);
    touchpad["disable_while_typing"] = toml_edit::value(config.disable_while_typing);
    touchpad["pointer_speed"] = toml_edit::value(f64::from(config.pointer_speed));
    touchpad["scroll_method"] = toml_edit::value(match config.scroll_method {
        TouchpadScrollMethod::TwoFinger => "two-finger",
        TouchpadScrollMethod::Edge => "edge",
    });
}

fn apply_output(document: &mut DocumentMut, settings: aegis_core::settings::DisplaySettings) {
    let aegis_core::settings::DisplaySettings {
        connector,
        mode,
        scale,
        position,
        primary,
    } = settings;
    if !document
        .get("output")
        .is_some_and(toml_edit::Item::is_array_of_tables)
    {
        document["output"] = toml_edit::Item::ArrayOfTables(toml_edit::ArrayOfTables::new());
    }
    let outputs = document["output"]
        .as_array_of_tables_mut()
        .expect("output was normalized to an array of tables");

    if primary {
        for table in outputs.iter_mut() {
            if table.get("connector").and_then(toml_edit::Item::as_str) != Some(connector.as_str())
            {
                table.remove("primary");
            }
        }
        for index in (0..outputs.len()).rev() {
            if outputs
                .get(index)
                .is_some_and(|table| !output_table_has_override(table))
            {
                outputs.remove(index);
            }
        }
    }

    let index = (0..outputs.len())
        .rev()
        .find(|index| {
            outputs.get(*index).is_some_and(|table| {
                table.get("connector").and_then(toml_edit::Item::as_str) == Some(connector.as_str())
            })
        })
        .unwrap_or_else(|| {
            let mut table = toml_edit::Table::new();
            table["connector"] = toml_edit::value(connector.as_str());
            outputs.push(table);
            outputs.len() - 1
        });
    let output = outputs
        .get_mut(index)
        .expect("selected output table must still exist");
    output["connector"] = toml_edit::value(connector);
    output["mode"] = toml_edit::value(format_mode_spec(mode));
    output["scale"] = toml_edit::value(scale);
    let mut position_table = toml_edit::InlineTable::new();
    position_table.insert("x", toml_edit::Value::from(i64::from(position.x)));
    position_table.insert("y", toml_edit::Value::from(i64::from(position.y)));
    output["position"] = toml_edit::Item::Value(toml_edit::Value::InlineTable(position_table));
    if primary {
        output["primary"] = toml_edit::value(true);
    } else {
        output.remove("primary");
    }
}

fn editable_document(path: &Path) -> Result<DocumentMut, LoadError> {
    match std::fs::read_to_string(path) {
        Ok(text) => text
            .parse::<DocumentMut>()
            .map_err(|error| LoadError::Invalid {
                path: path.into(),
                diagnostics: vec![Diagnostic::new(None, format!("existing file: {error}"))],
            }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut document = DocumentMut::new();
            document["schema_version"] = toml_edit::value(i64::from(SUPPORTED_SCHEMA_VERSION));
            Ok(document)
        }
        Err(source) => Err(LoadError::Read {
            path: path.into(),
            source,
        }),
    }
}

fn output_table_has_override(table: &toml_edit::Table) -> bool {
    ["scale", "mode", "position", "transform"]
        .iter()
        .any(|key| table.contains_key(key))
        || table
            .get("primary")
            .and_then(toml_edit::Item::as_bool)
            .unwrap_or(false)
}

fn format_mode_spec(mode: aegis_core::output::ModeSpec) -> String {
    match mode.refresh_hz {
        Some(refresh) => format!("{}x{}@{refresh}", mode.width, mode.height),
        None => format!("{}x{}", mode.width, mode.height),
    }
}

fn write_document_atomic(path: &Path, contents: &str) -> Result<(), LoadError> {
    let Some(parent) = path.parent() else {
        return Err(LoadError::Write {
            path: path.into(),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "configuration path has no parent directory",
            ),
        });
    };
    std::fs::create_dir_all(parent).map_err(|source| LoadError::Write {
        path: path.into(),
        source,
    })?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("config.toml");
    let mut temporary = None;
    for attempt in 0..32_u32 {
        let candidate = parent.join(format!(
            ".{file_name}.ass-{}-{attempt}.tmp",
            std::process::id()
        ));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => {
                temporary = Some((candidate, file));
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(source) => {
                return Err(LoadError::Write {
                    path: path.into(),
                    source,
                });
            }
        }
    }
    let Some((temporary_path, mut file)) = temporary else {
        return Err(LoadError::Write {
            path: path.into(),
            source: std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "could not allocate a temporary configuration file",
            ),
        });
    };
    let write_result = (|| -> std::io::Result<()> {
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temporary_path, path)?;
        if let Ok(directory) = std::fs::File::open(parent) {
            let _ = directory.sync_all();
        }
        Ok(())
    })();
    if let Err(source) = write_result {
        let _ = std::fs::remove_file(&temporary_path);
        return Err(LoadError::Write {
            path: path.into(),
            source,
        });
    }
    Ok(())
}

/// The default config path: `$XDG_CONFIG_HOME/ass/config.toml`, falling back
/// to `~/.config/aegis/config.toml` per the `dirs` crate. `None` when the home
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
    use aegis_core::input::Mods as M;
    use aegis_core::keybind::Action;

    #[test]
    fn minimal_valid_config_loads() {
        let cfg = Config::parse("schema_version = 1\n").unwrap();
        assert_eq!(cfg.schema_version, 1);
        assert!(cfg.keybinds.is_empty());
    }

    #[test]
    fn statusbar_defaults_to_enabled() {
        let cfg = Config::parse("schema_version = 1\n").unwrap();
        assert!(cfg.statusbar.enabled);
    }

    #[test]
    fn appearance_defaults_to_no_preference() {
        let cfg = Config::parse("schema_version = 1\n").unwrap();
        assert_eq!(cfg.appearance.color_scheme, ColorScheme::System);
    }

    #[test]
    fn appearance_color_scheme_parses_portal_vocabulary() {
        let cfg =
            Config::parse("schema_version = 1\n[appearance]\ncolor_scheme = \"dark\"\n").unwrap();
        assert_eq!(cfg.appearance.color_scheme, ColorScheme::Dark);
        let cfg =
            Config::parse("schema_version = 1\n[appearance]\ncolor_scheme = \"light\"\n").unwrap();
        assert_eq!(cfg.appearance.color_scheme, ColorScheme::Light);
        assert!(
            Config::parse("schema_version = 1\n[appearance]\ncolor_scheme = \"neon\"\n").is_err()
        );
    }

    #[test]
    fn dock_defaults_to_an_empty_user_owned_strip() {
        let cfg = Config::parse("schema_version = 1\n").unwrap();
        assert!(cfg.dock.pinned.is_empty());
        assert!(!cfg.dock.autopopulate);
    }

    #[test]
    fn dock_autopopulation_remains_an_explicit_opt_in() {
        let cfg = Config::parse(
            "schema_version = 1\n\
             [dock]\n\
             autopopulate = true\n",
        )
        .unwrap();
        assert!(cfg.dock.autopopulate);
    }

    #[test]
    fn statusbar_can_be_disabled() {
        let cfg = Config::parse("schema_version = 1\n[statusbar]\nenabled = false\n").unwrap();
        assert!(!cfg.statusbar.enabled);
    }

    fn temp_config_path(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("ass-config-test-{}-{tag}.toml", std::process::id()))
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
        let original = "schema_version = 1\n\n# my apps\n[dock]\npinned = [\"a.desktop\"]\n\n[ui]\nreduced_motion = true\n";
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
        let original = "schema_version = 1\n\n# keep this\n[ui]\nreduced_motion = true\n";
        std::fs::write(&path, original).unwrap();
        let err = ConfigStore::new(&path)
            .apply(ConfigEdit::SetOutput {
                settings: aegis_core::settings::DisplaySettings {
                    connector: String::new(),
                    mode: aegis_core::output::ModeSpec {
                        width: 1920,
                        height: 1080,
                        refresh_hz: Some(60),
                    },
                    scale: 5.0,
                    position: aegis_core::Point::default(),
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
                settings: aegis_core::settings::DisplaySettings {
                    connector: "DP-1".into(),
                    mode: aegis_core::output::ModeSpec {
                        width: 1920,
                        height: 1080,
                        refresh_hz: Some(60),
                    },
                    scale: 1.0,
                    position: aegis_core::Point::default(),
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
        let defaults = Config::parse("schema_version = 1\n").unwrap();
        assert_eq!(defaults.input.touchpad, TouchpadConfig::default());
        assert!(defaults.input.touchpad.natural_scroll);

        let cfg = Config::parse(
            "schema_version = 1\n\
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

        let err = Config::parse("schema_version = 1\n[input.touchpad]\npointer_speed = 1.5\n")
            .unwrap_err();
        assert!(
            err.iter()
                .any(|d| d.field.as_deref() == Some("input.touchpad.pointer_speed"))
        );
    }

    #[test]
    fn config_store_sets_touchpad_profile_and_preserves_other_content() {
        let path = temp_config_path("touchpad");
        let original = "schema_version = 1\n\n# keep this\n[ui]\nreduced_motion = true\n";
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
        assert!(
            errs.iter()
                .all(|d| d.field.as_deref().unwrap_or("").starts_with("keybind["))
        );
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
            km.match_key(M::SUPER, aegis_core::input::XKB_KEY_Tab),
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
        let p = aegis_core::layout::LayoutParams::from(cfg.layout.clone());
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
    fn ui_window_decorations_default_to_borderless_and_accept_client_side() {
        let default = Config::parse("schema_version = 1\n").unwrap();
        assert_eq!(
            default.ui.window_decorations,
            aegis_core::window::DecorationPolicy::Borderless
        );

        let client = Config::parse(
            "schema_version = 1\n\
             [ui]\n\
             window_decorations = \"client-side\"\n",
        )
        .unwrap();
        assert_eq!(
            client.ui.window_decorations,
            aegis_core::window::DecorationPolicy::ClientSide
        );

        assert!(
            Config::parse(
                "schema_version = 1\n\
                 [ui]\n\
                 window_decorations = \"server-side\"\n",
            )
            .is_err()
        );
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
        assert_eq!(cfg.outputs[0].scale, Some(1.5));
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
            "schema_version = 1\n\
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
            "schema_version = 1\n\
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
        assert!(err
            .iter()
            .any(|d| d.field.as_deref() == Some("output.0.mode") && d.message.contains("1080p")));
        assert!(
            err.iter()
                .any(|d| d.field.as_deref() == Some("output.0.transform")
                    && d.message.contains("upside-down"))
        );
        assert!(err
            .iter()
            .any(|d| d.field.as_deref() == Some("output.1.mode") && d.message.contains("16384")));
        assert!(
            err.iter()
                .any(|d| d.field.as_deref() == Some("output.1.mode") && d.message.contains("1000"))
        );
    }

    #[test]
    fn output_entry_with_no_effect_is_diagnosed() {
        let err =
            Config::parse("schema_version = 1\n[[output]]\nconnector = \"DP-1\"\n").unwrap_err();
        assert_eq!(err.len(), 1);
        assert_eq!(err[0].field.as_deref(), Some("output.0"));
        assert!(err[0].message.contains("no effect"), "{err:?}");
        // Any single field is enough to make the entry meaningful.
        let cfg =
            Config::parse("schema_version = 1\n[[output]]\nconnector = \"DP-1\"\nprimary = true\n")
                .unwrap();
        assert!(cfg.outputs[0].primary);
    }

    #[test]
    fn output_policies_resolve_and_later_duplicate_wins() {
        let cfg = Config::parse(
            "schema_version = 1\n\
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
        assert_eq!(dp.position, Some(aegis_core::Point { x: 1920, y: 0 }));
        assert_eq!(dp.transform, Some(aegis_core::Transform::Rotate180));
        assert!(dp.primary);
        // The later HDMI-A-1 entry replaces the earlier one wholesale.
        let hdmi = &policies["HDMI-A-1"];
        assert_eq!(hdmi.scale, Some(2.0));
        assert!(!hdmi.primary);
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
            Some(aegis_core::layout::LayoutRole::Tiled)
        );
        assert_eq!(cfg.window_rules[1].title.as_deref(), Some("calculator"));
        assert_eq!(
            cfg.window_rules[1].role,
            Some(aegis_core::layout::LayoutRole::Floating)
        );
        assert!(cfg.window_rules[1].matches(None, Some("GNOME Calculator")));
    }

    #[test]
    fn screenshot_config_defaults_and_parses() {
        let cfg = Config::parse("schema_version = 1\n").unwrap();
        assert!(!cfg.screenshot.save_dir.is_empty());
        assert!(cfg.screenshot.save_dir.ends_with("screenshots"));

        let cfg2 =
            Config::parse("schema_version = 1\n[screenshot]\nsave_dir = \"/tmp/shots\"\n").unwrap();
        assert_eq!(cfg2.screenshot.save_dir, "/tmp/shots");

        let err = Config::parse("schema_version = 1\n[screenshot]\nsave_dir = \"\"\n").unwrap_err();
        assert!(
            err.iter()
                .any(|d| d.field.as_deref() == Some("screenshot.save_dir"))
        );
    }

    #[test]
    fn realm_sandbox_policy_is_default_deny_and_app_overrides_are_last_wins() {
        let cfg = Config::parse(
            "schema_version = 1\n\
             [realm_sandbox]\n\
             readable_paths = [\"/srv/reference\"]\n\
             memory_max_mib = 4096\n\
             [[realm_sandbox.app]]\n\
             desktop_id = \"browser.desktop\"\n\
             network = true\n\
             writable_paths = [\"/home/alice/Downloads\"]\n\
             [[realm_sandbox.app]]\n\
             desktop_id = \"browser.desktop\"\n\
             memory_max_mib = 2048\n",
        )
        .unwrap();
        let default = cfg.realm_sandbox.policy_for("editor.desktop");
        assert!(!default.network);
        assert_eq!(default.memory_max_bytes, 4096 * 1024 * 1024);
        assert_eq!(
            default.readable_paths,
            vec![PathBuf::from("/srv/reference")]
        );
        assert!(default.writable_paths.is_empty());

        let browser = cfg.realm_sandbox.policy_for("browser.desktop");
        assert!(browser.network);
        assert_eq!(browser.memory_max_bytes, 2048 * 1024 * 1024);
        assert_eq!(
            browser.writable_paths,
            vec![PathBuf::from("/home/alice/Downloads")]
        );
    }

    #[test]
    fn realm_sandbox_policy_rejects_unbounded_limits_and_relative_paths() {
        let diagnostics = Config::parse(
            "schema_version = 1\n\
             [realm_sandbox]\n\
             memory_max_mib = 1\n\
             pids_max = 1\n\
             cpu_weight = 0\n\
             readable_paths = [\"relative\"]\n",
        )
        .unwrap_err();
        for field in [
            "realm_sandbox.memory_max_mib",
            "realm_sandbox.pids_max",
            "realm_sandbox.cpu_weight",
            "realm_sandbox.readable_paths.0",
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
            "ass-config-output-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("config.toml");
        std::fs::write(
            &path,
            "schema_version = 1 # keep this comment\n\
             [[output]]\nconnector = \"DP-1\"\nprimary = true\n\
             [[output]]\nconnector = \"HDMI-A-1\"\ntransform = \"180\"\n",
        )
        .unwrap();

        ConfigStore::new(&path)
            .apply(ConfigEdit::SetOutput {
                settings: aegis_core::settings::DisplaySettings {
                    connector: "HDMI-A-1".into(),
                    mode: aegis_core::output::ModeSpec {
                        width: 2560,
                        height: 1440,
                        refresh_hz: Some(144),
                    },
                    scale: 1.5,
                    position: aegis_core::Point { x: 120, y: -40 },
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
        assert_eq!(policy.position, Some(aegis_core::Point { x: 120, y: -40 }));
        assert!(policy.primary);
        assert_eq!(policy.transform, Some(aegis_core::Transform::Rotate180));
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
}
