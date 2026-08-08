//! The user-consent application picker: a modal, centered panel listing the
//! candidate applications of one `PickApp` IPC request (the AppChooser
//! portal's compositor side).
//!
//! [`ChromeCommand::StartAppPick`] opens the
//! panel, and the user's confirm or cancel travels back through
//! [`ChromeEvents::app_pick_confirmed`] / [`ChromeEvents::app_pick_cancelled`].
//! Ordinary modal chrome over the live scene: no freeze, no screen-content
//! capture. Candidate names and icons resolve against the shell's pushed
//! [`AppCatalog`] snapshot at render time, so catalog updates apply to an
//! open panel.

use std::time::{Duration, Instant};

use lens::{Align, Color, Frame, Input, LayoutOpts, OverlayOpts, Rect};

use crate::{
    AppCatalog, BackdropRegion, Chrome, ChromeCommand, ChromeEvents, ChromeUpdate, CursorShape,
    LiquidGlassFocus, LiquidGlassRegion, Localizer, Reserved, ellipsize,
};
use aegis_design::{Design, GlassRole, materials, themes};
use aegis_model::input::{KeyAction, KeyChar, key_action};
use aegis_model::window::Window;

const PANEL_W: f32 = 440.0;
const PANEL_PAD: f32 = 16.0;
const TITLE_H: f32 = 24.0;
const SUBJECT_H: f32 = 18.0;
const ROW_H: f32 = 36.0;
const ICON: f32 = 20.0;
const VISIBLE_ROWS: usize = 8;
const BUTTON_H: f32 = 30.0;
const BUTTON_W: f32 = 88.0;
const BACKDROP_BLUR_SIGMA: f32 = 18.0;
/// Two presses on the same row within this window count as a double-click.
const DOUBLE_CLICK: Duration = Duration::from_millis(400);
/// Rows scrolled per wheel detent over the list.
const WHEEL_ROWS: f32 = 3.0;

/// Parameters of one user-consent application pick, mapped from the IPC
/// request by the compositor runtime.
#[derive(Debug, Clone)]
pub struct AppPickParams {
    /// Candidate desktop file ids, in the order the requester supplied.
    pub choices: Vec<String>,
    /// Human-readable context line (the file, URI, or content type the app
    /// is chosen for), shown under the title.
    pub subject: Option<String>,
    /// The previously used app id, pre-highlighted when still a candidate.
    pub last_choice: Option<String>,
}

/// One candidate row: the requested id plus its catalog-resolved display
/// name (the id stem when the catalog has no entry for it).
#[derive(Debug, Clone)]
struct Row {
    id: String,
    name: String,
}

/// The resolved geometry of the panel for one frame.
#[derive(Debug, Clone, Copy)]
struct PickerLayout {
    panel: Rect,
    title: Rect,
    subject: Option<Rect>,
    list: Rect,
    cancel: Rect,
    accept: Rect,
    visible_rows: usize,
}

impl PickerLayout {
    fn for_display(display: (f32, f32), reserved: Reserved, has_subject: bool) -> PickerLayout {
        let left = reserved.left.max(0) as f32;
        let top = reserved.top.max(0) as f32;
        let usable_w = (display.0 - left - reserved.right.max(0) as f32).max(1.0);
        let usable_h = (display.1 - top - reserved.bottom.max(0) as f32).max(1.0);

        let panel_w = PANEL_W.min((usable_w - 32.0).max(240.0));
        let subject_block = if has_subject { SUBJECT_H + 4.0 } else { 0.0 };
        let fixed = PANEL_PAD + TITLE_H + subject_block + 10.0 + 10.0 + BUTTON_H + PANEL_PAD;
        let max_h = (usable_h - 32.0).max(160.0);
        let visible_rows =
            (((max_h - fixed).max(ROW_H) / ROW_H).floor() as usize).clamp(1, VISIBLE_ROWS);
        let list_h = visible_rows as f32 * ROW_H;
        let panel_h = fixed + list_h;
        let panel = Rect {
            x: left + ((usable_w - panel_w) * 0.5).max(0.0),
            y: top + ((usable_h - panel_h) * 0.5).max(0.0),
            w: panel_w,
            h: panel_h,
        };

        let inner_x = panel.x + PANEL_PAD;
        let inner_w = panel.w - 2.0 * PANEL_PAD;
        let title = Rect {
            x: inner_x,
            y: panel.y + PANEL_PAD,
            w: inner_w,
            h: TITLE_H,
        };
        let subject = has_subject.then_some(Rect {
            x: inner_x,
            y: title.y + title.h + 4.0,
            w: inner_w,
            h: SUBJECT_H,
        });
        let list_y = subject.map(|r| r.y + r.h).unwrap_or(title.y + title.h) + 10.0;
        let list = Rect {
            x: inner_x,
            y: list_y,
            w: inner_w,
            h: list_h,
        };
        let buttons_y = panel.y + panel.h - PANEL_PAD - BUTTON_H;
        let accept = Rect {
            x: panel.x + panel.w - PANEL_PAD - BUTTON_W,
            y: buttons_y,
            w: BUTTON_W,
            h: BUTTON_H,
        };
        let cancel = Rect {
            x: accept.x - BUTTON_W - 8.0,
            y: buttons_y,
            w: BUTTON_W,
            h: BUTTON_H,
        };
        PickerLayout {
            panel,
            title,
            subject,
            list,
            cancel,
            accept,
            visible_rows,
        }
    }
}

/// The app-picker chrome component. Inert until the runtime opens it with
/// [`ChromeCommand::StartAppPick`].
pub struct AppPicker {
    active: bool,
    rows: Vec<Row>,
    subject: Option<String>,
    selected: usize,
    scroll_row: usize,
    wheel: f32,
    last_click: Option<(usize, Instant)>,
    visible_rows: usize,
    modal_reserved: Reserved,
    catalog: AppCatalog,
    /// The design snapshot the picker paints from, from
    /// [`ChromeUpdate::Appearance`]. Seeded on registration by
    /// [`crate::Shell::add`] and refreshed when the desktop color scheme
    /// changes; defaults to the dark appearance until the first update arrives.
    design: Design,
}

impl AppPicker {
    pub fn new() -> AppPicker {
        AppPicker {
            active: false,
            rows: Vec::new(),
            subject: None,
            selected: 0,
            scroll_row: 0,
            wheel: 0.0,
            last_click: None,
            visible_rows: VISIBLE_ROWS,
            modal_reserved: Reserved::default(),
            catalog: AppCatalog::default(),
            design: Design::dark(),
        }
    }

    /// Confirm the highlighted candidate and close.
    fn confirm(&mut self, out: &mut ChromeEvents) {
        if let Some(row) = self.rows.get(self.selected) {
            out.app_pick_confirmed = Some(row.id.clone());
        } else {
            out.app_pick_cancelled = true;
        }
        self.close();
    }

    /// Emit a cancellation and close.
    fn cancel(&mut self, out: &mut ChromeEvents) {
        out.app_pick_cancelled = true;
        self.close();
    }

    fn close(&mut self) {
        self.active = false;
        self.rows = Vec::new();
        self.last_click = None;
    }

    fn start_app_pick(&mut self, params: AppPickParams) {
        self.rows = params
            .choices
            .into_iter()
            .map(|id| {
                let name = Self::resolve(&self.catalog, &id)
                    .map(|entry| entry.name.clone())
                    .unwrap_or_else(|| id.strip_suffix(".desktop").unwrap_or(&id).to_string());
                Row { id, name }
            })
            .collect();
        self.selected = params
            .last_choice
            .as_deref()
            .and_then(|last| self.rows.iter().position(|row| row.id == last))
            .unwrap_or(0);
        self.subject = params.subject;
        self.scroll_row = 0;
        self.wheel = 0.0;
        self.last_click = None;
        self.follow_selection();
        self.active = !self.rows.is_empty();
    }

    #[cfg(test)]
    fn update_app_catalog(&mut self, catalog: &AppCatalog) {
        <Self as Chrome>::update(self, ChromeUpdate::AppCatalog(catalog));
    }

    /// Keep the highlighted row inside the list's scroll window after
    /// keyboard motion.
    fn follow_selection(&mut self) {
        if self.selected < self.scroll_row {
            self.scroll_row = self.selected;
        } else if self.selected >= self.scroll_row + self.visible_rows {
            self.scroll_row = self.selected + 1 - self.visible_rows;
        }
    }

    /// The catalog entry for one candidate id: exact desktop-id match, then
    /// a StartupWMClass match (requesters sometimes hand over wm classes).
    fn resolve<'c>(catalog: &'c AppCatalog, id: &str) -> Option<&'c aegis_model::app::Entry> {
        catalog
            .apps
            .iter()
            .find(|entry| entry.id == id)
            .or_else(|| {
                catalog
                    .apps
                    .iter()
                    .find(|entry| entry.startup_wm_class.as_deref() == Some(id))
            })
    }

    /// The icon texture for one row, mirroring the launcher's lookup chain:
    /// lowercase id, StartupWMClass, then the declared icon name.
    fn icon(&self, row: &Row) -> Option<*mut std::ffi::c_void> {
        let get = |key: &str| self.catalog.icons.get(&key.to_lowercase());
        get(&row.id)
            .or_else(|| get(row.id.strip_suffix(".desktop").unwrap_or(&row.id)))
            .or_else(|| {
                Self::resolve(&self.catalog, &row.id)
                    .and_then(|entry| entry.icon.as_deref())
                    .and_then(get)
            })
    }
}

impl Default for AppPicker {
    fn default() -> Self {
        AppPicker::new()
    }
}

impl Chrome for AppPicker {
    fn render(
        &mut self,
        frame: &mut Frame,
        input: &Input,
        _windows: &[Window],
        _workspaces: &crate::WorkspaceSnapshot,
        _i18n: &Localizer,
        out: &mut ChromeEvents,
    ) {
        if !self.active {
            return;
        }
        let raw = input.as_raw();
        let display = (raw.display_size.x, raw.display_size.y);
        let cursor = raw.cursor;
        let pressed = raw.mouse_pressed.first().copied().unwrap_or(false);
        let design = self.design;
        let layout =
            PickerLayout::for_display(display, self.modal_reserved, self.subject.is_some());
        self.visible_rows = layout.visible_rows;

        frame.layer(
            "aegis-app-picker-scrim",
            Rect {
                x: 0.0,
                y: 0.0,
                w: display.0,
                h: display.1,
            },
            &OverlayOpts {
                bg: design.colors.scrim,
                ..Default::default()
            },
            |_| {},
        );

        let original_theme = frame.theme();
        frame.set_theme(themes::application(&design));

        // Minimal foreground tint only. The compositor-owned analytic pass
        // supplies the body, refraction, rim light, and shadow.
        frame.layer(
            "aegis-app-picker-panel",
            layout.panel,
            &materials::glass_panel(&design),
            |_| {},
        );

        frame.layer(
            "aegis-app-picker-title",
            layout.title,
            &transparent(),
            |frame| {
                frame.row_ex(&stretch(layout.title), |frame| {
                    frame.label_sized("Choose Application", 15.0);
                });
            },
        );

        if let Some(subject_rect) = layout.subject {
            let subject = ellipsize(
                frame,
                self.subject.as_deref().unwrap_or_default(),
                11.5,
                subject_rect.w,
            );
            frame.layer(
                "aegis-app-picker-subject",
                subject_rect,
                &transparent(),
                |frame| {
                    frame.row_ex(&stretch(subject_rect), |frame| {
                        frame.label_compact_sized(&subject, 11.5);
                    });
                },
            );
        }

        // The candidate list: a sliding window over the rows.
        let max_scroll = self.rows.len().saturating_sub(layout.visible_rows);
        self.scroll_row = self.scroll_row.min(max_scroll);
        let mut clicked_row = None;
        for pos in 0..layout.visible_rows {
            let row_index = self.scroll_row + pos;
            if row_index >= self.rows.len() {
                break;
            }
            let rect = Rect {
                x: layout.list.x,
                y: layout.list.y + pos as f32 * ROW_H,
                w: layout.list.w,
                h: ROW_H,
            };
            let hovered = contains(rect, cursor.x, cursor.y);
            if pressed && hovered {
                clicked_row = Some(row_index);
            }
            let selected = self.selected == row_index;
            let row = &self.rows[row_index];
            let icon = self.icon(row);
            let row_name = ellipsize(frame, &row.name, 13.0, (rect.w - 48.0).max(0.0));
            // Selection is the panel's single optical focus (declared in
            // `liquid_glass_regions`); the painted layer is only the shared
            // neutral fallback wash, never a structural accent fill.
            let mut material = if selected {
                materials::glass_focus(&design, true, 1.0)
            } else if hovered {
                materials::glass_focus(&design, false, 1.0)
            } else {
                OverlayOpts {
                    bg: Color::TRANSPARENT,
                    ..Default::default()
                }
            };
            material.radius = design.radii.menu_item;
            material.pad = 0.0;
            let id = format!("aegis-app-picker-row-{pos}");
            frame.layer(&id, rect, &material, |frame| {
                frame.row_ex(&stretch(rect), |frame| {
                    frame.spacer(10.0);
                    frame.column_ex(
                        &LayoutOpts {
                            width: ICON,
                            height: ROW_H,
                            cross: Align::Center,
                            ..Default::default()
                        },
                        |frame| match icon {
                            Some(pointer) => unsafe {
                                frame.image(pointer as *mut lens::sys::flux_image, ICON, ICON);
                            },
                            None => {
                                frame.spacer(0.0);
                            }
                        },
                    );
                    frame.spacer(8.0);
                    frame.label_compact_sized(&row_name, 13.0);
                });
            });
        }

        if self.rows.is_empty() {
            frame.layer(
                "aegis-app-picker-empty",
                layout.list,
                &transparent(),
                |frame| {
                    frame.column_ex(
                        &LayoutOpts {
                            width: layout.list.w,
                            height: layout.list.h,
                            cross: Align::Center,
                            ..Default::default()
                        },
                        |frame| {
                            frame.label_sized("No applications", 13.0);
                        },
                    );
                },
            );
        }

        let cancel_hovered = contains(layout.cancel, cursor.x, cursor.y);
        let accept_hovered = contains(layout.accept, cursor.x, cursor.y);
        let clicked_cancel = pressed && cancel_hovered;
        let clicked_accept = pressed && accept_hovered;
        frame.layer(
            "aegis-app-picker-cancel",
            layout.cancel,
            &OverlayOpts {
                bg: if cancel_hovered {
                    design.colors.application_hover
                } else {
                    design.colors.card_surface
                },
                radius: design.radii.control,
                pad: 0.0,
                cross: Align::Center,
                ..Default::default()
            },
            |frame| {
                frame.column_ex(&stretch(layout.cancel), |frame| {
                    frame.label_sized("Cancel", 13.0);
                });
            },
        );
        frame.layer(
            "aegis-app-picker-accept",
            layout.accept,
            &OverlayOpts {
                bg: design.colors.application_accent,
                radius: design.radii.control,
                pad: 0.0,
                cross: Align::Center,
                ..Default::default()
            },
            |frame| {
                frame.column_ex(&stretch(layout.accept), |frame| {
                    frame.label_sized("Open", 13.0);
                });
            },
        );

        frame.set_theme(original_theme);

        if pressed && !contains(layout.panel, cursor.x, cursor.y) {
            self.cancel(out);
            return;
        }
        if clicked_cancel {
            self.cancel(out);
            return;
        }
        if clicked_accept {
            self.confirm(out);
            return;
        }
        if let Some(row_index) = clicked_row {
            let now = Instant::now();
            let double = self.last_click.is_some_and(|(prev, at)| {
                prev == row_index && now.duration_since(at) <= DOUBLE_CLICK
            });
            self.last_click = Some((row_index, now));
            self.selected = row_index;
            if double {
                self.last_click = None;
                self.confirm(out);
                return;
            }
        }

        // Shell input carries lens-convention scroll deltas: scrolling down
        // is negative. Negate so wheel-down advances the list downward.
        let wheel = -(raw.scroll_y * WHEEL_ROWS + raw.scroll_pixels_y / ROW_H);
        if contains(layout.list, cursor.x, cursor.y) && wheel != 0.0 {
            self.wheel += wheel;
            let steps = self.wheel.trunc() as i32;
            self.wheel -= steps as f32;
            if steps != 0 {
                self.scroll_row =
                    (self.scroll_row as i32 + steps).clamp(0, max_scroll as i32) as usize;
            }
        }
    }

    fn captures_keyboard(&self) -> bool {
        self.active
    }

    fn captures_pointer(
        &self,
        _x: f32,
        _y: f32,
        _display: (f32, f32),
        _windows: &[Window],
        _workspaces: &crate::WorkspaceSnapshot,
    ) -> bool {
        self.active
    }

    fn modal_active(&self) -> bool {
        self.active
    }

    // A pending consent owns the complete chrome layer: the Dock, HUD, and
    // toasts stay suppressed until the prompt is answered.
    fn exclusive_presentation_active(&self) -> bool {
        self.active
    }

    fn visible_during_modal(&self) -> bool {
        true
    }

    fn requires_composition(&self) -> bool {
        self.active
    }

    fn cursor_shape_at(
        &self,
        x: f32,
        y: f32,
        display: (f32, f32),
        _windows: &[Window],
        _workspaces: &crate::WorkspaceSnapshot,
    ) -> Option<CursorShape> {
        if !self.active {
            return None;
        }
        let layout =
            PickerLayout::for_display(display, self.modal_reserved, self.subject.is_some());
        Some(
            if contains(layout.accept, x, y) || contains(layout.cancel, x, y) {
                CursorShape::Pointer
            } else {
                CursorShape::Default
            },
        )
    }

    fn update(&mut self, update: ChromeUpdate<'_>) {
        match update {
            ChromeUpdate::ModalReserved(reserved) => self.modal_reserved = reserved,
            ChromeUpdate::AppCatalog(catalog) => self.catalog = catalog.clone(),
            ChromeUpdate::Appearance(design) => self.design = *design,
            _ => {}
        }
    }

    fn key_char(&mut self, key: &KeyChar, out: &mut ChromeEvents) {
        if !self.active {
            return;
        }
        match key_action(key.keysym, key.ch) {
            KeyAction::Up => {
                if !self.rows.is_empty() {
                    self.selected = self.selected.checked_sub(1).unwrap_or(self.rows.len() - 1);
                }
                self.follow_selection();
            }
            KeyAction::Down => {
                if !self.rows.is_empty() {
                    self.selected = (self.selected + 1) % self.rows.len();
                }
                self.follow_selection();
            }
            KeyAction::Enter => self.confirm(out),
            KeyAction::Escape => self.cancel(out),
            _ => {}
        }
    }

    fn command(&mut self, command: &ChromeCommand<'_>, _out: &mut ChromeEvents) {
        match command {
            ChromeCommand::StartAppPick(params) => self.start_app_pick((**params).clone()),
            ChromeCommand::CancelAppPick if self.active => self.close(),
            _ => {}
        }
    }

    fn app_pick_active(&self) -> bool {
        self.active
    }

    fn backdrop_blur_sigma(&self) -> f32 {
        if self.active {
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
        if !self.active {
            return Vec::new();
        }
        let layout =
            PickerLayout::for_display(display, self.modal_reserved, self.subject.is_some());
        // One region exactly matching the glass body below: the runtime drops
        // it from the rectangular frost set, so the analytic pass alone owns
        // the rounded panel.
        vec![BackdropRegion::from(layout.panel)]
    }

    fn liquid_glass_regions(
        &self,
        display: (f32, f32),
        _windows: &[Window],
        _workspaces: &crate::WorkspaceSnapshot,
    ) -> Vec<LiquidGlassRegion> {
        if !self.active {
            return Vec::new();
        }
        let design = self.design;
        let layout =
            PickerLayout::for_display(display, self.modal_reserved, self.subject.is_some());
        // The highlighted row carries the panel's single optical focus. A
        // selection scrolled out of the list window carries no field: the
        // bounds must stay inside the parent body (ADR-0105).
        let scroll_row = self
            .scroll_row
            .min(self.rows.len().saturating_sub(layout.visible_rows));
        let focus = (self.selected < self.rows.len()
            && self.selected >= scroll_row
            && self.selected < scroll_row + layout.visible_rows)
            .then(|| LiquidGlassFocus {
                bounds: BackdropRegion::from(Rect {
                    x: layout.list.x,
                    y: layout.list.y + (self.selected - scroll_row) as f32 * ROW_H,
                    w: layout.list.w,
                    h: ROW_H,
                }),
                corner_radius: design.radii.menu_item,
                strength: design.glass_focus.field_strength,
            });
        vec![
            LiquidGlassRegion::from_role(
                &design,
                GlassRole::ProminentPanel,
                BackdropRegion::from(layout.panel),
                design.radii.glass_panel,
                1.0,
            )
            .with_focus(focus),
        ]
    }
}

fn contains(rect: Rect, x: f32, y: f32) -> bool {
    x >= rect.x && y >= rect.y && x < rect.x + rect.w && y < rect.y + rect.h
}

fn stretch(rect: Rect) -> LayoutOpts {
    LayoutOpts {
        width: rect.w,
        height: rect.h,
        cross: Align::Center,
        ..Default::default()
    }
}

fn transparent() -> OverlayOpts {
    OverlayOpts {
        bg: Color::TRANSPARENT,
        pad: 0.0,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picker_with_catalog() -> AppPicker {
        let mut picker = AppPicker::new();
        let mut catalog = AppCatalog::default();
        catalog.apps.push(aegis_model::app::Entry {
            id: "firefox.desktop".to_string(),
            name: "Firefox".to_string(),
            ..aegis_model::app::Entry::default()
        });
        catalog.apps.push(aegis_model::app::Entry {
            id: "org.example.Editor.desktop".to_string(),
            name: "Example Editor".to_string(),
            startup_wm_class: Some("example-editor".to_string()),
            ..aegis_model::app::Entry::default()
        });
        picker.update_app_catalog(&catalog);
        picker
    }

    fn params(choices: &[&str], last_choice: Option<&str>) -> AppPickParams {
        AppPickParams {
            choices: choices.iter().map(|c| c.to_string()).collect(),
            subject: None,
            last_choice: last_choice.map(|c| c.to_string()),
        }
    }

    #[test]
    fn choices_resolve_names_from_the_catalog() {
        let mut picker = picker_with_catalog();
        picker.start_app_pick(params(&["firefox.desktop", "unknown.app.desktop"], None));
        assert!(picker.app_pick_active());
        assert_eq!(picker.rows[0].name, "Firefox");
        assert_eq!(picker.rows[1].name, "unknown.app");
    }

    #[test]
    fn startup_wm_class_resolves_to_the_catalog_entry() {
        let mut picker = picker_with_catalog();
        picker.start_app_pick(params(&["example-editor"], None));
        assert_eq!(picker.rows[0].name, "Example Editor");
    }

    #[test]
    fn last_choice_is_preselected_when_present() {
        let mut picker = picker_with_catalog();
        picker.start_app_pick(params(
            &["firefox.desktop", "org.example.Editor.desktop"],
            Some("org.example.Editor.desktop"),
        ));
        assert_eq!(picker.selected, 1);
    }

    #[test]
    fn enter_confirms_the_highlighted_candidate() {
        let mut picker = picker_with_catalog();
        picker.start_app_pick(params(&["firefox.desktop"], None));
        let mut out = ChromeEvents::default();
        picker.key_char(
            &KeyChar {
                keysym: aegis_model::input::XKB_KEY_Return,
                ch: None,
                mods: aegis_model::input::Mods::NONE,
            },
            &mut out,
        );
        assert_eq!(out.app_pick_confirmed.as_deref(), Some("firefox.desktop"));
        assert!(!picker.app_pick_active());
    }

    #[test]
    fn escape_cancels() {
        let mut picker = picker_with_catalog();
        picker.start_app_pick(params(&["firefox.desktop"], None));
        let mut out = ChromeEvents::default();
        picker.key_char(
            &KeyChar {
                keysym: aegis_model::input::XKB_KEY_Escape,
                ch: None,
                mods: aegis_model::input::Mods::NONE,
            },
            &mut out,
        );
        assert!(out.app_pick_cancelled);
        assert!(!picker.app_pick_active());
    }

    #[test]
    fn empty_choices_stay_closed() {
        let mut picker = picker_with_catalog();
        picker.start_app_pick(params(&[], None));
        assert!(!picker.app_pick_active());
    }

    #[test]
    fn the_active_panel_is_one_analytic_glass_body() {
        let mut picker = picker_with_catalog();
        let display = (1280.0, 800.0);
        let workspaces = crate::WorkspaceSnapshot {
            outputs: Vec::new(),
        };
        assert!(
            picker
                .liquid_glass_regions(display, &[], &workspaces)
                .is_empty()
        );
        assert!(!picker.exclusive_presentation_active());

        picker.start_app_pick(params(&["firefox.desktop"], None));
        let backdrop = picker.backdrop_regions(display, &[], &workspaces);
        let glass = picker.liquid_glass_regions(display, &[], &workspaces);
        assert_eq!(backdrop.len(), 1);
        assert_eq!(glass.len(), 1);
        assert_eq!(glass[0].bounds, backdrop[0]);
        assert_eq!(glass[0].corner_radius, Design::dark().radii.glass_panel);
        assert_eq!(glass[0].opacity, 1.0);
        assert!(picker.exclusive_presentation_active());
    }

    #[test]
    fn the_highlighted_row_carries_the_panels_optical_focus() {
        let mut picker = picker_with_catalog();
        picker.start_app_pick(params(
            &["firefox.desktop", "org.example.Editor.desktop"],
            None,
        ));
        let display = (1280.0, 800.0);
        let workspaces = crate::WorkspaceSnapshot {
            outputs: Vec::new(),
        };
        let layout = PickerLayout::for_display(display, Reserved::default(), false);
        let glass = picker.liquid_glass_regions(display, &[], &workspaces);
        let focus = glass[0].focus.expect("the highlighted row is focused");
        assert_eq!(
            focus.bounds,
            BackdropRegion {
                x: layout.list.x,
                y: layout.list.y,
                w: layout.list.w,
                h: ROW_H,
            }
        );
        assert_eq!(focus.corner_radius, Design::dark().radii.menu_item);
        assert_eq!(focus.strength, Design::dark().glass_focus.field_strength);
    }
}
