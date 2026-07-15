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

use std::collections::HashMap;
use std::ffi::c_void;

use lens::{Align, Color, Frame, Icon, Input, LayoutOpts, OverlayOpts, Rect};

use crate::{Chrome, ChromeEvents};
use ass_core::app::Entry;
use ass_core::window::Window;

/// Visual height of the dock bar. Tiles rest inside it; magnified tiles pop
/// above its top edge (they are drawn as their own layers, unclipped).
const DOCK_PANEL_HEIGHT: f32 = 74.0;
/// Gap between the dock bar and the bottom edge of the output.
const DOCK_BOTTOM_MARGIN: f32 = 12.0;
/// Height of the strip at the bar's bottom reserved for the running-indicator
/// dots, below the icon baseline.
const DOCK_DOT_AREA: f32 = 8.0;
/// Distance from the bar's bottom edge up to the icon baseline (the bottom of
/// every tile). Leaves room for [`DOCK_DOT_AREA`] plus a small gap.
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
/// under 1 give the slight macOS-style bounce-back. ~0.72 keeps one tiny
/// overshoot without ringing.
const SPRING_DAMPING: f32 = 0.72;
/// Side length a brand-new tile grows in from. Springs up over the first few
/// frames instead of popping in at full size.
const DOCK_TILE_BIRTH: f32 = 6.0;
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
    /// An empty dock (no pinned apps, no icons) — used by tests and as a base.
    pub fn new() -> Dock {
        Dock {
            apps: Vec::new(),
            icons: HashMap::new(),
            sizes: HashMap::new(),
            anim_active: false,
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
            anim_active: false,
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

    /// The rest (un-magnified) centre of tile `i` in a row of `n`, assuming the
    /// standard all-rest layout, used to measure cursor distance for the
    /// magnify factor. The live centre drifts as tiles widen, but macOS drives
    /// the *factor* from the fixed rest position so the wave does not chase its
    /// own tail (a wider tile pulling the cursor closer, magnifying more, …).
    fn rest_centre_estimate(i: usize, n: usize, disp_w: f32) -> f32 {
        let bar_w = n as f32 * DOCK_TILE + (n as f32 - 1.0) * DOCK_TILE_GAP + 2.0 * DOCK_PAD;
        let bar_x = (disp_w - bar_w) * 0.5;
        bar_x + DOCK_PAD + i as f32 * (DOCK_TILE + DOCK_TILE_GAP) + DOCK_TILE * 0.5
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

    /// Bounds of the live dock interaction surface. Uses the current spring
    /// widths (or rest width before a tile's first render) so pointer routing
    /// follows the bar as it expands without claiming the entire bottom edge.
    fn pointer_bounds(&self, windows: &[Window], display: (f32, f32)) -> Rect {
        let mut tiles = vec![Tile::launchpad()];
        tiles.extend(self.tiles(windows));
        let widths: Vec<f32> = tiles
            .iter()
            .map(|t| {
                self.sizes
                    .get(&t.key)
                    .map(|s| s.value.max(DOCK_TILE))
                    .unwrap_or(DOCK_TILE)
            })
            .collect();
        let gaps = tiles.len().saturating_sub(1) as f32 * DOCK_TILE_GAP;
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

        // Vertical activation band: from `MAGNIFY_APPROACH_BAND` above the bar
        // down to the bottom of the display.
        let in_band = cursor.y >= panel_y - MAGNIFY_APPROACH_BAND;

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
                Self::magnify_factor(cursor.x - Self::rest_centre_estimate(i, n, disp.x))
            } else {
                0.0
            };
            let target = DOCK_TILE + (DOCK_TILE_MAX - DOCK_TILE) * factor;
            let state = self.sizes.entry(t.key.clone()).or_insert(SpringState {
                value: DOCK_TILE_BIRTH,
                vel: 0.0,
            });
            eased.push(Self::spring(state, target, dt));
            // A spring is still animating while it is meaningfully off its
            // target or still moving. Sub-pixel drift is ignored so we don't
            // tick forever chasing float noise.
            let drifting = (state.value - target).abs() > 0.15 || state.vel.abs() > 0.5;
            unsettled |= drifting;
        }
        self.anim_active = unsettled;

        // Sum the eased widths (plus the inter-tile gap) to get the live bar
        // width. The gap is constant; only the tiles widen. Centred horizontally.
        let total_tiles: f32 = eased.iter().sum();
        let bar_w = total_tiles + (n as f32 - 1.0) * DOCK_TILE_GAP + 2.0 * DOCK_PAD;
        let bar_x = (disp.x - bar_w) * 0.5;

        // The running x-offset of each tile's centre, left to right.
        let mut centres = Vec::with_capacity(n);
        let mut x = bar_x + DOCK_PAD;
        for (i, s) in eased.iter().enumerate() {
            if i > 0 {
                x += DOCK_TILE_GAP;
            }
            centres.push(x + s * 0.5);
            x += *s;
        }
        let centre = |i: usize| centres[i];

        // Bottom of every tile (icons are bottom-anchored and grow upward).
        let icon_bottom = panel_y + DOCK_PANEL_HEIGHT - DOCK_BASELINE_INSET;

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
        f.layer("ass-dock", panel_rect, &panel_opts(), |f| {
            f.column_ex(&sized(bar_w, DOCK_PANEL_HEIGHT), |_| {});
        });

        // Hit-test the cursor against tile slots. A slot spans each tile's live
        // width (so magnified tiles are fully clickable) and the whole bar
        // height plus the popped-icon band; ties resolve to the nearest centre.
        let slot_top = icon_bottom - DOCK_TILE_MAX;
        let slot_bottom = panel_y + DOCK_PANEL_HEIGHT;
        let mut hit: Option<usize> = None;
        if cursor.y >= slot_top && cursor.y <= slot_bottom {
            let mut best = f32::MAX;
            for i in 0..n {
                let cx = centre(i);
                let half = eased[i] * 0.5 + DOCK_TILE_GAP * 0.5;
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

    /// The dock's magnify wave eases over many frames; report it as pending so
    /// the main loop keeps rendering (instead of blocking on the host queue)
    /// until every spring has rested.
    fn anim_pending(&self) -> bool {
        self.anim_active
    }

    fn captures_pointer(
        &self,
        x: f32,
        y: f32,
        display: (f32, f32),
        windows: &[Window],
        _workspaces: &crate::WorkspaceSnapshot,
    ) -> bool {
        let r = self.pointer_bounds(windows, display);
        x >= r.x && y >= r.y && x < r.x + r.w && y < r.y + r.h
    }

    fn visible_during_modal(&self) -> bool {
        true
    }

    fn update_app_catalog(
        &mut self,
        _apps: &[Entry],
        dock_apps: &[DockApp],
        icons: &HashMap<String, *mut c_void>,
    ) {
        self.apps = dock_apps.to_vec();
        self.icons.clone_from(icons);
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
    fn spring_approaches_target_at_rest() {
        // No time elapses → nothing moves.
        let mut s = SpringState { value: 10.0, vel: 0.0 };
        assert_eq!(Dock::spring(&mut s, 20.0, 0.0), 10.0);
    }

    #[test]
    fn spring_settles_on_target() {
        // Many small steps from rest must converge to the target.
        let mut s = SpringState { value: 10.0, vel: 0.0 };
        for _ in 0..2000 {
            Dock::spring(&mut s, 20.0, 1.0 / 120.0);
        }
        assert!((s.value - 20.0).abs() < 0.01, "settled at {}", s.value);
    }

    #[test]
    fn spring_overshoots_then_settles() {
        // Under-damped: from rest it should cross past the target at least once
        // before settling (the macOS lift-and-bounce).
        let mut s = SpringState { value: 0.0, vel: 0.0 };
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
        let mut s = SpringState { value: 0.0, vel: 0.0 };
        let v = Dock::spring(&mut s, 100.0, 1.0 / 5.0);
        assert!(v.is_finite(), "value diverged: {v}");
        assert!(s.vel.is_finite(), "velocity diverged: {}", s.vel);
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
