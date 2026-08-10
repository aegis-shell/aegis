//! Shared application context menu used by the launcher and dock.

use aegis_design::{Design, GlassRole, materials, themes};
use aegis_model::app::Entry;
use aegis_model::window::{Window, WindowId};
use lens::{Frame, Input, LayoutOpts, Rect};

use crate::{
    BackdropRegion, ChromeEvents, ChromeUpdate, LiquidGlassRegion, Localizer, Message, PopupSide,
    WindowAction, ellipsize, place_popup_side,
};

const MENU_WIDTH: f32 = 236.0;
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
#[derive(Debug, Clone, PartialEq, Eq)]
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
    SetMaximized(WindowId, bool),
    SetAlwaysOnTop(WindowId, bool),
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
    maximize_controls: bool,
    /// The side of the owning tile the popup opens toward. Owners anchored
    /// to a screen side edge (the dock on the left or right edge) flip this
    /// so the menu opens into the output instead of off-screen.
    side: PopupSide,
    /// The design snapshot the menu paints from, from
    /// [`ChromeUpdate::Appearance`] relayed by the owning component. Seeded on
    /// registration by [`Shell::add`](crate::Shell::add) and refreshed when the
    /// desktop color scheme changes; defaults to the dark appearance until the
    /// first update arrives.
    design: Design,
}

impl AppMenu {
    pub fn new(layer_id: &'static str, maximize_controls: bool) -> Self {
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
            maximize_controls,
            side: PopupSide::default(),
            design: Design::dark(),
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

    /// Open the popup toward `side` of the owner from now on (the dock sets
    /// this from its configured screen edge). Existing popups re-anchor on
    /// the next bounds computation.
    pub fn set_side(&mut self, side: PopupSide) {
        self.side = side;
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
        let window_actions = if target.windows.is_empty() {
            0
        } else {
            2 + 2 * usize::from(self.maximize_controls)
        };
        let rows = window_rows + paging + app_row + pin_row + window_actions;
        let groups = usize::from(!target.windows.is_empty())
            + usize::from(target.entry.is_some())
            + pin_row
            + usize::from(!target.windows.is_empty());
        let separators = groups.saturating_sub(1);
        let height = menu_height(rows, separators);
        Some(place_popup_side(
            self.owner,
            (MENU_WIDTH, height),
            display,
            self.side,
        ))
    }

    pub fn contains(&self, x: f32, y: f32, display: (f32, f32)) -> bool {
        self.bounds(display)
            .is_some_and(|rect| contains(rect, x, y))
    }

    /// Receive a host-owned snapshot relayed by the owning component. Only
    /// the appearance update belongs to the menu; the owner keeps the rest.
    pub fn update(&mut self, update: ChromeUpdate<'_>) {
        if let ChromeUpdate::Appearance(design) = update {
            self.design = *design;
        }
    }

    /// The menu's analytic glass body while open. Owners chain this into
    /// [`Chrome::backdrop_regions`](crate::Chrome::backdrop_regions) and
    /// [`Chrome::liquid_glass_regions`](crate::Chrome::liquid_glass_regions)
    /// so the compositor backs the popover with real backdrop blur and the
    /// glass treatment on any background, not only where a blur happens to
    /// sit underneath.
    pub fn liquid_glass_region(&self, display: (f32, f32)) -> Option<LiquidGlassRegion> {
        let bounds = self.bounds(display)?;
        Some(LiquidGlassRegion::from_role(
            &self.design,
            GlassRole::FloatingPanel,
            BackdropRegion::from(bounds),
            self.design.radii.popover,
            1.0,
        ))
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
                label,
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
        if self.maximize_controls {
            let maximize_target = live
                .iter()
                .copied()
                .find(|window| {
                    window.state.activated
                        && !window.read_only
                        && !window.minimized
                        && !window.state.fullscreen
                })
                .or_else(|| {
                    live.iter().rev().copied().find(|window| {
                        !window.read_only && !window.minimized && !window.state.fullscreen
                    })
                });
            if let Some(window) = maximize_target {
                lifecycle_rows.push(Row {
                    label: if window.state.maximized {
                        i18n.text(Message::RestoreWindow).to_string()
                    } else {
                        i18n.text(Message::MaximizeWindow).to_string()
                    },
                    action: MenuAction::SetMaximized(window.id, !window.state.maximized),
                });
                lifecycle_rows.push(always_on_top_row(window, i18n));
            }
        }
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
        let bounds = place_popup_side(self.owner, (MENU_WIDTH, height), display, self.side);
        let mut selected = None;
        let original_theme = frame.theme();
        let design = self.design;
        let menu_theme = themes::menu(original_theme, &design);
        frame.set_theme(menu_theme);
        frame.place(
            self.layer_id,
            &materials::chrome_place(bounds, materials::popover(&design)),
            |frame| {
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
                        frame.set_theme(themes::menu_heading(menu_theme, &design));
                        let heading = ellipsize(
                            frame,
                            &target.label,
                            11.5,
                            (bounds.w - MENU_PAD * 2.0).max(0.0),
                        );
                        frame.label_compact_sized(&heading, 11.5);
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
                                let label = ellipsize(
                                    frame,
                                    &row.label,
                                    frame.theme().font_size(),
                                    (bounds.w - MENU_PAD * 2.0 - frame.theme().padding() * 2.0)
                                        .max(0.0),
                                );
                                if frame.selectable(&label, false) {
                                    selected = Some(row.action);
                                }
                                frame.pop_id();
                                row_index += 1;
                            }
                        }
                    },
                );
            },
        );
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
                MenuAction::SetMaximized(id, maximized) => out
                    .window_actions
                    .push(WindowAction::SetMaximized(id, maximized)),
                MenuAction::SetAlwaysOnTop(id, on_top) => out
                    .window_actions
                    .push(WindowAction::SetAlwaysOnTop(id, on_top)),
                MenuAction::Close(ids) => out
                    .window_actions
                    .extend(ids.into_iter().map(WindowAction::Close)),
                MenuAction::Pin(id) => out.dock_pin_actions.push(PinAction::Pin(id)),
                MenuAction::Unpin(id) => out.dock_pin_actions.push(PinAction::Unpin(id)),
            }
            self.dismiss();
        }
    }
}

fn menu_height(rows: usize, separators: usize) -> f32 {
    MENU_PAD * 2.0 + HEADER_HEIGHT + rows as f32 * ROW_HEIGHT + separators as f32 * SECTION_HEIGHT
}

/// Build the always-on-top lifecycle row for `window`: the label flips with
/// the current flag and the action carries its negation.
fn always_on_top_row(window: &Window, i18n: &Localizer) -> Row {
    Row {
        label: if window.always_on_top {
            i18n.text(Message::NotAlwaysOnTop).to_string()
        } else {
            i18n.text(Message::AlwaysOnTop).to_string()
        },
        action: MenuAction::SetAlwaysOnTop(window.id, !window.always_on_top),
    }
}

fn contains(rect: Rect, x: f32, y: f32) -> bool {
    x >= rect.x && y >= rect.y && x < rect.x + rect.w && y < rect.y + rect.h
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{POPUP_GAP, POPUP_MARGIN, place_popup};

    #[test]
    fn popup_centres_above_a_bottom_tile_and_stays_on_screen() {
        let owner = Rect {
            x: 700.0,
            y: 520.0,
            w: 72.0,
            h: 72.0,
        };
        let rect = place_popup(owner, (MENU_WIDTH, 180.0), (800.0, 600.0));
        assert!(rect.x >= POPUP_MARGIN);
        assert!(rect.x + rect.w <= 800.0 - POPUP_MARGIN);
        assert!(rect.y + rect.h <= owner.y - POPUP_GAP);
        assert!(rect.y >= POPUP_MARGIN);
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
        assert!(rect.y >= owner.y + owner.h + POPUP_GAP);
    }

    #[test]
    fn unicode_truncation_preserves_character_boundaries() {
        assert_eq!(crate::truncate("窗口操作", 3), "窗口…");
    }

    #[test]
    fn open_menu_declares_a_glass_region_over_its_bounds() {
        let mut menu = AppMenu::new("test-menu", true);
        let display = (800.0, 600.0);
        assert!(menu.liquid_glass_region(display).is_none());

        let owner = Rect {
            x: 700.0,
            y: 520.0,
            w: 72.0,
            h: 72.0,
        };
        menu.open("App", None, [WindowId(1)], owner, None);
        let region = menu
            .liquid_glass_region(display)
            .expect("open menu declares a glass body");
        let bounds = menu.bounds(display).expect("open menu has bounds");
        assert_eq!(region.bounds, BackdropRegion::from(bounds));
        assert_eq!(region.corner_radius, menu.design.radii.popover);
        assert_eq!(region.opacity, 1.0);
    }

    #[test]
    fn appearance_update_replaces_the_design_snapshot() {
        let mut menu = AppMenu::new("test-menu", true);
        assert!(!menu.design.is_light());
        let light = Design::light();
        menu.update(ChromeUpdate::Appearance(&light));
        assert!(menu.design.is_light());
    }

    #[test]
    fn always_on_top_row_label_and_action_flip_with_state() {
        let i18n = Localizer::new("en-US");
        let mut window = Window::new(WindowId(7));

        window.always_on_top = false;
        let row = always_on_top_row(&window, &i18n);
        assert_eq!(row.label, "Always on Top");
        assert!(matches!(row.action, MenuAction::SetAlwaysOnTop(id, true) if id == WindowId(7)));

        window.always_on_top = true;
        let row = always_on_top_row(&window, &i18n);
        assert_eq!(row.label, "Not Always on Top");
        assert!(matches!(row.action, MenuAction::SetAlwaysOnTop(id, false) if id == WindowId(7)));
    }
}
