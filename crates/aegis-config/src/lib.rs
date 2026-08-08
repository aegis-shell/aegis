//! Declarative configuration for aegis.
//!
//! A single TOML file at `$XDG_CONFIG_HOME/aegis/config.toml` (defaulting to
//! `~/.config/aegis/config.toml`) is the source of truth for user-tunable
//! behavior. The file carries an explicit [`SUPPORTED_SCHEMA_VERSION`]; a
//! future-incompatible change bumps it and ships a migration note in the
//! CHANGELOG. The loader never silently ignores a problem: a malformed or
//! unsupported file is reported as structured [`Diagnostic`]s.
//!
//! The crate is pure: it depends on [`aegis_model`] for the shared keymap model
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

use aegis_model::input::{Mods, TouchpadConfig, TouchpadScrollMethod};
use aegis_model::keybind::{Keybind, Keymap};
pub use aegis_model::settings::{
    AccentColor, BatterySettings, ColorScheme, Contrast, DesktopPreferences, IdleSettings,
};
use toml_edit::DocumentMut;

mod migration;
pub use migration::{ConfigMigration, MigrationOutcome, migrate_file, migrate_text};

/// The schema `schema_version` this build understands. A file whose
/// `schema_version` differs is rejected with a precise diagnostic rather
/// than guessed at; bumping this is a real event documented in the CHANGELOG.
pub const SUPPORTED_SCHEMA_VERSION: u32 = 2;

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

    /// Touchpad swipe bindings, an array-of-tables written `[[gesture]]` in
    /// the file. Resolved against the built-in defaults with
    /// [`Config::gesture_map`].
    #[serde(default, rename = "gesture")]
    pub gestures: Vec<GestureEntry>,

    /// Window rules, an array-of-tables written `[[window_rule]]`. Evaluated
    /// on first map; the first match applies (move to workspace, force a
    /// layout role). See [`aegis_model::window_rule::WindowRule`].
    #[serde(default, rename = "window_rule")]
    pub window_rules: Vec<aegis_model::window_rule::WindowRule>,

    /// Tiling policy parameters (ADR-0024), written as a `[layout]` table.
    #[serde(default)]
    pub layout: LayoutConfig,

    /// Dock configuration, written as a `[dock]` table. Controls which apps
    /// are pinned to the dock and which screen edge it anchors to.
    #[serde(default)]
    pub dock: DockConfig,

    /// HUD configuration, written as a `[hud]` table. Controls
    /// whether the display-only HUD status chips are registered.
    #[serde(default)]
    pub hud: HudConfig,

    /// Desktop-wide UI and window-presentation policy, written as a `[ui]`
    /// table.
    #[serde(default)]
    pub ui: UiConfig,

    /// Physical input-device policy, written as an `[input]` table.
    #[serde(default)]
    pub input: InputConfig,

    /// Desktop wallpaper source and presentation mode. Environment overrides
    /// are resolved by the compositor runtime and take precedence over this
    /// persistent configuration.
    #[serde(default)]
    pub wallpaper: WallpaperConfig,

    /// Lock-screen presentation and its independently selected background,
    /// written as a `[lock_screen]` table.
    #[serde(default)]
    pub lock_screen: LockScreenConfig,

    /// Per-output display policy (ADR-0028), written as `[[output]]`
    /// array-of-tables. Each entry overrides the backend-reported mode,
    /// scale, position, transform, or primary-output selection for one connector.
    #[serde(default, rename = "output")]
    pub outputs: Vec<OutputConfig>,

    /// Agent authorization policy (ADR-0088). Capability ceilings for
    /// borrowing agents live in the compositor-held principal registry, not
    /// in this file; the old `[[agent.scope]]` declarations were removed in
    /// protocol v18.
    #[serde(default)]
    pub agent: AgentConfig,

    /// Default-deny process sandbox policy for applications launched inside
    /// Interaction Domains. Per-desktop-entry overrides are applied only to new launches.
    #[serde(default)]
    pub interaction_domain_sandbox: InteractionDomainSandboxConfig,

    /// Screenshot tool configuration, written as `[screenshot]`.
    #[serde(default)]
    pub screenshot: ScreenshotConfig,

    /// Desktop-wide appearance preference, written as `[appearance]`.
    /// Together with `[ui]`, this is resolved by the compositor into the
    /// single effective desktop-preferences snapshot.
    #[serde(default)]
    pub appearance: AppearanceConfig,

    /// Staged inactivity, locking, display-power, and suspend policy, written
    /// as `[idle]`.
    #[serde(default)]
    pub idle: IdleSettings,

    /// Low-battery warning thresholds, written as `[battery]`.
    #[serde(default)]
    pub battery: BatterySettings,

    /// Development-only escape hatches, written as a `[dev]` table.
    /// Development-only; will be removed before release. Do not rely on it.
    #[serde(default)]
    pub dev: DevConfig,
}

/// The `[dev]` section: development-only escape hatches.
///
/// Development-only escape hatch; will be removed before release. Do not
/// rely on it.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DevConfig {
    /// Allow the `Quit` binding (Super+Ctrl+Q by default) to match while the
    /// session is locked. Development-only escape hatch; will be removed
    /// before release. Do not rely on it.
    #[serde(default)]
    pub allow_quit_while_locked: bool,
}

/// The `[appearance]` section: desktop-wide visual and typography policy.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppearanceConfig {
    /// Preferred color scheme. `system` (the default) advertises no
    /// preference; `dark` and `light` map to the portal `color-scheme`
    /// values 1 and 2.
    #[serde(default)]
    pub color_scheme: ColorScheme,
    /// Optional sRGB accent color in canonical `#RRGGBB` form. Omission
    /// means no accent-color preference.
    #[serde(default)]
    pub accent_color: Option<String>,
    /// Normal or high visual contrast.
    #[serde(default)]
    pub contrast: Contrast,
    /// GTK/Pango-style proportional interface font description.
    #[serde(default)]
    pub font_name: Option<String>,
    /// GTK/Pango-style monospace font description.
    #[serde(default)]
    pub monospace_font_name: Option<String>,
    /// UI text scaling multiplier.
    #[serde(default)]
    pub text_scale: Option<f64>,
}

/// The composition used by the first-party lock screen.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LockScreenStyle {
    /// Clock and credentials share a centered, portrait-friendly column.
    Centered,
    /// Full-bleed artwork with peripheral clock and lower-right credentials.
    #[default]
    Cinematic,
}

/// Source type for the lock screen's background. This is intentionally
/// independent from [`WallpaperConfig`]: selecting a lock image must never
/// mutate or implicitly inherit the desktop wallpaper.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LockScreenBackgroundMode {
    /// The Aegis-provided lock artwork compiled into the lock client.
    #[default]
    Builtin,
    /// A flat color, useful for restrained or high-contrast installations.
    Solid,
    /// A user-selected static image decoded by the wallpaper engine.
    Image,
}

/// The `[lock_screen.background]` section.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LockScreenBackgroundConfig {
    #[serde(default)]
    pub mode: LockScreenBackgroundMode,
    /// Image path for `image` mode. Relative paths resolve beside
    /// `config.toml`.
    #[serde(default)]
    pub source: Option<String>,
    /// Solid background in `#RRGGBB` form. Omission uses the scheme-aware
    /// built-in solid.
    #[serde(default)]
    pub color: Option<String>,
    /// Strength of the legibility scrim placed over artwork.
    #[serde(default = "default_lock_screen_dim")]
    pub dim: f32,
}

const fn default_lock_screen_dim() -> f32 {
    0.28
}

impl Default for LockScreenBackgroundConfig {
    fn default() -> Self {
        Self {
            mode: LockScreenBackgroundMode::Builtin,
            source: None,
            color: None,
            dim: default_lock_screen_dim(),
        }
    }
}

/// The `[lock_screen]` section.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LockScreenConfig {
    #[serde(default)]
    pub style: LockScreenStyle,
    #[serde(default)]
    pub background: LockScreenBackgroundConfig,
}

/// The `[agent]` section (ADR-0088): runtime authorization policy for
/// capability-borrowing agents. Capability ceilings live in the
/// compositor-held principal registry, not in configuration; the named
/// `[[agent.scope]]` declarations from ADR-0034 were removed in protocol
/// v18 and are rejected here as unknown fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentConfig {
    /// Strip privileged capabilities from connections that neither present a
    /// built-in scope nor pair as an agent. Defaults to `true`; owner tools
    /// use an explicit built-in scope instead of ambient privilege.
    #[serde(default = "default_true")]
    pub lockdown: bool,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self { lockdown: true }
    }
}

fn default_true() -> bool {
    true
}

/// The `[interaction_domain_sandbox]` policy and `[[interaction_domain_sandbox.app]]` overrides.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InteractionDomainSandboxConfig {
    #[serde(default = "default_interaction_domain_memory_mib")]
    pub memory_max_mib: u64,
    #[serde(default = "default_interaction_domain_pids")]
    pub pids_max: u32,
    #[serde(default = "default_interaction_domain_cpu_weight")]
    pub cpu_weight: u16,
    #[serde(default, rename = "app")]
    pub apps: Vec<InteractionDomainSandboxAppConfig>,
}

impl Default for InteractionDomainSandboxConfig {
    fn default() -> Self {
        Self {
            memory_max_mib: default_interaction_domain_memory_mib(),
            pids_max: default_interaction_domain_pids(),
            cpu_weight: default_interaction_domain_cpu_weight(),
            apps: Vec::new(),
        }
    }
}

impl InteractionDomainSandboxConfig {
    pub fn policy_for(&self, desktop_id: &str) -> InteractionDomainSandboxPolicy {
        let mut memory_max_mib = self.memory_max_mib;
        let mut pids_max = self.pids_max;
        let mut cpu_weight = self.cpu_weight;

        for app in self.apps.iter().filter(|app| app.desktop_id == desktop_id) {
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

        InteractionDomainSandboxPolicy {
            memory_max_bytes: memory_max_mib * 1024 * 1024,
            pids_max,
            cpu_weight,
        }
    }
}

/// One exact desktop-entry override. Omitted fields inherit the enclosing
/// `[interaction_domain_sandbox]` resource-budget value.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InteractionDomainSandboxAppConfig {
    pub desktop_id: String,
    #[serde(default)]
    pub memory_max_mib: Option<u64>,
    #[serde(default)]
    pub pids_max: Option<u32>,
    #[serde(default)]
    pub cpu_weight: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InteractionDomainSandboxPolicy {
    pub memory_max_bytes: u64,
    pub pids_max: u32,
    pub cpu_weight: u16,
}

fn default_interaction_domain_memory_mib() -> u64 {
    8192
}

fn default_interaction_domain_pids() -> u32 {
    1024
}

fn default_interaction_domain_cpu_weight() -> u16 {
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
    /// Include the physical-seat cursor in saved screenshots. This defaults
    /// to true for the desktop screenshot UX and does not change output-
    /// capture or screencast IPC policy.
    #[serde(default = "default_screenshot_include_cursor")]
    pub include_cursor: bool,
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

const fn default_screenshot_include_cursor() -> bool {
    true
}

impl Default for ScreenshotConfig {
    fn default() -> Self {
        Self {
            save_dir: default_screenshot_save_dir(),
            include_cursor: default_screenshot_include_cursor(),
        }
    }
}

/// The `[dock]` section. `pinned` lists the apps to keep on the dock in order;
/// each value matches an enumerated `.desktop` entry by its id, desktop-file
/// stem, `StartupWMClass`, or icon name (case-insensitive). An empty list keeps
/// the dock free of persistent application tiles by default. `autopopulate`
/// remains available as an explicit opt-in for selecting the first handful of
/// apps that have usable icons. Once the user pins or unpins an app from the
/// dock's context menu, `autopopulate` is written as `false` so the persisted
/// list is the sole source of truth. `position` selects the screen edge the
/// dock anchors to (left, bottom, or right); it defaults to `bottom`.
/// `minimize_animation` selects the effect played when a window minimizes
/// into its dock tile (`genie`, `scale`, or `suck`); it defaults to `genie`.
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
    #[serde(default)]
    pub position: aegis_model::dock::DockPosition,
    #[serde(default)]
    pub minimize_animation: aegis_model::dock::MinimizeAnimationStyle,
}

fn default_dock_autohide_timeout() -> f32 {
    2.5
}

/// The `[hud]` section. `enabled` controls whether the display-only HUD
/// status chips are registered at startup; it defaults to `true` so an
/// unconfigured session keeps the HUD.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HudConfig {
    #[serde(default = "default_hud_enabled")]
    pub enabled: bool,
}

fn default_hud_enabled() -> bool {
    true
}

impl Default for HudConfig {
    fn default() -> HudConfig {
        HudConfig {
            enabled: default_hud_enabled(),
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
    /// Freedesktop application icon theme name for shell chrome such as the
    /// launcher and Dock. `$AEGIS_ICON_THEME` is an explicit startup
    /// override; no other desktop's settings database is consulted.
    #[serde(default)]
    pub icon_theme: Option<String>,
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
    pub window_decorations: aegis_model::window::DecorationPolicy,
}

/// The `[input]` section.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputConfig {
    /// Touchpad policy, written as `[input.touchpad]`.
    #[serde(default)]
    pub touchpad: TouchpadConfig,
}

/// The wallpaper rendering strategy selected by [`WallpaperConfig`]. Keeping
/// the four user-facing modes explicit prevents source-specific options from
/// accumulating as ambiguous combinations of booleans.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WallpaperMode {
    #[default]
    Image,
    Video,
    #[serde(rename = "3d")]
    ThreeD,
    Parallax,
}

/// One image in a parallax wallpaper, ordered from farthest to nearest.
/// `depth` is normalized: `0.0` remains fixed and `1.0` receives the full
/// configured pointer displacement.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WallpaperLayerConfig {
    pub path: String,
    pub depth: f32,
}

/// The `[wallpaper]` section.
///
/// Image and video modes use `source`; 3D uses `source` for `builtin` or a
/// `.glb` and may place it over `background`; parallax uses two to eight
/// `[[wallpaper.layer]]` images. Relative paths are resolved beside the
/// configuration file by the compositor.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WallpaperConfig {
    #[serde(default)]
    pub mode: WallpaperMode,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub background: Option<String>,
    #[serde(default = "default_wallpaper_max_shift")]
    pub max_shift: f32,
    #[serde(default = "default_wallpaper_transition_ms")]
    pub transition_ms: u32,
    #[serde(default, rename = "layer")]
    pub layers: Vec<WallpaperLayerConfig>,
}

const fn default_wallpaper_max_shift() -> f32 {
    32.0
}

const fn default_wallpaper_transition_ms() -> u32 {
    240
}

impl Default for WallpaperConfig {
    fn default() -> Self {
        Self {
            mode: WallpaperMode::Image,
            source: None,
            background: None,
            max_shift: default_wallpaper_max_shift(),
            transition_ms: default_wallpaper_transition_ms(),
            layers: Vec::new(),
        }
    }
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
    /// [`aegis_model::Transform::from_name`]). Parsed and validated now;
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
/// built-in `aegis_model::layout::LayoutParams`.
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

impl From<LayoutConfig> for aegis_model::layout::LayoutParams {
    fn from(c: LayoutConfig) -> aegis_model::layout::LayoutParams {
        aegis_model::layout::LayoutParams {
            gaps: c.gaps,
            master_ratio: c.master_ratio,
        }
    }
}

/// One key binding: a modifier set, a key, and the action it triggers.
///
/// Field names match the [`aegis_model::keybind`] name resolvers: `mods` is a
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

/// One touchpad swipe binding: a finger count, an axis, and the action it
/// triggers.
///
/// Field names match the [`aegis_model::gesture`] name resolvers. Unknown
/// names produce a per-entry diagnostic rather than aborting the whole file.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GestureEntry {
    /// Number of fingers on the touchpad. Swipe gestures require at least
    /// three fingers; one- and two-finger motion is pointer movement and
    /// scrolling, not a swipe.
    pub fingers: u8,
    /// Swipe axis: `horizontal` or `vertical`.
    pub axis: String,
    /// Action name: `workspace_switch`, `window_cycle`, `command_panel`, or
    /// `none` to shadow the built-in default on this axis.
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

fn validate_interaction_domain_sandbox_limits(
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
        validate_wallpaper(&cfg.wallpaper, &mut diagnostics);
        validate_lock_screen(&cfg.lock_screen, &mut diagnostics);
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
                match mode.parse::<aegis_model::output::ModeSpec>() {
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
                && aegis_model::Transform::from_name(transform).is_none()
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
        for (field, value) in [
            ("ui.icon_theme", cfg.ui.icon_theme.as_deref()),
            ("ui.cursor_theme", cfg.ui.cursor_theme.as_deref()),
            ("appearance.font_name", cfg.appearance.font_name.as_deref()),
            (
                "appearance.monospace_font_name",
                cfg.appearance.monospace_font_name.as_deref(),
            ),
        ] {
            if value.is_some_and(|value| value.trim().is_empty() || value.len() > 256) {
                diagnostics.push(Diagnostic::new(
                    Some(field.into()),
                    "must not be empty or longer than 256 bytes",
                ));
            }
        }
        if let Some(value) = cfg.appearance.accent_color.as_deref()
            && AccentColor::parse_hex(value).is_err()
        {
            diagnostics.push(Diagnostic::new(
                Some("appearance.accent_color".into()),
                "must use #RRGGBB",
            ));
        }
        if let Some(scale) = cfg.appearance.text_scale
            && (!scale.is_finite() || !(0.5..=3.0).contains(&scale))
        {
            diagnostics.push(Diagnostic::new(
                Some("appearance.text_scale".into()),
                "must be between 0.5 and 3.0",
            ));
        }
        if let Err(message) = cfg.idle.validate() {
            diagnostics.push(Diagnostic::new(Some("idle".into()), message));
        }
        if let Err(message) = cfg.battery.validate() {
            diagnostics.push(Diagnostic::new(Some("battery".into()), message));
        }
        if !cfg.input.touchpad.pointer_speed.is_finite()
            || !(-1.0..=1.0).contains(&cfg.input.touchpad.pointer_speed)
        {
            diagnostics.push(Diagnostic::new(
                Some("input.touchpad.pointer_speed".into()),
                "must be between -1.0 and 1.0",
            ));
        }
        validate_interaction_domain_sandbox_limits(
            "interaction_domain_sandbox",
            cfg.interaction_domain_sandbox.memory_max_mib,
            cfg.interaction_domain_sandbox.pids_max,
            cfg.interaction_domain_sandbox.cpu_weight,
            &mut diagnostics,
        );
        for (index, app) in cfg.interaction_domain_sandbox.apps.iter().enumerate() {
            let prefix = format!("interaction_domain_sandbox.app.{index}");
            if app.desktop_id.trim().is_empty() {
                diagnostics.push(Diagnostic::new(
                    Some(format!("{prefix}.desktop_id")),
                    "must not be empty",
                ));
            }
            validate_interaction_domain_sandbox_limits(
                &prefix,
                app.memory_max_mib
                    .unwrap_or(cfg.interaction_domain_sandbox.memory_max_mib),
                app.pids_max
                    .unwrap_or(cfg.interaction_domain_sandbox.pids_max),
                app.cpu_weight
                    .unwrap_or(cfg.interaction_domain_sandbox.cpu_weight),
                &mut diagnostics,
            );
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

    /// Resolve optional configuration fields against Aegis' deterministic
    /// built-in defaults. Explicit process-environment overrides are applied
    /// by the compositor runtime, not by this pure schema crate.
    pub fn desktop_preferences(&self) -> DesktopPreferences {
        let defaults = DesktopPreferences::default();
        DesktopPreferences {
            color_scheme: self.appearance.color_scheme,
            accent_color: self
                .appearance
                .accent_color
                .as_deref()
                .and_then(|value| AccentColor::parse_hex(value).ok()),
            contrast: self.appearance.contrast,
            reduced_motion: self.ui.reduced_motion,
            font_name: self
                .appearance
                .font_name
                .clone()
                .unwrap_or(defaults.font_name),
            monospace_font_name: self
                .appearance
                .monospace_font_name
                .clone()
                .unwrap_or(defaults.monospace_font_name),
            text_scale: self.appearance.text_scale.unwrap_or(defaults.text_scale),
            icon_theme: self.ui.icon_theme.clone().unwrap_or(defaults.icon_theme),
            cursor_theme: self
                .ui
                .cursor_theme
                .clone()
                .unwrap_or(defaults.cursor_theme),
            cursor_size: self.ui.cursor_size.unwrap_or(defaults.cursor_size),
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
                match aegis_model::keybind::mod_from_name(m) {
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
            let Some(keysym) = aegis_model::keybind::keysym_from_name(&entry.key) else {
                errs.push(Diagnostic::new(
                    Some(field.clone()),
                    format!("unknown key '{}'", entry.key),
                ));
                continue;
            };
            let Some(action) = aegis_model::keybind::action_from_name(&entry.action) else {
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

    /// Resolve the configured swipe bindings into
    /// [`aegis_model::gesture::GestureBinding`]s.
    /// Returns the resolved bindings plus one diagnostic per entry that
    /// could not resolve (bad finger count, unknown axis, or unknown
    /// action). Good entries are kept so a file with one typo still yields
    /// the rest.
    pub fn resolve_gestures(&self) -> (Vec<aegis_model::gesture::GestureBinding>, Vec<Diagnostic>) {
        let mut binds = Vec::new();
        let mut errs = Vec::new();
        for (i, entry) in self.gestures.iter().enumerate() {
            let field = format!("gesture[{i}]");
            if entry.fingers < 3 {
                errs.push(Diagnostic::new(
                    Some(field.clone()),
                    format!(
                        "swipe gestures need at least 3 fingers, got {}",
                        entry.fingers
                    ),
                ));
                continue;
            }
            let Some(axis) = aegis_model::gesture::gesture_axis_from_name(&entry.axis) else {
                errs.push(Diagnostic::new(
                    Some(field.clone()),
                    format!("unknown axis '{}'", entry.axis),
                ));
                continue;
            };
            let Some(action) = aegis_model::gesture::gesture_action_from_name(&entry.action) else {
                errs.push(Diagnostic::new(
                    Some(field.clone()),
                    format!("unknown action '{}'", entry.action),
                ));
                continue;
            };
            binds.push(aegis_model::gesture::GestureBinding {
                fingers: entry.fingers,
                axis,
                action,
            });
        }
        (binds, errs)
    }

    /// Build the active [`aegis_model::gesture::GestureMap`]: built-in defaults
    /// overridden by the configured entries, plus the diagnostics from
    /// resolution. Callers log the diagnostics and install the returned map.
    pub fn gesture_map(&self) -> (aegis_model::gesture::GestureMap, Vec<Diagnostic>) {
        let (overrides, errs) = self.resolve_gestures();
        (
            aegis_model::gesture::GestureMap::defaults().with_overrides(overrides),
            errs,
        )
    }

    /// Resolve the `[[output]]` entries into per-connector
    /// [`aegis_model::output::OutputPolicy`]s (ADR-0028). Mode and transform
    /// strings were validated at parse time, so unresolvable ones degrade to
    /// `None` rather than failing here. When several entries name the same
    /// connector, the later entry wins.
    pub fn output_policies(
        &self,
    ) -> std::collections::HashMap<String, aegis_model::output::OutputPolicy> {
        let mut policies = std::collections::HashMap::new();
        for output in &self.outputs {
            policies.insert(
                output.connector.clone(),
                aegis_model::output::OutputPolicy {
                    scale: output.scale,
                    mode: output.mode.as_deref().and_then(|m| m.parse().ok()),
                    position: output
                        .position
                        .map(|p| aegis_model::Point { x: p.x, y: p.y }),
                    transform: output
                        .transform
                        .as_deref()
                        .and_then(aegis_model::Transform::from_name),
                    primary: output.primary,
                },
            );
        }
        policies
    }
}

fn validate_wallpaper(config: &WallpaperConfig, diagnostics: &mut Vec<Diagnostic>) {
    let nonempty = |value: &Option<String>| {
        value
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
    };
    let reject_layers = |diagnostics: &mut Vec<Diagnostic>| {
        if !config.layers.is_empty() {
            diagnostics.push(Diagnostic::new(
                Some("wallpaper.layer".into()),
                "is only valid when wallpaper.mode is 'parallax'",
            ));
        }
    };

    match config.mode {
        WallpaperMode::Image => {
            if config
                .source
                .as_ref()
                .is_some_and(|path| path.trim().is_empty())
            {
                diagnostics.push(Diagnostic::new(
                    Some("wallpaper.source".into()),
                    "must not be empty",
                ));
            }
            if config.background.is_some() {
                diagnostics.push(Diagnostic::new(
                    Some("wallpaper.background".into()),
                    "is only valid when wallpaper.mode is '3d'",
                ));
            }
            reject_layers(diagnostics);
        }
        WallpaperMode::Video => {
            if !nonempty(&config.source) {
                diagnostics.push(Diagnostic::new(
                    Some("wallpaper.source".into()),
                    "is required when wallpaper.mode is 'video'",
                ));
            }
            if config.background.is_some() {
                diagnostics.push(Diagnostic::new(
                    Some("wallpaper.background".into()),
                    "is only valid when wallpaper.mode is '3d'",
                ));
            }
            reject_layers(diagnostics);
        }
        WallpaperMode::ThreeD => {
            if !nonempty(&config.source) {
                diagnostics.push(Diagnostic::new(
                    Some("wallpaper.source".into()),
                    "is required when wallpaper.mode is '3d'",
                ));
            }
            if config
                .background
                .as_ref()
                .is_some_and(|path| path.trim().is_empty())
            {
                diagnostics.push(Diagnostic::new(
                    Some("wallpaper.background".into()),
                    "must not be empty",
                ));
            }
            reject_layers(diagnostics);
        }
        WallpaperMode::Parallax => {
            if config.source.is_some() {
                diagnostics.push(Diagnostic::new(
                    Some("wallpaper.source".into()),
                    "is not used when wallpaper.mode is 'parallax'; use [[wallpaper.layer]]",
                ));
            }
            if config.background.is_some() {
                diagnostics.push(Diagnostic::new(
                    Some("wallpaper.background".into()),
                    "is not used when wallpaper.mode is 'parallax'; use [[wallpaper.layer]]",
                ));
            }
            if !(2..=8).contains(&config.layers.len()) {
                diagnostics.push(Diagnostic::new(
                    Some("wallpaper.layer".into()),
                    "parallax requires between 2 and 8 layers",
                ));
            }
            if !config.max_shift.is_finite() || !(1.0..=256.0).contains(&config.max_shift) {
                diagnostics.push(Diagnostic::new(
                    Some("wallpaper.max_shift".into()),
                    "must be between 1 and 256 logical pixels",
                ));
            }
            if !(80..=2_000).contains(&config.transition_ms) {
                diagnostics.push(Diagnostic::new(
                    Some("wallpaper.transition_ms".into()),
                    "must be between 80 and 2000 milliseconds",
                ));
            }
            let mut previous_depth = f32::NEG_INFINITY;
            for (index, layer) in config.layers.iter().enumerate() {
                let prefix = format!("wallpaper.layer.{index}");
                if layer.path.trim().is_empty() {
                    diagnostics.push(Diagnostic::new(
                        Some(format!("{prefix}.path")),
                        "must not be empty",
                    ));
                }
                if !layer.depth.is_finite() || !(0.0..=1.0).contains(&layer.depth) {
                    diagnostics.push(Diagnostic::new(
                        Some(format!("{prefix}.depth")),
                        "must be between 0.0 (fixed/far) and 1.0 (nearest)",
                    ));
                } else if layer.depth < previous_depth {
                    diagnostics.push(Diagnostic::new(
                        Some(format!("{prefix}.depth")),
                        "layers must be ordered from farthest to nearest (ascending depth)",
                    ));
                }
                previous_depth = previous_depth.max(layer.depth);
            }
        }
    }
}

fn validate_lock_screen(config: &LockScreenConfig, diagnostics: &mut Vec<Diagnostic>) {
    let background = &config.background;
    if !background.dim.is_finite() || !(0.0..=0.85).contains(&background.dim) {
        diagnostics.push(Diagnostic::new(
            Some("lock_screen.background.dim".into()),
            "must be between 0.0 and 0.85",
        ));
    }
    match background.mode {
        LockScreenBackgroundMode::Builtin => {
            if background.source.is_some() {
                diagnostics.push(Diagnostic::new(
                    Some("lock_screen.background.source".into()),
                    "is only valid when lock_screen.background.mode is 'image'",
                ));
            }
            if background.color.is_some() {
                diagnostics.push(Diagnostic::new(
                    Some("lock_screen.background.color".into()),
                    "is only valid when lock_screen.background.mode is 'solid'",
                ));
            }
        }
        LockScreenBackgroundMode::Solid => {
            if background.source.is_some() {
                diagnostics.push(Diagnostic::new(
                    Some("lock_screen.background.source".into()),
                    "is only valid when lock_screen.background.mode is 'image'",
                ));
            }
            if background
                .color
                .as_deref()
                .is_some_and(|value| AccentColor::parse_hex(value).is_err())
            {
                diagnostics.push(Diagnostic::new(
                    Some("lock_screen.background.color".into()),
                    "must use #RRGGBB",
                ));
            }
        }
        LockScreenBackgroundMode::Image => {
            if background
                .source
                .as_deref()
                .is_none_or(|source| source.trim().is_empty())
            {
                diagnostics.push(Diagnostic::new(
                    Some("lock_screen.background.source".into()),
                    "is required when lock_screen.background.mode is 'image'",
                ));
            }
            if background.color.is_some() {
                diagnostics.push(Diagnostic::new(
                    Some("lock_screen.background.color".into()),
                    "is only valid when lock_screen.background.mode is 'solid'",
                ));
            }
        }
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
    /// Move the dock to a different screen edge (left, bottom, or right).
    SetDockPosition {
        position: aegis_model::dock::DockPosition,
    },
    /// Select the animation played when a window minimizes into the dock.
    SetDockMinimizeAnimation {
        style: aegis_model::dock::MinimizeAnimationStyle,
    },
    /// Replace the complete `[input.touchpad]` profile.
    SetTouchpad { config: TouchpadConfig },
    /// Replace the user-editable fields for one `[[output]]` entry.
    SetOutput {
        settings: aegis_model::settings::DisplaySettings,
    },
    /// Replace all compositor-owned desktop appearance and UI preference
    /// fields while preserving unrelated presentation policy.
    SetDesktopPreferences { preferences: DesktopPreferences },
    /// Replace the complete `[idle]` staged inactivity policy.
    SetIdle { settings: IdleSettings },
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
            ConfigEdit::SetDockPosition { position } => {
                apply_dock_position(&mut document, position)
            }
            ConfigEdit::SetDockMinimizeAnimation { style } => {
                apply_dock_minimize_animation(&mut document, style)
            }
            ConfigEdit::SetTouchpad { config } => apply_touchpad(&mut document, &config),
            ConfigEdit::SetOutput { settings } => apply_output(&mut document, settings),
            ConfigEdit::SetDesktopPreferences { preferences } => {
                apply_desktop_preferences(&mut document, &preferences)
            }
            ConfigEdit::SetIdle { settings } => apply_idle(&mut document, settings),
        }
        let contents = document.to_string();
        Config::parse(&contents).map_err(|diagnostics| LoadError::Invalid {
            path: self.path.clone(),
            diagnostics,
        })?;
        write_document_atomic(&self.path, &contents)
    }
}

fn apply_idle(document: &mut DocumentMut, settings: IdleSettings) {
    if !document.get("idle").is_some_and(toml_edit::Item::is_table) {
        document["idle"] = toml_edit::Item::Table(toml_edit::Table::new());
    }
    let idle = &mut document["idle"];
    idle["enabled"] = toml_edit::value(settings.enabled);
    idle["dim_after_seconds"] = toml_edit::value(i64::from(settings.dim_after_seconds));
    idle["lock_after_seconds"] = toml_edit::value(i64::from(settings.lock_after_seconds));
    idle["display_off_after_seconds"] =
        toml_edit::value(i64::from(settings.display_off_after_seconds));
    idle["suspend_after_seconds"] = toml_edit::value(i64::from(settings.suspend_after_seconds));
    idle["dim_percent"] = toml_edit::value(i64::from(settings.dim_percent));
}

fn apply_desktop_preferences(document: &mut DocumentMut, preferences: &DesktopPreferences) {
    if !document
        .get("appearance")
        .is_some_and(toml_edit::Item::is_table)
    {
        document["appearance"] = toml_edit::Item::Table(toml_edit::Table::new());
    }
    if !document.get("ui").is_some_and(toml_edit::Item::is_table) {
        document["ui"] = toml_edit::Item::Table(toml_edit::Table::new());
    }

    let appearance = &mut document["appearance"];
    appearance["color_scheme"] = toml_edit::value(match preferences.color_scheme {
        ColorScheme::System => "system",
        ColorScheme::Dark => "dark",
        ColorScheme::Light => "light",
    });
    if let Some(accent) = preferences.accent_color {
        appearance["accent_color"] = toml_edit::value(accent.to_hex());
    } else {
        appearance
            .as_table_mut()
            .expect("appearance was normalized to a table")
            .remove("accent_color");
    }
    appearance["contrast"] = toml_edit::value(match preferences.contrast {
        Contrast::Normal => "normal",
        Contrast::High => "high",
    });
    appearance["font_name"] = toml_edit::value(preferences.font_name.as_str());
    appearance["monospace_font_name"] = toml_edit::value(preferences.monospace_font_name.as_str());
    appearance["text_scale"] = toml_edit::value(preferences.text_scale);

    let ui = &mut document["ui"];
    ui["reduced_motion"] = toml_edit::value(preferences.reduced_motion);
    ui["icon_theme"] = toml_edit::value(preferences.icon_theme.as_str());
    ui["cursor_theme"] = toml_edit::value(preferences.cursor_theme.as_str());
    ui["cursor_size"] = toml_edit::value(i64::from(preferences.cursor_size));
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

fn apply_dock_position(document: &mut DocumentMut, position: aegis_model::dock::DockPosition) {
    if !document.get("dock").is_some_and(toml_edit::Item::is_table) {
        document["dock"] = toml_edit::Item::Table(toml_edit::Table::new());
    }
    document["dock"]["position"] = toml_edit::value(position.name());
}

fn apply_dock_minimize_animation(
    document: &mut DocumentMut,
    style: aegis_model::dock::MinimizeAnimationStyle,
) {
    if !document.get("dock").is_some_and(toml_edit::Item::is_table) {
        document["dock"] = toml_edit::Item::Table(toml_edit::Table::new());
    }
    document["dock"]["minimize_animation"] = toml_edit::value(style.name());
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

fn apply_output(document: &mut DocumentMut, settings: aegis_model::settings::DisplaySettings) {
    let aegis_model::settings::DisplaySettings {
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

fn format_mode_spec(mode: aegis_model::output::ModeSpec) -> String {
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
            ".{file_name}.aegis-{}-{attempt}.tmp",
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

/// The default config path: `$XDG_CONFIG_HOME/aegis/config.toml`, falling back
/// to `~/.config/aegis/config.toml` per the `dirs` crate. `None` when the home
/// directory cannot be determined.
pub fn default_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("aegis").join("config.toml"))
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
mod tests;
