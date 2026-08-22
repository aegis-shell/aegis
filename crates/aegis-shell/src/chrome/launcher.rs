//! The application launcher chrome: a full-screen, Launchpad-inspired library
//! of every enumerated `.desktop` entry, backed by the pure
//! [`aegis_model::launcher::Launcher`] state machine.
//!
//! The component owns presentation state only: responsive grid geometry,
//! paging, hover/click hit-testing, and the opening/closing spring. Search,
//! running-app matching, selection, and launch outcomes stay in `aegis-model`.
//! The compositor host captures and multi-resolution-blurs the desktop when
//! [`Chrome::backdrop_blur_sigma`] is non-zero, so the overlay remains legible
//! without replacing the user's spatial context with an opaque panel.

use std::ffi::c_void;

use aegis_design::Design;
use aegis_design::materials::{chrome_place, glass_panel, sized, sized_fill, surface_layout};
use lens::{Align, Color, Frame, Icon, Input, LayoutOpts, Rect};

use crate::{
    BackdropRegion, Chrome, ChromeCommand, ChromeEvents, ChromeUpdate, CursorShape, IconSet,
    LiquidGlassRegion, Localizer, Message, Reserved, WindowAction, ellipsize,
};
use aegis_model::app::Entry;
use aegis_model::input::{KeyAction, KeyChar, key_action};
use aegis_model::launcher::{Launch, Launcher as Brain};
use aegis_model::window::Window;
use aegis_ui::{contains, ease_out_cubic};

use super::app_menu::AppMenu;

/// Blur width requested from the compositor host, in logical pixels. The host
/// scales it to its quarter-resolution capture and evaluates a fixed-cost
/// multi-resolution filter while the desktop remains live.
const BACKDROP_BLUR_SIGMA: f32 = 10.0;
const SEARCH_TOP: f32 = 38.0;
const SEARCH_H: f32 = 44.0;
const SEARCH_MAX_W: f32 = 520.0;
const SEARCH_MIN_W: f32 = 280.0;
const SEARCH_TEXT_X: f32 = 43.0;
const SEARCH_CARET_W: f32 = 2.0;
const GRID_TOP: f32 = 126.0;
/// Space inside the modal work area reserved for pagination and breathing
/// room. Persistent chrome (notably the dock) is subtracted separately from
/// the work area, so this value never has to guess the dock's height.
const GRID_BOTTOM_RESERVE: f32 = 44.0;
const GRID_MAX_W: f32 = 1180.0;
const TARGET_CELL_W: f32 = 145.0;
const TARGET_CELL_H: f32 = 110.0;
const MAX_COLUMNS: usize = 8;
const MAX_ROWS: usize = 5;
const OPEN_STIFFNESS: f32 = 360.0;
const OPEN_DAMPING: f32 = 0.86;
/// Scroll distance (logical pixels) one touchpad swipe must travel before it
/// can turn the page. Wheel detents are multiplied into the same pixel scale,
/// so one detent lands far past the threshold while a resting two-finger
/// jitter never reaches it. The old ±0.05 px trigger made an accidental
/// feather-touch of the touchpad flip pages.
const PAGE_SCROLL_THRESHOLD: f32 = 48.0;
/// Once a swipe has paged, it must travel this far *beyond* the trigger
/// before it can page again: a continuous two-finger flick does not rattle
/// through three pages in one gesture. Reset when the scroll axis goes quiet.
const PAGE_REPEAT_DISTANCE: f32 = 160.0;

/// The application launcher chrome component.
pub struct Launcher {
    brain: Brain,
    /// `app_id`/icon-name (lowercased) → borrowed icon texture pointer. Shared
    /// with the other catalog components; the composition root's icon cache
    /// owns the textures (see [`IconSet`]).
    icons: IconSet,
    page: usize,
    columns: usize,
    page_capacity: usize,
    page_shift: f32,
    /// Signed scroll accumulation for the in-flight page gesture, in logical
    /// pixels along the dominant axis. Non-zero only while a two-finger swipe
    /// (or wheel run) is actively feeding the page axis; it decays back to
    /// zero once the axis rests, re-arming the threshold.
    page_gesture: f32,
    /// Whether the in-flight gesture has already turned a page. The first
    /// turn needs `PAGE_SCROLL_THRESHOLD` of travel; every further turn of
    /// the same gesture needs `PAGE_REPEAT_DISTANCE`, so a deliberate long
    /// flick walks pages while a graze never leaves the first.
    paged_this_gesture: bool,
    visibility: SpringState,
    anim_active: bool,
    /// Level edge tracking prevents a held dock click from activating the
    /// launcher cell underneath it on the next frame.
    prev_down: bool,
    /// Visual focus for the compositor-owned search field. Text editing lives
    /// in the launcher brain, so the field cannot rely on lens widget focus to
    /// draw its focus ring and caret.
    search_focused: bool,
    /// Edge space reserved by chrome that remains visible during the modal.
    /// Updated by the shell before every render; keeps cells and pagination
    /// above the dock even when its dimensions change.
    modal_reserved: Reserved,
    /// Right-click application menu. It resolves stored window ids against
    /// the live snapshot on every frame, so closed windows disappear safely.
    app_menu: AppMenu,
    /// Accessibility reduced-motion (ADR-0029): the reveal spring and page
    /// slide resolve to their targets in one frame.
    reduced_motion: bool,
    /// The design snapshot the launcher paints from, from
    /// [`ChromeUpdate::Appearance`]. Seeded on registration by
    /// [`crate::Shell::add`] and refreshed when the desktop color scheme
    /// changes; defaults to the dark appearance until the first update arrives.
    design: Design,
}

/// The reveal spring state, shared from `aegis-ui::motion` (ADR-0139).
/// The launcher owns the feel (`OPEN_STIFFNESS`/`OPEN_DAMPING`); the
/// integrator and the reduced-motion rule are written once.
type SpringState = aegis_ui::motion::Spring;
/// One resolved grid cell for the current frame.
struct Cell {
    app_index: usize,
    filtered_position: usize,
    label: String,
    selected: bool,
    icon: Option<*mut c_void>,
}

#[derive(Debug, Clone, Copy)]
struct GridLayout {
    x: f32,
    y: f32,
    height: f32,
    cell_w: f32,
    cell_h: f32,
    columns: usize,
    rows: usize,
}

impl GridLayout {
    fn for_display(width: f32, height: f32, reserved: Reserved) -> GridLayout {
        let width = width.max(1.0);
        let height = height.max(1.0);
        let content_top = reserved.top.max(0) as f32;
        let content_bottom = (height - reserved.bottom.max(0) as f32).max(content_top + 1.0);
        let content_height = content_bottom - content_top;
        let side = if width < 700.0 { 24.0 } else { 64.0 };
        let grid_w = (width - 2.0 * side).clamp(1.0, GRID_MAX_W);
        let min_columns = if width < 320.0 { 1 } else { 2 };
        let columns = ((grid_w / TARGET_CELL_W).floor() as usize).clamp(min_columns, MAX_COLUMNS);

        let grid_top = content_top
            + if content_height < 560.0 {
                106.0_f32.min(content_height * 0.28)
            } else {
                GRID_TOP
            };
        let bottom = if content_height < 560.0 {
            42.0_f32.min(content_height * 0.14)
        } else {
            GRID_BOTTOM_RESERVE
        };
        let available_h = (content_bottom - grid_top - bottom).max(1.0);
        let rows = ((available_h / TARGET_CELL_H).floor() as usize).clamp(1, MAX_ROWS);
        let cell_h = (available_h / rows as f32).min(134.0);
        let grid_h = cell_h * rows as f32;

        GridLayout {
            x: (width - grid_w) * 0.5,
            y: grid_top + (available_h - grid_h) * 0.5,
            height: grid_h,
            cell_w: grid_w / columns as f32,
            cell_h,
            columns,
            rows,
        }
    }

    fn capacity(self) -> usize {
        self.columns * self.rows
    }

    fn cell(self, slot: usize, slide_y: f32) -> Rect {
        let column = slot % self.columns;
        let row = slot / self.columns;
        Rect {
            x: self.x + column as f32 * self.cell_w,
            y: self.y + row as f32 * self.cell_h + slide_y,
            w: self.cell_w,
            h: self.cell_h,
        }
    }
}

impl Launcher {
    /// Construct an empty launcher. The launchable entries and icons arrive
    /// through [`ChromeUpdate::AppCatalog`], seeded on registration by
    /// [`crate::Shell::add`].
    pub fn new() -> Launcher {
        Launcher {
            brain: Brain::new(Vec::new()),
            icons: IconSet::default(),
            page: 0,
            columns: 1,
            page_capacity: 1,
            page_shift: 0.0,
            page_gesture: 0.0,
            paged_this_gesture: false,
            visibility: SpringState::default(),
            anim_active: false,
            prev_down: false,
            search_focused: false,
            modal_reserved: Reserved::default(),
            app_menu: AppMenu::new("aegis-launcher-context-menu", false),
            reduced_motion: false,
            design: Design::dark(),
        }
    }

    /// Whether the launcher still owns the modal surface for input and
    /// composition purposes. Runs the whole fade out: releasing the modal at
    /// the 1% alpha mark let the un-blurred desktop pop through the
    /// still-fading scrim — the "bright flash on close". The fade must be
    /// fully finished before the overlay stops participating.
    ///
    /// Keyed on `anim_active`, not the spring value: the reveal spring is
    /// underdamped and crosses zero while settling, and keying the backdrop
    /// on `value > 0` alone would toggle the blur off/on at each overshoot
    /// crossing — a full capture teardown and rebuild mid-fade, visible as a
    /// one-frame flash.
    fn active(&self) -> bool {
        self.brain.is_open() || self.anim_active || self.visibility.value > 0.0
    }

    fn toggle(&mut self, _out: &mut ChromeEvents) {
        self.app_menu.dismiss();
        if !self.brain.is_open() {
            self.page = 0;
            self.search_focused = false;
        }
        self.brain.toggle();
        self.anim_active = true;
    }

    fn set_reduced_motion(&mut self, reduced: bool) {
        self.reduced_motion = reduced;
    }

    /// Resolve an entry's icon texture from the borrowed map, trying the same
    /// ids the icon cache files textures under.
    fn entry_icon(&self, entry: &Entry) -> Option<*mut c_void> {
        let get = |key: &str| {
            let key = key.to_ascii_lowercase();
            if key.is_empty() {
                None
            } else {
                self.icons.get(&key)
            }
        };
        if let Some(wm_class) = &entry.startup_wm_class
            && let Some(icon) = get(wm_class)
        {
            return Some(icon);
        }
        if let Some(icon) = get(entry.id.strip_suffix(".desktop").unwrap_or(&entry.id)) {
            return Some(icon);
        }
        entry.icon.as_deref().and_then(get)
    }

    fn emit(outcome: Option<Launch>, out: &mut ChromeEvents) {
        match outcome {
            Some(Launch::Spawn(entry)) => out.activate_entry(*entry),
            Some(Launch::Focus(window_id)) => out.clicked = Some(window_id),
            Some(Launch::BuiltIn(app)) => out.open_builtin = Some(app),
            None => {}
        }
    }

    fn advance_visibility(&mut self, target: f32, dt: f32) -> f32 {
        if self.reduced_motion {
            // ADR-0029: the reveal resolves to its end state in one frame.
            self.visibility.snap_to(target);
            self.anim_active = false;
            return target;
        }
        // Shared analytic spring integrator (ADR-0139); `dt` is clamped
        // inside. The reveal is a 0..=1 progress scalar, so the launch
        // overshoot envelope is bounded by the clamp below as before.
        self.visibility
            .advance(target, OPEN_STIFFNESS, OPEN_DAMPING, dt);
        self.visibility.value = self.visibility.value.clamp(-0.04, 1.04);

        self.anim_active = !self.visibility.settled_on(target, 0.002, 0.02);
        if !self.anim_active {
            self.visibility.snap_to(target);
        }
        self.visibility.value.clamp(0.0, 1.0)
    }

    fn sync_page_to_selection(&mut self) {
        let page = self.brain.selection() / self.page_capacity.max(1);
        self.change_page(page);
    }

    fn change_page(&mut self, page: usize) {
        if page == self.page {
            return;
        }
        self.page_shift = if page > self.page { 28.0 } else { -28.0 };
        self.page = page;
    }

    fn search_rect_for_display(display: (f32, f32), progress: f32) -> Rect {
        let search_w = (display.0 * 0.40)
            .clamp(SEARCH_MIN_W, SEARCH_MAX_W)
            .min((display.0 - 40.0).max(1.0));
        let slide_y = (1.0 - ease_out_cubic(progress)) * 18.0;
        let search_y = if display.1 < 560.0 { 22.0 } else { SEARCH_TOP } + slide_y;
        Rect {
            x: (display.0 - search_w) * 0.5,
            y: search_y,
            w: search_w,
            h: SEARCH_H,
        }
    }

    fn search_rect(&self, display: (f32, f32)) -> Rect {
        Self::search_rect_for_display(display, self.visibility.value.clamp(0.0, 1.0))
    }
}

impl Default for Launcher {
    fn default() -> Self {
        Launcher::new()
    }
}

impl Chrome for Launcher {
    fn render(
        &mut self,
        frame: &mut Frame,
        input: &Input,
        windows: &[Window],
        _workspaces: &crate::WorkspaceSnapshot,
        i18n: &Localizer,
        out: &mut ChromeEvents,
    ) {
        let raw = input.as_raw();
        let display = raw.display_size;
        let cursor = raw.cursor;
        let down = raw.mouse_down.first().copied().unwrap_or(false);

        // Prefer the active window, then the topmost/recent windows. The core
        // uses the first match for ordinary left-click activation, while the
        // context menu still receives every matching toplevel.
        let running: Vec<(String, aegis_model::window::WindowId)> = windows
            .iter()
            .filter(|window| window.state.activated)
            .chain(
                windows
                    .iter()
                    .rev()
                    .filter(|window| !window.state.activated),
            )
            .filter_map(|window| window.app_id.as_ref().map(|id| (id.clone(), window.id)))
            .collect();
        self.brain.set_running(running);

        let target = if self.brain.is_open() { 1.0 } else { 0.0 };
        let progress = self.advance_visibility(target, raw.dt_seconds.max(0.0));
        if !self.brain.is_open() && progress <= 0.001 {
            self.page = 0;
            self.page_shift = 0.0;
            self.page_gesture = 0.0;
            self.paged_this_gesture = false;
            self.prev_down = down;
            self.search_focused = false;
            return;
        }

        // Fade the whole overlay with the visibility spring: the frame
        // opacity stamps into every draw command's colours, so painted
        // layers, text, and icons fade together.
        frame.set_opacity(progress);
        // The launcher's content sits on the scrim veil, which stays a dark
        // wash in both appearances; re-tone the frame foreground onto that
        // tonal side so labels, bare icons, and the search field stay legible
        // in the light appearance too (the page-appropriate theme foreground
        // turns to dark ink there). Restored at the end of the render.
        let original_theme = frame.theme();
        frame.set_theme(original_theme.with_fg(self.design.colors.on_scrim_text));

        let dt = raw.dt_seconds.clamp(0.0, 1.0 / 15.0);
        if self.reduced_motion {
            // ADR-0029: no page-change slide.
            self.page_shift = 0.0;
        } else if self.page_shift.abs() > 0.05 {
            // Shared exponential decay (ADR-0139); the local clamp keeps a
            // long stall from erasing the slide in one frame.
            self.page_shift = aegis_ui::motion::decay::toward_zero(self.page_shift, 18.0, dt);
        } else {
            self.page_shift = 0.0;
        }

        let layout = GridLayout::for_display(display.x, display.y, self.modal_reserved);
        self.columns = layout.columns;
        self.page_capacity = layout.capacity().max(1);
        let typography = self.design.typography;

        // The brain caches the filtered list (recomputed only when the query
        // or catalog changes); clone the indices so this frame can still call
        // `&mut` brain methods (page selection, launch) while iterating.
        let filtered = self.brain.filtered().clone();
        let page_total = page_count(filtered.len(), self.page_capacity);
        self.page = self.page.min(page_total.saturating_sub(1));

        // Shell input carries lens-convention scroll deltas: scrolling down
        // is negative (the compositor inverts the Wayland axis sign at the
        // input boundary). Paging keeps the number of live lens nodes
        // bounded and mirrors Launchpad's spatial model better than one
        // enormous vertically scrolling list.
        //
        // Wheel detents are discrete, deliberate steps and page exactly once
        // each. Two-finger touchpad swipes arrive as raw pixel deltas, where
        // the old ±0.05 px trigger made an accidental graze flip pages: the
        // dominant axis now accumulates until it crosses
        // `PAGE_SCROLL_THRESHOLD`, and after a turn the same gesture must
        // travel `PAGE_REPEAT_DISTANCE` further before it may turn again.
        // The accumulator re-arms whenever the axis rests.
        let mut page_changed = false;
        if self.brain.is_open() && page_total > 1 {
            let wheel = wheel_page_axis(raw.scroll_x, raw.scroll_y);
            let finger = finger_page_axis(raw.scroll_pixels_x, raw.scroll_pixels_y);
            if wheel.abs() > 0.5 {
                if let Some(page) = page_step(self.page, page_total, wheel < 0.0) {
                    self.change_page(page);
                    page_changed = true;
                }
                self.page_gesture = 0.0;
            } else if finger.abs() > 0.5 {
                // A direction reversal re-arms the cheap first-page
                // threshold: turning back past a boundary you just hit is a
                // new intent, not a continuation of the blocked gesture.
                if self.page_gesture.signum() == -finger.signum() && self.page_gesture != 0.0 {
                    self.paged_this_gesture = false;
                }
                self.page_gesture += finger;
                let direction = self.page_gesture.signum();
                // The first page of a gesture turns at the threshold; the
                // same gesture must then travel the (longer) repeat
                // distance for every further page, so a long deliberate
                // flick walks pages while a graze stays on one.
                let required = if self.paged_this_gesture {
                    PAGE_REPEAT_DISTANCE
                } else {
                    PAGE_SCROLL_THRESHOLD
                };
                if self.page_gesture.abs() >= required {
                    match page_step(self.page, page_total, direction < 0.0) {
                        Some(page) => {
                            self.change_page(page);
                            // Consume the travel: the next page of this
                            // gesture needs the full repeat distance again.
                            self.page_gesture = 0.0;
                            self.paged_this_gesture = true;
                            page_changed = true;
                        }
                        // The gesture ran past the first/last page: consume
                        // it too. Holding at the trigger would let a blocked
                        // gesture re-fire every frame; reversing direction
                        // then needs a fresh, deliberate swipe.
                        None => {
                            self.page_gesture = 0.0;
                        }
                    }
                }
            } else if self.page_gesture != 0.0 {
                // The axis rested: re-arm the threshold for the next gesture.
                self.page_gesture = 0.0;
                self.paged_this_gesture = false;
            }
        } else {
            self.page_gesture = 0.0;
            self.paged_this_gesture = false;
        }
        if page_changed {
            self.brain.select_filtered(self.page * self.page_capacity);
        }

        let selection = self.brain.selection();
        let start = self.page * self.page_capacity;
        let end = (start + self.page_capacity).min(filtered.len());
        let cells: Vec<Cell> = filtered[start..end]
            .iter()
            .enumerate()
            .map(|(slot, &app_index)| {
                let entry = &self.brain.apps()[app_index];
                let filtered_position = start + slot;
                let label = if self.brain.is_running(app_index) {
                    format!("• {}", entry.name)
                } else {
                    entry.name.clone()
                };
                Cell {
                    app_index,
                    filtered_position,
                    label: ellipsize(
                        frame,
                        &label,
                        typography.label,
                        (layout.cell_w - 14.0).max(0.0),
                    ),
                    selected: filtered_position == selection,
                    icon: self.entry_icon(entry),
                }
            })
            .collect();

        let slide_y = (1.0 - ease_out_cubic(progress)) * 18.0;
        let pressed = down && !self.prev_down && self.brain.is_open() && !self.app_menu.is_open();
        let right_pressed =
            raw.mouse_pressed.get(1).copied().unwrap_or(false) && self.brain.is_open();
        let mut clicked_cell = None;
        let mut clicked_page = None;
        let mut context_app = None;

        // Paint the scrim on the fixed full-screen placement itself. A nested
        // child would inherit the surface layout's default padding and leave
        // visible seams along the top and sides.
        let full = Rect {
            x: 0.0,
            y: 0.0,
            w: display.x,
            h: display.y,
        };
        frame.place(
            "aegis-launcher-backdrop",
            &chrome_place(full, backdrop_layer(&self.design)),
            |_| {},
        );

        let search_rect = Self::search_rect_for_display((display.x, display.y), progress);
        let search_w = search_rect.w;
        let search_y = search_rect.y;
        let search_text_width = (search_w - SEARCH_TEXT_X - 16.0).max(0.0);
        let shown_query = ellipsize(
            frame,
            self.brain.query(),
            typography.headline,
            search_text_width,
        );
        let shown_placeholder = ellipsize(
            frame,
            i18n.text(Message::SearchApplications),
            typography.headline,
            search_text_width,
        );
        let query_metrics = frame.measure_text(&shown_query, typography.headline);
        let font_metrics = frame.measure_text("Ag", typography.headline);
        let caret_rect = search_caret_rect(search_rect, query_metrics.width, font_metrics.height);
        if pressed {
            self.search_focused = contains(search_rect, cursor.x, cursor.y);
        }
        // Frosted-glass search field: the shared glass-panel material carries
        // the layout defaults while the painted layer keeps the launcher's
        // own scheme-following field tone over the compositor's backdrop
        // blur — dark translucent glass in the dark appearance (the shared
        // popover/menu tokens are white in both and would read as an opaque
        // bright bar here), white glass in the light one. No glass_focus
        // token carries the focused edge, so the border alpha and widths
        // stay numeric overrides.
        let surface = self.design.colors.launcher_field_surface;
        let edge = self.design.colors.launcher_field_border;
        let (_, _, _, surface_alpha) = surface.components();
        let (_, _, _, edge_alpha) = edge.components();
        let search_panel = LayoutOpts {
            bg: surface.with_alpha(surface_alpha),
            border: edge.with_alpha(if self.search_focused { 150 } else { edge_alpha }),
            border_width: if self.search_focused { 1.5 } else { 1.0 },
            radius: SEARCH_H * 0.5,
            ..glass_panel(&self.design)
        };
        frame.place(
            "aegis-launcher-search",
            &chrome_place(search_rect, search_panel),
            |frame| {
                // The search field is a popover-surface body, not bare scrim
                // content: its glyphs take the page-appropriate text tone so
                // they sit on the right side of the light scheme's white
                // glass (the outer on-scrim override would keep them light).
                let outer = frame.theme();
                frame.set_theme(outer.with_fg(self.design.colors.application_text));
                frame.row_ex(
                    &LayoutOpts {
                        width: search_w,
                        height: SEARCH_H,
                        gap: 0.0,
                        pad: 0.0,
                        cross: Align::Center,
                        ..Default::default()
                    },
                    |frame| {
                        frame.spacer(16.0);
                        frame.icon(Icon::Search, 17.0);
                        frame.spacer(10.0);
                        if self.brain.query().is_empty() {
                            // Keep the placeholder on the exact same text
                            // origin as a real query. Its caret is a separate
                            // placement below so the caret does not consume
                            // layout width.
                            frame.label_compact_sized(&shown_placeholder, typography.headline);
                        } else {
                            // The regular label carries theme padding, which
                            // shifts text inside this fixed-height field. The
                            // compact form keeps its measured box vertically
                            // centred; the caret is placed at the shaped text
                            // edge below so it does not alter layout.
                            frame.label_compact_sized(&shown_query, typography.headline);
                        }
                    },
                );
                frame.set_theme(outer);
            },
        );
        if self.search_focused {
            frame.place(
                "aegis-launcher-search-caret",
                &chrome_place(caret_rect, search_caret_layer(&self.design)),
                |_| {},
            );
        }

        let result_text = i18n.application_count(filtered.len());
        let result_rect = Rect {
            x: 0.0,
            y: search_y + SEARCH_H + 10.0,
            w: display.x,
            h: 20.0,
        };
        frame.place(
            "aegis-launcher-result-count",
            &chrome_place(result_rect, centered_layer()),
            |frame| {
                frame.centered(display.x, 20.0, |frame| {
                    frame.label_compact_sized(&result_text, typography.footnote);
                });
            },
        );

        if cells.is_empty() {
            let empty = Rect {
                x: 0.0,
                y: layout.y + layout.height * 0.40 + slide_y,
                w: display.x,
                h: 32.0,
            };
            frame.place(
                "aegis-launcher-empty",
                &chrome_place(empty, centered_layer()),
                |frame| {
                    frame.centered(display.x, 32.0, |frame| {
                        frame.label_compact_sized(
                            i18n.text(Message::TryAnotherSearch),
                            typography.headline,
                        );
                    });
                },
            );
        }

        let colors = self.design.colors;
        for (slot, cell) in cells.iter().enumerate() {
            let mut rect = layout.cell(slot, slide_y);
            rect.x += self.page_shift;
            let hovered = self.brain.is_open() && contains(rect, cursor.x, cursor.y);
            if pressed && hovered {
                clicked_cell = Some(cell.filtered_position);
            }
            if right_pressed && hovered {
                context_app = Some((cell.app_index, rect));
            }

            let icon_size = (layout.cell_w * 0.52)
                .min(layout.cell_h - 42.0)
                .clamp(44.0, 82.0);
            // Selection and hover are scheme-following surfaces like the
            // search field: translucent dark ink in the dark appearance and
            // translucent white in the light one (透黑/透白 随主题), instead of
            // the fixed light glow the scrim-anchored tone produced.
            let selection = colors.launcher_selection;
            let (_, _, _, selection_alpha) = selection.components();
            let cell_bg = if cell.selected {
                selection
            } else if hovered {
                selection.with_alpha((selection_alpha as f32 * 0.62) as u8)
            } else {
                Color::TRANSPARENT
            };
            let id = format!("aegis-launcher-cell-{}", cell.filtered_position);
            frame.place(
                &id,
                &chrome_place(
                    rect,
                    LayoutOpts {
                        bg: cell_bg,
                        border: if cell.selected {
                            self.design.colors.launcher_field_border
                        } else {
                            Color::TRANSPARENT
                        },
                        border_width: if cell.selected { 1.0 } else { 0.0 },
                        radius: self.design.radii.cell,
                        pad: 0.0,
                        cross: Align::Center,
                        ..surface_layout()
                    },
                ),
                |frame| {
                    frame.column_ex(
                        &LayoutOpts {
                            width: rect.w,
                            height: rect.h,
                            gap: 6.0,
                            pad: 7.0,
                            cross: Align::Center,
                            ..Default::default()
                        },
                        |frame| {
                            render_app_icon(frame, &self.design, cell.icon, icon_size, progress);
                            frame.label_compact_sized(&cell.label, typography.label);
                        },
                    );
                },
            );
        }

        let modal_bottom = (display.y - self.modal_reserved.bottom.max(0) as f32).max(1.0);
        let footer_y = (layout.y + layout.height + 13.0 + slide_y).min(modal_bottom - 28.0);
        if page_total > 1 && page_total <= 12 {
            let group_w = page_total as f32 * 18.0;
            let group_x = (display.x - group_w) * 0.5;
            for page in 0..page_total {
                let hit = Rect {
                    x: group_x + page as f32 * 18.0,
                    y: footer_y,
                    w: 18.0,
                    h: 20.0,
                };
                if pressed && contains(hit, cursor.x, cursor.y) {
                    clicked_page = Some(page);
                }
                let diameter = if page == self.page { 8.0 } else { 6.0 };
                let dot = Rect {
                    x: hit.x + (hit.w - diameter) * 0.5,
                    y: hit.y + (hit.h - diameter) * 0.5,
                    w: diameter,
                    h: diameter,
                };
                let id = format!("aegis-launcher-page-{page}");
                frame.place(&id, &chrome_place(dot, surface_layout()), |frame| {
                    frame.column_ex(
                        &LayoutOpts {
                            cross: Align::Center,
                            ..sized_fill(
                                diameter,
                                diameter,
                                colors.on_scrim_text.with_alpha(if page == self.page {
                                    220
                                } else {
                                    84
                                }),
                                diameter * 0.5,
                            )
                        },
                        |_| {},
                    );
                });
            }
        } else if page_total > 12 {
            let previous = Rect {
                x: display.x * 0.5 - 86.0,
                y: footer_y,
                w: 32.0,
                h: 24.0,
            };
            let next = Rect {
                x: display.x * 0.5 + 54.0,
                ..previous
            };
            if pressed && contains(previous, cursor.x, cursor.y) && self.page > 0 {
                clicked_page = Some(self.page - 1);
            }
            if pressed && contains(next, cursor.x, cursor.y) && self.page + 1 < page_total {
                clicked_page = Some(self.page + 1);
            }
            frame.place(
                "aegis-launcher-page-previous",
                &chrome_place(previous, centered_layer()),
                |frame| {
                    frame.centered(previous.w, previous.h, |frame| {
                        frame.icon(Icon::ChevronLeft, 16.0);
                    });
                },
            );
            frame.place(
                "aegis-launcher-page-label",
                &chrome_place(
                    Rect {
                        x: display.x * 0.5 - 54.0,
                        y: footer_y,
                        w: 108.0,
                        h: 24.0,
                    },
                    centered_layer(),
                ),
                |frame| {
                    frame.centered(108.0, 24.0, |frame| {
                        frame.label_compact_sized(
                            &format!("{} / {}", self.page + 1, page_total),
                            typography.footnote,
                        );
                    });
                },
            );
            frame.place(
                "aegis-launcher-page-next",
                &chrome_place(next, centered_layer()),
                |frame| {
                    frame.centered(next.w, next.h, |frame| {
                        frame.icon(Icon::ChevronRight, 16.0);
                    });
                },
            );
        }

        // The shell shares one lens frame across every chrome component.
        // Restore full opacity and the ambient theme so the launcher's fade
        // and tonal override cannot affect a component rendered after it.
        frame.set_opacity(1.0);
        frame.set_theme(original_theme);

        if let Some(page) = clicked_page {
            self.change_page(page);
            self.brain.select_filtered(self.page * self.page_capacity);
        } else if let Some(filtered_position) = clicked_cell {
            Self::emit(self.brain.launch_filtered(filtered_position), out);
        }
        if let Some((app_index, owner)) = context_app {
            let entry = self.brain.apps()[app_index].clone();
            self.app_menu.open(
                entry.name.clone(),
                Some(entry),
                self.brain.running_surfaces(app_index),
                owner,
                None,
            );
        }
        let action_start = out.window_actions.len();
        let had_activation = out.spawn.is_some() || out.open_builtin.is_some();
        self.app_menu.render(frame, input, windows, i18n, out);
        let activated = out.window_actions[action_start..]
            .iter()
            .any(|action| matches!(action, WindowAction::Focus(_)));
        let activated_app = !had_activation && (out.spawn.is_some() || out.open_builtin.is_some());
        if activated || activated_app {
            self.brain.close();
            self.anim_active = true;
        }
        self.prev_down = down;
    }

    fn captures_keyboard(&self) -> bool {
        self.brain.is_open()
    }

    fn captures_pointer(
        &self,
        _x: f32,
        _y: f32,
        _display: (f32, f32),
        _windows: &[Window],
        _workspaces: &crate::WorkspaceSnapshot,
    ) -> bool {
        self.active()
    }

    fn modal_active(&self) -> bool {
        self.active()
    }

    fn visible_during_modal(&self) -> bool {
        true
    }

    fn cursor_shape_at(
        &self,
        x: f32,
        y: f32,
        display: (f32, f32),
        _windows: &[Window],
        _workspaces: &crate::WorkspaceSnapshot,
    ) -> Option<CursorShape> {
        Some(if self.app_menu.contains(x, y, display) {
            CursorShape::Pointer
        } else if contains(self.search_rect(display), x, y) {
            CursorShape::Text
        } else {
            CursorShape::Default
        })
    }

    fn update(&mut self, update: ChromeUpdate<'_>) {
        match update {
            ChromeUpdate::ModalReserved(reserved) => self.modal_reserved = reserved,
            ChromeUpdate::AppCatalog(catalog) => {
                self.app_menu.dismiss();
                self.brain.replace_apps(catalog.apps.clone());
                self.icons = catalog.icons.clone();
                self.sync_page_to_selection();
            }
            ChromeUpdate::ReducedMotion(reduced) => self.set_reduced_motion(reduced),
            ChromeUpdate::Appearance(design) => {
                self.design = *design;
                self.app_menu.update(update);
            }
            _ => {}
        }
    }

    fn key_char(&mut self, key: &KeyChar, out: &mut ChromeEvents) {
        if self.app_menu.is_open() {
            if matches!(key_action(key.keysym, key.ch), KeyAction::Escape) {
                self.app_menu.dismiss();
            }
            return;
        }
        if self.brain.is_open() {
            self.search_focused = true;
        }
        let action = key_action(key.keysym, key.ch);
        let outcome = match action {
            KeyAction::Left => {
                self.brain.move_selection_by(-1);
                None
            }
            KeyAction::Right => {
                self.brain.move_selection_by(1);
                None
            }
            KeyAction::Up => {
                self.brain.move_selection_by(-(self.columns as i32));
                None
            }
            KeyAction::Down => {
                self.brain.move_selection_by(self.columns as i32);
                None
            }
            other => self.brain.handle(other),
        };
        Self::emit(outcome, out);
        if self.brain.is_open() {
            self.sync_page_to_selection();
        }
    }

    fn command(&mut self, command: &ChromeCommand<'_>, _out: &mut ChromeEvents) {
        match command {
            ChromeCommand::ToggleLauncher => {
                self.toggle(_out);
            }
            ChromeCommand::CloseLauncher if self.brain.is_open() => {
                // Fade out through the same exit path as a user close: the
                // old hard reset (`visibility = SpringState::default()`)
                // dropped the scrim and the backdrop blur in a single frame
                // — a bright pop whenever Prism opened over the launcher.
                self.app_menu.dismiss();
                self.brain.close();
                self.anim_active = true;
            }
            _ => {}
        }
    }

    fn launcher_active(&self) -> bool {
        self.brain.is_open()
    }

    fn anim_pending(&self) -> bool {
        self.anim_active
            || self.page_shift.abs() > 0.05
            || if self.brain.is_open() {
                (self.visibility.value - 1.0).abs() > 0.002
            } else {
                self.visibility.value > 0.002
            }
    }

    fn requires_composition(&self) -> bool {
        self.active()
    }

    /// The blur radius stays constant for the whole session, including the
    /// exit fade. The radius is part of the compositor's
    /// `BackdropCacheKey`: easing it per frame forced a full-screen
    /// re-capture + effect rebuild on *every* frame of the fade, and any
    /// frame where that rebuild did not deliver fell back to drawing the
    /// sharp, unblurred desktop under the thinning scrim — the visible
    /// "bright flash on close". A constant radius keeps the exit on the
    /// zero-rebuild `Cached` path; the veil itself fades via the component
    /// opacity, so the session still eases away visually.
    fn backdrop_blur_sigma(&self) -> f32 {
        if self.active() {
            BACKDROP_BLUR_SIGMA
        } else {
            0.0
        }
    }

    fn backdrop_regions(
        &self,
        display: (f32, f32),
        _windows: &[Window],
        _workspaces: &crate::WorkspaceSnapshot,
    ) -> Vec<BackdropRegion> {
        if self.active() {
            vec![BackdropRegion {
                x: 0.0,
                y: 0.0,
                w: display.0,
                h: display.1,
            }]
        } else {
            Vec::new()
        }
    }

    /// The context menu's glass body. The full-screen frost region above
    /// already blurs underneath; this adds the analytic glass treatment to
    /// the menu itself so it reads like the dock's menu anywhere.
    fn liquid_glass_regions(
        &self,
        display: (f32, f32),
        _windows: &[Window],
        _workspaces: &crate::WorkspaceSnapshot,
    ) -> Vec<LiquidGlassRegion> {
        self.app_menu
            .liquid_glass_region(display)
            .into_iter()
            .collect()
    }
}

fn page_count(items: usize, capacity: usize) -> usize {
    items.div_ceil(capacity.max(1)).max(1)
}

/// Wheel detents scaled into the shared pixel space, reduced to the
/// dominant axis. One detent is 40 px here, matching the legacy multiplier.
fn wheel_page_axis(scroll_x: f32, scroll_y: f32) -> f32 {
    let x = scroll_x * 40.0;
    let y = scroll_y * 40.0;
    if x.abs() > y.abs() { x } else { y }
}

/// Touchpad pixel deltas reduced to the dominant axis.
fn finger_page_axis(scroll_x: f32, scroll_y: f32) -> f32 {
    if scroll_x.abs() > scroll_y.abs() {
        scroll_x
    } else {
        scroll_y
    }
}

/// The next page index for a `forward` (true = later pages) step, or `None`
/// at either end of the pager.
fn page_step(page: usize, page_total: usize, forward: bool) -> Option<usize> {
    if forward && page + 1 < page_total {
        Some(page + 1)
    } else if !forward && page > 0 {
        Some(page - 1)
    } else {
        None
    }
}

fn icon_visibility_scale(progress: f32) -> f32 {
    // Area, rather than diameter, tracks visibility linearly. This keeps the
    // texture readable during entry while still reducing its visible footprint
    // every frame during exit.
    progress.clamp(0.0, 1.0).sqrt()
}

fn centered_layer() -> LayoutOpts {
    LayoutOpts {
        cross: Align::Center,
        bg: Color::TRANSPARENT,
        border: Color::TRANSPARENT,
        pad: 0.0,
        ..surface_layout()
    }
}

fn backdrop_layer(design: &Design) -> LayoutOpts {
    LayoutOpts {
        gap: 0.0,
        pad: 0.0,
        bg: design.colors.scrim.with_alpha(126),
        ..surface_layout()
    }
}

fn search_caret_layer(design: &Design) -> LayoutOpts {
    LayoutOpts {
        gap: 0.0,
        pad: 0.0,
        // The caret lives inside the search field's popover surface, so it
        // takes the page-appropriate text tone (dark ink on the light
        // scheme's white glass), not the scrim-anchored tone around it.
        bg: design.colors.application_text.with_alpha(230),
        radius: SEARCH_CARET_W * 0.5,
        ..surface_layout()
    }
}

fn search_caret_rect(search: Rect, query_width: f32, caret_height: f32) -> Rect {
    Rect {
        // Match Lens text fields: the 2 px caret is centred on the shaped
        // insertion edge instead of sitting after a layout gap.
        x: search.x + SEARCH_TEXT_X + query_width - SEARCH_CARET_W * 0.5,
        y: search.y + (search.h - caret_height) * 0.5,
        w: SEARCH_CARET_W,
        h: caret_height,
    }
}

/// Draw a real application texture or the same generic app glyph used by the
/// dock. Both variants live in one fixed slot and use the same visibility
/// curve, so missing-icon entries participate in launcher entry/exit motion
/// exactly like resolved raster icons.
fn render_app_icon(
    frame: &mut Frame,
    design: &Design,
    icon: Option<*mut c_void>,
    icon_size: f32,
    progress: f32,
) {
    let slot = LayoutOpts {
        cross: Align::Center,
        ..sized(icon_size, icon_size)
    };
    frame.column_ex(&slot, |frame| {
        let visible_size = icon_size * icon_visibility_scale(progress);
        if visible_size <= 0.5 {
            return;
        }
        frame.spacer((icon_size - visible_size) * 0.5);
        match icon {
            // The pointer crosses from the binary's flux binding type to
            // lens's ABI-identical flux_image.
            Some(pointer) => unsafe {
                frame.image(
                    pointer as *mut lens::sys::flux_image,
                    visible_size,
                    visible_size,
                );
            },
            None => {
                let glyph_size = visible_size * 0.50;
                let chip = LayoutOpts {
                    cross: Align::Center,
                    ..sized_fill(
                        visible_size,
                        visible_size,
                        // The scheme-invariant neutral slate shared with the
                        // dock's generic tile.
                        design.colors.generic_icon_surface,
                        visible_size * 0.24,
                    )
                };
                frame.column_ex(&chip, |frame| {
                    frame.spacer((visible_size - glyph_size) * 0.5);
                    frame.icon(Icon::FileText, glyph_size);
                });
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AppCatalog;
    use lens::Ui;

    #[test]
    fn standard_layout_is_a_complete_page_above_the_dock() {
        let reserved = Reserved {
            bottom: 86,
            ..Reserved::default()
        };
        let layout = GridLayout::for_display(1280.0, 720.0, reserved);
        assert_eq!(layout.columns, 7);
        assert_eq!(layout.rows, 4);
        assert_eq!(layout.capacity(), 28);
        assert!(layout.x >= 0.0);
        assert!(layout.y >= SEARCH_TOP + SEARCH_H);
        assert!(layout.y + layout.height <= 720.0 - reserved.bottom as f32 - 40.0);
    }

    #[test]
    fn compact_layout_stays_usable() {
        let layout = GridLayout::for_display(360.0, 480.0, Reserved::default());
        assert!(layout.columns >= 2);
        assert!(layout.rows >= 1);
        assert!(layout.capacity() >= 2);
    }

    #[test]
    fn backdrop_layer_has_no_layout_inset() {
        let opts = backdrop_layer(&Design::dark());
        assert_eq!(opts.pad, 0.0);
        assert_eq!(opts.gap, 0.0);
        assert_ne!(opts.bg, Color::TRANSPARENT);
    }

    #[test]
    fn pages_cover_every_application_without_a_render_cap() {
        let capacity = GridLayout::for_display(1280.0, 720.0, Reserved::default()).capacity();
        let pages = page_count(257, capacity);
        assert!(pages * capacity >= 257);
        assert!((pages - 1) * capacity < 257);
    }

    #[test]
    fn raster_icon_scale_tracks_launcher_visibility() {
        assert_eq!(icon_visibility_scale(0.0), 0.0);
        assert_eq!(icon_visibility_scale(1.0), 1.0);
        assert!(icon_visibility_scale(0.25) < icon_visibility_scale(0.75));
    }

    #[test]
    fn opening_launcher_keeps_search_caret_hidden() {
        let mut launcher = Launcher::new();
        launcher.toggle(&mut ChromeEvents::default());
        assert!(launcher.brain.is_open());
        assert!(!launcher.search_focused);
    }

    #[test]
    fn search_caret_is_centered_on_the_shaped_text_edge() {
        let search = Rect {
            x: 100.0,
            y: 38.0,
            w: 520.0,
            h: SEARCH_H,
        };
        let caret = search_caret_rect(search, 90.0, 18.0);
        assert_eq!(caret.x + caret.w * 0.5, search.x + SEARCH_TEXT_X + 90.0);
        assert_eq!(caret.y + caret.h * 0.5, search.y + search.h * 0.5);
        assert_eq!(caret.w, SEARCH_CARET_W);
    }

    #[test]
    fn reduced_motion_snaps_visibility_in_one_frame() {
        let mut launcher = Launcher::new();
        // Without the policy the reveal spring eases over many frames.
        let eased = launcher.advance_visibility(1.0, 0.016);
        assert!(eased < 1.0, "spring eases: {eased}");
        assert!(launcher.anim_active);

        // With the policy the first frame lands on the target, settled.
        let mut reduced = Launcher::new();
        reduced.set_reduced_motion(true);
        let snapped = reduced.advance_visibility(1.0, 0.016);
        assert_eq!(snapped, 1.0, "one frame to the end state");
        assert!(!reduced.anim_active, "nothing left in flight");
        let snapped_down = reduced.advance_visibility(0.0, 0.016);
        assert_eq!(snapped_down, 0.0);
        assert!(!reduced.anim_active);
    }

    // ---- paging gesture ---------------------------------------------------

    /// A launcher open over a multi-page catalog, plus the (input, frame)
    /// pair needed to drive its render headlessly.
    fn paged_launcher(pages: usize) -> (Launcher, Input) {
        let mut launcher = Launcher::new();
        let capacity = GridLayout::for_display(1280.0, 720.0, Reserved::default()).capacity();
        let apps = (0..pages * capacity + 2)
            .map(|index| Entry {
                id: format!("app{index}.desktop"),
                name: format!("App {index}"),
                ..Entry::default()
            })
            .collect();
        launcher.update(ChromeUpdate::AppCatalog(&AppCatalog {
            apps,
            ..AppCatalog::default()
        }));
        launcher.toggle(&mut ChromeEvents::default());
        launcher.visibility = SpringState {
            value: 1.0,
            velocity: 0.0,
        };
        launcher.anim_active = false;
        let input = Input::new((1280.0, 720.0), 1.0 / 60.0);
        (launcher, input)
    }

    fn render_frame(launcher: &mut Launcher, input: &Input) {
        let mut ui = Ui::headless().expect("create headless Lens context");
        let i18n = Localizer::new("en-US");
        ui.frame(input, |frame| {
            launcher.render(
                frame,
                input,
                &[],
                &crate::WorkspaceSnapshot { outputs: vec![] },
                &i18n,
                &mut ChromeEvents::default(),
            );
        });
    }

    #[test]
    fn touchpad_swipe_needs_a_deliberate_distance_before_paging() {
        let (mut launcher, mut input) = paged_launcher(3);

        // A graze far below the threshold never pages.
        input.set_scroll_pixels(-20.0, 0.0);
        render_frame(&mut launcher, &input);
        assert_eq!(launcher.page, 0, "a 20 px graze must not page");

        // Accumulating past the threshold pages exactly once.
        for _ in 0..6 {
            input.set_scroll_pixels(-10.0, 0.0);
            render_frame(&mut launcher, &input);
        }
        assert_eq!(launcher.page, 1, "80 px of travel pages once");

        // The same continued flick does not page again until it has
        // travelled the repeat distance.
        for _ in 0..4 {
            input.set_scroll_pixels(-10.0, 0.0);
            render_frame(&mut launcher, &input);
        }
        assert_eq!(launcher.page, 1, "a short continuation must not page again");
        for _ in 0..14 {
            input.set_scroll_pixels(-10.0, 0.0);
            render_frame(&mut launcher, &input);
        }
        assert_eq!(launcher.page, 2, "the full repeat distance pages again");

        // Reversing direction after the axis rests pages back.
        input.set_scroll_pixels(0.0, 0.0);
        render_frame(&mut launcher, &input);
        for _ in 0..6 {
            input.set_scroll_pixels(12.0, 0.0);
            render_frame(&mut launcher, &input);
        }
        assert_eq!(launcher.page, 1, "reversing pages back");
    }

    #[test]
    fn reversing_mid_gesture_re_arms_the_threshold() {
        let (mut launcher, mut input) = paged_launcher(3);

        // Page forward once.
        for _ in 0..6 {
            input.set_scroll_pixels(-10.0, 0.0);
            render_frame(&mut launcher, &input);
        }
        assert_eq!(launcher.page, 1);

        // Reverse without lifting: the reversal itself is a new intent, so
        // the cheap threshold — not the repeat distance — applies.
        for _ in 0..6 {
            input.set_scroll_pixels(12.0, 0.0);
            render_frame(&mut launcher, &input);
        }
        assert_eq!(launcher.page, 0, "a mid-gesture reversal pages back");
    }

    #[test]
    fn swipe_past_the_last_page_is_consumed() {
        // Two pages of content: one full page plus a second partial page.
        let capacity = GridLayout::for_display(1280.0, 720.0, Reserved::default()).capacity();
        let apps = (0..capacity + 2)
            .map(|index| Entry {
                id: format!("app{index}.desktop"),
                name: format!("App {index}"),
                ..Entry::default()
            })
            .collect();
        let mut launcher = Launcher::new();
        launcher.update(ChromeUpdate::AppCatalog(&AppCatalog {
            apps,
            ..AppCatalog::default()
        }));
        launcher.toggle(&mut ChromeEvents::default());
        launcher.visibility = SpringState {
            value: 1.0,
            velocity: 0.0,
        };
        launcher.anim_active = false;
        let input = Input::new((1280.0, 720.0), 1.0 / 60.0);
        let mut input = input;

        // Walk to the last page, then keep swiping forward.
        for _ in 0..16 {
            input.set_scroll_pixels(-14.0, 0.0);
            render_frame(&mut launcher, &input);
        }
        assert_eq!(launcher.page, 1, "arrived at the last page");
        for _ in 0..8 {
            input.set_scroll_pixels(-14.0, 0.0);
            render_frame(&mut launcher, &input);
        }
        assert_eq!(launcher.page, 1, "swiping past the end stays put");
    }

    #[test]
    fn wheel_detents_page_once_each() {
        let (mut launcher, mut input) = paged_launcher(3);
        input.set_scroll(0.0, -1.0);
        render_frame(&mut launcher, &input);
        assert_eq!(launcher.page, 1, "one wheel detent pages exactly once");

        input.set_scroll(0.0, 0.0);
        render_frame(&mut launcher, &input);
        input.set_scroll(0.0, -1.0);
        render_frame(&mut launcher, &input);
        assert_eq!(launcher.page, 2);
    }

    // ---- fade lifetime ----------------------------------------------------

    #[test]
    fn closing_fades_instead_of_snapping_and_keeps_the_backdrop_alive() {
        let mut launcher = Launcher::new();
        launcher.toggle(&mut ChromeEvents::default());
        launcher.visibility = SpringState {
            value: 1.0,
            velocity: 0.0,
        };

        // Closing via the Prism-open path fades instead of resetting.
        launcher.command(&ChromeCommand::CloseLauncher, &mut ChromeEvents::default());
        assert!(!launcher.brain.is_open());
        assert!(
            launcher.visibility.value > 0.9,
            "the close path starts a fade, not a snap: {}",
            launcher.visibility.value
        );
        assert!(launcher.anim_active, "the exit animation is in flight");
        assert!(launcher.active(), "still modal while the fade runs");
        assert_eq!(
            launcher
                .backdrop_regions(
                    (1280.0, 720.0),
                    &[],
                    &crate::WorkspaceSnapshot { outputs: vec![] }
                )
                .len(),
            1,
            "the backdrop stays declared through the fade"
        );

        // Mid-fade the blur radius stays at full strength: easing the
        // radius keyed the compositor's capture cache on every frame and any
        // failed rebuild fell through to the sharp desktop — the flash.
        // The veil fades via component opacity; the blur hands over only
        // once, when the spring has fully settled.
        launcher.advance_visibility(0.0, 1.0 / 60.0);
        assert_eq!(launcher.backdrop_blur_sigma(), BACKDROP_BLUR_SIGMA);
        assert!(launcher.active(), "the fade is still running");

        // The underdamped spring crosses zero while settling; the gate must
        // hold through those overshoot frames instead of toggling the blur.
        launcher.visibility.value = -0.02;
        assert!(
            launcher.active(),
            "an overshoot crossing must not drop the modal"
        );
        assert_eq!(
            launcher
                .backdrop_regions(
                    (1280.0, 720.0),
                    &[],
                    &crate::WorkspaceSnapshot { outputs: vec![] }
                )
                .len(),
            1,
            "the backdrop survives the overshoot"
        );

        // Fully settled: one clean handover to the direct path.
        launcher.visibility.value = 0.0;
        launcher.anim_active = false;
        assert!(!launcher.active());
        assert_eq!(launcher.backdrop_blur_sigma(), 0.0);
    }

    #[test]
    fn fully_settled_launcher_is_composition_free() {
        let launcher = Launcher::new();
        assert!(!launcher.active());
        assert!(!launcher.requires_composition());
        assert_eq!(launcher.backdrop_blur_sigma(), 0.0);
        assert!(
            launcher
                .backdrop_regions(
                    (1280.0, 720.0),
                    &[],
                    &crate::WorkspaceSnapshot { outputs: vec![] }
                )
                .is_empty()
        );
    }
}
