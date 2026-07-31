//! The user-consent file picker: a modal, centered panel for choosing
//! filesystem paths, backed by the pure
//! [`aegis_core::file_picker::FilePickerModel`] state machine.
//!
//! This is the compositor side of the FileChooser portal: a `PickFile` IPC
//! request opens the panel through [`Chrome::start_file_pick`], and the
//! user's confirm or cancel travels back through
//! [`ChromeEvents::file_pick_confirmed`] /
//! [`ChromeEvents::file_pick_cancelled`]. Unlike the target picker
//! (ADR-0054) the panel never freezes the screen and captures no screen
//! content; it is ordinary modal chrome over the live scene.
//!
//! Rows show plain names only: directories carry a trailing `/` and marked
//! rows an ASCII `[x]`. Symbolic file-type icons are a follow-up once the
//! compositor ships an icon theme for them.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use lens::{Align, Color, Frame, Input, LayoutOpts, OverlayOpts, Rect};

use crate::{BackdropRegion, Chrome, ChromeEvents, CursorShape, Localizer, Reserved, truncate};
use aegis_core::file_picker::{ConfirmOutcome, FilePickMode, FilePickerModel, Filter};
use aegis_core::input::{KeyAction, KeyChar, Mods, key_action};
use aegis_core::window::Window;
use aegis_design::{Design, themes};

const PANEL_W: f32 = 640.0;
const PANEL_PAD: f32 = 16.0;
const TITLE_H: f32 = 24.0;
const PATH_H: f32 = 18.0;
const ROW_H: f32 = 28.0;
const VISIBLE_ROWS: usize = 10;
const FILTER_H: f32 = 22.0;
const FILENAME_H: f32 = 32.0;
const BUTTON_H: f32 = 30.0;
const BUTTON_W: f32 = 88.0;
const BACKDROP_BLUR_SIGMA: f32 = 18.0;
/// Two presses on the same row within this window count as a double-click.
const DOUBLE_CLICK: Duration = Duration::from_millis(400);
/// Rows scrolled per wheel detent over the list.
const WHEEL_ROWS: f32 = 3.0;

/// Parameters of one user-consent file pick, mapped from the IPC options
/// by the compositor runtime.
#[derive(Debug, Clone)]
pub struct FilePickParams {
    pub mode: FilePickMode,
    pub multiple: bool,
    pub directory: bool,
    pub title: Option<String>,
    pub accept_label: Option<String>,
    pub start_dir: PathBuf,
    pub suggested_name: Option<String>,
    /// `(label, patterns)` pairs; patterns are globs or MIME types.
    pub filters: Vec<(String, Vec<String>)>,
}

/// The resolved geometry of the panel for one frame.
#[derive(Debug, Clone, Copy)]
struct PickerLayout {
    panel: Rect,
    title: Rect,
    path: Rect,
    list: Rect,
    filter: Option<Rect>,
    filename: Option<Rect>,
    cancel: Rect,
    accept: Rect,
    visible_rows: usize,
}

impl PickerLayout {
    fn for_display(
        display: (f32, f32),
        reserved: Reserved,
        save: bool,
        has_filters: bool,
    ) -> PickerLayout {
        let left = reserved.left.max(0) as f32;
        let top = reserved.top.max(0) as f32;
        let usable_w = (display.0 - left - reserved.right.max(0) as f32).max(1.0);
        let usable_h = (display.1 - top - reserved.bottom.max(0) as f32).max(1.0);

        let panel_w = PANEL_W.min((usable_w - 32.0).max(240.0));
        let filter_block = if has_filters { FILTER_H + 8.0 } else { 0.0 };
        let filename_block = if save { FILENAME_H + 8.0 } else { 0.0 };
        let fixed = PANEL_PAD
            + TITLE_H
            + 4.0
            + PATH_H
            + 10.0
            + filter_block
            + filename_block
            + 10.0
            + BUTTON_H
            + PANEL_PAD;
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
        let path = Rect {
            x: inner_x,
            y: title.y + title.h + 4.0,
            w: inner_w,
            h: PATH_H,
        };
        let list = Rect {
            x: inner_x,
            y: path.y + path.h + 10.0,
            w: inner_w,
            h: list_h,
        };
        let list_bottom = list.y + list.h;
        let filter = has_filters.then_some(Rect {
            x: inner_x,
            y: list_bottom + 8.0,
            w: inner_w,
            h: FILTER_H,
        });
        let after_filter = filter.map(|r| r.y + r.h).unwrap_or(list_bottom);
        let filename = save.then_some(Rect {
            x: inner_x,
            y: after_filter + 8.0,
            w: inner_w,
            h: FILENAME_H,
        });
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
            path,
            list,
            filter,
            filename,
            cancel,
            accept,
            visible_rows,
        }
    }
}

/// The file-picker chrome component. Inert until the runtime opens it with
/// [`Chrome::start_file_pick`].
pub struct FilePicker {
    active: bool,
    model: Option<FilePickerModel>,
    title: String,
    accept_label: String,
    /// First visible row in the filtered list: the list is a sliding window
    /// over the filtered view, paged by the wheel and by keyboard motion
    /// (the launcher grid's approach rather than a smooth pixel scroll).
    scroll_row: usize,
    /// Fractional wheel accumulator so small axis steps still scroll.
    wheel: f32,
    /// The last row press for double-click detection: (entry index, when).
    last_click: Option<(usize, Instant)>,
    /// Rows the current panel geometry shows; refreshed each render so
    /// keyboard-follow uses the live window height.
    visible_rows: usize,
    /// Edge space reserved by chrome that stays visible during the modal.
    modal_reserved: Reserved,
}

impl FilePicker {
    pub fn new() -> FilePicker {
        FilePicker {
            active: false,
            model: None,
            title: String::new(),
            accept_label: String::new(),
            scroll_row: 0,
            wheel: 0.0,
            last_click: None,
            visible_rows: VISIBLE_ROWS,
            modal_reserved: Reserved::default(),
        }
    }

    /// Whether the panel currently shows the Save-mode filename field.
    fn save_mode(&self) -> bool {
        self.model
            .as_ref()
            .is_some_and(|m| m.mode() == FilePickMode::Save)
    }

    /// Confirm through the model and emit the outcome. A directory
    /// navigation keeps the panel open; paths close it.
    fn confirm(&mut self, out: &mut ChromeEvents) {
        let Some(model) = &mut self.model else {
            return;
        };
        match model.confirm() {
            ConfirmOutcome::Paths(paths) => {
                let filter = (!model.filters().is_empty()).then_some(model.active_filter() as u32);
                out.file_pick_confirmed = Some((paths, filter));
                self.close();
            }
            ConfirmOutcome::Navigated => {
                self.scroll_row = 0;
                self.wheel = 0.0;
            }
            ConfirmOutcome::Ignored => {}
        }
    }

    /// Emit a cancellation and close.
    fn cancel(&mut self, out: &mut ChromeEvents) {
        out.file_pick_cancelled = true;
        self.close();
    }

    fn close(&mut self) {
        self.active = false;
        self.model = None;
        self.last_click = None;
    }

    /// Keep the highlighted row inside the list's scroll window after
    /// keyboard motion.
    fn follow_selection(&mut self) {
        let Some(model) = &self.model else {
            return;
        };
        let Some(selected) = model.selected() else {
            return;
        };
        let filtered = model.filtered_indices();
        let Some(pos) = filtered.iter().position(|&idx| idx == selected) else {
            return;
        };
        if pos < self.scroll_row {
            self.scroll_row = pos;
        } else if pos >= self.scroll_row + self.visible_rows {
            self.scroll_row = pos + 1 - self.visible_rows;
        }
    }

    fn row_label(model: &FilePickerModel, entry_idx: usize) -> String {
        let entry = &model.entries()[entry_idx];
        let mark = if model.multiple() && !entry.is_dir {
            if model.is_marked(entry_idx) {
                "[x] "
            } else {
                "[ ] "
            }
        } else {
            ""
        };
        let suffix = if entry.is_dir { "/" } else { "" };
        truncate(&format!("{mark}{}{suffix}", entry.name), 72)
    }
}

impl Default for FilePicker {
    fn default() -> Self {
        FilePicker::new()
    }
}

impl Chrome for FilePicker {
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
        let Some(model) = &mut self.model else {
            self.active = false;
            return;
        };
        let raw = input.as_raw();
        let display = (raw.display_size.x, raw.display_size.y);
        let cursor = raw.cursor;
        let pressed = raw.mouse_pressed.first().copied().unwrap_or(false);
        let design = Design::dark();
        let save = model.mode() == FilePickMode::Save;
        let layout = PickerLayout::for_display(
            display,
            self.modal_reserved,
            save,
            !model.filters().is_empty(),
        );
        self.visible_rows = layout.visible_rows;

        // Full-screen dimmed scrim; the panel is ordinary modal chrome over
        // the live scene (no freeze, unlike the target picker).
        frame.layer(
            "aegis-file-picker-scrim",
            Rect {
                x: 0.0,
                y: 0.0,
                w: display.0,
                h: display.1,
            },
            &OverlayOpts {
                bg: Color::rgba(8, 10, 18, 118),
                ..Default::default()
            },
            |_| {},
        );

        let original_theme = frame.theme();
        frame.set_theme(themes::application(&design));

        frame.layer(
            "aegis-file-picker-panel",
            layout.panel,
            &OverlayOpts {
                bg: design.colors.application_surface.with_alpha(238),
                border: design.colors.application_border,
                border_width: design.strokes.hairline,
                radius: design.radii.card,
                pad: 0.0,
                ..Default::default()
            },
            |_| {},
        );

        frame.layer(
            "aegis-file-picker-title",
            layout.title,
            &transparent(),
            |frame| {
                frame.row_ex(&stretch(layout.title), |frame| {
                    frame.label_sized(&self.title, 15.0);
                });
            },
        );

        let path_text = truncate(&model.dir().display().to_string(), 84);
        frame.layer(
            "aegis-file-picker-path",
            layout.path,
            &transparent(),
            |frame| {
                frame.row_ex(&stretch(layout.path), |frame| {
                    frame.label_compact_sized(&path_text, 11.5);
                });
            },
        );

        // The entry list: a sliding window over the filtered view.
        let filtered = model.filtered_indices();
        let max_scroll = filtered.len().saturating_sub(layout.visible_rows);
        self.scroll_row = self.scroll_row.min(max_scroll);
        let mut clicked_row = None;
        for pos in 0..layout.visible_rows {
            let row_index = self.scroll_row + pos;
            if row_index >= filtered.len() {
                break;
            }
            let entry_idx = filtered[row_index];
            let rect = Rect {
                x: layout.list.x,
                y: layout.list.y + pos as f32 * ROW_H,
                w: layout.list.w,
                h: ROW_H,
            };
            let hovered = contains(rect, cursor.x, cursor.y);
            if pressed && hovered {
                clicked_row = Some(entry_idx);
            }
            let selected = model.selected() == Some(entry_idx);
            let bg = if selected {
                design.colors.application_active
            } else if hovered {
                design.colors.application_hover
            } else {
                Color::TRANSPARENT
            };
            let label = Self::row_label(model, entry_idx);
            let id = format!("aegis-file-picker-row-{pos}");
            frame.layer(
                &id,
                rect,
                &OverlayOpts {
                    bg,
                    radius: design.radii.menu_item,
                    pad: 0.0,
                    ..Default::default()
                },
                |frame| {
                    frame.row_ex(&stretch(rect), |frame| {
                        frame.spacer(10.0);
                        frame.label_compact_sized(&label, 13.0);
                    });
                },
            );
        }

        // A directory read failure keeps the previous listing and is
        // surfaced here; an empty visible set gets a quiet placeholder.
        if let Some(error) = model.error() {
            let error = truncate(error, 96);
            frame.layer(
                "aegis-file-picker-error",
                layout.list,
                &transparent(),
                |frame| {
                    frame.row_ex(&stretch(layout.list), |frame| {
                        frame.spacer(10.0);
                        frame.label_compact_sized(&error, 12.0);
                    });
                },
            );
        } else if filtered.is_empty() {
            frame.layer(
                "aegis-file-picker-empty",
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
                            frame.label_sized("Empty folder", 13.0);
                        },
                    );
                },
            );
        }

        // A proportional scroll thumb once the listing overflows the window.
        if filtered.len() > layout.visible_rows {
            let track_h = layout.list.h;
            let thumb_h = (layout.visible_rows as f32 / filtered.len() as f32 * track_h).max(16.0);
            let thumb_y = layout.list.y
                + if max_scroll == 0 {
                    0.0
                } else {
                    self.scroll_row as f32 / max_scroll as f32 * (track_h - thumb_h)
                };
            frame.layer(
                "aegis-file-picker-scroll-thumb",
                Rect {
                    x: layout.list.x + layout.list.w - design.strokes.scrollbar - 2.0,
                    y: thumb_y,
                    w: design.strokes.scrollbar,
                    h: thumb_h,
                },
                &OverlayOpts {
                    bg: design.colors.slider_fill.with_alpha(140),
                    radius: design.radii.scrollbar,
                    pad: 0.0,
                    ..Default::default()
                },
                |_| {},
            );
        }

        if let Some(filter_rect) = layout.filter {
            let label_text = model
                .filters()
                .get(model.active_filter())
                .map(|f| f.label.clone())
                .unwrap_or_default();
            let text = truncate(&format!("Filter: {label_text}  (Tab to change)"), 84);
            frame.layer(
                "aegis-file-picker-filter",
                filter_rect,
                &transparent(),
                |frame| {
                    frame.row_ex(&stretch(filter_rect), |frame| {
                        frame.spacer(2.0);
                        frame.label_compact_sized(&text, 11.5);
                    });
                },
            );
        }

        if let Some(filename_rect) = layout.filename {
            let text = model.filename().to_owned();
            let metrics = frame.measure_text(&text, 14.0);
            let font_metrics = frame.measure_text("Ag", 14.0);
            frame.layer(
                "aegis-file-picker-filename",
                filename_rect,
                &OverlayOpts {
                    bg: design.colors.card_surface,
                    border: design.colors.application_accent,
                    border_width: design.strokes.hairline,
                    radius: design.radii.control,
                    pad: 0.0,
                    ..Default::default()
                },
                |frame| {
                    frame.row_ex(
                        &LayoutOpts {
                            width: filename_rect.w,
                            height: filename_rect.h,
                            cross: Align::Center,
                            ..Default::default()
                        },
                        |frame| {
                            frame.spacer(12.0);
                            frame.label_compact_sized(&text, 14.0);
                        },
                    );
                },
            );
            // The caret is compositor-owned like the launcher's: the field's
            // text lives in the model, not in a lens text widget.
            frame.layer(
                "aegis-file-picker-caret",
                Rect {
                    x: filename_rect.x + 12.0 + metrics.width,
                    y: filename_rect.y + (filename_rect.h - font_metrics.height) * 0.5,
                    w: 2.0,
                    h: font_metrics.height,
                },
                &OverlayOpts {
                    bg: design.colors.application_text,
                    pad: 0.0,
                    ..Default::default()
                },
                |_| {},
            );
        }

        let cancel_hovered = contains(layout.cancel, cursor.x, cursor.y);
        let accept_hovered = contains(layout.accept, cursor.x, cursor.y);
        let clicked_cancel = pressed && cancel_hovered;
        let clicked_accept = pressed && accept_hovered;
        frame.layer(
            "aegis-file-picker-cancel",
            layout.cancel,
            &OverlayOpts {
                bg: if cancel_hovered {
                    design.colors.application_hover
                } else {
                    design.colors.card_surface
                },
                border: design.colors.application_border,
                border_width: design.strokes.hairline,
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
            "aegis-file-picker-accept",
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
                    frame.label_sized(&self.accept_label.clone(), 13.0);
                });
            },
        );

        // The shell shares one lens frame across every chrome component.
        frame.set_theme(original_theme);

        // Interaction edges, applied after the frame's hit-testing above.
        if pressed && !contains(layout.panel, cursor.x, cursor.y) {
            // Click-away dismisses, like the modal application helper.
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
        if let Some(entry_idx) = clicked_row {
            let now = Instant::now();
            let double = self.last_click.is_some_and(|(prev, at)| {
                prev == entry_idx && now.duration_since(at) <= DOUBLE_CLICK
            });
            self.last_click = Some((entry_idx, now));
            let Some(model) = &mut self.model else {
                return;
            };
            model.select(entry_idx);
            if double {
                self.last_click = None;
                if model.entries()[entry_idx].is_dir {
                    if model.enter_selected_dir() {
                        self.scroll_row = 0;
                        self.wheel = 0.0;
                    }
                } else {
                    self.confirm(out);
                    return;
                }
            }
        }

        // Wheel over the list slides the row window.
        let wheel = raw.scroll_y * WHEEL_ROWS + raw.scroll_pixels_y / ROW_H;
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
        let save = self.save_mode();
        let has_filters = self.model.as_ref().is_some_and(|m| !m.filters().is_empty());
        let layout = PickerLayout::for_display(display, self.modal_reserved, save, has_filters);
        Some(
            if contains(layout.accept, x, y) || contains(layout.cancel, x, y) {
                CursorShape::Pointer
            } else if layout.filename.is_some_and(|rect| contains(rect, x, y)) {
                CursorShape::Text
            } else {
                CursorShape::Default
            },
        )
    }

    fn set_modal_reserved(&mut self, reserved: Reserved) {
        self.modal_reserved = reserved;
    }

    fn key_char(&mut self, key: &KeyChar, out: &mut ChromeEvents) {
        if !self.active {
            return;
        }
        // Ctrl+H toggles dotfiles in every mode, before the Save-mode text
        // path below can claim the character.
        if key.mods.has(Mods::CTRL) && matches!(key.ch, Some('h') | Some('H')) {
            if let Some(model) = &mut self.model {
                model.toggle_hidden();
            }
            self.scroll_row = 0;
            self.wheel = 0.0;
            return;
        }
        let save = self
            .model
            .as_ref()
            .is_some_and(|m| m.mode() == FilePickMode::Save);
        let action = key_action(key.keysym, key.ch);
        match action {
            KeyAction::Up => {
                if let Some(model) = &mut self.model {
                    model.move_selection(-1);
                }
                self.follow_selection();
            }
            KeyAction::Down => {
                if let Some(model) = &mut self.model {
                    model.move_selection(1);
                }
                self.follow_selection();
            }
            KeyAction::Enter => self.confirm(out),
            KeyAction::Escape => self.cancel(out),
            KeyAction::Tab => {
                let delta = if key.mods.has(Mods::SHIFT) { -1 } else { 1 };
                if let Some(model) = &mut self.model {
                    model.cycle_filter(delta);
                }
                self.scroll_row = 0;
                self.wheel = 0.0;
            }
            KeyAction::Backspace if save => {
                if let Some(model) = &mut self.model {
                    model.apply_key(KeyAction::Backspace);
                }
            }
            // Outside Save mode Backspace is the keyboard's way up.
            KeyAction::Backspace => {
                if let Some(model) = &mut self.model {
                    model.go_parent();
                }
                self.scroll_row = 0;
                self.wheel = 0.0;
            }
            // Space toggles a mark in multi-open; in Save mode it is an
            // ordinary filename character.
            KeyAction::Char(' ') if !save => {
                if let Some(model) = &mut self.model {
                    model.toggle_mark();
                }
            }
            KeyAction::Char(_) if save => {
                if let Some(model) = &mut self.model {
                    model.apply_key(action);
                }
            }
            _ => {}
        }
    }

    fn start_file_pick(&mut self, params: FilePickParams) {
        let folder = params.directory || params.mode == FilePickMode::ChooseDir;
        self.title = params.title.unwrap_or_else(|| match params.mode {
            FilePickMode::Open if folder => "Choose Folder".to_owned(),
            FilePickMode::Open if params.multiple => "Open Files".to_owned(),
            FilePickMode::Open => "Open File".to_owned(),
            FilePickMode::Save => "Save File".to_owned(),
            FilePickMode::ChooseDir => "Choose Folder".to_owned(),
        });
        self.accept_label = params.accept_label.unwrap_or_else(|| match params.mode {
            FilePickMode::Open if folder => "Select".to_owned(),
            FilePickMode::Open => "Open".to_owned(),
            FilePickMode::Save => "Save".to_owned(),
            FilePickMode::ChooseDir => "Select".to_owned(),
        });
        let filters = params
            .filters
            .into_iter()
            .map(|(label, patterns)| Filter { label, patterns })
            .collect();
        self.model = Some(FilePickerModel::new(
            params.mode,
            params.multiple,
            params.directory,
            params.start_dir,
            params.suggested_name,
            filters,
        ));
        self.scroll_row = 0;
        self.wheel = 0.0;
        self.last_click = None;
        self.active = true;
    }

    fn cancel_file_pick(&mut self) {
        if self.active {
            self.close();
        }
    }

    fn file_pick_active(&self) -> bool {
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
        let save = self
            .model
            .as_ref()
            .is_some_and(|m| m.mode() == FilePickMode::Save);
        let has_filters = self.model.as_ref().is_some_and(|m| !m.filters().is_empty());
        let layout = PickerLayout::for_display(display, self.modal_reserved, save, has_filters);
        // Approximate the rounded panel with two rectangles for shared
        // backdrop capture (the modal helper's approach), avoiding a
        // full-output blur request.
        let panel = layout.panel;
        let radius = Design::dark().radii.card;
        vec![
            BackdropRegion {
                x: panel.x + radius,
                y: panel.y,
                w: (panel.w - radius * 2.0).max(0.0),
                h: panel.h,
            },
            BackdropRegion {
                x: panel.x,
                y: panel.y + radius,
                w: panel.w,
                h: (panel.h - radius * 2.0).max(0.0),
            },
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

    fn params(mode: FilePickMode, start_dir: PathBuf) -> FilePickParams {
        FilePickParams {
            mode,
            multiple: false,
            directory: false,
            title: None,
            accept_label: None,
            start_dir,
            suggested_name: None,
            filters: Vec::new(),
        }
    }

    #[test]
    fn start_file_pick_opens_with_per_mode_defaults() {
        let mut picker = FilePicker::new();
        assert!(!picker.file_pick_active());
        picker.start_file_pick(params(FilePickMode::Open, PathBuf::from("/")));
        assert!(picker.file_pick_active());
        assert_eq!(picker.title, "Open File");
        assert_eq!(picker.accept_label, "Open");
        picker.cancel_file_pick();
        assert!(!picker.file_pick_active());

        picker.start_file_pick(params(FilePickMode::Save, PathBuf::from("/")));
        assert_eq!(picker.title, "Save File");
        assert_eq!(picker.accept_label, "Save");
        picker.cancel_file_pick();

        picker.start_file_pick(params(FilePickMode::ChooseDir, PathBuf::from("/")));
        assert_eq!(picker.title, "Choose Folder");
        assert_eq!(picker.accept_label, "Select");
        picker.cancel_file_pick();
    }

    #[test]
    fn supplied_title_and_accept_label_win() {
        let mut picker = FilePicker::new();
        let mut p = params(FilePickMode::Open, PathBuf::from("/"));
        p.title = Some("Import a thing".into());
        p.accept_label = Some("Import".into());
        picker.start_file_pick(p);
        assert_eq!(picker.title, "Import a thing");
        assert_eq!(picker.accept_label, "Import");
    }

    #[test]
    fn escape_emits_a_cancellation() {
        let mut picker = FilePicker::new();
        picker.start_file_pick(params(FilePickMode::Open, PathBuf::from("/")));
        let mut out = ChromeEvents::default();
        picker.key_char(
            &KeyChar {
                keysym: aegis_core::input::XKB_KEY_Escape,
                ch: None,
                mods: Mods::NONE,
            },
            &mut out,
        );
        assert!(out.file_pick_cancelled);
        assert!(!picker.file_pick_active());
    }

    #[test]
    fn enter_confirms_the_highlighted_path() {
        // A directory we can rely on: the temp dir exists and holds files
        // from the aegis-core model tests rarely; navigate-confirm instead.
        let mut picker = FilePicker::new();
        picker.start_file_pick(params(FilePickMode::ChooseDir, PathBuf::from("/")));
        let mut out = ChromeEvents::default();
        picker.key_char(
            &KeyChar {
                keysym: aegis_core::input::XKB_KEY_Return,
                ch: None,
                mods: Mods::NONE,
            },
            &mut out,
        );
        let (paths, filter) = out.file_pick_confirmed.expect("a directory pick confirms");
        assert_eq!(paths.len(), 1);
        assert!(paths[0].is_absolute());
        assert_eq!(filter, None, "no filters were supplied");
        assert!(!picker.file_pick_active());
    }

    #[test]
    fn ctrl_h_toggles_hidden_files() {
        let mut picker = FilePicker::new();
        picker.start_file_pick(params(FilePickMode::Open, PathBuf::from("/")));
        let before = picker.model.as_ref().unwrap().show_hidden();
        let mut out = ChromeEvents::default();
        picker.key_char(
            &KeyChar {
                keysym: 0,
                ch: Some('h'),
                mods: Mods::CTRL,
            },
            &mut out,
        );
        assert_ne!(picker.model.as_ref().unwrap().show_hidden(), before);
        assert!(!out.file_pick_cancelled);
    }

    #[test]
    fn layout_stays_inside_small_outputs_and_reserved_edges() {
        let reserved = Reserved {
            top: 32,
            bottom: 64,
            ..Reserved::default()
        };
        let layout = PickerLayout::for_display((360.0, 480.0), reserved, true, true);
        assert!(layout.panel.x >= 0.0 && layout.panel.y >= 32.0);
        assert!(layout.panel.x + layout.panel.w <= 360.0);
        assert!(layout.panel.y + layout.panel.h <= 480.0 - 64.0);
        assert!(layout.visible_rows >= 1);
        // The buttons sit at the panel's bottom padding.
        assert_eq!(
            layout.accept.y + layout.accept.h,
            layout.panel.y + layout.panel.h - PANEL_PAD
        );
    }
}
