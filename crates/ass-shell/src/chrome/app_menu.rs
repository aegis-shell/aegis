//! Shared application context menu used by the launcher and dock.

use ass_core::app::Entry;
use ass_core::window::{Window, WindowId};
use lens::{Color, Frame, Input, LayoutOpts, OverlayOpts, Rect};

use crate::{ChromeEvents, Localizer, Message, WindowAction};

const MENU_WIDTH: f32 = 236.0;
const MENU_MARGIN: f32 = 8.0;
const MENU_GAP: f32 = 8.0;
const MENU_PAD: f32 = 7.0;
const ROW_HEIGHT: f32 = 28.0;
const HEADER_HEIGHT: f32 = 23.0;
const SECTION_HEIGHT: f32 = 7.0;
const MAX_WINDOW_ROWS: usize = 6;

#[derive(Clone)]
struct Target {
    label: String,
    entry: Option<Entry>,
    windows: Vec<WindowId>,
    pin_action: Option<PinAction>,
}

/// Dock-specific pin/unpin action carried by the shared application menu.
#[derive(Clone)]
pub enum PinAction {
    Pin(String),
    Unpin(String),
}

#[derive(Clone)]
enum MenuAction {
    None,
    Spawn(Box<Entry>),
    Focus(WindowId),
    Minimize(Vec<WindowId>),
    Close(Vec<WindowId>),
    Page(usize),
    Pin(String),
    Unpin(String),
}

struct Row {
    label: String,
    action: MenuAction,
}

/// App-anchored popup state. The target stores durable window ids rather than
/// borrowed frame data; every render resolves them against the current
/// snapshot so a window disappearing while the menu is open is harmless.
pub struct AppMenu {
    layer_id: &'static str,
    target: Option<Target>,
    owner: Rect,
    just_opened: bool,
    window_offset: usize,
}

impl AppMenu {
    pub fn new(layer_id: &'static str) -> Self {
        AppMenu {
            layer_id,
            target: None,
            owner: Rect {
                x: 0.0,
                y: 0.0,
                w: 0.0,
                h: 0.0,
            },
            just_opened: false,
            window_offset: 0,
        }
    }

    pub fn is_open(&self) -> bool {
        self.target.is_some()
    }

    pub fn open(
        &mut self,
        label: impl Into<String>,
        entry: Option<Entry>,
        windows: impl IntoIterator<Item = WindowId>,
        owner: Rect,
        pin_action: Option<PinAction>,
    ) {
        let mut ids = Vec::new();
        for id in windows {
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
        self.target = Some(Target {
            label: label.into(),
            entry,
            windows: ids,
            pin_action,
        });
        self.owner = owner;
        self.just_opened = true;
        self.window_offset = 0;
    }

    /// Keep the popup attached to an animated launcher/dock tile while it is
    /// open. The menu position is recomputed from this rect on every frame.
    pub fn set_owner(&mut self, owner: Rect) {
        self.owner = owner;
    }

    pub fn dismiss(&mut self) {
        self.target = None;
        self.just_opened = false;
        self.window_offset = 0;
    }

    /// Conservative popup bounds for pointer routing. The rendered menu may
    /// be shorter after stale window ids are removed, but it is never taller.
    pub fn bounds(&self, display: (f32, f32)) -> Option<Rect> {
        let target = self.target.as_ref()?;
        let window_rows = target.windows.len().min(MAX_WINDOW_ROWS);
        // Reserve both paging controls when needed. A first/last page renders
        // only one of them, so pointer capture remains conservatively larger.
        let paging = if target.windows.len() > MAX_WINDOW_ROWS {
            2
        } else {
            0
        };
        let app_row = usize::from(target.entry.is_some());
        let pin_row = usize::from(target.pin_action.is_some());
        let window_actions = if target.windows.is_empty() { 0 } else { 2 };
        let rows = window_rows + paging + app_row + pin_row + window_actions;
        let groups = usize::from(!target.windows.is_empty())
            + usize::from(target.entry.is_some())
            + pin_row
            + usize::from(!target.windows.is_empty());
        let separators = groups.saturating_sub(1);
        let height = menu_height(rows, separators);
        Some(place_popup(self.owner, (MENU_WIDTH, height), display))
    }

    pub fn contains(&self, x: f32, y: f32, display: (f32, f32)) -> bool {
        self.bounds(display)
            .is_some_and(|rect| contains(rect, x, y))
    }

    pub fn render(
        &mut self,
        frame: &mut Frame,
        input: &Input,
        windows: &[Window],
        i18n: &Localizer,
        out: &mut ChromeEvents,
    ) {
        let Some(target) = self.target.clone() else {
            return;
        };
        let raw = input.as_raw();
        let display = (raw.display_size.x, raw.display_size.y);
        let pointer = (raw.cursor.x, raw.cursor.y);
        let Some(conservative_bounds) = self.bounds(display) else {
            return;
        };

        let pointer_pressed = raw.mouse_pressed.iter().copied().any(|pressed| pressed);
        if !self.just_opened
            && pointer_pressed
            && !contains(conservative_bounds, pointer.0, pointer.1)
        {
            self.dismiss();
            return;
        }
        self.just_opened = false;

        let live: Vec<&Window> = target
            .windows
            .iter()
            .filter_map(|id| windows.iter().find(|window| window.id == *id))
            .collect();
        if live.is_empty() && target.entry.is_none() {
            self.dismiss();
            return;
        }

        let last_page = live.len().saturating_sub(1) / MAX_WINDOW_ROWS * MAX_WINDOW_ROWS;
        self.window_offset = self.window_offset.min(last_page);

        let mut window_rows = Vec::new();
        if self.window_offset > 0 {
            window_rows.push(Row {
                label: i18n.text(Message::PreviousWindows).to_string(),
                action: MenuAction::Page(self.window_offset.saturating_sub(MAX_WINDOW_ROWS)),
            });
        }
        for window in live.iter().skip(self.window_offset).take(MAX_WINDOW_ROWS) {
            let state = if window.read_only {
                "◉"
            } else if window.minimized {
                "◌"
            } else if window.state.activated {
                "●"
            } else {
                "•"
            };
            let title = window
                .title
                .as_deref()
                .unwrap_or_else(|| i18n.text(Message::UntitledWindow));
            let label = if window.read_only {
                format!("{state} {title} · {}", i18n.text(Message::ReadOnlyMirror))
            } else {
                format!("{state} {title}")
            };
            window_rows.push(Row {
                label: truncate(&label, 29),
                action: if window.read_only {
                    MenuAction::None
                } else {
                    MenuAction::Focus(window.id)
                },
            });
        }
        if self.window_offset + MAX_WINDOW_ROWS < live.len() {
            window_rows.push(Row {
                label: i18n.text(Message::MoreWindows).to_string(),
                action: MenuAction::Page(self.window_offset + MAX_WINDOW_ROWS),
            });
        }

        let mut launch_rows = Vec::new();
        if let Some(entry) = target.entry.clone() {
            launch_rows.push(Row {
                label: if live.is_empty() {
                    i18n.text(Message::Open).to_string()
                } else {
                    i18n.text(Message::NewWindow).to_string()
                },
                action: MenuAction::Spawn(Box::new(entry)),
            });
        }

        let mut pin_rows = Vec::new();
        if let Some(pin_action) = &target.pin_action {
            pin_rows.push(match pin_action {
                PinAction::Pin(id) => Row {
                    label: i18n.text(Message::PinToDock).to_string(),
                    action: MenuAction::Pin(id.clone()),
                },
                PinAction::Unpin(id) => Row {
                    label: i18n.text(Message::UnpinFromDock).to_string(),
                    action: MenuAction::Unpin(id.clone()),
                },
            });
        }

        let mut lifecycle_rows = Vec::new();
        let visible: Vec<WindowId> = live
            .iter()
            .filter(|window| !window.read_only && !window.minimized)
            .map(|window| window.id)
            .collect();
        if !visible.is_empty() {
            lifecycle_rows.push(Row {
                label: if live.len() == 1 {
                    i18n.text(Message::MinimizeWindow).to_string()
                } else {
                    i18n.text(Message::MinimizeAllWindows).to_string()
                },
                action: MenuAction::Minimize(visible),
            });
        }
        let all_ids: Vec<WindowId> = live
            .iter()
            .filter(|window| !window.read_only)
            .map(|window| window.id)
            .collect();
        if !all_ids.is_empty() {
            lifecycle_rows.push(Row {
                label: if all_ids.len() == 1 {
                    i18n.text(Message::CloseWindow).to_string()
                } else {
                    i18n.text(Message::CloseAllWindows).to_string()
                },
                action: MenuAction::Close(all_ids),
            });
        }

        let groups: Vec<Vec<Row>> = [window_rows, launch_rows, pin_rows, lifecycle_rows]
            .into_iter()
            .filter(|group| !group.is_empty())
            .collect();
        let row_count = groups.iter().map(Vec::len).sum();
        let height = menu_height(row_count, groups.len().saturating_sub(1));
        let bounds = place_popup(self.owner, (MENU_WIDTH, height), display);
        let mut selected = None;
        let original_theme = frame.theme();
        let menu_theme = original_theme
            .with_fg(Color::rgba(238, 240, 248, 255))
            .with_border(Color::rgba(255, 255, 255, 78))
            .with_hover(Color::rgba(255, 255, 255, 22))
            .with_active(Color::rgba(255, 255, 255, 36))
            .with_corner_radius(7.0)
            .with_border_width(0.0)
            .with_active_indicator_width(0.0);
        frame.set_theme(menu_theme);
        frame.layer(self.layer_id, bounds, &glass_panel_opts(12.0), |frame| {
            frame.column_ex(
                &LayoutOpts {
                    width: bounds.w,
                    height: bounds.h,
                    gap: 0.0,
                    pad: MENU_PAD,
                    ..Default::default()
                },
                |frame| {
                    frame.size_next(bounds.w - MENU_PAD * 2.0, HEADER_HEIGHT);
                    frame.set_theme(menu_theme.with_fg(Color::rgba(183, 188, 207, 255)));
                    frame.label_compact_sized(&truncate(&target.label, 29), 11.5);
                    frame.set_theme(menu_theme);
                    let mut row_index = 0;
                    for (group_index, group) in groups.into_iter().enumerate() {
                        if group_index > 0 {
                            frame.size_next(bounds.w - MENU_PAD * 2.0, SECTION_HEIGHT);
                            frame.separator();
                        }
                        for row in group {
                            frame.size_next(bounds.w - MENU_PAD * 2.0, ROW_HEIGHT);
                            frame.push_id(&format!("menu-row-{row_index}"));
                            if frame.selectable(&row.label, false) {
                                selected = Some(row.action);
                            }
                            frame.pop_id();
                            row_index += 1;
                        }
                    }
                },
            );
        });
        frame.set_theme(original_theme);

        if let Some(action) = selected {
            match action {
                MenuAction::None => return,
                MenuAction::Page(offset) => {
                    self.window_offset = offset;
                    return;
                }
                MenuAction::Spawn(entry) => out.activate_entry(*entry),
                MenuAction::Focus(id) => out.window_actions.push(WindowAction::Focus(id)),
                MenuAction::Minimize(ids) => out
                    .window_actions
                    .extend(ids.into_iter().map(WindowAction::Minimize)),
                MenuAction::Close(ids) => out
                    .window_actions
                    .extend(ids.into_iter().map(WindowAction::Close)),
                MenuAction::Pin(id) | MenuAction::Unpin(id) => {
                    out.dock_pin_toggles.push(id);
                }
            }
            self.dismiss();
        }
    }
}

/// Frosted-glass material shared by the dock, its tooltips, and context
/// menus: a light translucent fill over the compositor's backdrop blur with a
/// bright 1px edge.
fn glass_panel_opts(radius: f32) -> OverlayOpts {
    OverlayOpts {
        bg: Color::rgba(255, 255, 255, 38),
        border: Color::rgba(255, 255, 255, 72),
        border_width: 1.0,
        radius,
        pad: 0.0,
        ..Default::default()
    }
}

fn menu_height(rows: usize, separators: usize) -> f32 {
    MENU_PAD * 2.0 + HEADER_HEIGHT + rows as f32 * ROW_HEIGHT + separators as f32 * SECTION_HEIGHT
}

fn place_popup(owner: Rect, size: (f32, f32), display: (f32, f32)) -> Rect {
    let w = size.0.min((display.0 - MENU_MARGIN * 2.0).max(1.0));
    let h = size.1.min((display.1 - MENU_MARGIN * 2.0).max(1.0));
    let max_x = (display.0 - w - MENU_MARGIN).max(MENU_MARGIN);
    let owner_centre = owner.x + owner.w * 0.5;
    let x = (owner_centre - w * 0.5).clamp(MENU_MARGIN, max_x);
    let above = owner.y - MENU_GAP - h;
    let below = owner.y + owner.h + MENU_GAP;
    let y = if above >= MENU_MARGIN {
        above
    } else if below + h <= display.1 - MENU_MARGIN {
        below
    } else {
        above.clamp(MENU_MARGIN, (display.1 - h - MENU_MARGIN).max(MENU_MARGIN))
    };
    Rect { x, y, w, h }
}

fn contains(rect: Rect, x: f32, y: f32) -> bool {
    x >= rect.x && y >= rect.y && x < rect.x + rect.w && y < rect.y + rect.h
}

fn truncate(text: &str, max_chars: usize) -> String {
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

    #[test]
    fn popup_centres_above_a_bottom_tile_and_stays_on_screen() {
        let owner = Rect {
            x: 700.0,
            y: 520.0,
            w: 72.0,
            h: 72.0,
        };
        let rect = place_popup(owner, (MENU_WIDTH, 180.0), (800.0, 600.0));
        assert!(rect.x >= MENU_MARGIN);
        assert!(rect.x + rect.w <= 800.0 - MENU_MARGIN);
        assert!(rect.y + rect.h <= owner.y - MENU_GAP);
        assert!(rect.y >= MENU_MARGIN);
    }

    #[test]
    fn popup_falls_below_a_top_tile() {
        let owner = Rect {
            x: 200.0,
            y: 5.0,
            w: 56.0,
            h: 56.0,
        };
        let rect = place_popup(owner, (MENU_WIDTH, 180.0), (800.0, 600.0));
        assert!(rect.y >= owner.y + owner.h + MENU_GAP);
    }

    #[test]
    fn unicode_truncation_preserves_character_boundaries() {
        assert_eq!(truncate("窗口操作", 3), "窗口…");
    }
}
