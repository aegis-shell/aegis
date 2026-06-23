# ADR-0026: Configuration system — one declarative file with live reload

- Status: Accepted
- Date: 2026-06-23

## Context

Configuration today is a single environment variable, `$ASS_KEYBINDS`,
parsed by [`ass-core::keybind`](../../crates/ass-core/src/keybind.rs) into a
`Keymap`. It is enough for key bindings and nothing else. The
[comparative survey](../explanation/comparative-survey.md#configuration)
records how the field configures a compositor: sway uses a plain-text file
in i3 syntax with a `set`/variable preprocessor; GNOME spreads settings
across `gsettings` schemas in `dconf` and per-extension folders; KDE uses
`~/.config` INI-like files; river has no file at all and is configured by a
shell script driving `riverctl`; niri uses a KDL file with full live reload;
Xfce uses `xfconf` channels edited through GUI dialogs.

The coming milestones all need configuration: workspace layout policy and
gaps ([ADR-0024](0024-layout-model.md)), per-workspace tiling rules, output
arrangement and scale ([ADR-0028](0028-output-and-monitor-model.md)),
animation and reduced-motion ([ADR-0029](0029-animation-and-effect-policy.md)),
key bindings, and window rules. A configuration surface that grows
organically as `$ASS_*` environment variables repeats the worst of the
field.

## Decision

ass standardizes on **one declarative TOML file** at
`$XDG_CONFIG_HOME/ass/config.toml` (defaulting to `~/.config/ass/config.toml`),
with a versioned schema and full live reload. The file is the single source
of truth; environment variables are reserved for build-time and
runtime-path concerns only (the existing `FLUX_BUILD_DIR` / `LENS_BUILD_DIR`
family) and never for behavior.

The schema carries an explicit `schema_version` field. A change that breaks
forward compatibility bumps the version and ships a migration note in the
[CHANGELOG](../../CHANGELOG.md); the loader rejects an unknown major version
with a precise error rather than guessing.

Live reload watches the file and applies the new configuration atomically.
Each section of the schema declares an `apply` contract: a change either
takes effect immediately (key bindings, gaps, theme) or is deferred to the
next safe point with the reason surfaced to the user. Reload errors are
reported as structured diagnostics — file path, line, field, message — and
never silently ignored. A successful reload reuses the existing keybinding
parser and matcher, which become one consumer of the configuration.

The existing `$ASS_KEYBINDS` path is retained as a transitional override
during M5 and removed before the desktop phase closes; the
[CHANGELOG](../../CHANGELOG.md) records the migration.

## Alternatives

- **Plain-text file with i3-style syntax (sway).** Rejected: the mixed
  expression/command grammar is inconsistent (some settings are
  expressions, some are commands), and live reload is partial. A parser
  that already distinguishes those cases is a maintenance cost the project
  does not need.
- **INI-like files across `~/.config` (KDE).** Rejected: spreading
  configuration across files defeats the one-source-of-truth principle and
  makes versioning the configuration as a unit impossible.
- **`gsettings` / `dconf` (GNOME).** Rejected: it is thorough but
  fragmented, and pulling in a GSettings/dconf dependency contradicts the
  hand-rolled, dependency-disciplined stance of
  [ADR-0002](0002-hand-rolled-wayland-server.md).
- **No file; configure purely through IPC (river).** Rejected as the
  primary path: an unstructured startup script is maximally expressive and
  minimally validatable. The IPC ([ADR-0027](0027-ipc-and-introspection.md))
  still mutates live state; it just does not own the persistent
  configuration.
- **KDL (niri).** Rejected narrowly: KDL is a good fit and a serious
  candidate, but TOML is chosen for tooling maturity, editor support, and a
  single well-known grammar the Rust ecosystem already ships
  (`serde` + `toml`).
- **YAML.** Rejected: significant whitespace and implicit typing make it a
  worse fit than TOML for a configuration a user hand-edits.
- **An embedded scripting language.** Rejected outright: it contradicts the
  "configuration is data, not code" principle in
  [Vision and Scope](../explanation/vision.md#design-principles) and the
  rejection of in-process scripting in
  [ADR-0027](0027-ipc-and-introspection.md).

## Consequences

- A new `ass-config` crate owns the schema, the loader, the watcher, and the
  migration logic. It depends only on `serde`, `toml`, and `ass-core`, so it
  is unit-testable in isolation, mirroring the discipline already applied to
  `ass-core::keybind`.
- The main loop gains a reload path that re-derives the runtime
  configuration and dispatches section-by-section `apply` calls to the
  window manager, the chrome, and the IPC.
- Every milestone that adds user-tunable behavior adds a schema section
  rather than an environment variable, which keeps the surface enumerable.
- The transitional `$ASS_KEYBINDS` override is deprecated and then removed;
  the migration is documented in the CHANGELOG.
- Live reload raises the bar on diagnostics: a malformed file must produce a
  precise, actionable error, or users lose trust in the reload path.
