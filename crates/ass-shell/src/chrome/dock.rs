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
//! the cursor and its neighbours along a cosine bell. Tiles are bottom-anchored
//! and scale upward from a fixed rest slot, so a magnified icon pops *above*
//! the bar — the classic macOS lift — while the bar itself stays put (its rest
//! width is fixed, so the cursor → tile mapping never slips). Each tile eases
//! its size toward the target per-frame so the wave stays silky as the cursor
//! moves and settles gracefully after it stops.

use std::collections::HashMap;
use std::ffi::c_void;

use lens::{Align, Color, Frame, Icon, Input, LayoutOpts, OverlayOpts, Rect};

use crate::{Chrome, ChromeEvents};
use ass_core::app::Entry;
use ass_core::window::Window;

/// Visual height of the dock bar. Tiles rest inside it; magnified tiles pop
/// above its top edge (they are drawn as their own layers, unclipped).
const DOCK_PANEL_HEIGHT: f32 = 64.0;
/// Gap between the dock bar and the bottom edge of the output.
const DOCK_BOTTOM_MARGIN: f32 = 12.0;
/// Height of the strip at the bar's bottom reserved for the running-indicator
/// dots, below the icon baseline.
const DOCK_DOT_AREA: f32 = 8.0;
/// Distance from the bar's bottom edge up to the icon baseline (the bottom of
/// every tile). Leaves room for [`DOCK_DOT_AREA`] plus a small gap.
const DOCK_BASELINE_INSET: f32 = 12.0;
/// Side length of a square dock tile at rest (the icon area).
const DOCK_TILE: f32 = 48.0;
/// Side length of a square dock tile at full magnification (1.5× rest).
const DOCK_TILE_MAX: f32 = 72.0;
/// How far (in rest-tile widths) the magnification reaches from the cursor.
const MAGNIFY_RADIUS_TILES: f32 = 2.0;
/// Exponential ease rate (1/seconds) for tile size → target. Higher is
/// snappier; ~14 tracks a moving cursor without lagging and settles in
/// roughly a twelfth of a second.
const MAGNIFY_EASE_RATE: f32 = 14.0;
/// Vertical band above the bar that still triggers magnification, in pixels.
/// Lets the wave start as the cursor approaches; outside it (pointer in a
/// window above) the row stays at rest.
const MAGNIFY_APPROACH_BAND: f32 = 48.0;
/// Gap between adjacent rest slots inside the bar.
const DOCK_TILE_GAP: f32 = 10.0;
/// Padding between the bar's edge and the first/last rest slot.
const DOCK_PAD: f32 = 10.0;
/// Diameter of a running-indicator dot.
const DOCK_DOT: f32 = 5.0;

/// One application pinned to the dock: the launchable entry plus the lowercased
/// `app_id`s a running toplevel might report, used to fold a running window
/// into its pinned tile.
pub struct DockApp {
    /// The entry spawned when the tile is clicked and no window matches.
    pub entry: Entry,
    /// Lowercased ids this app may run as (`StartupWMClass`, the desktop-id
    /// stem, the icon name). Matched against a window's `app_id`.
    pub keys: Vec<String>,
}

/// One resolved tile for the current frame: a pinned app, a running window, or
/// a pinned app that also has a running window folded into it.
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
    focus: Option<ass_core::window::WindowId>,
    /// Index into [`Dock::apps`] to spawn on click when nothing is running.
    spawn: Option<usize>,
    /// The Launchpad tile (always the first tile): clicking it toggles the
    /// launcher rather than focusing or spawning. Drawn as a 3×3 grid glyph.
    launchpad: bool,
}

impl Tile {
    /// The leading Launchpad tile — a macOS-style "show all apps" button that
    /// opens the launcher. Always present, never marked running.
    fn launchpad() -> Tile {
        Tile {
            key: "launchpad".to_string(),
            icon: None,
            running: false,
            activated: false,
            focus: None,
            spawn: None,
            launchpad: true,
        }
    }
}

/// The macOS-style dock.
pub struct Dock {
    /// Pinned launchable apps, in dock order. Built by the binary from the
    /// enumerated `.desktop` entries (and an optional config pin list).
    apps: Vec<DockApp>,
    /// `app_id` (lowercased) → borrowed icon texture pointer. Borrowed from
    /// the binary's `IconCache`, which owns the `flux::Image`s and outlives
    /// this component. Shared by pinned tiles and unpinned running windows.
    icons: HashMap<String, *mut c_void>,
    /// Per-tile eased size in logical px, keyed by [`Tile::key`]. Entries for
    /// tiles that disappear are dropped each frame.
    sizes: HashMap<String, f32>,
    /// Whether the left button was held last frame, so a click fires once on
    /// the press edge. The host's per-frame `mouse_pressed` flag is not cleared
    /// between frames, so we track the `mouse_down` level transition ourselves.
    prev_down: bool,
}

impl Dock {
    /// An empty dock (no pinned apps, no icons) — used by tests and as a base.
    pub fn new() -> Dock {
        Dock {
            apps: Vec::new(),
            icons: HashMap::new(),
            sizes: HashMap::new(),
            prev_down: false,
        }
    }

    /// Construct with the pinned apps and a pre-decoded icon map (`app_id` →
    /// `flux_image` pointer erased to `c_void`). The caller retains ownership
    /// of the textures.
    pub fn with_apps(apps: Vec<DockApp>, icons: HashMap<String, *mut c_void>) -> Dock {
        Dock {
            apps,
            icons,
            sizes: HashMap::new(),
            prev_down: false,
        }
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

    /// Exponential ease of `cur` toward `target` using `dt_seconds`.
    fn ease(cur: f32, target: f32, dt: f32) -> f32 {
        let k = (dt * MAGNIFY_EASE_RATE).min(1.0);
        cur + (target - cur) * k
    }

    /// Resolve the current frame's tiles: every pinned app (with any running
    /// window folded in), followed by running windows that match no pinned
    /// app. A window matches an app when its lowercased `app_id` is among the
    /// app's [`DockApp::keys`].
    fn tiles(&self, windows: &[Window]) -> Vec<Tile> {
        let win_appid: Vec<Option<String>> = windows
            .iter()
            .map(|w| w.app_id.as_ref().map(|a| a.to_ascii_lowercase()))
            .collect();
        let mut claimed = vec![false; windows.len()];
        let mut tiles = Vec::with_capacity(self.apps.len() + windows.len());

        for (i, app) in self.apps.iter().enumerate() {
            let mut running = false;
            let mut activated = false;
            let mut focus = None;
            for (wi, w) in windows.iter().enumerate() {
                let Some(a) = &win_appid[wi] else { continue };
                if app.keys.iter().any(|k| k == a) {
                    claimed[wi] = true;
                    running = true;
                    // Prefer the activated window as the focus target.
                    if w.state.activated {
                        activated = true;
                        focus = Some(w.id);
                    } else if focus.is_none() {
                        focus = Some(w.id);
                    }
                }
            }
            let icon = app.keys.iter().find_map(|k| self.icons.get(k).copied());
            tiles.push(Tile {
                key: format!("app:{}", app.entry.id),
                icon,
                running,
                activated,
                focus,
                spawn: if running { None } else { Some(i) },
                launchpad: false,
            });
        }

        for (wi, w) in windows.iter().enumerate() {
            if claimed[wi] {
                continue;
            }
            let icon = win_appid[wi]
                .as_ref()
                .and_then(|a| self.icons.get(a).copied());
            tiles.push(Tile {
                key: format!("win:{}", w.id.0),
                icon,
                running: true,
                activated: w.state.activated,
                focus: Some(w.id),
                spawn: None,
                launchpad: false,
            });
        }
        tiles
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
        _workspaces: &crate::WorkspaceSnapshot,
        out: &mut ChromeEvents,
    ) {
        let disp = input.as_raw().display_size;
        let dt = input.as_raw().dt_seconds.max(0.0);
        let cursor = input.as_raw().cursor;

        // The Launchpad tile always leads the strip (macOS-style), followed by
        // the pinned apps and any unpinned running windows.
        let mut tiles = Vec::with_capacity(self.apps.len() + windows.len() + 1);
        tiles.push(Tile::launchpad());
        tiles.extend(self.tiles(windows));
        let n = tiles.len();

        // Drop eased sizes for tiles no longer present so the map does not
        // grow unbounded across long sessions.
        self.sizes.retain(|k, _| tiles.iter().any(|t| &t.key == k));

        let panel_y = disp.y - DOCK_PANEL_HEIGHT - DOCK_BOTTOM_MARGIN;

        // The bar's width is fixed at the rest layout (icons scale in place,
        // they do not widen the bar), so it stays centred and the cursor →
        // tile mapping never slips while magnifying.
        let bar_w =
            n as f32 * DOCK_TILE + (n as f32 - 1.0) * DOCK_TILE_GAP + 2.0 * DOCK_PAD;
        let bar_x = (disp.x - bar_w) * 0.5;
        let rest_centre =
            |i: usize| bar_x + DOCK_PAD + i as f32 * (DOCK_TILE + DOCK_TILE_GAP) + DOCK_TILE * 0.5;
        // Bottom of every tile (icons are bottom-anchored and grow upward).
        let icon_bottom = panel_y + DOCK_PANEL_HEIGHT - DOCK_BASELINE_INSET;

        // Vertical activation band: from `MAGNIFY_APPROACH_BAND` above the bar
        // down to the bottom of the display.
        let in_band = cursor.y >= panel_y - MAGNIFY_APPROACH_BAND;

        // Ease each tile's size toward its magnification target.
        let mut eased: Vec<f32> = Vec::with_capacity(n);
        for (i, t) in tiles.iter().enumerate() {
            let factor = if in_band {
                Self::magnify_factor(cursor.x - rest_centre(i))
            } else {
                0.0
            };
            let target = DOCK_TILE + (DOCK_TILE_MAX - DOCK_TILE) * factor;
            let cur = *self.sizes.get(&t.key).unwrap_or(&DOCK_TILE);
            let next = Self::ease(cur, target, dt);
            self.sizes.insert(t.key.clone(), next);
            eased.push(next);
        }

        // The bar background, drawn first so icons stack above it.
        let panel_rect = Rect {
            x: bar_x,
            y: panel_y,
            w: bar_w,
            h: DOCK_PANEL_HEIGHT,
        };
        // A layer with an empty body collapses to ~0 (the rect is only an
        // anchor, not a size); a fixed-size child forces it to the bar size.
        f.layer("ass-dock", panel_rect, &panel_opts(), |f| {
            f.column_ex(&sized(bar_w, DOCK_PANEL_HEIGHT), |_| {});
        });

        // Hit-test the cursor against tile slots. A slot spans its rest cell
        // horizontally and the whole bar (plus the popped-icon band) vertically
        // so the entire tile is clickable; ties resolve to the nearest centre.
        let slot_top = icon_bottom - DOCK_TILE_MAX;
        let slot_bottom = panel_y + DOCK_PANEL_HEIGHT;
        let mut hit: Option<usize> = None;
        if cursor.y >= slot_top && cursor.y <= slot_bottom {
            let half = (DOCK_TILE + DOCK_TILE_GAP) * 0.5;
            let mut best = f32::MAX;
            for i in 0..n {
                let cx = rest_centre(i);
                let d = (cursor.x - cx).abs();
                if d <= half && d < best {
                    best = d;
                    hit = Some(i);
                }
            }
        }

        // Draw each tile's icon, then its running dot.
        for (i, t) in tiles.iter().enumerate() {
            let s = eased[i];
            let cx = rest_centre(i);
            let rect = Rect {
                x: cx - s * 0.5,
                y: icon_bottom - s,
                w: s,
                h: s,
            };
            let icon_id = format!("ass-dock-icon-{}", t.key);
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
                let dot_y = panel_y + DOCK_PANEL_HEIGHT - DOCK_DOT_AREA * 0.5;
                let dot_rect = Rect {
                    x: cx - DOCK_DOT * 0.5,
                    y: dot_y - DOCK_DOT * 0.5,
                    w: DOCK_DOT,
                    h: DOCK_DOT,
                };
                let color = if t.activated {
                    Color::rgba(236, 238, 245, 255)
                } else {
                    Color::rgba(200, 204, 220, 170)
                };
                let dot_id = format!("ass-dock-dot-{}", t.key);
                f.layer(&dot_id, dot_rect, &OverlayOpts::default(), |f| {
                    f.column_ex(&sized_fill(DOCK_DOT, DOCK_DOT, color, DOCK_DOT * 0.5), |_| {});
                });
            }
        }

        // Fire a click once on the press edge (the host does not clear the
        // per-frame pressed flag, so track the button-down level transition).
        let down = input.as_raw().mouse_down.first().copied().unwrap_or(false);
        if down && !self.prev_down {
            if let Some(i) = hit {
                let t = &tiles[i];
                if t.launchpad {
                    out.toggle_launcher = true;
                } else if let Some(id) = t.focus {
                    out.clicked = Some(id);
                } else if let Some(ai) = t.spawn {
                    out.spawn = Some(self.apps[ai].entry.clone());
                }
            }
        }
        self.prev_down = down;
    }

    /// The dock reserves the bottom edge so tiled windows do not render under
    /// the bar (ADR-0024 chrome-aware work-area). The magnified-icon overshoot
    /// above the bar is intentionally not reserved — chrome draws over windows.
    fn reserved(&self) -> crate::Reserved {
        crate::Reserved {
            top: 0,
            bottom: (DOCK_PANEL_HEIGHT + DOCK_BOTTOM_MARGIN) as i32,
            left: 0,
            right: 0,
        }
    }
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

/// The dock bar background: a rounded translucent panel.
fn panel_opts() -> OverlayOpts {
    OverlayOpts {
        bg: Color::rgba(28, 30, 44, 205),
        border: Color::rgba(70, 74, 96, 140),
        border_width: 1.0,
        radius: 18.0,
        pad: 0.0,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn app(id: &str, keys: &[&str]) -> DockApp {
        DockApp {
            entry: Entry {
                id: id.to_string(),
                ..Default::default()
            },
            keys: keys.iter().map(|k| k.to_string()).collect(),
        }
    }

    fn window(id: u64, app_id: &str, activated: bool) -> Window {
        let mut w = Window::default();
        w.id = ass_core::window::WindowId(id);
        w.app_id = Some(app_id.to_string());
        w.state.activated = activated;
        w
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
    fn ease_approaches_target_and_clamps() {
        assert_eq!(Dock::ease(10.0, 20.0, 0.0), 10.0);
        assert_eq!(Dock::ease(10.0, 20.0, 1.0), 20.0);
        let mid = Dock::ease(10.0, 20.0, 1.0 / MAGNIFY_EASE_RATE * 0.5);
        assert!(mid > 10.0 && mid < 20.0, "got {mid}");
    }

    #[test]
    fn pinned_apps_show_without_any_running_window() {
        let dock = Dock::with_apps(
            vec![app("firefox.desktop", &["firefox"]), app("term.desktop", &["term"])],
            HashMap::new(),
        );
        let tiles = dock.tiles(&[]);
        assert_eq!(tiles.len(), 2, "both pinned apps are tiles even with no windows");
        assert!(tiles.iter().all(|t| !t.running));
        // No running window → clicking launches (spawn), not focus.
        assert!(tiles.iter().all(|t| t.spawn.is_some() && t.focus.is_none()));
    }

    #[test]
    fn running_window_folds_into_its_pinned_tile() {
        let dock = Dock::with_apps(vec![app("firefox.desktop", &["firefox"])], HashMap::new());
        let tiles = dock.tiles(&[window(7, "firefox", true)]);
        assert_eq!(tiles.len(), 1, "the window folds into the pinned tile, not a new one");
        assert!(tiles[0].running);
        assert!(tiles[0].activated);
        assert_eq!(tiles[0].focus, Some(ass_core::window::WindowId(7)), "clicking focuses the running window");
        assert!(tiles[0].spawn.is_none());
    }

    #[test]
    fn unpinned_running_window_is_appended() {
        let dock = Dock::with_apps(vec![app("firefox.desktop", &["firefox"])], HashMap::new());
        let tiles = dock.tiles(&[window(3, "gimp", false)]);
        assert_eq!(tiles.len(), 2, "pinned firefox plus the unpinned gimp window");
        let gimp = tiles.iter().find(|t| t.key == "win:3").expect("gimp tile");
        assert!(gimp.running);
        assert_eq!(gimp.focus, Some(ass_core::window::WindowId(3)));
    }
}
