//! The application launcher chrome: a full-screen, Launchpad-inspired library
//! of every enumerated `.desktop` entry, backed by the pure
//! [`ass_core::launcher::Launcher`] state machine.
//!
//! The component owns presentation state only: responsive grid geometry,
//! paging, hover/click hit-testing, and the opening/closing spring. Search,
//! running-app matching, selection, and launch outcomes stay in `ass-core`.
//! The compositor host captures and Gaussian-blurs the desktop when
//! [`Chrome::backdrop_blur_sigma`] is non-zero, so the overlay remains legible
//! without replacing the user's spatial context with an opaque panel.

use std::collections::HashMap;
use std::ffi::c_void;

use lens::{Align, Color, Frame, Icon, Input, LayoutOpts, OverlayOpts, Rect};

use crate::{Chrome, ChromeEvents};
use ass_core::app::Entry;
use ass_core::input::{key_action, KeyAction, KeyChar};
use ass_core::launcher::{Launch, Launcher as Brain};
use ass_core::window::Window;

/// Blur radius requested from the compositor host, in logical pixels. The host
/// scales it to the physical framebuffer and caches the result for one open
/// session, so this does not run a full-screen compute pass every frame.
const BACKDROP_BLUR_SIGMA: f32 = 10.0;
const SEARCH_TOP: f32 = 38.0;
const SEARCH_H: f32 = 44.0;
const SEARCH_MAX_W: f32 = 520.0;
const SEARCH_MIN_W: f32 = 280.0;
const GRID_TOP: f32 = 126.0;
const GRID_BOTTOM_RESERVE: f32 = 102.0;
const GRID_MAX_W: f32 = 1180.0;
const TARGET_CELL_W: f32 = 145.0;
const TARGET_CELL_H: f32 = 118.0;
const MAX_COLUMNS: usize = 8;
const MAX_ROWS: usize = 5;
const OPEN_STIFFNESS: f32 = 360.0;
const OPEN_DAMPING: f32 = 0.86;

/// The application launcher chrome component.
pub struct Launcher {
    brain: Brain,
    /// `app_id`/icon-name (lowercased) → borrowed icon texture pointer. Shared
    /// with the dock; the binary's `IconCache` owns the textures.
    icons: HashMap<String, *mut c_void>,
    page: usize,
    columns: usize,
    page_capacity: usize,
    page_shift: f32,
    visibility: SpringState,
    anim_active: bool,
    /// Level edge tracking prevents a held dock click from activating the
    /// launcher cell underneath it on the next frame.
    prev_down: bool,
}

#[derive(Clone, Copy, Default)]
struct SpringState {
    value: f32,
    velocity: f32,
}

/// One resolved grid cell for the current frame.
struct Cell {
    filtered_position: usize,
    label: String,
    running: bool,
    selected: bool,
    icon: Option<*mut c_void>,
    fallback_initial: String,
    fallback_color: (u8, u8, u8),
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
    fn for_display(width: f32, height: f32) -> GridLayout {
        let width = width.max(1.0);
        let height = height.max(1.0);
        let side = if width < 700.0 { 24.0 } else { 64.0 };
        let grid_w = (width - 2.0 * side).clamp(1.0, GRID_MAX_W);
        let min_columns = if width < 320.0 { 1 } else { 2 };
        let columns = ((grid_w / TARGET_CELL_W).floor() as usize).clamp(min_columns, MAX_COLUMNS);

        let grid_top = if height < 560.0 {
            106.0_f32.min(height * 0.28)
        } else {
            GRID_TOP
        };
        let bottom = if height < 560.0 {
            72.0_f32.min(height * 0.18)
        } else {
            GRID_BOTTOM_RESERVE
        };
        let available_h = (height - grid_top - bottom).max(1.0);
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
    /// Construct with the launchable entries the binary enumerated, no icons.
    pub fn new(apps: Vec<Entry>) -> Launcher {
        Launcher::with_icons(apps, HashMap::new())
    }

    /// Construct with entries and a borrowed icon map. The caller retains
    /// ownership of the textures, which must outlive the launcher.
    pub fn with_icons(apps: Vec<Entry>, icons: HashMap<String, *mut c_void>) -> Launcher {
        Launcher {
            brain: Brain::new(apps),
            icons,
            page: 0,
            columns: 1,
            page_capacity: 1,
            page_shift: 0.0,
            visibility: SpringState::default(),
            anim_active: false,
            prev_down: false,
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
                self.icons.get(&key).copied()
            }
        };
        if let Some(wm_class) = &entry.startup_wm_class {
            if let Some(icon) = get(wm_class) {
                return Some(icon);
            }
        }
        if let Some(icon) = get(entry.id.strip_suffix(".desktop").unwrap_or(&entry.id)) {
            return Some(icon);
        }
        entry.icon.as_deref().and_then(get)
    }

    fn emit(outcome: Option<Launch>, out: &mut ChromeEvents) {
        match outcome {
            Some(Launch::Spawn(entry)) => out.spawn = Some(*entry),
            Some(Launch::Focus(window_id)) => out.clicked = Some(window_id),
            None => {}
        }
    }

    fn advance_visibility(&mut self, target: f32, dt: f32) -> f32 {
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
}

impl Chrome for Launcher {
    fn render(
        &mut self,
        frame: &mut Frame,
        input: &Input,
        windows: &[Window],
        _workspaces: &crate::WorkspaceSnapshot,
        out: &mut ChromeEvents,
    ) {
        let raw = input.as_raw();
        let display = raw.display_size;
        let cursor = raw.cursor;
        let down = raw.mouse_down.first().copied().unwrap_or(false);

        let running: Vec<(String, ass_core::window::WindowId)> = windows
            .iter()
            .filter_map(|window| window.app_id.as_ref().map(|id| (id.clone(), window.id)))
            .collect();
        self.brain.set_running(running);

        let target = if self.brain.is_open() { 1.0 } else { 0.0 };
        let progress = self.advance_visibility(target, raw.dt_seconds.max(0.0));
        if !self.brain.is_open() && progress <= 0.001 {
            self.page = 0;
            self.page_shift = 0.0;
            self.prev_down = down;
            return;
        }

        let dt = raw.dt_seconds.clamp(0.0, 1.0 / 15.0);
        if self.page_shift.abs() > 0.05 {
            self.page_shift *= (-18.0 * dt).exp();
        } else {
            self.page_shift = 0.0;
        }

        let layout = GridLayout::for_display(display.x, display.y);
        self.columns = layout.columns;
        self.page_capacity = layout.capacity().max(1);

        let filtered = self.brain.filtered();
        let page_total = page_count(filtered.len(), self.page_capacity);
        self.page = self.page.min(page_total.saturating_sub(1));

        // Wayland axis values are positive when scrolling down. Paging keeps
        // the number of live lens nodes bounded and mirrors Launchpad's
        // spatial model better than one enormous vertically scrolling list.
        let mut page_changed = false;
        if self.brain.is_open() && page_total > 1 {
            let page_axis = if raw.scroll_x.abs() > raw.scroll_y.abs() {
                raw.scroll_x
            } else {
                raw.scroll_y
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
                let (fallback_initial, fallback_color) = fallback_style(entry);
                Cell {
                    filtered_position,
                    label: truncate_label(&entry.name, layout.cell_w),
                    running: self.brain.is_running(app_index),
                    selected: filtered_position == selection,
                    icon: self.entry_icon(entry),
                    fallback_initial,
                    fallback_color,
                }
            })
            .collect();

        let slide_y = (1.0 - ease_out_cubic(progress)) * 18.0;
        let pressed = down && !self.prev_down && self.brain.is_open();
        let mut clicked_cell = None;
        let mut clicked_page = None;

        // A fixed-size child, rather than the layer's anchor alone, guarantees
        // that the dim surface covers every logical output pixel.
        let full = Rect {
            x: 0.0,
            y: 0.0,
            w: display.x,
            h: display.y,
        };
        frame.layer(
            "ass-launcher-backdrop",
            full,
            &OverlayOpts::default(),
            |frame| {
                frame.column_ex(
                    &sized_fill(
                        display.x,
                        display.y,
                        Color::rgba(8, 10, 20, alpha(126, progress)),
                        0.0,
                    ),
                    |_| {},
                );
            },
        );

        let search_w = (display.x * 0.40)
            .clamp(SEARCH_MIN_W, SEARCH_MAX_W)
            .min((display.x - 40.0).max(1.0));
        let search_y = if display.y < 560.0 { 22.0 } else { SEARCH_TOP } + slide_y;
        let search_rect = Rect {
            x: (display.x - search_w) * 0.5,
            y: search_y,
            w: search_w,
            h: SEARCH_H,
        };
        frame.layer(
            "ass-launcher-search",
            search_rect,
            &glass_panel(progress, SEARCH_H * 0.5),
            |frame| {
                frame.row_ex(
                    &LayoutOpts {
                        width: search_w,
                        height: SEARCH_H,
                        gap: 10.0,
                        pad: 13.0,
                        cross: Align::Center,
                        ..Default::default()
                    },
                    |frame| {
                        frame.icon(Icon::Search, 17.0);
                        let text = if self.brain.query().is_empty() {
                            "Search applications".to_string()
                        } else {
                            format!("{}▏", self.brain.query())
                        };
                        frame.label_sized(&text, 15.0);
                    },
                );
            },
        );

        let result_text = match filtered.len() {
            0 => "No applications found".to_string(),
            1 => "1 application".to_string(),
            count => format!("{count} applications"),
        };
        let result_rect = Rect {
            x: 0.0,
            y: search_y + SEARCH_H + 10.0,
            w: display.x,
            h: 20.0,
        };
        frame.layer(
            "ass-launcher-result-count",
            result_rect,
            &centered_layer(),
            |frame| {
                frame.column_ex(&sized(display.x, 20.0), |frame| {
                    frame.label_sized(&result_text, 11.0);
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
            frame.layer("ass-launcher-empty", empty, &centered_layer(), |frame| {
                frame.column_ex(&sized(display.x, 32.0), |frame| {
                    frame.label_sized("Try another search", 16.0);
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
            let id = format!("ass-launcher-cell-{}", cell.filtered_position);
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
                            match cell.icon {
                                Some(pointer) => unsafe {
                                    frame.image(
                                        pointer as *mut lens::sys::flux_image,
                                        icon_size,
                                        icon_size,
                                    )
                                },
                                None => frame.column_ex(
                                    &sized_fill(
                                        icon_size,
                                        icon_size,
                                        Color::rgba(
                                            cell.fallback_color.0,
                                            cell.fallback_color.1,
                                            cell.fallback_color.2,
                                            alpha(238, progress),
                                        ),
                                        icon_size * 0.24,
                                    ),
                                    |frame| {
                                        frame.label_sized(&cell.fallback_initial, icon_size * 0.38)
                                    },
                                ),
                            }
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

        let footer_y = (layout.y + layout.height + 13.0 + slide_y).min(display.y - 34.0);
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
                let id = format!("ass-launcher-page-{page}");
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
                "ass-launcher-page-previous",
                previous,
                &centered_layer(),
                |frame| {
                    frame.column_ex(&sized(previous.w, previous.h), |frame| {
                        frame.icon(Icon::ChevronLeft, 16.0);
                    });
                },
            );
            frame.layer(
                "ass-launcher-page-label",
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
            frame.layer("ass-launcher-page-next", next, &centered_layer(), |frame| {
                frame.column_ex(&sized(next.w, next.h), |frame| {
                    frame.icon(Icon::ChevronRight, 16.0);
                });
            });
        }

        if let Some(page) = clicked_page {
            self.change_page(page);
            self.brain.select_filtered(self.page * self.page_capacity);
        } else if let Some(filtered_position) = clicked_cell {
            Self::emit(self.brain.launch_filtered(filtered_position), out);
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

    fn update_app_catalog(
        &mut self,
        apps: &[Entry],
        _dock_apps: &[crate::DockApp],
        icons: &HashMap<String, *mut c_void>,
    ) {
        self.brain.replace_apps(apps.to_vec());
        self.icons.clone_from(icons);
        self.sync_page_to_selection();
    }

    fn key_char(&mut self, key: &KeyChar, out: &mut ChromeEvents) {
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
        if !self.brain.is_open() {
            self.page = 0;
        }
        self.brain.toggle();
        self.anim_active = true;
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

    fn backdrop_blur_sigma(&self) -> f32 {
        if self.brain.is_open() || self.visibility.value > 0.01 {
            BACKDROP_BLUR_SIGMA
        } else {
            0.0
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

/// Produce a stable, recognizable fallback when a desktop entry has no
/// decodable icon. A varied system palette plus the app's first character is
/// easier to scan than repeating one generic glyph for every missing asset.
fn fallback_style(entry: &Entry) -> (String, (u8, u8, u8)) {
    let initial = entry
        .name
        .chars()
        .find(|character| character.is_alphanumeric())
        .and_then(|character| character.to_uppercase().next())
        .unwrap_or('•')
        .to_string();
    let hash = entry.id.bytes().fold(2_166_136_261_u32, |hash, byte| {
        (hash ^ byte as u32).wrapping_mul(16_777_619)
    });
    let color = match hash % 8 {
        0 => (72, 118, 222),
        1 => (126, 87, 194),
        2 => (42, 151, 160),
        3 => (52, 142, 91),
        4 => (202, 102, 54),
        5 => (196, 76, 112),
        6 => (80, 107, 148),
        _ => (152, 93, 171),
    };
    (initial, color)
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

fn glass_panel(progress: f32, radius: f32) -> OverlayOpts {
    OverlayOpts {
        bg: Color::rgba(248, 248, 255, alpha(32, progress)),
        border: Color::rgba(255, 255, 255, alpha(64, progress)),
        border_width: 1.0,
        radius,
        pad: 0.0,
        cross: Align::Center,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_layout_is_a_complete_page_above_the_dock() {
        let layout = GridLayout::for_display(1280.0, 720.0);
        assert_eq!(layout.columns, 7);
        assert_eq!(layout.rows, 4);
        assert_eq!(layout.capacity(), 28);
        assert!(layout.x >= 0.0);
        assert!(layout.y >= SEARCH_TOP + SEARCH_H);
        assert!(layout.y + layout.height <= 720.0 - 70.0);
    }

    #[test]
    fn compact_layout_stays_usable() {
        let layout = GridLayout::for_display(360.0, 480.0);
        assert!(layout.columns >= 2);
        assert!(layout.rows >= 1);
        assert!(layout.capacity() >= 2);
    }

    #[test]
    fn pages_cover_every_application_without_a_render_cap() {
        let capacity = GridLayout::for_display(1280.0, 720.0).capacity();
        let pages = page_count(257, capacity);
        assert!(pages * capacity >= 257);
        assert!((pages - 1) * capacity < 257);
    }

    #[test]
    fn label_truncation_is_unicode_safe() {
        let label = truncate_label("非常长的应用程序名称不会切断字符", 80.0);
        assert!(label.ends_with('…'));
        assert!(label.is_char_boundary(label.len()));
    }

    #[test]
    fn fallback_style_is_stable_and_uses_the_app_initial() {
        let entry = Entry {
            id: "org.example.Editor.desktop".into(),
            name: "editor".into(),
            ..Default::default()
        };
        let first = fallback_style(&entry);
        let second = fallback_style(&entry);
        assert_eq!(first.0, "E");
        assert_eq!(first.1, second.1);
    }
}
