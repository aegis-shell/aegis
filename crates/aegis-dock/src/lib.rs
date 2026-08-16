//! macOS-style dock: a rounded translucent bar anchored to a screen edge of
//! the output (bottom by default; the left or right edge via
//! `[dock] position`) holding a persistent strip of pinned application
//! icons. Unlike a window list, the dock is populated from launchable
//! `.desktop` entries the binary pins (plus any running window that is not
//! already pinned), so it shows real XDG icons even when nothing is running.
//!
//! Each tile is one app — pinned or not, an application's running windows
//! fold into a single tile:
//!   - Clicking a tile whose app has a running window focuses that window;
//!     clicking a tile with no running window launches the app (`out.spawn`).
//!   - Dragging a pinned tile past a neighbour's midpoint reorders the pinned
//!     strip (`out.dock_reorder`); dragging it past the section divider
//!     unpins it, and dragging a transient tile back across the divider pins
//!     it at the hovered slot. Dragging empty panel space toward another
//!     screen edge moves the dock there (`out.dock_position`).
//!   - A small dot beside a tile (beneath it on a bottom dock) marks a
//!     running app; the dot brightens for the activated window.
//!
//! Visuals are deliberately icon-first, not button-first: tiles are drawn as
//! bare raster icons (no pill / border), and hovering magnifies the tile under
//! the cursor and its neighbours along a cosine bell. Unlike a fixed-slot
//! dock, the bar *reflows*: it widens to fit the magnified widths and the
//! neighbouring tiles spread apart around the cursor — the classic macOS
//! squeeze-and-lift. Tiles are anchored to the bar's inner baseline and scale
//! toward the screen centre, so a magnified icon pops *out* of the bar. Each
//! tile's size is driven by a damped spring with a slight under-damped
//! overshoot, so the wave tracks a moving cursor and settles with a gentle
//! bounce. Brand-new tiles (a window just mapped) spring up from a seed size
//! instead of popping in.

use std::cell::{Ref, RefCell};
use std::collections::HashMap;
use std::ffi::c_void;
use std::hash::Hasher;

use aegis_design::materials::{chrome_place, surface_layout};
use aegis_design::{Design, GlassRole, materials};
use lens::{Align, Color, Frame, Icon, Input, LayoutOpts, Rect};

use aegis_model::app::Entry;
use aegis_model::dock::DockPosition;
use aegis_model::input::{KeyAction, KeyChar, key_action};
use aegis_model::window::{SpaceUse, Window};
use aegis_model::workspace::WorkspaceSnapshot;
use aegis_shell::{
    AppCatalog, AppMenu, BackdropRegion, Chrome, ChromeEvents, ChromeUpdate, CursorShape, IconSet,
    LiquidGlassRegion, LivePreviewPresentation, Localizer, Message, PinAction, PopupSide,
    PreviewCard, Reserved, ellipsize, liquid_glass_region_id, preview,
};

/// Visual height of the dock bar. Tiles rest inside it; magnified tiles pop
/// above its top edge (they are drawn as their own placed subtrees, unclipped). On a
/// side edge this is the panel's thickness (width).
const DOCK_PANEL_HEIGHT: f32 = 74.0;
/// Gap between the dock bar and the screen edge it is anchored to.
const DOCK_EDGE_MARGIN: f32 = 12.0;
/// Distance from the bar's bottom edge up to the icon baseline (the bottom of
/// every tile). Leaves room for the running-indicator dot strip plus a small
/// gap, while keeping the dot clear of the panel's rounded bottom corners.
const DOCK_BASELINE_INSET: f32 = 13.0;
/// Side length of a square dock tile at rest (the icon area).
const DOCK_TILE: f32 = 56.0;
/// Side length of a square dock tile at full magnification (1.5× rest).
const DOCK_TILE_MAX: f32 = 84.0;
/// Envelope headroom for the underdamped magnification spring: at ζ≈0.85 a
/// tile can overshoot its target by a few percent, and the capture footprint
/// must contain even that peak.
const SPRING_OVERSHOOT_MARGIN: f32 = 1.05;
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
/// Gap between adjacent rest slots inside the bar.
const DOCK_TILE_GAP: f32 = 10.0;
/// Edge-to-edge gap between the pinned strip and the transient (running,
/// unpinned) section — the macOS-style separator between kept apps and apps
/// that only live while they run. The boundary *replaces* the ordinary tile
/// gap and the divider hairline sits at its midpoint, so each section keeps
/// exactly one ordinary tile gap of clearance beside the divider.
const DOCK_SECTION_GAP: f32 = 2.0 * DOCK_TILE_GAP;
/// Padding between the bar's edge and the first/last rest slot.
const DOCK_PAD: f32 = 10.0;
/// Diameter of a single running-indicator dot.
const DOCK_DOT: f32 = 5.0;
/// Width of a running-indicator stadium (pill) for multiple instances.
const DOCK_DOT_STADIUM: f32 = 12.0;
/// Inactivity timeout in seconds before an autohiding dock collapses.
const AUTOHIDE_IDLE_TIMEOUT: f32 = 2.5;
/// Width of the thin stadium handle shown when the Dock is autohidden.
const AUTOHIDE_HANDLE_WIDTH: f32 = 140.0;
/// Height of the thin stadium handle shown when the Dock is autohidden.
const AUTOHIDE_HANDLE_HEIGHT: f32 = 6.0;
/// Reveal progress below which iconography has completely drained into the
/// collapsing surface. The panel continues morphing into the stadium handle
/// after its content is gone, so neither icons nor running dots linger beside
/// the final indicator.
const AUTOHIDE_CONTENT_DRAIN_END: f32 = 0.28;
/// Content at or below this scale is visually drained into the collapsed
/// handle and must no longer expose per-tile hover or click targets.
const AUTOHIDE_CONTENT_INTERACTION_MIN: f32 = 0.01;
/// Pointer dwell before an application name appears. This keeps labels from
/// flashing while the pointer merely crosses the dock.
const TOOLTIP_DWELL: f32 = 0.30;
/// Exponential fade speed for the dock application-name tooltip.
const TOOLTIP_FADE_SPEED: f32 = 18.0;
const TOOLTIP_HEIGHT: f32 = 28.0;
const TOOLTIP_GAP: f32 = 9.0;
/// Pointer travel (logical px) from the press point that promotes a held
/// left-button press into a drag — a tile reorder or a panel edge drag.
const DRAG_THRESHOLD: f32 = 6.0;
/// Distance from a screen edge (logical px) within which a panel edge drag
/// snaps the dock to that edge.
const EDGE_DRAG_PROXIMITY: f32 = 96.0;
/// Scale bump applied to a reordered tile while it floats at the cursor —
/// the "lifted" read without touching the raster icon's alpha.
const DRAG_LIFT_SCALE: f32 = 1.12;
/// Geometry for the live window cards shown above a running Dock tile.
const PREVIEW_CARD_MAX_WIDTH: f32 = 224.0;
const PREVIEW_CARD_MIN_WIDTH: f32 = 112.0;
const PREVIEW_ASPECT: f32 = 0.62;
const PREVIEW_LABEL_HEIGHT: f32 = 34.0;
const PREVIEW_PANEL_PAD: f32 = 12.0;
const PREVIEW_CARD_GAP: f32 = 10.0;
const PREVIEW_PANEL_GAP: f32 = 12.0;
const PREVIEW_SCREEN_MARGIN: f32 = 8.0;

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

/// One resolved tile for the current frame: a pinned app or a transient
/// running application (one tile per app in both cases, its windows folded
/// in — a pinned app and an unpinned one behave identically).
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
    focus: Option<aegis_model::window::WindowId>,
    /// Every running window folded into this application tile. Right-click
    /// actions operate on this complete set rather than an arbitrary match.
    windows: Vec<aegis_model::window::WindowId>,
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
    /// pinned app). Transient tiles — unpinned running applications — sit on
    /// the right of the section separator and disappear when they close.
    pinned: bool,
    /// The desktop-entry id a transient tile pins as ("Keep in Dock", or a
    /// drag across the section divider). `None` for pinned tiles, the
    /// Launchpad, and windows that match no enumerated entry.
    pin_entry: Option<String>,
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
            pin_entry: None,
        }
    }
}

/// One transient-strip application grouping: every running window that
/// shares a resolved desktop entry (or, unresolved, a lowercased `app_id`).
/// Mirroring the pinned strip, one group renders as exactly one tile.
struct TransientGroup {
    /// The tile key: `transient:<entry id>` or `transient:<app_id>` for an
    /// application group, `win:<id>` for a window that reported no app_id.
    key: String,
    /// The first member's lowercased `app_id` (icon lookup), if any.
    app_id: Option<String>,
    /// Index into `all_apps` of the resolved desktop entry, if any.
    entry: Option<usize>,
    /// Member window indices into `windows`, in first-observed order.
    windows: Vec<usize>,
}

/// What a left-button press on the dock panel landed on.
#[derive(Clone)]
enum PressTarget {
    /// A pinned application tile (identified by its stable tile key): the
    /// reorderable class. The strip index is re-resolved from the key every
    /// frame, so windows opening or closing mid-gesture cannot shift the
    /// drag anchor.
    PinnedTile(String),
    /// A tile that does not reorder inside the pinned strip — the Launchpad
    /// or a transient running application. The press stays a pending click
    /// until release, except that a transient tile that resolves to a
    /// desktop entry promotes to a drag so it can be pinned by crossing the
    /// section divider.
    OtherTile(String),
    /// Empty panel space (padding, gaps, or the collapsed autohide handle).
    /// A press-and-drag here moves the dock between screen edges.
    Panel,
}

/// The strip section under a dragged tile: the kept (pinned) strip, or the
/// transient running section past the section divider. A tile dropped in
/// the foreign section pins or unpins it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum DropSection {
    Pinned,
    Transient,
}

/// A held left-button press on the dock. Movement past [`DRAG_THRESHOLD`]
/// promotes it: a pinned-app tile press becomes a reorder drag and an
/// empty-panel press becomes an edge drag; anything else remains a pending
/// click fired on release. Click activation deliberately happens on the
/// release edge, never the press edge, so the threshold gets first say.
#[derive(Clone)]
struct PressState {
    /// Cursor position at the press; the drag-threshold reference.
    origin: (f32, f32),
    /// What the press landed on.
    target: PressTarget,
    /// Whether the press has been promoted to a drag.
    dragging: bool,
    /// The strip section the drag currently hovers. Starts as the tile's own
    /// section; crossing the section divider flips it, and the release then
    /// pins or unpins the dragged tile instead of reordering it.
    section: DropSection,
    /// The latest reorder insertion slot previewed during a tile drag (an
    /// index into the pinned-app sequence with the dragged tile removed).
    /// The release commits it.
    insert: Option<usize>,
    /// The dock position when the press started. An edge drag that ends
    /// back on the original edge persists nothing.
    start_position: DockPosition,
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
    /// Geometry shared with the compositor for a running app's live previews.
    /// It is prepared from the exact icon owner and retained while the pointer
    /// crosses the gap between the Dock and the popover.
    live_preview: Option<LivePreviewPresentation>,
    hover_surface_bounds: Option<Rect>,
    hover_owner_bounds: Option<Rect>,
    hovered_preview: Option<aegis_model::window::WindowId>,
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
    /// Whether a pointer entry may reveal the collapsed Dock. Entering
    /// maximized mode disarms it until the pointer leaves the capsule,
    /// preventing a Dock under a stationary pointer from reopening on the
    /// very frame it is collapsed.
    hidden_trigger_armed: bool,
    /// Compositor-derived cover state for the current visible windows. Kept
    /// outside render state because reserved edges, pointer capture, and
    /// backdrop capture are queried before the dock renders.
    space_use: SpaceUse,
    /// Whether a visible window intersects the Dock's stable, un-magnified
    /// occupied rectangle. This geometry, rather than a maximized state bit,
    /// decides whether the Dock becomes an overlay and hides.
    dock_obscured: bool,
    /// Entering an obscured state must complete one uninterrupted trip below
    /// the output before the reveal capsule becomes active. Otherwise a
    /// stationary pointer inside the old Dock rect cancels the hide animation.
    collapse_pending: bool,
    /// Most recently rendered logical output size. `update_windows` has no
    /// display argument, so it uses this cached size to update collision state
    /// before the next render and the render path verifies it again.
    last_display: Option<(f32, f32)>,
    /// Resolved tile strip cache (Launchpad tile first), shared by rendering,
    /// visual backdrop geometry, and stable pointer geometry so the strip is
    /// built once per change instead of up to three times per frame.
    /// Interior mutability: the pointer-side trait methods take `&self`.
    tile_cache: RefCell<TileCache>,
    /// Bumped on every catalog push so the tile cache notices pinned-app and
    /// icon changes without diffing the entries.
    catalog_revision: u64,
    /// Every mapped toplevel across all workspaces, retained from
    /// [`ChromeUpdate::AllWindows`]. Tile building, live previews, and the
    /// context menu read this global list; window-geometry policies (autohide
    /// collision, fullscreen lock) keep the visible-set snapshot pushed via
    /// [`ChromeUpdate::Windows`].
    all_windows: Vec<Window>,
    /// Device-pixel scale of the output, from [`ChromeUpdate::Scale`]. Used to
    /// snap hairline geometry (the section divider) to the device pixel grid.
    scale: f32,
    /// The design snapshot the dock paints from, from
    /// [`ChromeUpdate::Appearance`]. Seeded on registration by
    /// [`aegis_shell::Shell::add`] and refreshed when the desktop color scheme
    /// changes; defaults to the dark appearance until the first update arrives.
    design: Design,
    /// The screen edge the panel anchors to. Carried on the pushed
    /// [`AppCatalog`] from the `[dock] position` configuration, switched
    /// optimistically by an edge drag, and persisted by the runtime through
    /// `ConfigEdit::SetDockPosition`. Drives panel geometry, strip
    /// orientation, the magnification axis, reserved space, popup placement,
    /// and the autohide trigger shape.
    position: DockPosition,
    /// The live left-button press lifecycle (pending click, reorder drag, or
    /// edge drag), `None` while no gesture is held.
    press: Option<PressState>,
    /// The committed order of a just-finished reorder drag (entry ids),
    /// applied to [`Dock::apps`] at the top of the next render so the strip
    /// does not wait on the catalog round-trip. Cleared by
    /// [`Dock::update_app_catalog`], whose push reconciles the same order.
    pending_order: Option<Vec<String>>,
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
    /// Window ids in first-observed (mapping) order. The compositor's window
    /// slice follows stacking order and therefore changes when focus changes;
    /// Dock placement must not.
    window_order: Vec<aegis_model::window::WindowId>,
    tiles: Vec<Tile>,
}

/// A damped-spring state for one animated scalar (a tile's edge length).
/// Integrated from the analytic solution each frame so it stays stable across
/// a wide range of `dt` and produces the macOS-style slight overshoot.
#[derive(Clone, Copy, Default)]
struct SpringState {
    /// Current eased value (logical px).
    value: f32,
    /// Current velocity (px/s).
    vel: f32,
}

impl Dock {
    /// An empty dock. The pinned apps and decoded icons arrive through
    /// [`ChromeUpdate::AppCatalog`], seeded on registration by
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
            live_preview: None,
            hover_surface_bounds: None,
            hover_owner_bounds: None,
            hovered_preview: None,
            reduced_motion: false,
            autohide: false,
            autohide_reveal: 1.0,
            autohide_idle: 0.0,
            autohide_timeout: AUTOHIDE_IDLE_TIMEOUT,
            hidden_trigger_armed: true,
            space_use: SpaceUse::Available,
            dock_obscured: false,
            collapse_pending: false,
            last_display: None,
            tile_cache: RefCell::new(TileCache {
                signature: None,
                label: String::new(),
                window_order: Vec::new(),
                tiles: Vec::new(),
            }),
            catalog_revision: 0,
            all_windows: Vec::new(),
            scale: 1.0,
            design: Design::dark(),
            position: DockPosition::default(),
            press: None,
            pending_order: None,
        }
    }

    /// Set whether the dock automatically hides after an inactivity period.
    pub fn set_autohide(&mut self, autohide: bool) {
        self.autohide = autohide;
        if !autohide
            && self.space_use == SpaceUse::Available
            && !self.dock_obscured
            && !self.fullscreen_locked()
        {
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

    /// Move the panel to a different screen edge. Panel geometry, strip
    /// orientation, and popup placement all derive from this value on the
    /// next frame; the autohide reveal state carries over unchanged.
    pub fn set_position(&mut self, position: DockPosition) {
        if position == self.position {
            return;
        }
        self.position = position;
        self.app_menu.set_side(Self::popup_side_for(position));
        self.anim_active = true;
    }

    /// The side of a tile its context menu, tooltip, and previews open
    /// toward: into the output, away from the dock's screen edge.
    fn popup_side_for(position: DockPosition) -> PopupSide {
        match position {
            DockPosition::Bottom => PopupSide::Above,
            DockPosition::Left => PopupSide::Right,
            DockPosition::Right => PopupSide::Left,
        }
    }

    /// Fullscreen owns the complete output: unlike maximized mode, it exposes
    /// neither Dock chrome nor a reveal target.
    fn fullscreen_locked(&self) -> bool {
        self.space_use == SpaceUse::Fullscreen
    }

    /// Maximized windows, an explicit user preference, and actual window/Dock
    /// intersections all enable the same overlay mechanics. Maximized mode is
    /// forced independently of the user preference so it gains the complete
    /// work area by default.
    fn effective_autohide(&self) -> bool {
        self.autohide || self.space_use == SpaceUse::Maximized || self.dock_obscured
    }

    /// Cubic easing with zero velocity at both ends. The reveal state remains
    /// the animation clock; this shapes the visible geometry so the Dock does
    /// not arrive at either the panel or handle with a hard stop.
    fn smoothstep(progress: f32) -> f32 {
        let progress = progress.clamp(0.0, 1.0);
        progress * progress * (3.0 - 2.0 * progress)
    }

    /// Geometric expansion of the single Dock surface: zero is the collapsed
    /// stadium handle and one is the full glass panel.
    fn collapse_surface_progress(reveal: f32) -> f32 {
        Self::smoothstep(reveal)
    }

    /// Content drains earlier than the containing surface. Icons, dots, and
    /// the section divider converge and shrink into the bottom-centre sink,
    /// then stay absent while the remaining glass finishes becoming a handle.
    fn collapse_content_progress(reveal: f32) -> f32 {
        let normalized = (reveal - AUTOHIDE_CONTENT_DRAIN_END) / (1.0 - AUTOHIDE_CONTENT_DRAIN_END);
        Self::smoothstep(normalized)
    }

    /// The Dock and its autohide indicator are one edge-anchored surface.
    /// Length (along the strip axis), thickness, and corner radius morph
    /// continuously instead of moving a fully rendered panel offscreen and
    /// fading in a second object.
    fn collapsed_panel_rect(
        position: DockPosition,
        display: (f32, f32),
        expanded_len: f32,
        reveal: f32,
    ) -> Rect {
        let progress = Self::collapse_surface_progress(reveal);
        let len = AUTOHIDE_HANDLE_WIDTH + (expanded_len - AUTOHIDE_HANDLE_WIDTH) * progress;
        let thick =
            AUTOHIDE_HANDLE_HEIGHT + (DOCK_PANEL_HEIGHT - AUTOHIDE_HANDLE_HEIGHT) * progress;
        match position {
            DockPosition::Bottom => Rect {
                x: (display.0 - len) * 0.5,
                y: display.1 - DOCK_EDGE_MARGIN - thick,
                w: len,
                h: thick,
            },
            DockPosition::Left => Rect {
                x: DOCK_EDGE_MARGIN,
                y: (display.1 - len) * 0.5,
                w: thick,
                h: len,
            },
            DockPosition::Right => Rect {
                x: display.0 - DOCK_EDGE_MARGIN - thick,
                y: (display.1 - len) * 0.5,
                w: thick,
                h: len,
            },
        }
    }

    /// The collapsed capsule is both the visible affordance and the complete
    /// reveal target. Pixels around it remain owned by the client below.
    fn collapsed_indicator_bounds(position: DockPosition, display: (f32, f32)) -> Rect {
        Self::collapsed_panel_rect(position, display, AUTOHIDE_HANDLE_WIDTH, 0.0)
    }

    fn collapsed_indicator_contains(
        position: DockPosition,
        cursor: (f32, f32),
        display: (f32, f32),
    ) -> bool {
        let indicator = Self::collapsed_indicator_bounds(position, display);
        cursor.0 >= indicator.x
            && cursor.1 >= indicator.y
            && cursor.0 < indicator.x + indicator.w
            && cursor.1 < indicator.y + indicator.h
    }

    /// While an autohiding Dock is expanded, keep the stable resting strip
    /// and the gap toward its screen edge as one continuous approach
    /// corridor. Without the gap, the pointer that revealed the Dock can
    /// fall out of its ownership as soon as the panel expands, starting an
    /// expand/collapse loop.
    fn expanded_trigger_contains(
        position: DockPosition,
        cursor: (f32, f32),
        rest_bounds: Rect,
        display: (f32, f32),
    ) -> bool {
        match position {
            DockPosition::Bottom => {
                cursor.0 >= rest_bounds.x
                    && cursor.1 >= rest_bounds.y
                    && cursor.0 < rest_bounds.x + rest_bounds.w
                    && cursor.1 < display.1
            }
            DockPosition::Left => {
                cursor.0 < rest_bounds.x + rest_bounds.w
                    && cursor.1 >= rest_bounds.y
                    && cursor.1 < rest_bounds.y + rest_bounds.h
            }
            DockPosition::Right => {
                cursor.0 >= rest_bounds.x
                    && cursor.0 < display.0
                    && cursor.1 >= rest_bounds.y
                    && cursor.1 < rest_bounds.y + rest_bounds.h
            }
        }
    }

    /// Resolve the single pointer region that may keep the Dock revealed.
    /// While collapsed, the caller-provided capsule entry is the only trigger;
    /// the resting Dock rectangle becomes active only after reveal has begun.
    #[allow(clippy::too_many_arguments)]
    fn pointer_keeps_revealed(
        effective_autohide: bool,
        reveal: f32,
        capsule_entry: bool,
        cursor: (f32, f32),
        rest_bounds: Rect,
        position: DockPosition,
        display: (f32, f32),
    ) -> bool {
        if !effective_autohide {
            return cursor.0 >= rest_bounds.x
                && cursor.1 >= rest_bounds.y
                && cursor.0 < rest_bounds.x + rest_bounds.w
                && cursor.1 < rest_bounds.y + rest_bounds.h;
        }
        if reveal < 0.2 {
            return capsule_entry;
        }
        Self::expanded_trigger_contains(position, cursor, rest_bounds, display)
    }

    /// A forced collapse must observe a pointer exit before the same pointer
    /// can reveal the Dock again. This turns reveal into an entry gesture
    /// instead of a level-triggered condition under a stationary cursor.
    fn hidden_reveal_requested(
        position: DockPosition,
        armed: &mut bool,
        cursor: (f32, f32),
        display: (f32, f32),
    ) -> bool {
        if !Self::collapsed_indicator_contains(position, cursor, display) {
            *armed = true;
            return false;
        }
        *armed
    }

    /// Close UI that must not survive an automatic dock hide.
    fn dismiss_transient_ui(&mut self) {
        self.app_menu.dismiss();
        self.menu_tile = None;
        self.press = None;
        self.hovered_tile = None;
        self.hover_elapsed = 0.0;
        self.tooltip_tile = None;
        self.tooltip_alpha = 0.0;
        self.live_preview = None;
        self.hover_surface_bounds = None;
        self.hover_owner_bounds = None;
        self.hovered_preview = None;
    }

    fn dismiss_hover_surface(&mut self) {
        self.hovered_tile = None;
        self.hover_elapsed = 0.0;
        self.tooltip_tile = None;
        self.tooltip_alpha = 0.0;
        self.live_preview = None;
        self.hover_surface_bounds = None;
        self.hover_owner_bounds = None;
        self.hovered_preview = None;
    }

    /// Keep a preview open while the pointer crosses the small air gap
    /// between the popover and its owner icon. The bridge spans the gap on
    /// whichever axis separates them — above the tile for a bottom dock,
    /// beside it for a side dock — so users can move diagonally toward any
    /// card in a multi-window group.
    fn hover_surface_contains(&self, x: f32, y: f32) -> bool {
        let contains =
            |rect: Rect| x >= rect.x && y >= rect.y && x < rect.x + rect.w && y < rect.y + rect.h;
        let Some(surface) = self.hover_surface_bounds else {
            return false;
        };
        if contains(surface) {
            return true;
        }
        let Some(owner) = self.hover_owner_bounds else {
            return false;
        };
        if contains(owner) {
            return true;
        }
        gap_bridge(surface, owner).is_some_and(contains)
    }

    fn set_dock_obscured(&mut self, obscured: bool) {
        if obscured == self.dock_obscured {
            return;
        }
        self.dock_obscured = obscured;
        self.anim_active = true;
        if obscured {
            self.dismiss_transient_ui();
            self.autohide_idle = self.autohide_timeout;
            self.hidden_trigger_armed = false;
            self.collapse_pending = true;
        } else {
            self.collapse_pending = false;
            if !self.effective_autohide() && !self.fullscreen_locked() {
                self.autohide_idle = 0.0;
                self.hidden_trigger_armed = true;
            }
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
    /// `pinned_count` accounts for the extra section gap between the kept strip
    /// and the transient running section. `axis_len` is the display extent
    /// along the strip axis (width for a bottom dock, height for a side dock);
    /// the bar is centred on its edge, so the math is orientation-independent.
    fn rest_centre_estimate(i: usize, n: usize, pinned_count: usize, axis_len: f32) -> f32 {
        let section_extra = Self::section_extra(pinned_count, n);
        let bar_w = n as f32 * DOCK_TILE
            + (n as f32 - 1.0) * DOCK_TILE_GAP
            + section_extra
            + 2.0 * DOCK_PAD;
        let bar_x = (axis_len - bar_w) * 0.5;
        let extra = if i >= pinned_count {
            section_extra
        } else {
            0.0
        };
        bar_x + DOCK_PAD + i as f32 * (DOCK_TILE + DOCK_TILE_GAP) + extra + DOCK_TILE * 0.5
    }

    /// The section boundary's extra width beyond the ordinary tile gap it
    /// replaces, or zero when either section is empty. Boundary tiles sit
    /// exactly [`DOCK_SECTION_GAP`] apart edge-to-edge, so the divider at
    /// its midpoint keeps one ordinary gap of clearance on each side.
    fn section_extra(pinned_count: usize, tile_count: usize) -> f32 {
        if pinned_count > 0 && tile_count > pinned_count {
            DOCK_SECTION_GAP - DOCK_TILE_GAP
        } else {
            0.0
        }
    }

    /// Panel rectangle for a strip of long-axis length `bar_len` anchored to
    /// `position`, centred on that edge. The panel is one tile thick on every
    /// edge; `bar_len` is the width of a bottom dock and the height of a side
    /// dock.
    fn panel_rect_for(position: DockPosition, bar_len: f32, display: (f32, f32)) -> Rect {
        match position {
            DockPosition::Bottom => Rect {
                x: (display.0 - bar_len) * 0.5,
                y: display.1 - DOCK_PANEL_HEIGHT - DOCK_EDGE_MARGIN,
                w: bar_len,
                h: DOCK_PANEL_HEIGHT,
            },
            DockPosition::Left => Rect {
                x: DOCK_EDGE_MARGIN,
                y: (display.1 - bar_len) * 0.5,
                w: DOCK_PANEL_HEIGHT,
                h: bar_len,
            },
            DockPosition::Right => Rect {
                x: display.0 - DOCK_PANEL_HEIGHT - DOCK_EDGE_MARGIN,
                y: (display.1 - bar_len) * 0.5,
                w: DOCK_PANEL_HEIGHT,
                h: bar_len,
            },
        }
    }

    /// Stable, unanimated panel rectangle used by hover activation, pointer
    /// capture, and click gating. Keeping this geometry in one place prevents
    /// the magnification spring from expanding chrome's input ownership.
    fn rest_bounds(
        tile_count: usize,
        pinned_count: usize,
        position: DockPosition,
        display: (f32, f32),
    ) -> Rect {
        let gaps = tile_count.saturating_sub(1) as f32 * DOCK_TILE_GAP
            + Self::section_extra(pinned_count, tile_count);
        let bar_len = tile_count as f32 * DOCK_TILE + gaps + 2.0 * DOCK_PAD;
        Self::panel_rect_for(position, bar_len, display)
    }

    /// Whether the cursor has travelled far enough from the press point to
    /// promote a held press into a drag.
    fn drag_threshold_exceeded(origin: (f32, f32), cursor: (f32, f32)) -> bool {
        let dx = cursor.0 - origin.0;
        let dy = cursor.1 - origin.1;
        dx * dx + dy * dy > DRAG_THRESHOLD * DRAG_THRESHOLD
    }

    /// The insertion slot for a dragged tile: the number of pinned-app rest
    /// centres — the dragged tile excluded for a reorder — the cursor has
    /// passed along the strip axis. The result ranges over the pinned strip
    /// only, so a drop can never displace the leading Launchpad tile.
    fn drop_insert_index(pinned_centres: &[f32], cursor_axis: f32) -> usize {
        pinned_centres
            .iter()
            .filter(|centre| cursor_axis > **centre)
            .count()
    }

    /// The strip-axis position of the section divider under resting
    /// geometry: half a section gap past the last pinned tile's edge.
    fn section_boundary_axis(n: usize, pinned_count: usize, axis_len: f32) -> f32 {
        Self::rest_centre_estimate(pinned_count - 1, n, pinned_count, axis_len)
            + DOCK_TILE * 0.5
            + DOCK_SECTION_GAP * 0.5
    }

    /// The strip section under a dragged tile's cursor. A pinned tile dragged
    /// past the divider enters (and commits to) the transient section; a
    /// transient tile dragged back across it enters the pinned strip.
    fn drop_section_at(
        cursor_axis: f32,
        n: usize,
        pinned_count: usize,
        axis_len: f32,
    ) -> DropSection {
        if cursor_axis > Self::section_boundary_axis(n, pinned_count, axis_len) {
            DropSection::Transient
        } else {
            DropSection::Pinned
        }
    }

    /// Move the element at `from` to insertion slot `insert`, an index into
    /// the sequence with the element already removed. Returns whether the
    /// order changed.
    fn move_element<T>(items: &mut Vec<T>, from: usize, insert: usize) -> bool {
        if from == insert || from >= items.len() || insert > items.len() {
            return false;
        }
        let item = items.remove(from);
        items.insert(insert.min(items.len()), item);
        true
    }

    /// The screen edge an in-progress panel drag snaps to, if the cursor has
    /// entered an edge's proximity zone. The nearest in-zone edge wins in a
    /// corner; outside every zone the drag keeps the dock's current edge.
    fn edge_drag_target(cursor: (f32, f32), display: (f32, f32)) -> Option<DockPosition> {
        let mut best: Option<(f32, DockPosition)> = None;
        for (distance, position) in [
            (cursor.0, DockPosition::Left),
            (display.1 - cursor.1, DockPosition::Bottom),
            (display.0 - cursor.0, DockPosition::Right),
        ] {
            if distance <= EDGE_DRAG_PROXIMITY
                && best.is_none_or(|(best_distance, _)| distance < best_distance)
            {
                best = Some((distance, position));
            }
        }
        best.map(|(_, position)| position)
    }

    /// Advance a damped spring one `dt` seconds toward `target`. The exact
    /// under-damped solution is stable at every accepted frame interval and
    /// retains the single gentle macOS-style overshoot. `value` and `vel` are
    /// updated in place and the new value is returned.
    fn spring(state: &mut SpringState, target: f32, dt: f32) -> f32 {
        // Clamp a long stall so the Dock catches up over subsequent frames
        // instead of jumping straight to rest.
        let dt = dt.clamp(0.0, 1.0 / 30.0);
        if dt == 0.0 {
            return state.value;
        }

        let displacement = state.value - target;
        let omega0 = SPRING_STIFFNESS.sqrt();
        let decay_rate = SPRING_DAMPING * omega0;
        let omega_d = omega0 * (1.0 - SPRING_DAMPING * SPRING_DAMPING).sqrt();
        let decay = (-decay_rate * dt).exp();
        let sin = (omega_d * dt).sin();
        let cos = (omega_d * dt).cos();
        let velocity_term = (state.vel + decay_rate * displacement) / omega_d;

        state.value = target + decay * (displacement * cos + velocity_term * sin);
        state.vel = decay
            * (state.vel * cos
                - (decay_rate * state.vel + omega0 * omega0 * displacement) / omega_d * sin);
        state.value
    }

    /// The current frame's tile strip: the Launchpad tile, then every pinned
    /// app (with any running window folded in), then every running
    /// application that matches no pinned app — one tile per app on both
    /// sides of the section separator. A window matches an app when its
    /// lowercased `app_id` is among the app's [`DockApp::keys`] (pinned) or a
    /// desktop entry's [`Entry::match_keys`] (transient grouping).
    ///
    /// The strip is cached: rendering, backdrop geometry, and pointer
    /// geometry all need it every frame, but it only changes when the window
    /// set, the pinned catalog, or the localized label does. Callers that do
    /// not know the localized label pass `None` and reuse the strip as long
    /// as the window signature matches.
    ///
    /// An associated function (not a method) so the returned borrow ties to
    /// `tile_cache` alone and `render` can keep mutating its other fields
    /// while iterating the strip.
    #[allow(clippy::too_many_arguments)]
    fn frame_tiles<'a>(
        tile_cache: &'a RefCell<TileCache>,
        apps: &[DockApp],
        all_apps: &[Entry],
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
            let mut cache = tile_cache.borrow_mut();
            cache
                .window_order
                .retain(|id| windows.iter().any(|window| window.id == *id));
            for window in windows {
                if !cache.window_order.contains(&window.id) {
                    cache.window_order.push(window.id);
                }
            }
            cache.tiles =
                Self::build_tiles(apps, all_apps, icons, windows, &cache.window_order, &label);
            cache.signature = Some(signature);
            cache.label = label;
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
        all_apps: &[Entry],
        icons: &IconSet,
        windows: &[Window],
        window_order: &[aegis_model::window::WindowId],
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
                pin_entry: None,
            });
        }

        // The transient section: one tile per running application, exactly
        // like the pinned strip. A window whose `app_id` resolves to an
        // enumerated desktop entry groups under that entry (and can be
        // pinned); any other window groups under its lowercased `app_id`, or
        // stands alone when the client reported none.
        let mut groups: Vec<TransientGroup> = Vec::new();
        for window_id in window_order {
            let Some((wi, w)) = windows
                .iter()
                .enumerate()
                .find(|(_, window)| window.id == *window_id)
            else {
                continue;
            };
            if claimed[wi] {
                continue;
            }
            let app_id = win_appid[wi].clone();
            let entry = app_id.as_ref().and_then(|a| {
                all_apps
                    .iter()
                    .position(|entry| rendering::entry_matches_app_id(entry, a))
            });
            let key = match (entry, &app_id) {
                (Some(i), _) => format!("transient:{}", all_apps[i].id),
                (None, Some(a)) => format!("transient:{a}"),
                (None, None) => format!("win:{}", w.id.0),
            };
            match groups.iter_mut().find(|group| group.key == key) {
                Some(group) => group.windows.push(wi),
                None => groups.push(TransientGroup {
                    key,
                    app_id,
                    entry,
                    windows: vec![wi],
                }),
            }
        }

        for group in groups {
            let mut activated = false;
            let mut focus = None;
            let mut window_ids = Vec::new();
            for &wi in &group.windows {
                let w = &windows[wi];
                if w.state.activated {
                    window_ids.insert(0, w.id);
                } else {
                    window_ids.push(w.id);
                }
                if !w.read_only && w.state.activated {
                    activated = true;
                    focus = Some(w.id);
                } else if !w.read_only && focus.is_none() {
                    focus = Some(w.id);
                }
            }
            let first = &windows[group.windows[0]];
            let icon = group.app_id.as_ref().and_then(|a| icons.get(a));
            let label = match group.entry {
                Some(i) => all_apps[i].name.clone(),
                None if group.windows.len() == 1 => first
                    .title
                    .clone()
                    .or_else(|| first.app_id.clone())
                    .unwrap_or_else(|| application_label.to_string()),
                None => first
                    .app_id
                    .clone()
                    .or_else(|| first.title.clone())
                    .unwrap_or_else(|| application_label.to_string()),
            };
            tiles.push(Tile {
                key: group.key,
                icon,
                running: true,
                activated,
                focus,
                windows: window_ids,
                app: None,
                spawn: None,
                label,
                launchpad: false,
                pinned: false,
                pin_entry: group.entry.map(|i| all_apps[i].id.clone()),
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
            &self.all_apps,
            &self.icons,
            self.catalog_revision,
            windows,
            None,
        )[1..]
            .to_vec()
    }

    /// Bounds of the resting dock interaction surface. Pointer ownership is
    /// intentionally stable while magnification animates: the visual spring
    /// may expand beyond this rectangle, but it must not make a larger part of
    /// an application window suddenly belong to chrome.
    fn pointer_bounds(&self, display: (f32, f32)) -> Rect {
        let tiles = Self::frame_tiles(
            &self.tile_cache,
            &self.apps,
            &self.all_apps,
            &self.icons,
            self.catalog_revision,
            &self.all_windows,
            None,
        );
        let pinned_count = tiles.iter().filter(|t| t.pinned).count();
        Self::rest_bounds(tiles.len(), pinned_count, self.position, display)
    }

    /// Resting dock-icon rectangles for every running window, keyed by window
    /// id — the compositor's minimize-animation flight targets. Computed from
    /// the same tile strip and resting layout math as pointer ownership, so
    /// the flight lands where the tile sits once the magnification springs
    /// settle, never on the live, spring-widened geometry.
    pub fn minimize_targets(
        &self,
        display: (f32, f32),
    ) -> Vec<(aegis_model::window::WindowId, aegis_model::Rect)> {
        let tiles = Self::frame_tiles(
            &self.tile_cache,
            &self.apps,
            &self.all_apps,
            &self.icons,
            self.catalog_revision,
            &self.all_windows,
            None,
        );
        let pinned_count = tiles.iter().filter(|t| t.pinned).count();
        let mut targets = Vec::new();
        for (i, tile) in tiles.iter().enumerate() {
            if tile.windows.is_empty() {
                continue;
            }
            let rect = Self::rest_icon_rect(i, tiles.len(), pinned_count, self.position, display);
            for id in &tile.windows {
                targets.push((*id, rect));
            }
        }
        targets
    }

    /// The resting (un-magnified) icon rectangle of tile `i` in a row of `n`,
    /// following the frame's baseline math with every spring at rest.
    fn rest_icon_rect(
        i: usize,
        n: usize,
        pinned_count: usize,
        position: DockPosition,
        display: (f32, f32),
    ) -> aegis_model::Rect {
        let axis_len = match position {
            DockPosition::Bottom => display.0,
            DockPosition::Left | DockPosition::Right => display.1,
        };
        let centre = Self::rest_centre_estimate(i, n, pinned_count, axis_len);
        let panel = Self::rest_bounds(n, pinned_count, position, display);
        let s = DOCK_TILE;
        let rect = match position {
            DockPosition::Bottom => Rect {
                x: centre - s * 0.5,
                y: panel.y + panel.h - DOCK_BASELINE_INSET - s,
                w: s,
                h: s,
            },
            DockPosition::Left => Rect {
                x: panel.x + DOCK_BASELINE_INSET,
                y: centre - s * 0.5,
                w: s,
                h: s,
            },
            DockPosition::Right => Rect {
                x: panel.x + panel.w - DOCK_BASELINE_INSET - s,
                y: centre - s * 0.5,
                w: s,
                h: s,
            },
        };
        aegis_model::Rect::new(
            rect.x.round() as i32,
            rect.y.round() as i32,
            rect.w.round() as i32,
            rect.h.round() as i32,
        )
    }

    fn window_overlaps_bounds(window: &Window, bounds: Rect) -> bool {
        if window.minimized || window.size.w <= 0 || window.size.h <= 0 {
            return false;
        }
        let left = window.position.x as f32;
        let top = window.position.y as f32;
        let right = left + window.size.w as f32;
        let bottom = top + window.size.h as f32;
        left < bounds.x + bounds.w
            && right > bounds.x
            && top < bounds.y + bounds.h
            && bottom > bounds.y
    }

    /// Whether a visible window overlaps the resting dock rectangle. Tile
    /// geometry comes from the workspace-global strip; the overlap check uses
    /// the visible-set snapshot because a window on a hidden workspace cannot
    /// cover the dock. Tests against the stable rest rectangle, never the
    /// widened animation bounds. The same rectangle owns normal pointer input,
    /// so a magnification wave cannot make a nearby window suddenly count as
    /// an invasion.
    fn obscured_by_windows(&self, windows: &[Window], display: (f32, f32)) -> bool {
        let bounds = self.pointer_bounds(display);
        windows
            .iter()
            .any(|window| Self::window_overlaps_bounds(window, bounds))
    }

    /// Bounds of the animated panel material. Unlike pointer ownership, the
    /// backdrop follows the spring width so the widened glass remains blurred
    /// all the way to its visible edges.
    fn visual_panel_bounds(&self, display: (f32, f32)) -> Rect {
        let tiles = Self::frame_tiles(
            &self.tile_cache,
            &self.apps,
            &self.all_apps,
            &self.icons,
            self.catalog_revision,
            &self.all_windows,
            None,
        );
        let widths = tiles.iter().map(|tile| {
            self.sizes
                .get(&tile.key)
                .map_or(DOCK_TILE, |state| state.value.max(DOCK_TILE))
        });
        let pinned_count = tiles.iter().filter(|tile| tile.pinned).count();
        let gaps = tiles.len().saturating_sub(1) as f32 * DOCK_TILE_GAP
            + Self::section_extra(pinned_count, tiles.len());
        let bar_len = widths.sum::<f32>() + gaps + 2.0 * DOCK_PAD;
        Self::panel_rect_for(self.position, bar_len, display)
    }

    /// Animation-stable capture footprint of the panel's glass body. The
    /// compositor keys its backdrop capture on exact region geometry, so the
    /// glass region declares this envelope — containing the reveal morph and
    /// the fully magnified spring wave — instead of its live shape: a reveal
    /// or magnification wave then only rebuilds the effect from a still-valid
    /// capture instead of re-rendering the whole scene into it every frame.
    /// The hidden-at-rest dock declares only its handle.
    fn capture_footprint(&self, display: (f32, f32)) -> Rect {
        if self.effective_autohide() && self.autohide_reveal <= 0.001 && !self.anim_active {
            return Self::collapsed_indicator_bounds(self.position, display);
        }
        let tiles = Self::frame_tiles(
            &self.tile_cache,
            &self.apps,
            &self.all_apps,
            &self.icons,
            self.catalog_revision,
            &self.all_windows,
            None,
        );
        let pinned_count = tiles.iter().filter(|tile| tile.pinned).count();
        let gaps = tiles.len().saturating_sub(1) as f32 * DOCK_TILE_GAP
            + Self::section_extra(pinned_count, tiles.len());
        let max_len =
            tiles.len() as f32 * (DOCK_TILE_MAX * SPRING_OVERSHOOT_MARGIN) + gaps + 2.0 * DOCK_PAD;
        Self::panel_rect_for(self.position, max_len, display)
    }
}

impl Default for Dock {
    fn default() -> Self {
        Dock::new()
    }
}

/// The rectangular air gap between two rects separated along one axis — a
/// popover and its owner tile — used as a pointer bridge. `None` when the
/// rects overlap or touch (then there is no gap to cross).
fn gap_bridge(a: Rect, b: Rect) -> Option<Rect> {
    let horizontal_span = |x0: f32, x1: f32, y: f32, h: f32| Rect {
        x: x0,
        y,
        w: x1 - x0,
        h,
    };
    let min_x = a.x.min(b.x);
    let max_x = (a.x + a.w).max(b.x + b.w);
    let min_y = a.y.min(b.y);
    let max_y = (a.y + a.h).max(b.y + b.h);
    if a.y + a.h <= b.y {
        // a above b.
        Some(horizontal_span(min_x, max_x, a.y + a.h, b.y - (a.y + a.h)))
    } else if b.y + b.h <= a.y {
        // a below b.
        Some(horizontal_span(min_x, max_x, b.y + b.h, a.y - (b.y + b.h)))
    } else if a.x + a.w <= b.x {
        // a left of b.
        Some(Rect {
            x: a.x + a.w,
            y: min_y,
            w: b.x - (a.x + a.w),
            h: max_y - min_y,
        })
    } else if b.x + b.w <= a.x {
        // a right of b.
        Some(Rect {
            x: b.x + b.w,
            y: min_y,
            w: a.x - (b.x + b.w),
            h: max_y - min_y,
        })
    } else {
        None
    }
}

mod rendering;

#[cfg(test)]
mod tests;
