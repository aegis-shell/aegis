//! The application launcher chrome: a full-screen, Launchpad-inspired library
//! of every enumerated `.desktop` entry, backed by the pure
//! [`aegis_core::launcher::Launcher`] state machine.
//!
//! The component owns presentation state only: responsive grid geometry,
//! paging, hover/click hit-testing, and the opening/closing spring. Search,
//! running-app matching, selection, and launch outcomes stay in `aegis-core`.
//! The compositor host captures and multi-resolution-blurs the desktop when
//! [`Chrome::backdrop_blur_sigma`] is non-zero, so the overlay remains legible
//! without replacing the user's spatial context with an opaque panel.

use std::ffi::c_void;

use lens::{Align, Color, Frame, Icon, Input, LayoutOpts, OverlayOpts, Rect, Theme};

use crate::{
    AppCatalog, BackdropRegion, Chrome, ChromeEvents, CursorShape, IconSet, Localizer, Message,
    Reserved, WindowAction,
};
use aegis_core::app::Entry;
use aegis_core::input::{KeyAction, KeyChar, key_action};
use aegis_core::launcher::{Launch, Launcher as Brain};
use aegis_core::window::Window;

use super::app_menu::AppMenu;

/// Blur width requested from the compositor host, in logical pixels. The host
/// scales it to its quarter-resolution capture and evaluates a fixed-cost
/// multi-resolution filter while the desktop remains live.
const BACKDROP_BLUR_SIGMA: f32 = 10.0;
const SEARCH_TOP: f32 = 38.0;
const SEARCH_H: f32 = 44.0;
const SEARCH_MAX_W: f32 = 520.0;
const SEARCH_MIN_W: f32 = 280.0;
const SEARCH_FONT_SIZE: f32 = 15.0;
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
}

#[derive(Clone, Copy, Default)]
struct SpringState {
    value: f32,
    velocity: f32,
}

/// One resolved grid cell for the current frame.
struct Cell {
    app_index: usize,
    filtered_position: usize,
    label: String,
    running: bool,
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
    /// through [`Chrome::update_app_catalog`], seeded on registration by
    /// [`crate::Shell::add`].
    pub fn new() -> Launcher {
        Launcher {
            brain: Brain::new(Vec::new()),
            icons: IconSet::default(),
            page: 0,
            columns: 1,
            page_capacity: 1,
            page_shift: 0.0,
            visibility: SpringState::default(),
            anim_active: false,
            prev_down: false,
            search_focused: false,
            modal_reserved: Reserved::default(),
            app_menu: AppMenu::new("aegis-launcher-context-menu", false),
            reduced_motion: false,
        }
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
            self.visibility.value = target;
            self.visibility.velocity = 0.0;
            self.anim_active = false;
            return target;
        }
        let omega = OPEN_STIFFNESS.sqrt();
        let damping = 2.0 * OPEN_DAMPING * omega;
        let dt = dt.clamp(0.0, 1.0 / 30.0);
        let force =
            OPEN_STIFFNESS * (target - self.visibility.value) - damping * self.visibility.velocity;
        self.visibility.velocity += force * dt;
        self.visibility.value += self.visibility.velocity * dt;
        self.visibility.value = self.visibility.value.clamp(-0.04, 1.04);

        self.anim_active =
            (self.visibility.value - target).abs() > 0.002 || self.visibility.velocity.abs() > 0.02;
        if !self.anim_active {
            self.visibility.value = target;
            self.visibility.velocity = 0.0;
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
        let running: Vec<(String, aegis_core::window::WindowId)> = windows
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
            self.prev_down = down;
            self.search_focused = false;
            return;
        }

        // Lens text and built-in glyphs inherit their colour from the active
        // theme. Fade those tokens with the visibility spring so they do not
        // remain fully opaque until their layers disappear.
        let original_theme = frame.theme();
        frame.set_theme(faded_theme(original_theme, progress));

        let dt = raw.dt_seconds.clamp(0.0, 1.0 / 15.0);
        if self.reduced_motion {
            // ADR-0029: no page-change slide.
            self.page_shift = 0.0;
        } else if self.page_shift.abs() > 0.05 {
            self.page_shift *= (-18.0 * dt).exp();
        } else {
            self.page_shift = 0.0;
        }

        let layout = GridLayout::for_display(display.x, display.y, self.modal_reserved);
        self.columns = layout.columns;
        self.page_capacity = layout.capacity().max(1);

        // The brain caches the filtered list (recomputed only when the query
        // or catalog changes); clone the indices so this frame can still call
        // `&mut` brain methods (page selection, launch) while iterating.
        let filtered = self.brain.filtered().clone();
        let page_total = page_count(filtered.len(), self.page_capacity);
        self.page = self.page.min(page_total.saturating_sub(1));

        // Wayland axis values are positive when scrolling down. Paging keeps
        // the number of live lens nodes bounded and mirrors Launchpad's
        // spatial model better than one enormous vertically scrolling list.
        let mut page_changed = false;
        if self.brain.is_open() && page_total > 1 {
            let scroll_x = raw.scroll_x * 40.0 + raw.scroll_pixels_x;
            let scroll_y = raw.scroll_y * 40.0 + raw.scroll_pixels_y;
            let page_axis = if scroll_x.abs() > scroll_y.abs() {
                scroll_x
            } else {
                scroll_y
            };
            if page_axis > 0.05 && self.page + 1 < page_total {
                self.change_page(self.page + 1);
                page_changed = true;
            } else if page_axis < -0.05 && self.page > 0 {
                self.change_page(self.page - 1);
                page_changed = true;
            }
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
                Cell {
                    app_index,
                    filtered_position,
                    label: truncate_label(&entry.name, layout.cell_w),
                    running: self.brain.is_running(app_index),
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

        // Paint the scrim on the fixed full-screen layer itself. A nested
        // child would inherit OverlayOpts' default padding and leave visible
        // seams along the top and sides.
        let full = Rect {
            x: 0.0,
            y: 0.0,
            w: display.x,
            h: display.y,
        };
        frame.layer(
            "aegis-launcher-backdrop",
            full,
            &backdrop_layer(progress),
            |_| {},
        );

        let search_rect = Self::search_rect_for_display((display.x, display.y), progress);
        let search_w = search_rect.w;
        let search_y = search_rect.y;
        let query_metrics = frame.measure_text(self.brain.query(), SEARCH_FONT_SIZE);
        let font_metrics = frame.measure_text("Ag", SEARCH_FONT_SIZE);
        let caret_rect = search_caret_rect(search_rect, query_metrics.width, font_metrics.height);
        if pressed {
            self.search_focused = contains(search_rect, cursor.x, cursor.y);
        }
        frame.layer(
            "aegis-launcher-search",
            search_rect,
            &glass_panel(progress, SEARCH_H * 0.5, self.search_focused),
            |frame| {
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
                            // origin as a real query. Its caret is an overlay
                            // below so the caret does not consume layout width.
                            frame.label_compact_sized(
                                i18n.text(Message::SearchApplications),
                                SEARCH_FONT_SIZE,
                            );
                        } else {
                            // The regular label carries theme padding, which
                            // shifts text inside this fixed-height field. The
                            // compact form keeps its measured box vertically
                            // centred; the caret is overlaid at the shaped text
                            // edge below so it does not alter layout.
                            frame.label_compact_sized(self.brain.query(), SEARCH_FONT_SIZE);
                        }
                    },
                );
            },
        );
        if self.search_focused {
            frame.layer(
                "aegis-launcher-search-caret",
                caret_rect,
                &search_caret_layer(progress),
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
        frame.layer(
            "aegis-launcher-result-count",
            result_rect,
            &centered_layer(),
            |frame| {
                frame.column_ex(&sized(display.x, 20.0), |frame| {
                    frame.label_compact_sized(&result_text, 11.0);
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
            frame.layer("aegis-launcher-empty", empty, &centered_layer(), |frame| {
                frame.column_ex(&sized(display.x, 32.0), |frame| {
                    frame.label_sized(i18n.text(Message::TryAnotherSearch), 16.0);
                });
            });
        }

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
            let cell_bg = if cell.selected {
                Color::rgba(255, 255, 255, alpha(42, progress))
            } else if hovered {
                Color::rgba(255, 255, 255, alpha(26, progress))
            } else {
                Color::TRANSPARENT
            };
            let id = format!("aegis-launcher-cell-{}", cell.filtered_position);
            frame.layer(
                &id,
                rect,
                &OverlayOpts {
                    bg: cell_bg,
                    border: if cell.selected {
                        Color::rgba(255, 255, 255, alpha(54, progress))
                    } else {
                        Color::TRANSPARENT
                    },
                    border_width: if cell.selected { 1.0 } else { 0.0 },
                    radius: 22.0,
                    pad: 0.0,
                    cross: Align::Center,
                    ..Default::default()
                },
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
                            render_app_icon(frame, cell.icon, icon_size, progress);
                            let label = if cell.running {
                                format!("• {}", cell.label)
                            } else {
                                cell.label.clone()
                            };
                            frame.label_sized(&label, 12.5);
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
                frame.layer(&id, dot, &OverlayOpts::default(), |frame| {
                    frame.column_ex(
                        &sized_fill(
                            diameter,
                            diameter,
                            Color::rgba(
                                255,
                                255,
                                255,
                                alpha(if page == self.page { 220 } else { 84 }, progress),
                            ),
                            diameter * 0.5,
                        ),
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
            frame.layer(
                "aegis-launcher-page-previous",
                previous,
                &centered_layer(),
                |frame| {
                    frame.column_ex(&sized(previous.w, previous.h), |frame| {
                        frame.icon(Icon::ChevronLeft, 16.0);
                    });
                },
            );
            frame.layer(
                "aegis-launcher-page-label",
                Rect {
                    x: display.x * 0.5 - 54.0,
                    y: footer_y,
                    w: 108.0,
                    h: 24.0,
                },
                &centered_layer(),
                |frame| {
                    frame.column_ex(&sized(108.0, 24.0), |frame| {
                        frame.label_sized(&format!("{} / {}", self.page + 1, page_total), 11.0);
                    });
                },
            );
            frame.layer(
                "aegis-launcher-page-next",
                next,
                &centered_layer(),
                |frame| {
                    frame.column_ex(&sized(next.w, next.h), |frame| {
                        frame.icon(Icon::ChevronRight, 16.0);
                    });
                },
            );
        }

        // The shell shares one lens frame across every chrome component.
        // Restore its theme so the launcher's transient alpha cannot affect a
        // component rendered after it.
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
        self.brain.is_open() || self.visibility.value > 0.01
    }

    fn modal_active(&self) -> bool {
        self.brain.is_open() || self.visibility.value > 0.01
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

    fn set_modal_reserved(&mut self, reserved: Reserved) {
        self.modal_reserved = reserved;
    }

    fn update_app_catalog(&mut self, catalog: &AppCatalog) {
        self.app_menu.dismiss();
        self.brain.replace_apps(catalog.apps.clone());
        self.icons = catalog.icons.clone();
        self.sync_page_to_selection();
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

    fn toggle(&mut self, _out: &mut ChromeEvents) {
        self.app_menu.dismiss();
        if !self.brain.is_open() {
            self.page = 0;
            // Keyboard input is still captured immediately, but the visual
            // caret appears only after a click or the first typed key.
            self.search_focused = false;
        }
        self.brain.toggle();
        self.anim_active = true;
    }

    fn launcher_active(&self) -> bool {
        self.brain.is_open()
    }

    fn close_launcher(&mut self) {
        if self.brain.is_open() {
            self.app_menu.dismiss();
            self.brain.close();
            self.visibility = SpringState::default();
            self.anim_active = false;
        }
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
        self.brain.is_open() || self.visibility.value > 0.01
    }

    fn set_reduced_motion(&mut self, reduced: bool) {
        self.reduced_motion = reduced;
    }

    fn backdrop_blur_sigma(&self) -> f32 {
        if self.brain.is_open() || self.visibility.value > 0.01 {
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
        if self.brain.is_open() || self.visibility.value > 0.01 {
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
}

fn page_count(items: usize, capacity: usize) -> usize {
    items.div_ceil(capacity.max(1)).max(1)
}

fn contains(rect: Rect, x: f32, y: f32) -> bool {
    x >= rect.x && y >= rect.y && x < rect.x + rect.w && y < rect.y + rect.h
}

fn ease_out_cubic(t: f32) -> f32 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3)
}

fn icon_visibility_scale(progress: f32) -> f32 {
    // Area, rather than diameter, tracks visibility linearly. This keeps the
    // texture readable during entry while still reducing its visible footprint
    // every frame during exit.
    progress.clamp(0.0, 1.0).sqrt()
}

fn alpha(base: u8, progress: f32) -> u8 {
    (base as f32 * progress.clamp(0.0, 1.0)).round() as u8
}

fn truncate_label(label: &str, cell_width: f32) -> String {
    let limit = ((cell_width / 7.2).floor() as usize).clamp(10, 22);
    let count = label.chars().count();
    if count <= limit {
        return label.to_string();
    }
    let mut shortened: String = label.chars().take(limit.saturating_sub(1)).collect();
    shortened.push('…');
    shortened
}

fn sized(width: f32, height: f32) -> LayoutOpts {
    LayoutOpts {
        width,
        height,
        cross: Align::Center,
        ..Default::default()
    }
}

fn sized_fill(width: f32, height: f32, bg: Color, radius: f32) -> LayoutOpts {
    LayoutOpts {
        width,
        height,
        bg,
        radius,
        cross: Align::Center,
        ..Default::default()
    }
}

fn centered_layer() -> OverlayOpts {
    OverlayOpts {
        cross: Align::Center,
        bg: Color::TRANSPARENT,
        border: Color::TRANSPARENT,
        pad: 0.0,
        ..Default::default()
    }
}

fn backdrop_layer(progress: f32) -> OverlayOpts {
    OverlayOpts {
        gap: 0.0,
        pad: 0.0,
        bg: Color::rgba(8, 10, 20, alpha(126, progress)),
        ..Default::default()
    }
}

/// Frosted-glass panel material shared with the dock: a light translucent
/// tint over the compositor's backdrop blur with a bright 1px edge.
fn glass_panel(progress: f32, radius: f32, focused: bool) -> OverlayOpts {
    OverlayOpts {
        bg: Color::rgba(255, 255, 255, alpha(38, progress)),
        border: Color::rgba(
            255,
            255,
            255,
            alpha(if focused { 150 } else { 72 }, progress),
        ),
        border_width: if focused { 1.5 } else { 1.0 },
        radius,
        pad: 0.0,
        cross: Align::Center,
        ..Default::default()
    }
}

fn search_caret_layer(progress: f32) -> OverlayOpts {
    OverlayOpts {
        gap: 0.0,
        pad: 0.0,
        bg: Color::rgba(255, 255, 255, alpha(230, progress)),
        radius: SEARCH_CARET_W * 0.5,
        ..Default::default()
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

fn faded_theme(theme: Theme, progress: f32) -> Theme {
    let fade = |color: Color| {
        let (_, _, _, opacity) = color.components();
        color.with_alpha(alpha(opacity, progress))
    };
    theme
        .with_fg(fade(theme.fg()))
        .with_accent(fade(theme.accent()))
        .with_border(fade(theme.border()))
        .with_hover(fade(theme.hover()))
        .with_active(fade(theme.active()))
        .with_disabled(fade(theme.disabled()))
        .with_error(fade(theme.error()))
}

/// Draw a real application texture or the same generic app glyph used by the
/// dock. Both variants live in one fixed slot and use the same visibility
/// curve, so missing-icon entries participate in launcher entry/exit motion
/// exactly like resolved raster icons.
fn render_app_icon(frame: &mut Frame, icon: Option<*mut c_void>, icon_size: f32, progress: f32) {
    frame.column_ex(&sized(icon_size, icon_size), |frame| {
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
                frame.column_ex(
                    &sized_fill(
                        visible_size,
                        visible_size,
                        Color::rgba(76, 85, 116, alpha(224, progress)),
                        visible_size * 0.24,
                    ),
                    |frame| {
                        frame.spacer((visible_size - glyph_size) * 0.5);
                        frame.icon(Icon::FileText, glyph_size);
                    },
                );
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let opts = backdrop_layer(1.0);
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

    #[test]
    fn label_truncation_is_unicode_safe() {
        let label = truncate_label("非常长的应用程序名称不会切断字符", 80.0);
        assert!(label.ends_with('…'));
        assert!(label.is_char_boundary(label.len()));
    }
}
