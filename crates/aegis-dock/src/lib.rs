//! macOS-style dock: a rounded translucent bar at the bottom-center of the
//! output holding a persistent strip of pinned application icons. Unlike a
//! window list, the dock is populated from launchable `.desktop` entries the
//! binary pins (plus any running window that is not already pinned), so it
//! shows real XDG icons even when nothing is running.
//!
//! Each tile is one app:
//!   - Clicking a tile whose app has a running window focuses that window;
//!     clicking a tile with no running window launches the app (`out.spawn`).
//!   - A small dot beneath a tile marks a running app; the dot brightens for
//!     the activated window.
//!
//! Visuals are deliberately icon-first, not button-first: tiles are drawn as
//! bare raster icons (no pill / border), and hovering magnifies the tile under
//! the cursor and its neighbours along a cosine bell. Unlike a fixed-slot
//! dock, the bar *reflows*: it widens to fit the magnified widths and the
//! neighbouring tiles spread apart around the cursor — the classic macOS
//! squeeze-and-lift. Tiles are bottom-anchored and scale upward, so a
//! magnified icon pops *above* the bar. Each tile's size is driven by a
//! damped spring with a slight under-damped overshoot, so the wave tracks a
//! moving cursor and settles with a gentle bounce. Brand-new tiles (a window
//! just mapped) spring up from a seed size instead of popping in.

use std::cell::{Ref, RefCell};
use std::collections::HashMap;
use std::ffi::c_void;
use std::hash::Hasher;

use aegis_design::{Design, materials};
use lens::{Align, Color, Frame, Icon, Input, LayoutOpts, OverlayOpts, Rect};

use aegis_core::app::Entry;
use aegis_core::input::{KeyAction, KeyChar, key_action};
use aegis_core::window::{SpaceUse, Window};
use aegis_core::workspace::WorkspaceSnapshot;
use aegis_shell::{
    AppCatalog, AppMenu, BackdropRegion, Chrome, ChromeEvents, CursorShape, IconSet, Localizer,
    Message, PinAction, Reserved,
};

/// Visual height of the dock bar. Tiles rest inside it; magnified tiles pop
/// above its top edge (they are drawn as their own layers, unclipped).
const DOCK_PANEL_HEIGHT: f32 = 74.0;
/// Gap between the dock bar and the bottom edge of the output.
const DOCK_BOTTOM_MARGIN: f32 = 12.0;
/// Distance from the bar's bottom edge up to the icon baseline (the bottom of
/// every tile). Leaves room for the running-indicator dot strip plus a small
/// gap, while keeping the dot clear of the panel's rounded bottom corners.
const DOCK_BASELINE_INSET: f32 = 13.0;
/// Side length of a square dock tile at rest (the icon area).
const DOCK_TILE: f32 = 56.0;
/// Side length of a square dock tile at full magnification (1.5× rest).
const DOCK_TILE_MAX: f32 = 84.0;
/// How far (in rest-tile widths) the magnification reaches from the cursor.
const MAGNIFY_RADIUS_TILES: f32 = 2.0;
/// Spring stiffness (ω₀²) for tile size → target. Drives how strongly the
/// eased size is pulled toward its target. ~900 gives a period near 0.2s —
/// snappy enough to track the cursor, slow enough to read as intentional.
const SPRING_STIFFNESS: f32 = 900.0;
/// Spring damping ratio. 1.0 is critically damped (no overshoot); values just
/// under 1 give the slight macOS-style bounce-back. ~0.85 keeps the wave
/// lively while suppressing the visible jitter the previous lighter damping
/// produced under variable frame times.
const SPRING_DAMPING: f32 = 0.85;
/// Side length a brand-new tile grows in from. Springs up over the first few
/// frames instead of popping in at full size.
const DOCK_TILE_BIRTH: f32 = 6.0;
/// Vertical band above the bar that still triggers magnification, in pixels.
/// Lets the wave start as the cursor approaches; outside it (pointer in a
/// window above) the row stays at rest.
const MAGNIFY_APPROACH_BAND: f32 = 48.0;
/// Gap between adjacent rest slots inside the bar.
const DOCK_TILE_GAP: f32 = 10.0;
/// Extra breathing room between the pinned strip and the transient (running,
/// unpinned) section — the macOS-style separator between kept apps and apps
/// that only live while they run.
const DOCK_SECTION_GAP: f32 = 22.0;
/// Padding between the bar's edge and the first/last rest slot.
const DOCK_PAD: f32 = 10.0;
/// Diameter of a single running-indicator dot.
const DOCK_DOT: f32 = 5.0;
/// Width of a running-indicator stadium (pill) for multiple instances.
const DOCK_DOT_STADIUM: f32 = 12.0;
/// Inactivity timeout in seconds before an autohiding dock collapses.
const AUTOHIDE_IDLE_TIMEOUT: f32 = 2.5;
/// Width of the thin stadium handle shown when the dock is autohidden.
const AUTOHIDE_HANDLE_WIDTH: f32 = 140.0;
/// Height of the thin stadium handle shown when the dock is autohidden.
const AUTOHIDE_HANDLE_HEIGHT: f32 = 6.0;
/// Horizontal breathing room around the collapsed handle that reveals the
/// Dock. The rest of the bottom edge remains client-owned.
const AUTOHIDE_TRIGGER_PAD_X: f32 = 40.0;
/// Vertical approach band around the collapsed handle.
const AUTOHIDE_TRIGGER_HEIGHT: f32 = 24.0;
/// Pointer dwell before an application name appears. This keeps labels from
/// flashing while the pointer merely crosses the dock.
const TOOLTIP_DWELL: f32 = 0.30;
/// Exponential fade speed for the dock application-name tooltip.
const TOOLTIP_FADE_SPEED: f32 = 18.0;
const TOOLTIP_HEIGHT: f32 = 28.0;
const TOOLTIP_GAP: f32 = 9.0;

/// One application pinned to the dock: the launchable entry plus the lowercased
/// `app_id`s a running toplevel might report, used to fold a running window
/// into its pinned tile.
#[derive(Clone)]
pub struct DockApp {
    /// The entry spawned when the tile is clicked and no window matches.
    pub entry: Entry,
    /// Lowercased ids this app may run as (`StartupWMClass`, the desktop-id
    /// stem, the icon name). Matched against a window's `app_id`.
    pub keys: Vec<String>,
}

/// One resolved tile for the current frame: a pinned app, a running window, or
/// a pinned app that also has a running window folded into it.
#[derive(Clone)]
struct Tile {
    /// Stable identity for per-frame size easing (survives across frames).
    key: String,
    /// Borrowed icon texture, if one was decoded for this app/window.
    icon: Option<*mut c_void>,
    /// Whether at least one window of this app is mapped.
    running: bool,
    /// Whether the (a) matching window is the activated one.
    activated: bool,
    /// Surface id to focus on click (a running window), if any.
    focus: Option<aegis_core::window::WindowId>,
    /// Every running window folded into this application tile. Right-click
    /// actions operate on this complete set rather than an arbitrary match.
    windows: Vec<aegis_core::window::WindowId>,
    /// Index into [`Dock::apps`] for pinned application metadata. Unlike
    /// `spawn`, this remains present while the application is running so the
    /// context menu can offer "New Window".
    app: Option<usize>,
    /// Index into [`Dock::apps`] to spawn on click when nothing is running.
    spawn: Option<usize>,
    /// Human-readable context-menu heading.
    label: String,
    /// The Launchpad tile (always the first tile): clicking it toggles the
    /// launcher rather than focusing or spawning. Drawn as a 3×3 grid glyph.
    launchpad: bool,
    /// Whether the tile belongs to the persistent strip (launchpad or a
    /// pinned app). Transient tiles — unpinned running windows — sit on the
    /// right of the section separator and disappear when they close.
    pinned: bool,
}

impl Tile {
    /// The leading Launchpad tile — a macOS-style "show all apps" button that
    /// opens the launcher. Always present, never marked running.
    fn launchpad(label: &str) -> Tile {
        Tile {
            key: "launchpad".to_string(),
            icon: None,
            running: false,
            activated: false,
            focus: None,
            windows: Vec::new(),
            app: None,
            spawn: None,
            label: label.to_string(),
            launchpad: true,
            pinned: true,
        }
    }
}

/// The macOS-style dock.
pub struct Dock {
    /// Pinned launchable apps, in dock order. Rebuilt from the pushed
    /// catalog's `pinned` entries, with match keys derived via
    /// [`Entry::match_keys`].
    apps: Vec<DockApp>,
    /// The complete enumerated application catalog, refreshed with every
    /// rescan. Kept so the context menu can offer "Keep in Dock" for a
    /// transient running window whose entry is not currently pinned.
    all_apps: Vec<Entry>,
    /// `app_id` (lowercased) → borrowed icon texture pointer. Borrowed from
    /// the composition root's icon cache, which owns the `flux::Image`s and
    /// outlives this component (see [`IconSet`]). Shared by pinned tiles and
    /// unpinned running windows.
    icons: IconSet,
    /// Per-tile spring state (size + velocity) keyed by [`Tile::key`]. Entries
    /// for tiles that disappear are dropped each frame. A first-seen key starts
    /// at [`DOCK_TILE_BIRTH`] so new tiles grow in instead of popping.
    sizes: HashMap<String, SpringState>,
    /// Whether any tile's spring is still settling this frame. Set during
    /// [`Chrome::render`] by inspecting the post-step states; read by
    /// [`Chrome::anim_pending`] so the main loop keeps ticking frames until the
    /// wave fully rests.
    anim_active: bool,
    /// Whether the left button was held last frame, so a click fires once on
    /// the press edge. The host's per-frame `mouse_pressed` flag is not cleared
    /// between frames, so we track the `mouse_down` level transition ourselves.
    prev_down: bool,
    /// Shared popup implementation also used by the full-screen launcher.
    app_menu: AppMenu,
    /// Stable tile identity for an open menu, used to keep the popup attached
    /// while the dock's magnification spring moves and resizes that tile.
    menu_tile: Option<String>,
    /// Hover dwell and fade state for the app-name tooltip.
    hovered_tile: Option<String>,
    hover_elapsed: f32,
    tooltip_tile: Option<String>,
    tooltip_alpha: f32,
    /// Accessibility reduced-motion (ADR-0029): magnification springs and
    /// tooltip fades resolve to their targets in one frame.
    reduced_motion: bool,
    /// Whether autohide mode is enabled. When enabled, the dock does not
    /// reserve space at the bottom edge (so windows use the full screen height),
    /// floating as an overlay on top when revealed.
    autohide: bool,
    /// Reveal progress in [0.0, 1.0] (0 = collapsed/hidden, 1 = expanded).
    autohide_reveal: f32,
    /// Inactivity timer tracking elapsed seconds since pointer left the dock area.
    autohide_idle: f32,
    /// Configurable inactivity timeout in seconds before an autohiding dock collapses.
    autohide_timeout: f32,
    /// Compositor-derived cover state for the current visible windows. Kept
    /// outside render state because reserved edges, pointer capture, and
    /// backdrop capture are queried before the dock renders.
    space_use: SpaceUse,
    /// Resolved tile strip cache (Launchpad tile first), shared by `render`
    /// and `pointer_bounds` (via `backdrop_regions`/`captures_pointer`) so the
    /// strip is built once per change instead of up to three times per frame.
    /// Interior mutability: the pointer-side trait methods take `&self`.
    tile_cache: RefCell<TileCache>,
    /// Bumped on every catalog push so the tile cache notices pinned-app and
    /// icon changes without diffing the entries.
    catalog_revision: u64,
}

/// The cached tile strip plus the signature of the inputs it was built from.
/// `signature` is `None` until the first build.
struct TileCache {
    /// Signature over the catalog revision and the window fields the tiles
    /// derive from (see [`Dock::tile_signature`]).
    signature: Option<u64>,
    /// The localized "Applications" label the strip was built with. Tracked
    /// separately so pointer-only callers, which do not know the label, can
    /// reuse the strip as long as the windows match.
    label: String,
    tiles: Vec<Tile>,
}

/// A damped-spring state for one animated scalar (a tile's edge length).
/// Integrated semi-implicitly each frame so it stays stable across a wide
/// range of `dt` and produces the macOS-style slight overshoot.
#[derive(Clone, Copy, Default)]
struct SpringState {
    /// Current eased value (logical px).
    value: f32,
    /// Current velocity (px/s).
    vel: f32,
}

impl Dock {
    /// An empty dock. The pinned apps and decoded icons arrive through
    /// [`Chrome::update_app_catalog`], seeded on registration by
    /// [`aegis_shell::Shell::add`].
    pub fn new() -> Dock {
        Dock {
            apps: Vec::new(),
            all_apps: Vec::new(),
            icons: IconSet::default(),
            sizes: HashMap::new(),
            anim_active: false,
            prev_down: false,
            app_menu: AppMenu::new("aegis-dock-context-menu", true),
            menu_tile: None,
            hovered_tile: None,
            hover_elapsed: 0.0,
            tooltip_tile: None,
            tooltip_alpha: 0.0,
            reduced_motion: false,
            autohide: false,
            autohide_reveal: 1.0,
            autohide_idle: 0.0,
            autohide_timeout: AUTOHIDE_IDLE_TIMEOUT,
            space_use: SpaceUse::Available,
            tile_cache: RefCell::new(TileCache {
                signature: None,
                label: String::new(),
                tiles: Vec::new(),
            }),
            catalog_revision: 0,
        }
    }

    /// Set whether the dock automatically hides after an inactivity period.
    pub fn set_autohide(&mut self, autohide: bool) {
        self.autohide = autohide;
        if !autohide && self.space_use == SpaceUse::Available {
            self.autohide_reveal = 1.0;
            self.autohide_idle = 0.0;
        }
    }

    /// Set the inactivity timeout in seconds before the dock autohides.
    pub fn set_autohide_timeout(&mut self, timeout_secs: f32) {
        self.autohide_timeout = timeout_secs.max(0.1);
    }

    /// Toggle autohide mode.
    pub fn toggle_autohide(&mut self) {
        let current = self.autohide;
        self.set_autohide(!current);
    }

    /// Fullscreen owns the complete output: unlike maximized mode, it exposes
    /// neither Dock chrome nor a reveal target.
    fn fullscreen_locked(&self) -> bool {
        self.space_use == SpaceUse::Fullscreen
    }

    /// Maximized mode forces the same collapsed overlay mechanics as the
    /// user's autohide preference, without changing that preference.
    fn effective_autohide(&self) -> bool {
        self.autohide || self.space_use == SpaceUse::Maximized
    }

    fn hidden_trigger_bounds(display: (f32, f32)) -> Rect {
        Rect {
            x: (display.0 - AUTOHIDE_HANDLE_WIDTH) * 0.5 - AUTOHIDE_TRIGGER_PAD_X,
            y: display.1 - AUTOHIDE_TRIGGER_HEIGHT,
            w: AUTOHIDE_HANDLE_WIDTH + AUTOHIDE_TRIGGER_PAD_X * 2.0,
            h: AUTOHIDE_TRIGGER_HEIGHT,
        }
    }

    fn hidden_trigger_contains(cursor: (f32, f32), display: (f32, f32)) -> bool {
        let trigger = Self::hidden_trigger_bounds(display);
        cursor.0 >= trigger.x
            && cursor.1 >= trigger.y
            && cursor.0 < trigger.x + trigger.w
            && cursor.1 < trigger.y + trigger.h
    }

    /// Close UI that must not survive an automatic dock hide.
    fn dismiss_transient_ui(&mut self) {
        self.app_menu.dismiss();
        self.menu_tile = None;
        self.hovered_tile = None;
        self.hover_elapsed = 0.0;
        self.tooltip_tile = None;
        self.tooltip_alpha = 0.0;
    }

    /// Cosine-bell magnification factor in `[0, 1]` for a tile whose rest
    /// centre is `dx` pixels from the cursor. Returns 0 outside
    /// `MAGNIFY_RADIUS_TILES * DOCK_TILE`.
    fn magnify_factor(dx: f32) -> f32 {
        let radius = MAGNIFY_RADIUS_TILES * DOCK_TILE;
        let d = dx.abs();
        if d >= radius {
            return 0.0;
        }
        // 0.5 * (1 + cos(π * d/r)) — 1 at the centre, 0 at the edge.
        0.5 * (1.0 + (std::f32::consts::PI * d / radius).cos())
    }

    /// The rest (un-magnified) centre of tile `i` in a row of `n`, assuming the
    /// standard all-rest layout, used to measure cursor distance for the
    /// magnify factor. The live centre drifts as tiles widen, but macOS drives
    /// the *factor* from the fixed rest position so the wave does not chase its
    /// own tail (a wider tile pulling the cursor closer, magnifying more, …).
    /// `pinned_count` accounts for the extra section gap between the kept strip
    /// and the transient running section.
    fn rest_centre_estimate(i: usize, n: usize, pinned_count: usize, disp_w: f32) -> f32 {
        let unpinned = n.saturating_sub(pinned_count);
        let section_gap = if pinned_count > 0 && unpinned > 0 {
            DOCK_SECTION_GAP
        } else {
            0.0
        };
        let bar_w =
            n as f32 * DOCK_TILE + (n as f32 - 1.0) * DOCK_TILE_GAP + section_gap + 2.0 * DOCK_PAD;
        let bar_x = (disp_w - bar_w) * 0.5;
        let extra = if i >= pinned_count && pinned_count > 0 && unpinned > 0 {
            section_gap
        } else {
            0.0
        };
        bar_x + DOCK_PAD + i as f32 * (DOCK_TILE + DOCK_TILE_GAP) + extra + DOCK_TILE * 0.5
    }

    /// Advance a damped spring one `dt` seconds toward `target`. Semi-implicit
    /// Euler keeps it stable at large `dt` (a clamped-vs-true hybrid blows up
    /// under frame hitches); the under-damped ratio gives a single gentle
    /// overshoot — the macOS lift-and-settle. `value` and `vel` are updated in
    /// place and the new value is returned.
    fn spring(state: &mut SpringState, target: f32, dt: f32) -> f32 {
        // ω₀ = √stiffness is the undamped angular frequency; c = 2·ζ·ω₀ the
        // damping coefficient derived from the chosen damping ratio ζ.
        let omega0 = SPRING_STIFFNESS.sqrt();
        let damping = 2.0 * SPRING_DAMPING * omega0;
        // Clamp dt so a long stall (paused tab, debugger) does not blow the
        // integrator up; the spring simply catches up over the cap.
        let dt = dt.min(1.0 / 30.0);
        // Semi-implicit: update velocity from the force at the current value,
        // then advance value with the new velocity. Energy-stable and matches
        // the analytic damped-oscillator feel.
        let force = SPRING_STIFFNESS * (target - state.value) - damping * state.vel;
        state.vel += force * dt;
        state.value += state.vel * dt;
        state.value
    }

    /// The current frame's tile strip: the Launchpad tile, then every pinned
    /// app (with any running window folded in), then running windows that
    /// match no pinned app. A window matches an app when its lowercased
    /// `app_id` is among the app's [`DockApp::keys`].
    ///
    /// The strip is cached: `render` and `pointer_bounds` (called by both
    /// `backdrop_regions` and `captures_pointer`) all need it every frame, but
    /// it only changes when the window set, the pinned catalog, or the
    /// localized label does. Callers that do not know the localized label
    /// (the pointer-side trait methods) pass `None` and reuse the strip as
    /// long as the window signature matches.
    ///
    /// An associated function (not a method) so the returned borrow ties to
    /// `tile_cache` alone and `render` can keep mutating its other fields
    /// while iterating the strip.
    #[allow(clippy::too_many_arguments)]
    fn frame_tiles<'a>(
        tile_cache: &'a RefCell<TileCache>,
        apps: &[DockApp],
        icons: &IconSet,
        catalog_revision: u64,
        windows: &[Window],
        application_label: Option<&str>,
    ) -> Ref<'a, Vec<Tile>> {
        let signature = Self::tile_signature(catalog_revision, windows);
        let stale = {
            let cache = tile_cache.borrow();
            cache.signature != Some(signature)
                || application_label.is_some_and(|label| label != cache.label)
        };
        if stale {
            let label = application_label
                .map(str::to_string)
                .unwrap_or_else(|| tile_cache.borrow().label.clone());
            let tiles = Self::build_tiles(apps, icons, windows, &label);
            *tile_cache.borrow_mut() = TileCache {
                signature: Some(signature),
                label,
                tiles,
            };
        }
        Ref::map(tile_cache.borrow(), |cache| &cache.tiles)
    }

    /// A cheap signature over everything the tile strip derives from that can
    /// change between frames: the catalog revision (pinned apps and icons)
    /// plus each window's id, `app_id`, title, activation and read-only flag.
    fn tile_signature(catalog_revision: u64, windows: &[Window]) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        hasher.write_u64(catalog_revision);
        for w in windows {
            hasher.write_u64(w.id.0);
            match &w.app_id {
                Some(app_id) => {
                    hasher.write_u8(1);
                    hasher.write(app_id.as_bytes());
                }
                None => hasher.write_u8(0),
            }
            match &w.title {
                Some(title) => {
                    hasher.write_u8(1);
                    hasher.write(title.as_bytes());
                }
                None => hasher.write_u8(0),
            }
            hasher.write_u8(w.state.activated as u8);
            hasher.write_u8(w.read_only as u8);
        }
        hasher.finish()
    }

    /// Build the full strip from scratch. Called by [`Dock::frame_tiles`]
    /// only on a cache miss, so the per-window lowercasing and key/label
    /// allocations here no longer happen every frame.
    fn build_tiles(
        apps: &[DockApp],
        icons: &IconSet,
        windows: &[Window],
        application_label: &str,
    ) -> Vec<Tile> {
        let win_appid: Vec<Option<String>> = windows
            .iter()
            .map(|w| w.app_id.as_ref().map(|a| a.to_ascii_lowercase()))
            .collect();
        let mut claimed = vec![false; windows.len()];
        let mut tiles = Vec::with_capacity(apps.len() + windows.len() + 1);
        tiles.push(Tile::launchpad(application_label));

        for (i, app) in apps.iter().enumerate() {
            let mut running = false;
            let mut activated = false;
            let mut focus = None;
            let mut window_ids = Vec::new();
            for (wi, w) in windows.iter().enumerate() {
                let Some(a) = &win_appid[wi] else { continue };
                if app.keys.iter().any(|k| k == a) {
                    claimed[wi] = true;
                    running = true;
                    if w.state.activated {
                        window_ids.insert(0, w.id);
                    } else {
                        window_ids.push(w.id);
                    }
                    // Prefer the activated window as the focus target.
                    if !w.read_only && w.state.activated {
                        activated = true;
                        focus = Some(w.id);
                    } else if !w.read_only && focus.is_none() {
                        focus = Some(w.id);
                    }
                }
            }
            let icon = app.keys.iter().find_map(|k| icons.get(k));
            tiles.push(Tile {
                key: format!("app:{}", app.entry.id),
                icon,
                running,
                activated,
                focus,
                windows: window_ids,
                app: Some(i),
                spawn: if running { None } else { Some(i) },
                label: app.entry.name.clone(),
                launchpad: false,
                pinned: true,
            });
        }

        for (wi, w) in windows.iter().enumerate() {
            if claimed[wi] {
                continue;
            }
            let icon = win_appid[wi].as_ref().and_then(|a| icons.get(a));
            tiles.push(Tile {
                key: format!("win:{}", w.id.0),
                icon,
                running: true,
                activated: w.state.activated,
                focus: (!w.read_only).then_some(w.id),
                windows: vec![w.id],
                app: None,
                spawn: None,
                label: w
                    .title
                    .clone()
                    .or_else(|| w.app_id.clone())
                    .unwrap_or_else(|| application_label.to_string()),
                launchpad: false,
                pinned: false,
            });
        }
        tiles
    }

    /// The strip without the leading Launchpad tile — the view the unit tests
    /// assert against.
    #[cfg(test)]
    fn tiles(&self, windows: &[Window]) -> Vec<Tile> {
        Self::frame_tiles(
            &self.tile_cache,
            &self.apps,
            &self.icons,
            self.catalog_revision,
            windows,
            None,
        )[1..]
            .to_vec()
    }

    /// Bounds of the live dock interaction surface. Uses the current spring
    /// widths (or rest width before a tile's first render) so pointer routing
    /// follows the bar as it expands without claiming the entire bottom edge.
    fn pointer_bounds(&self, windows: &[Window], display: (f32, f32)) -> Rect {
        let tiles = Self::frame_tiles(
            &self.tile_cache,
            &self.apps,
            &self.icons,
            self.catalog_revision,
            windows,
            None,
        );
        let widths: Vec<f32> = tiles
            .iter()
            .map(|t| {
                self.sizes
                    .get(&t.key)
                    .map(|s| s.value.max(DOCK_TILE))
                    .unwrap_or(DOCK_TILE)
            })
            .collect();
        let pinned_count = tiles.iter().filter(|t| t.pinned).count();
        let unpinned = tiles.len().saturating_sub(pinned_count);
        let section_gap = if pinned_count > 0 && unpinned > 0 {
            DOCK_SECTION_GAP
        } else {
            0.0
        };
        let gaps = tiles.len().saturating_sub(1) as f32 * DOCK_TILE_GAP + section_gap;
        let bar_w = widths.iter().sum::<f32>() + gaps + 2.0 * DOCK_PAD;
        let panel_y = display.1 - DOCK_PANEL_HEIGHT - DOCK_BOTTOM_MARGIN;
        let icon_bottom = panel_y + DOCK_PANEL_HEIGHT - DOCK_BASELINE_INSET;
        Rect {
            x: (display.0 - bar_w) * 0.5,
            y: icon_bottom - DOCK_TILE_MAX,
            w: bar_w,
            h: panel_y + DOCK_PANEL_HEIGHT - (icon_bottom - DOCK_TILE_MAX),
        }
    }
}

impl Default for Dock {
    fn default() -> Self {
        Dock::new()
    }
}

impl Chrome for Dock {
    fn render(
        &mut self,
        f: &mut Frame,
        input: &Input,
        windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
        i18n: &Localizer,
        out: &mut ChromeEvents,
    ) {
        let disp = input.as_raw().display_size;
        let dt = input.as_raw().dt_seconds.max(0.0);
        let cursor = input.as_raw().cursor;
        let down = input.as_raw().mouse_down.first().copied().unwrap_or(false);

        // A fullscreen client owns the whole output edge: no animation,
        // handle, hover target, popup, or residual tooltip may surface above
        // it. Maximized is intentionally different and continues through the
        // forced-autohide path below.
        if self.fullscreen_locked() {
            self.dismiss_transient_ui();
            self.autohide_reveal = 0.0;
            self.autohide_idle = self.autohide_timeout;
            self.anim_active = false;
            self.prev_down = down;
            return;
        }

        let menu_was_open = self.app_menu.is_open();

        // The Launchpad tile always leads the strip (macOS-style), followed by
        // the pinned apps and any unpinned running windows. The strip comes
        // from the cache shared with `pointer_bounds`, so it is rebuilt only
        // when the window set, the catalog, or the localized label changes.
        let application_label = i18n.text(Message::Applications);
        let tiles = Self::frame_tiles(
            &self.tile_cache,
            &self.apps,
            &self.icons,
            self.catalog_revision,
            windows,
            Some(application_label),
        );
        let n = tiles.len();
        let pinned_count = tiles.iter().filter(|t| t.pinned).count();
        let unpinned_count = n.saturating_sub(pinned_count);
        let section_gap = if pinned_count > 0 && unpinned_count > 0 {
            DOCK_SECTION_GAP
        } else {
            0.0
        };

        // Drop eased sizes for tiles no longer present so the map does not
        // grow unbounded across long sessions.
        let live_keys: std::collections::HashSet<&str> =
            tiles.iter().map(|t| t.key.as_str()).collect();
        self.sizes.retain(|key, _| live_keys.contains(key.as_str()));

        let rest_panel_y = disp.y - DOCK_PANEL_HEIGHT - DOCK_BOTTOM_MARGIN;
        let effective_autohide = self.effective_autohide();

        // Pointer activation band for magnification and autohide reveal.
        let in_band = if effective_autohide && self.autohide_reveal < 0.2 {
            Self::hidden_trigger_contains((cursor.x, cursor.y), (disp.x, disp.y))
        } else {
            cursor.y >= rest_panel_y - MAGNIFY_APPROACH_BAND
        };
        let menu_open = self.app_menu.is_open();

        if effective_autohide {
            if in_band || menu_open {
                self.autohide_idle = 0.0;
            } else {
                self.autohide_idle += dt;
            }
        }

        let target_reveal = if effective_autohide {
            if self.autohide_idle >= self.autohide_timeout && !menu_open {
                0.0
            } else {
                1.0
            }
        } else {
            1.0
        };

        if self.reduced_motion {
            self.autohide_reveal = target_reveal;
        } else {
            let blend = 1.0 - (-12.0 * dt.min(1.0 / 30.0)).exp();
            self.autohide_reveal += (target_reveal - self.autohide_reveal) * blend;
            if (target_reveal - self.autohide_reveal).abs() < 0.002 {
                self.autohide_reveal = target_reveal;
            }
        }
        let autohide_moving = (target_reveal - self.autohide_reveal).abs() > 0.002;

        let hidden_y = disp.y + 10.0;
        let panel_y = hidden_y + (rest_panel_y - hidden_y) * self.autohide_reveal;

        // ---- contiguous reflow layout -------------------------------------
        // Unlike a fixed-rest dock, the bar widens to fit the magnified tiles
        // and neighbouring tiles spread apart around the cursor — the classic
        // macOS squeeze-and-lift. Because the total width changes, centres are
        // derived *from* the eased widths rather than from fixed slots, so the
        // cursor → tile mapping tracks the live layout.
        //
        // First ease each tile's size toward its magnification target. A
        // first-seen key springs up from DOCK_TILE_BIRTH (grow-in) instead of
        // snapping to rest size.
        let mut eased: Vec<f32> = Vec::with_capacity(n);
        // Track per-tile (target, velocity) so the anim-pending check below can
        // tell when every spring has fully rested.
        let mut unsettled = false;
        for (i, t) in tiles.iter().enumerate() {
            let factor = if in_band {
                Self::magnify_factor(
                    cursor.x - Self::rest_centre_estimate(i, n, pinned_count, disp.x),
                )
            } else {
                0.0
            };
            let target = DOCK_TILE + (DOCK_TILE_MAX - DOCK_TILE) * factor;
            // Look up before inserting so an existing tile does not pay a
            // key clone every frame.
            let state = match self.sizes.get_mut(&t.key) {
                Some(state) => state,
                None => self.sizes.entry(t.key.clone()).or_insert(SpringState {
                    value: if menu_was_open {
                        DOCK_TILE
                    } else {
                        DOCK_TILE_BIRTH
                    },
                    vel: 0.0,
                }),
            };
            // A context menu must not become a moving target. Freeze the
            // complete wave exactly where it was opened; once the menu closes,
            // the same springs resume toward the live pointer targets.
            if menu_was_open {
                state.vel = 0.0;
                eased.push(state.value);
                continue;
            }
            if self.reduced_motion {
                // ADR-0029: springs resolve to their target in one frame.
                state.value = target;
                state.vel = 0.0;
                eased.push(target);
                continue;
            }
            eased.push(Self::spring(state, target, dt));
            // A spring is still animating while it is meaningfully off its
            // target or still moving. Sub-pixel drift is ignored so we don't
            // tick forever chasing float noise.
            let drifting = (state.value - target).abs() > 0.15 || state.vel.abs() > 0.5;
            unsettled |= drifting;
        }
        self.anim_active = unsettled || autohide_moving;

        // Sum the eased widths (plus the inter-tile gap) to get the live bar
        // width. The gap is constant; only the tiles widen. Centred horizontally.
        let total_tiles: f32 = eased.iter().sum();
        let bar_w = total_tiles + (n as f32 - 1.0) * DOCK_TILE_GAP + section_gap + 2.0 * DOCK_PAD;
        let bar_x = (disp.x - bar_w) * 0.5;

        // The running x-offset of each tile's centre, left to right. The
        // pinned strip and the transient running section are separated by the
        // wider section gap instead of the ordinary tile gap.
        let mut centres = Vec::with_capacity(n);
        let mut x = bar_x + DOCK_PAD;
        for (i, s) in eased.iter().enumerate() {
            if i > 0 {
                let gap = if !tiles[i].pinned && tiles[i - 1].pinned {
                    section_gap
                } else {
                    DOCK_TILE_GAP
                };
                x += gap;
            }
            centres.push(x + s * 0.5);
            x += *s;
        }
        let centre = |i: usize| centres[i];

        // Bottom of every tile (icons are bottom-anchored and grow upward).
        let icon_bottom = panel_y + DOCK_PANEL_HEIGHT - DOCK_BASELINE_INSET;
        let icon_rects: Vec<Rect> = (0..n)
            .map(|i| {
                let s = eased[i].max(1.0);
                Rect {
                    x: centre(i) - s * 0.5,
                    y: icon_bottom - s,
                    w: s,
                    h: s,
                }
            })
            .collect();

        // The popup belongs to a tile rather than the pointer coordinate.
        // Re-anchor every frame so it follows the tile's live spring geometry.
        if self.app_menu.is_open() {
            if let Some((index, _)) = self
                .menu_tile
                .as_ref()
                .and_then(|key| tiles.iter().enumerate().find(|(_, tile)| &tile.key == key))
            {
                self.app_menu.set_owner(icon_rects[index]);
            } else {
                self.app_menu.dismiss();
                self.menu_tile = None;
            }
        }

        // The bar background, drawn first so icons stack above it. Its width
        // follows the live reflow, so the panel visibly widens on hover.
        let panel_rect = Rect {
            x: bar_x,
            y: panel_y,
            w: bar_w,
            h: DOCK_PANEL_HEIGHT,
        };
        // A layer with an empty body collapses to ~0 (the rect is only an
        // anchor, not a size); a fixed-size child forces it to the bar size.
        f.layer(
            "aegis-dock",
            panel_rect,
            &materials::dock(&Design::dark()),
            |f| {
                f.column_ex(&sized(bar_w, DOCK_PANEL_HEIGHT), |_| {});
            },
        );

        if effective_autohide && self.autohide_reveal < 0.99 {
            let handle_w = AUTOHIDE_HANDLE_WIDTH;
            let handle_h = AUTOHIDE_HANDLE_HEIGHT;
            let handle_x = (disp.x - handle_w) * 0.5;
            let handle_y = disp.y - DOCK_BOTTOM_MARGIN - handle_h;
            let handle_rect = Rect {
                x: handle_x,
                y: handle_y,
                w: handle_w,
                h: handle_h,
            };
            let alpha_factor = (1.0 - self.autohide_reveal).clamp(0.0, 1.0);
            let color = Color::rgba(240, 243, 252, (150.0 * alpha_factor) as u8);
            f.layer(
                "aegis-dock-autohide-stadium-handle",
                handle_rect,
                &tile_opts(),
                |f| {
                    f.column_ex(
                        &sized_fill(handle_w, handle_h, color, handle_h * 0.5),
                        |_| {},
                    );
                },
            );
        }

        // Hit-test the cursor against tile slots. A slot spans each tile's live
        // width (so magnified tiles are fully clickable) and the whole bar
        // height plus the popped-icon band; ties resolve to the nearest centre.
        let slot_top = icon_bottom - DOCK_TILE_MAX;
        let slot_bottom = panel_y + DOCK_PANEL_HEIGHT;
        let mut hit: Option<usize> = None;
        if cursor.y >= slot_top && cursor.y <= slot_bottom {
            let mut best = f32::MAX;
            for (i, width) in eased.iter().enumerate() {
                let cx = centre(i);
                let half = *width * 0.5 + DOCK_TILE_GAP * 0.5;
                let d = (cursor.x - cx).abs();
                if d <= half && d < best {
                    best = d;
                    hit = Some(i);
                }
            }
        }

        // Draw each tile's icon, then its running dot.
        for (i, t) in tiles.iter().enumerate() {
            let s = eased[i].max(1.0);
            let cx = centre(i);
            let rect = icon_rects[i];
            let icon_id = format!("aegis-dock-icon-{}", t.key);
            if t.launchpad {
                // A rounded "app tile" with a 3×3 grid, so it reads as macOS's
                // Launchpad button. The grid (real content) sizes the layer;
                // the layer paints the rounded background behind it.
                let bg = OverlayOpts {
                    bg: Color::rgba(70, 78, 110, 240),
                    border: Color::rgba(150, 160, 195, 180),
                    border_width: 1.0,
                    radius: s * 0.22,
                    pad: s * 0.2,
                    cross: Align::Center,
                    ..Default::default()
                };
                let gap = s * 0.1;
                let d = (s - 2.0 * (s * 0.2) - 2.0 * gap) / 3.0;
                f.layer(&icon_id, rect, &bg, |f| {
                    f.column_ex(&grid(gap), |f| {
                        for _ in 0..3 {
                            f.row_ex(&grid(gap), |f| {
                                for _ in 0..3 {
                                    f.column_ex(
                                        &sized_fill(d, d, Color::rgba(236, 238, 248, 245), d * 0.3),
                                        |_| {},
                                    );
                                }
                            });
                        }
                    });
                });
            } else {
                f.layer(&icon_id, rect, &tile_opts(), |f| match t.icon {
                    // The pointer crosses from the binary's flux binding type to
                    // lens's ABI-identical flux_image.
                    Some(ptr) => unsafe { f.image(ptr as *mut lens::sys::flux_image, s, s) },
                    None => f.icon(Icon::FileText, s * 0.6),
                });
            }

            if t.running {
                // Centre the dot in the flat strip between the icon baseline
                // and the panel bottom, so it never falls into the rounded
                // corner region (and outside the bar) on the leftmost or
                // rightmost tiles.
                let dot_w = if t.windows.len() > 1 {
                    DOCK_DOT_STADIUM
                } else {
                    DOCK_DOT
                };
                let strip_h = DOCK_BASELINE_INSET.max(DOCK_DOT);
                let dot_y = icon_bottom + (strip_h - DOCK_DOT) * 0.5 + DOCK_DOT * 0.5;
                let dot_rect = Rect {
                    x: cx - dot_w * 0.5,
                    y: dot_y - DOCK_DOT * 0.5,
                    w: dot_w,
                    h: DOCK_DOT,
                };
                let color = if t.activated {
                    Color::rgba(236, 238, 245, 255)
                } else {
                    Color::rgba(200, 204, 220, 170)
                };
                let dot_id = format!("aegis-dock-dot-{}", t.key);
                f.layer(&dot_id, dot_rect, &tile_opts(), |f| {
                    f.column_ex(&sized_fill(dot_w, DOCK_DOT, color, DOCK_DOT * 0.5), |_| {});
                });
            }
        }

        // A slim divider in the section gap separates the kept strip from the
        // transient running apps, like macOS's Dock.
        if section_gap > 0.0 {
            let divider_x = (centre(pinned_count - 1) + centre(pinned_count)) * 0.5;
            let divider_h = DOCK_TILE * 0.55;
            let divider_rect = Rect {
                x: divider_x - 0.5,
                y: panel_y + (DOCK_PANEL_HEIGHT - divider_h) * 0.5,
                w: 1.0,
                h: divider_h,
            };
            f.layer(
                "aegis-dock-section-divider",
                divider_rect,
                &OverlayOpts::default(),
                |f| {
                    f.column_ex(
                        &sized_fill(1.0, divider_h, Color::rgba(255, 255, 255, 56), 0.5),
                        |_| {},
                    );
                },
            );
        }

        // Fire a click once on the press edge (the host does not clear the
        // per-frame pressed flag, so track the button-down level transition).
        if down
            && !self.prev_down
            && !menu_was_open
            && let Some(i) = hit
        {
            let t = &tiles[i];
            if t.launchpad {
                out.toggle_launcher = true;
            } else if let Some(id) = t.focus {
                out.clicked = Some(id);
            } else if let Some(ai) = t.spawn {
                out.activate_entry(self.apps[ai].entry.clone());
            }
        }
        let right_pressed = input
            .as_raw()
            .mouse_pressed
            .get(1)
            .copied()
            .unwrap_or(false);
        if right_pressed && let Some(i) = hit {
            let tile = &tiles[i];
            if !tile.launchpad {
                let pin_action = if let Some(ai) = tile.app {
                    // A pinned tile always offers removal from the strip.
                    Some(PinAction::Unpin(self.apps[ai].entry.id.clone()))
                } else {
                    // A transient running window offers "Keep in Dock"
                    // only when its app_id resolves to an enumerated
                    // desktop entry.
                    let window_app_id = tile
                        .windows
                        .first()
                        .and_then(|id| windows.iter().find(|w| w.id == *id))
                        .and_then(|w| w.app_id.as_deref());
                    window_app_id.and_then(|app_id| {
                        self.all_apps
                            .iter()
                            .find(|entry| entry_matches_app_id(entry, app_id))
                            .map(|entry| PinAction::Pin(entry.id.clone()))
                    })
                };
                self.app_menu.open(
                    tile.label.clone(),
                    tile.app.map(|app| self.apps[app].entry.clone()),
                    tile.windows.iter().copied(),
                    icon_rects[i],
                    pin_action,
                );
                self.menu_tile = Some(tile.key.clone());
            }
        }

        // Reveal an app name only after a short dwell, then keep it centred
        // above the current animated icon. Switching tiles resets the dwell so
        // a sweep across the dock does not produce a trail of labels.
        let hovered_tile = hit.map(|i| tiles[i].key.clone());
        if self.hovered_tile != hovered_tile {
            self.hovered_tile = hovered_tile;
            self.hover_elapsed = 0.0;
            self.tooltip_tile = None;
            self.tooltip_alpha = 0.0;
        } else if self.hovered_tile.is_some() && !self.app_menu.is_open() {
            self.hover_elapsed += dt;
        }

        if self.app_menu.is_open() {
            self.tooltip_tile = None;
            self.tooltip_alpha = 0.0;
        } else {
            let wants_tooltip = self.hovered_tile.is_some() && self.hover_elapsed >= TOOLTIP_DWELL;
            if wants_tooltip {
                self.tooltip_tile.clone_from(&self.hovered_tile);
            }
            let target = if wants_tooltip { 1.0 } else { 0.0 };
            if self.reduced_motion {
                // ADR-0029: no fade; the tooltip appears/disappears at once.
                self.tooltip_alpha = target;
            } else {
                let blend = 1.0 - (-TOOLTIP_FADE_SPEED * dt.min(1.0 / 30.0)).exp();
                self.tooltip_alpha += (target - self.tooltip_alpha) * blend;
            }
            if target == 0.0 && self.tooltip_alpha < 0.01 {
                self.tooltip_alpha = 0.0;
                self.tooltip_tile = None;
            }
            let waiting = self.hovered_tile.is_some() && self.hover_elapsed < TOOLTIP_DWELL;
            let fading = (target - self.tooltip_alpha).abs() > 0.01;
            self.anim_active |= waiting || fading;
        }

        if let Some((index, tile)) = self
            .tooltip_tile
            .as_ref()
            .and_then(|key| tiles.iter().enumerate().find(|(_, tile)| &tile.key == key))
        {
            render_tooltip(
                f,
                &tile.label,
                icon_rects[index],
                (disp.x, disp.y),
                self.tooltip_alpha,
            );
        }
        self.app_menu.render(f, input, windows, i18n, out);
        if !self.app_menu.is_open() {
            self.menu_tile = None;
        }
        self.prev_down = down;
    }

    /// The dock reserves the bottom edge so tiled windows do not render under
    /// the bar (ADR-0024 chrome-aware work-area). The magnified-icon overshoot
    /// above the bar is intentionally not reserved — chrome draws over windows.
    fn reserved(&self) -> Reserved {
        if self.effective_autohide() || self.fullscreen_locked() {
            Reserved::default()
        } else {
            Reserved {
                top: 0,
                bottom: (DOCK_PANEL_HEIGHT + DOCK_BOTTOM_MARGIN) as i32,
                left: 0,
                right: 0,
            }
        }
    }

    fn backdrop_blur_sigma(&self) -> f32 {
        if self.fullscreen_locked() || (self.effective_autohide() && self.autohide_reveal <= 0.05) {
            0.0
        } else {
            12.0
        }
    }

    fn backdrop_regions(
        &self,
        display: (f32, f32),
        windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) -> Vec<BackdropRegion> {
        if self.fullscreen_locked() || (self.effective_autohide() && self.autohide_reveal <= 0.05) {
            return Vec::new();
        }
        let bounds = self.pointer_bounds(windows, display);
        let rest_panel_y = display.1 - DOCK_PANEL_HEIGHT - DOCK_BOTTOM_MARGIN;
        let hidden_y = display.1 + 10.0;
        let panel_y = hidden_y + (rest_panel_y - hidden_y) * self.autohide_reveal;
        let radius = 18.0;
        vec![
            BackdropRegion {
                x: bounds.x + radius,
                y: panel_y,
                w: (bounds.w - radius * 2.0).max(0.0),
                h: DOCK_PANEL_HEIGHT,
            },
            BackdropRegion {
                x: bounds.x,
                y: panel_y + radius,
                w: bounds.w,
                h: DOCK_PANEL_HEIGHT - radius * 2.0,
            },
        ]
    }

    fn captures_keyboard(&self) -> bool {
        self.app_menu.is_open()
    }

    fn key_char(&mut self, key: &KeyChar, _out: &mut ChromeEvents) {
        if matches!(key_action(key.keysym, key.ch), KeyAction::Escape) {
            self.app_menu.dismiss();
            self.menu_tile = None;
        }
    }

    /// The dock's magnify wave eases over many frames; report it as pending so
    /// the main loop keeps rendering (instead of blocking on the host queue)
    /// until every spring has rested.
    fn anim_pending(&self) -> bool {
        if self.fullscreen_locked() {
            return false;
        }
        let effective_autohide = self.effective_autohide();
        let target = if effective_autohide {
            if self.autohide_idle >= self.autohide_timeout && !self.app_menu.is_open() {
                0.0
            } else {
                1.0
            }
        } else {
            1.0
        };
        self.anim_active || (effective_autohide && (target - self.autohide_reveal).abs() > 0.002)
    }

    fn set_reduced_motion(&mut self, reduced: bool) {
        self.reduced_motion = reduced;
    }

    fn captures_pointer(
        &self,
        x: f32,
        y: f32,
        display: (f32, f32),
        windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) -> bool {
        if self.fullscreen_locked() {
            return false;
        }
        if self.app_menu.contains(x, y, display) {
            return true;
        }
        if self.effective_autohide() && self.autohide_reveal < 0.1 {
            return Self::hidden_trigger_contains((x, y), display);
        }
        let r = self.pointer_bounds(windows, display);
        x >= r.x && y >= r.y && x < r.x + r.w && y < r.y + r.h
    }

    fn visible_during_modal(&self) -> bool {
        true
    }

    fn cursor_shape_at(
        &self,
        _x: f32,
        _y: f32,
        _display: (f32, f32),
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) -> Option<CursorShape> {
        Some(CursorShape::Pointer)
    }

    fn update_windows(&mut self, windows: &[Window]) {
        let space_use = SpaceUse::from_windows(windows);
        if space_use == self.space_use {
            return;
        }

        self.space_use = space_use;
        match space_use {
            SpaceUse::Fullscreen => {
                // Lock immediately; fullscreen must not expose even the
                // hidden-dock edge trigger between snapshot and render.
                self.dismiss_transient_ui();
                self.autohide_reveal = 0.0;
                self.autohide_idle = self.autohide_timeout;
                self.anim_active = false;
            }
            SpaceUse::Maximized => {
                // Collapse to the visible handle immediately. Rendering and
                // hit-testing remain active so hovering near that handle can
                // reveal the overlay Dock.
                self.dismiss_transient_ui();
                self.autohide_reveal = 0.0;
                self.autohide_idle = self.autohide_timeout;
                self.anim_active = false;
            }
            SpaceUse::Available => {
                self.autohide_idle = 0.0;
            }
        }
    }

    fn update_app_catalog(&mut self, catalog: &AppCatalog) {
        self.app_menu.dismiss();
        self.menu_tile = None;
        self.all_apps = catalog.apps.clone();
        self.apps = catalog
            .pinned
            .iter()
            .map(|e| DockApp {
                entry: e.clone(),
                keys: e.match_keys(),
            })
            .collect();
        self.icons = catalog.icons.clone();
        self.catalog_revision = self.catalog_revision.wrapping_add(1);
    }
}

/// Whether `entry` is the desktop entry a running `app_id` belongs to. The
/// match mirrors the launcher's running-app heuristic: `StartupWMClass`,
/// the desktop-id stem, or the icon name (case-insensitive).
fn entry_matches_app_id(entry: &Entry, app_id: &str) -> bool {
    let want = app_id.to_ascii_lowercase();
    if want.is_empty() {
        return false;
    }
    entry
        .startup_wm_class
        .as_deref()
        .is_some_and(|wm| wm.to_ascii_lowercase() == want)
        || entry
            .id
            .trim_end_matches(".desktop")
            .eq_ignore_ascii_case(app_id)
        || entry
            .icon
            .as_deref()
            .is_some_and(|icon| icon.eq_ignore_ascii_case(app_id))
}

/// A fixed-size, transparent container used to force a layer (whose `rect` is
/// only an anchor, not a size) to a known width and height.
fn sized(w: f32, h: f32) -> LayoutOpts {
    LayoutOpts {
        width: w,
        height: h,
        ..Default::default()
    }
}

/// A fixed-size container that paints a rounded `bg` — the reliable filled-rect
/// primitive (lens paints a container's background at its solved size).
fn sized_fill(w: f32, h: f32, bg: Color, radius: f32) -> LayoutOpts {
    LayoutOpts {
        width: w,
        height: h,
        bg,
        radius,
        ..Default::default()
    }
}

/// A centred grid row/column with the given gap, for the Launchpad glyph.
fn grid(gap: f32) -> LayoutOpts {
    LayoutOpts {
        gap,
        cross: Align::Center,
        ..Default::default()
    }
}

/// A single icon tile: no fill, no border, no padding — just the raster icon
/// (or glyph fallback), centred so a glyph smaller than the cell is centred.
fn tile_opts() -> OverlayOpts {
    OverlayOpts {
        bg: Color::TRANSPARENT,
        border: Color::TRANSPARENT,
        border_width: 0.0,
        radius: 0.0,
        pad: 0.0,
        cross: Align::Center,
        ..Default::default()
    }
}

/// A compact app-name bubble that follows the owning dock icon. It is kept
/// visually quieter than a context menu and never obscures the icon itself.
fn render_tooltip(frame: &mut Frame, label: &str, owner: Rect, display: (f32, f32), alpha: f32) {
    let label = truncate_tooltip(label, 32);
    let text = frame.measure_text(&label, 12.5);
    let width = (text.width + 22.0).clamp(54.0, 224.0);
    let x = (owner.x + owner.w * 0.5 - width * 0.5).clamp(8.0, (display.0 - width - 8.0).max(8.0));
    let y = (owner.y - TOOLTIP_GAP - TOOLTIP_HEIGHT - (1.0 - alpha) * 3.0).max(8.0);
    let opacity = |base: u8| (base as f32 * alpha.clamp(0.0, 1.0)).round() as u8;
    let rect = Rect {
        x,
        y,
        w: width,
        h: TOOLTIP_HEIGHT,
    };
    let original = frame.theme();
    frame.set_theme(original.with_fg(Color::rgba(242, 244, 250, opacity(255))));
    frame.layer(
        "aegis-dock-app-name",
        rect,
        &OverlayOpts {
            // Frosted glass over the dock's backdrop-blur band: a light tint
            // with a bright edge, matching the bar's material instead of the
            // old opaque dark bubble.
            bg: Color::rgba(255, 255, 255, opacity(40)),
            border: Color::rgba(255, 255, 255, opacity(78)),
            border_width: 1.0,
            radius: TOOLTIP_HEIGHT * 0.5,
            pad: 0.0,
            cross: Align::Center,
            ..Default::default()
        },
        |frame| {
            frame.row_ex(
                &LayoutOpts {
                    height: TOOLTIP_HEIGHT,
                    cross: Align::Center,
                    ..Default::default()
                },
                |frame| frame.label_compact_sized(&label, 12.5),
            );
        },
    );
    frame.set_theme(original);
}

fn truncate_tooltip(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut value: String = text.chars().take(max_chars.saturating_sub(1)).collect();
    value.push('…');
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(id: &str) -> Entry {
        Entry {
            id: id.to_string(),
            ..Default::default()
        }
    }

    /// Register `pinned` entries the way the composition root does: one
    /// catalog push that rebuilds the pinned list from [`Entry::match_keys`].
    fn dock_with(pinned: Vec<Entry>) -> Dock {
        let mut dock = Dock::new();
        dock.update_app_catalog(&AppCatalog {
            apps: pinned.clone(),
            pinned,
            icons: IconSet::default(),
        });
        dock
    }

    fn window(id: u64, app_id: &str, activated: bool) -> Window {
        let mut w = Window {
            id: aegis_core::window::WindowId(id),
            app_id: Some(app_id.to_string()),
            ..Default::default()
        };
        w.state.activated = activated;
        w
    }

    fn workspace_snapshot() -> WorkspaceSnapshot {
        WorkspaceSnapshot {
            outputs: Vec::new(),
        }
    }

    #[test]
    fn magnify_factor_is_one_at_cursor() {
        assert!(Dock::magnify_factor(0.0) > 0.999);
    }

    #[test]
    fn magnify_factor_is_zero_outside_radius() {
        let radius = MAGNIFY_RADIUS_TILES * DOCK_TILE;
        assert_eq!(Dock::magnify_factor(radius), 0.0);
        assert_eq!(Dock::magnify_factor(radius + 1.0), 0.0);
        assert_eq!(Dock::magnify_factor(-radius), 0.0);
    }

    #[test]
    fn magnify_factor_is_symmetric() {
        for d in [5.0, 12.0, 33.0, 50.0] {
            let r = MAGNIFY_RADIUS_TILES * DOCK_TILE;
            if d < r {
                assert!((Dock::magnify_factor(d) - Dock::magnify_factor(-d)).abs() < 1e-5);
            }
        }
    }

    #[test]
    fn spring_approaches_target_at_rest() {
        // No time elapses → nothing moves.
        let mut s = SpringState {
            value: 10.0,
            vel: 0.0,
        };
        assert_eq!(Dock::spring(&mut s, 20.0, 0.0), 10.0);
    }

    #[test]
    fn spring_settles_on_target() {
        // Many small steps from rest must converge to the target.
        let mut s = SpringState {
            value: 10.0,
            vel: 0.0,
        };
        for _ in 0..2000 {
            Dock::spring(&mut s, 20.0, 1.0 / 120.0);
        }
        assert!((s.value - 20.0).abs() < 0.01, "settled at {}", s.value);
    }

    #[test]
    fn spring_overshoots_then_settles() {
        // Under-damped: from rest it should cross past the target at least once
        // before settling (the macOS lift-and-bounce).
        let mut s = SpringState {
            value: 0.0,
            vel: 0.0,
        };
        let mut overshot = false;
        for _ in 0..2000 {
            Dock::spring(&mut s, 100.0, 1.0 / 120.0);
            if s.value > 100.0 {
                overshot = true;
            }
        }
        assert!(overshot, "spring never overshot the target");
        assert!((s.value - 100.0).abs() < 0.01, "settled at {}", s.value);
    }

    #[test]
    fn spring_is_dt_stable() {
        // A single large step (a long frame stall) must not blow up.
        let mut s = SpringState {
            value: 0.0,
            vel: 0.0,
        };
        let v = Dock::spring(&mut s, 100.0, 1.0 / 5.0);
        assert!(v.is_finite(), "value diverged: {v}");
        assert!(s.vel.is_finite(), "velocity diverged: {}", s.vel);
    }

    #[test]
    fn pinned_apps_show_without_any_running_window() {
        let dock = dock_with(vec![app("firefox.desktop"), app("term.desktop")]);
        let tiles = dock.tiles(&[]);
        assert_eq!(
            tiles.len(),
            2,
            "both pinned apps are tiles even with no windows"
        );
        assert!(tiles.iter().all(|t| !t.running));
        // No running window → clicking launches (spawn), not focus.
        assert!(tiles.iter().all(|t| t.spawn.is_some() && t.focus.is_none()));
    }

    #[test]
    fn running_window_folds_into_its_pinned_tile() {
        let dock = dock_with(vec![app("firefox.desktop")]);
        let tiles = dock.tiles(&[window(7, "firefox", true)]);
        assert_eq!(
            tiles.len(),
            1,
            "the window folds into the pinned tile, not a new one"
        );
        assert!(tiles[0].running);
        assert!(tiles[0].activated);
        assert_eq!(
            tiles[0].focus,
            Some(aegis_core::window::WindowId(7)),
            "clicking focuses the running window"
        );
        assert!(tiles[0].spawn.is_none());
    }

    #[test]
    fn multiple_running_windows_fold_into_one_tile_with_multiple_instances() {
        let dock = dock_with(vec![app("firefox.desktop")]);
        let tiles = dock.tiles(&[window(7, "firefox", true), window(8, "firefox", false)]);
        assert_eq!(tiles.len(), 1);
        assert_eq!(tiles[0].windows.len(), 2);
    }

    #[test]
    fn unpinned_running_window_is_appended() {
        let dock = dock_with(vec![app("firefox.desktop")]);
        let tiles = dock.tiles(&[window(3, "gimp", false)]);
        assert_eq!(
            tiles.len(),
            2,
            "pinned firefox plus the unpinned gimp window"
        );
        let gimp = tiles.iter().find(|t| t.key == "win:3").expect("gimp tile");
        assert!(gimp.running);
        assert!(!gimp.pinned, "the window tile is transient, not kept");
        assert_eq!(gimp.focus, Some(aegis_core::window::WindowId(3)));
    }

    #[test]
    fn read_only_mirror_has_no_physical_focus_action() {
        let dock = Dock::new();
        let mut mirror = window(7, "org.example.App", false);
        mirror.read_only = true;
        let tiles = dock.tiles(&[mirror]);
        assert_eq!(tiles.len(), 1);
        assert_eq!(tiles[0].focus, None);
        assert_eq!(tiles[0].windows, vec![aegis_core::window::WindowId(7)]);
    }

    #[test]
    fn maximized_window_collapses_to_a_local_reveal_handle() {
        let mut dock = Dock::new();
        let mut maximized = window(7, "org.example.Game", true);
        maximized.state.maximized = true;
        dock.update_windows(&[maximized]);

        assert_eq!(dock.space_use, SpaceUse::Maximized);
        assert_eq!(dock.autohide_reveal, 0.0);
        assert_eq!(dock.reserved(), Reserved::default());
        assert_eq!(dock.backdrop_blur_sigma(), 0.0);
        assert!(
            dock.backdrop_regions((1920.0, 1080.0), &[], &workspace_snapshot())
                .is_empty()
        );
        assert!(
            dock.captures_pointer(960.0, 1079.0, (1920.0, 1080.0), &[], &workspace_snapshot(),)
        );
        assert!(
            !dock.captures_pointer(20.0, 1079.0, (1920.0, 1080.0), &[], &workspace_snapshot(),)
        );
        assert!(!dock.anim_pending());
    }

    #[test]
    fn fullscreen_window_locks_dock_hidden_without_hot_edge() {
        let mut dock = Dock::new();
        let mut fullscreen = window(7, "org.example.Game", true);
        fullscreen.state.fullscreen = true;
        dock.update_windows(&[fullscreen]);

        assert_eq!(dock.space_use, SpaceUse::Fullscreen);
        assert_eq!(dock.autohide_reveal, 0.0);
        assert_eq!(dock.reserved(), Reserved::default());
        assert_eq!(dock.backdrop_blur_sigma(), 0.0);
        assert!(
            dock.backdrop_regions((1920.0, 1080.0), &[], &workspace_snapshot())
                .is_empty()
        );
        assert!(!dock.captures_pointer(
            960.0,
            1079.0,
            (1920.0, 1080.0),
            &[],
            &workspace_snapshot(),
        ));
    }

    #[test]
    fn fullscreen_policy_wins_and_minimized_windows_do_not_hide_dock() {
        let mut dock = Dock::new();
        let mut maximized = window(7, "org.example.Editor", true);
        maximized.state.maximized = true;
        let mut fullscreen = window(8, "org.example.Game", false);
        fullscreen.state.fullscreen = true;
        dock.update_windows(&[maximized, fullscreen.clone()]);
        assert_eq!(dock.space_use, SpaceUse::Fullscreen);

        fullscreen.minimized = true;
        dock.update_windows(&[fullscreen]);
        assert_eq!(dock.space_use, SpaceUse::Available);
        assert_eq!(
            dock.reserved().bottom,
            (DOCK_PANEL_HEIGHT + DOCK_BOTTOM_MARGIN) as i32
        );
    }

    #[test]
    fn entry_matches_app_id_like_the_launcher_heuristic() {
        let mut e = Entry {
            id: "org.mozilla.firefox.desktop".to_string(),
            icon: Some("firefox-icon".to_string()),
            ..Default::default()
        };
        e.startup_wm_class = Some("Firefox".to_string());
        assert!(entry_matches_app_id(&e, "firefox")); // WM class, case-insensitive
        assert!(entry_matches_app_id(&e, "org.mozilla.firefox")); // desktop-id stem
        assert!(entry_matches_app_id(&e, "Firefox-Icon")); // icon name
        assert!(!entry_matches_app_id(&e, "chromium"));
        assert!(!entry_matches_app_id(&e, ""));
    }

    #[test]
    fn rest_centres_include_the_section_gap_for_transient_tiles() {
        // 2 pinned tiles (incl. launchpad) + 1 transient window tile.
        let pinned = Dock::rest_centre_estimate(1, 3, 2, 1920.0);
        let transient = Dock::rest_centre_estimate(2, 3, 2, 1920.0);
        let pitch = DOCK_TILE + DOCK_TILE_GAP;
        assert!(
            (transient - pinned - pitch - DOCK_SECTION_GAP).abs() < 1e-5,
            "the first transient tile sits one pitch plus the section gap right of the last pinned tile"
        );
        // No transient tiles → no extra gap.
        let a = Dock::rest_centre_estimate(1, 2, 2, 1920.0);
        let b = Dock::rest_centre_estimate(0, 2, 2, 1920.0);
        assert!((a - b - pitch).abs() < 1e-5);
    }
}
