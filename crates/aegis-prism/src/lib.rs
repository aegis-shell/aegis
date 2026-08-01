//! Prism is a compact, Spotlight-style application search surface.
//!
//! The component owns only search presentation and interaction state. It
//! receives the shared application catalog and borrowed icon handles through
//! [`Chrome::update_app_catalog`], then emits launch or focus intents through
//! [`ChromeEvents`]. Process creation and Wayland focus remain in the
//! compositor composition root.

use std::ffi::c_void;
use std::ops::Range;

use aegis_core::app::Entry;
use aegis_core::input::{KeyChar, key_action};
use aegis_core::launcher::{Launch, Launcher as SearchBrain};
use aegis_core::window::Window;
use aegis_core::workspace::WorkspaceSnapshot;
use aegis_design::{Design, materials};
use aegis_shell::{
    AppCatalog, BackdropRegion, Chrome, ChromeEvents, CursorShape, IconSet, LiquidGlassRegion,
    Localizer, Message, truncate,
};
use lens::{Align, Color, Frame, Icon, Input, LayoutOpts, OverlayOpts, Rect, Theme};

const PANEL_MAX_WIDTH: f32 = 680.0;
const PANEL_SIDE_MARGIN: f32 = 20.0;
const PANEL_TOP_MIN: f32 = 64.0;
const PANEL_TOP_FRACTION: f32 = 0.14;
const SEARCH_HEIGHT: f32 = 70.0;
const RESULT_HEIGHT: f32 = 58.0;
const EMPTY_HEIGHT: f32 = 72.0;
const MAX_VISIBLE_RESULTS: usize = 6;
const ICON_SIZE: f32 = 38.0;
const BACKDROP_BLUR_SIGMA: f32 = 18.0;
const ANIMATION_SPEED: f32 = 22.0;
const PANEL_RADIUS: f32 = 18.0;

/// Spotlight-style application search, opened by the default
/// `Super+Space` binding.
pub struct Prism {
    brain: SearchBrain,
    icons: IconSet,
    visibility: f32,
    anim_active: bool,
    prev_down: bool,
    reduced_motion: bool,
}

impl Prism {
    /// Construct an empty Prism component. The shell seeds the catalog when
    /// the component is registered.
    #[must_use]
    pub fn new() -> Self {
        Self {
            brain: SearchBrain::new(Vec::new()),
            icons: IconSet::default(),
            visibility: 0.0,
            anim_active: false,
            prev_down: false,
            reduced_motion: false,
        }
    }

    fn advance_visibility(&mut self, target: f32, dt: f32) -> f32 {
        if self.reduced_motion {
            self.visibility = target;
            self.anim_active = false;
            return target;
        }
        let blend = 1.0 - (-ANIMATION_SPEED * dt.clamp(0.0, 1.0 / 30.0)).exp();
        self.visibility += (target - self.visibility) * blend;
        self.anim_active = (self.visibility - target).abs() > 0.002;
        if !self.anim_active {
            self.visibility = target;
        }
        self.visibility.clamp(0.0, 1.0)
    }

    fn entry_icon(&self, entry: &Entry) -> Option<*mut c_void> {
        let get = |key: &str| {
            let key = key.to_ascii_lowercase();
            (!key.is_empty()).then(|| self.icons.get(&key)).flatten()
        };
        entry
            .startup_wm_class
            .as_deref()
            .and_then(get)
            .or_else(|| get(entry.id.strip_suffix(".desktop").unwrap_or(&entry.id)))
            .or_else(|| entry.icon.as_deref().and_then(get))
    }

    fn emit(outcome: Option<Launch>, out: &mut ChromeEvents) {
        match outcome {
            Some(Launch::Spawn(entry)) => out.activate_entry(*entry),
            Some(Launch::Focus(window)) => out.clicked = Some(window),
            Some(Launch::BuiltIn(app)) => out.open_builtin = Some(app),
            None => {}
        }
    }

    fn top(display: (f32, f32)) -> f32 {
        (display.1 * PANEL_TOP_FRACTION)
            .max(PANEL_TOP_MIN)
            .min((display.1 - SEARCH_HEIGHT - PANEL_SIDE_MARGIN).max(PANEL_SIDE_MARGIN))
    }

    fn result_capacity(display: (f32, f32)) -> usize {
        let available =
            (display.1 - Self::top(display) - SEARCH_HEIGHT - PANEL_SIDE_MARGIN).max(0.0);
        ((available / RESULT_HEIGHT).floor() as usize).min(MAX_VISIBLE_RESULTS)
    }

    fn panel_rect(display: (f32, f32), result_count: usize, progress: f32) -> Rect {
        let width = PANEL_MAX_WIDTH.min((display.0 - PANEL_SIDE_MARGIN * 2.0).max(1.0));
        let rows = result_count.min(Self::result_capacity(display));
        let results_height = if rows == 0 {
            EMPTY_HEIGHT
        } else {
            rows as f32 * RESULT_HEIGHT
        };
        Rect {
            x: (display.0 - width) * 0.5,
            y: Self::top(display) - (1.0 - progress.clamp(0.0, 1.0)) * 12.0,
            w: width,
            h: (SEARCH_HEIGHT + results_height)
                .min((display.1 - Self::top(display) - PANEL_SIDE_MARGIN).max(1.0)),
        }
    }

    fn visible_range(total: usize, selection: usize, capacity: usize) -> Range<usize> {
        if capacity == 0 {
            return 0..0;
        }
        if total <= capacity {
            return 0..total;
        }
        let start = selection
            .saturating_sub(capacity - 1)
            .min(total.saturating_sub(capacity));
        start..start + capacity
    }

    fn row_rect(panel: Rect, visible_position: usize) -> Rect {
        Rect {
            x: panel.x,
            y: panel.y + SEARCH_HEIGHT + visible_position as f32 * RESULT_HEIGHT,
            w: panel.w,
            h: RESULT_HEIGHT,
        }
    }

    fn close(&mut self) {
        if self.brain.is_open() {
            self.brain.close();
            self.anim_active = true;
        }
    }
}

impl Default for Prism {
    fn default() -> Self {
        Self::new()
    }
}

impl Chrome for Prism {
    fn render(
        &mut self,
        frame: &mut Frame,
        input: &Input,
        windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
        i18n: &Localizer,
        out: &mut ChromeEvents,
    ) {
        let raw = input.as_raw();
        let display = raw.display_size;
        let down = raw.mouse_down.first().copied().unwrap_or(false);
        let pressed = down && !self.prev_down;

        let running = windows
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
            self.prev_down = down;
            return;
        }

        let filtered = self.brain.filtered().clone();
        let selection = self.brain.selection();
        let capacity = Self::result_capacity((display.x, display.y));
        let range = Self::visible_range(filtered.len(), selection, capacity);
        let visible_indices = filtered[range.clone()].to_vec();
        let panel = Self::panel_rect((display.x, display.y), filtered.len(), progress);
        let cursor = raw.cursor;

        if pressed && self.brain.is_open() && !contains(panel, cursor.x, cursor.y) {
            self.close();
        }

        let original_theme = frame.theme();
        frame.set_theme(faded_theme(original_theme, progress));
        frame.layer(
            "aegis-prism-scrim",
            Rect {
                x: 0.0,
                y: 0.0,
                w: display.x,
                h: display.y,
            },
            &OverlayOpts {
                bg: Color::rgba(4, 6, 14, alpha(54, progress)),
                pad: 0.0,
                ..Default::default()
            },
            |_| {},
        );

        let design = Design::dark();
        let mut panel_material = materials::dock(&design);
        panel_material.bg = Color::rgba(255, 255, 255, alpha(12, progress));
        panel_material.radius = PANEL_RADIUS;
        frame.layer("aegis-prism-panel", panel, &panel_material, |_| {});

        let search = Rect {
            x: panel.x,
            y: panel.y,
            w: panel.w,
            h: SEARCH_HEIGHT,
        };
        let shown_query = truncate(self.brain.query(), 72);
        let query_metrics = frame.measure_text(&shown_query, 20.0);
        frame.layer(
            "aegis-prism-search",
            search,
            &OverlayOpts {
                bg: Color::TRANSPARENT,
                pad: 0.0,
                cross: Align::Center,
                ..Default::default()
            },
            |frame| {
                frame.row_ex(
                    &LayoutOpts {
                        width: search.w,
                        height: search.h,
                        gap: 12.0,
                        pad: 20.0,
                        cross: Align::Center,
                        ..Default::default()
                    },
                    |frame| {
                        frame.icon(Icon::Search, 23.0);
                        if shown_query.is_empty() {
                            frame.label_compact_sized(i18n.text(Message::SearchApplications), 20.0);
                        } else {
                            frame.label_compact_sized(&shown_query, 20.0);
                        }
                    },
                );
            },
        );
        if self.brain.is_open() {
            frame.layer(
                "aegis-prism-caret",
                Rect {
                    x: (search.x + 55.0 + query_metrics.width).min(search.x + search.w - 20.0),
                    y: search.y + 22.0,
                    w: 2.0,
                    h: 26.0,
                },
                &OverlayOpts {
                    bg: Color::rgba(242, 245, 255, alpha(235, progress)),
                    radius: 1.0,
                    pad: 0.0,
                    ..Default::default()
                },
                |_| {},
            );
        }
        frame.layer(
            "aegis-prism-divider",
            Rect {
                x: panel.x + 16.0,
                y: panel.y + SEARCH_HEIGHT - 1.0,
                w: (panel.w - 32.0).max(0.0),
                h: 1.0,
            },
            &OverlayOpts {
                bg: Color::rgba(255, 255, 255, alpha(38, progress)),
                pad: 0.0,
                ..Default::default()
            },
            |_| {},
        );

        let mut clicked = None;
        if filtered.is_empty() {
            frame.layer(
                "aegis-prism-empty",
                Rect {
                    x: panel.x,
                    y: panel.y + SEARCH_HEIGHT,
                    w: panel.w,
                    h: (panel.h - SEARCH_HEIGHT).max(1.0),
                },
                &OverlayOpts {
                    bg: Color::TRANSPARENT,
                    pad: 0.0,
                    cross: Align::Center,
                    ..Default::default()
                },
                |frame| {
                    frame.column_ex(
                        &LayoutOpts {
                            width: panel.w,
                            height: EMPTY_HEIGHT,
                            cross: Align::Center,
                            ..Default::default()
                        },
                        |frame| {
                            frame.flex(1.0);
                            frame
                                .label_compact_sized(i18n.text(Message::NoApplicationsFound), 13.0);
                            frame.flex(1.0);
                        },
                    );
                },
            );
        } else {
            for (visible_position, app_index) in visible_indices.iter().copied().enumerate() {
                let filtered_position = range.start + visible_position;
                let entry = &self.brain.apps()[app_index];
                let row = Self::row_rect(panel, visible_position);
                let hovered = self.brain.is_open() && contains(row, cursor.x, cursor.y);
                if pressed && hovered {
                    clicked = Some(filtered_position);
                }
                let selected = filtered_position == selection;
                let icon = self.entry_icon(entry);
                let name = truncate(&entry.name, 48);
                let subtitle = entry
                    .generic_name
                    .as_deref()
                    .or(entry.comment.as_deref())
                    .map(|text| truncate(text, 64));
                let running = self.brain.is_running(app_index);
                frame.layer(
                    &format!("aegis-prism-result-{filtered_position}"),
                    row,
                    &OverlayOpts {
                        bg: if selected {
                            Color::rgba(106, 155, 255, alpha(76, progress))
                        } else if hovered {
                            Color::rgba(255, 255, 255, alpha(24, progress))
                        } else {
                            Color::TRANSPARENT
                        },
                        pad: 0.0,
                        ..Default::default()
                    },
                    |frame| {
                        frame.row_ex(
                            &LayoutOpts {
                                width: row.w,
                                height: row.h,
                                gap: 12.0,
                                pad: 10.0,
                                cross: Align::Center,
                                ..Default::default()
                            },
                            |frame| {
                                render_icon(frame, icon, progress);
                                frame.column_ex(
                                    &LayoutOpts {
                                        width: (row.w - 92.0).max(1.0),
                                        height: 42.0,
                                        gap: 2.0,
                                        ..Default::default()
                                    },
                                    |frame| {
                                        frame.label_compact_sized(&name, 13.5);
                                        if let Some(subtitle) = &subtitle {
                                            frame.label_compact_sized(subtitle, 10.5);
                                        }
                                    },
                                );
                                frame.flex(1.0);
                                if running {
                                    frame.label_compact_sized("●", 9.0);
                                }
                            },
                        );
                    },
                );
            }
        }
        frame.set_theme(original_theme);

        if let Some(filtered_position) = clicked {
            Self::emit(self.brain.launch_filtered(filtered_position), out);
            self.anim_active = true;
        }
        self.prev_down = down;
    }

    fn captures_keyboard(&self) -> bool {
        self.brain.is_open()
    }

    fn key_char(&mut self, key: &KeyChar, out: &mut ChromeEvents) {
        if !self.brain.is_open() {
            return;
        }
        let outcome = self.brain.handle(key_action(key.keysym, key.ch));
        if outcome.is_some() || !self.brain.is_open() {
            self.anim_active = true;
        }
        Self::emit(outcome, out);
    }

    fn captures_pointer(
        &self,
        _x: f32,
        _y: f32,
        _display: (f32, f32),
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) -> bool {
        self.brain.is_open() || self.visibility > 0.01
    }

    fn cursor_shape_at(
        &self,
        x: f32,
        y: f32,
        display: (f32, f32),
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) -> Option<CursorShape> {
        let count = self.brain.filtered().len();
        let panel = Self::panel_rect(display, count, self.visibility);
        if !contains(panel, x, y) {
            return Some(CursorShape::Default);
        }
        if y < panel.y + SEARCH_HEIGHT {
            Some(CursorShape::Text)
        } else {
            Some(CursorShape::Pointer)
        }
    }

    fn modal_active(&self) -> bool {
        self.brain.is_open() || self.visibility > 0.01
    }

    fn visible_during_modal(&self) -> bool {
        true
    }

    fn toggle_prism(&mut self, _out: &mut ChromeEvents) {
        self.brain.toggle();
        self.anim_active = true;
    }

    fn prism_active(&self) -> bool {
        self.brain.is_open()
    }

    fn close_prism(&mut self) {
        self.brain.close();
        self.visibility = 0.0;
        self.anim_active = false;
    }

    fn update_app_catalog(&mut self, catalog: &AppCatalog) {
        self.brain.replace_apps(catalog.apps.clone());
        self.icons = catalog.icons.clone();
    }

    fn set_reduced_motion(&mut self, reduced: bool) {
        self.reduced_motion = reduced;
    }

    fn anim_pending(&self) -> bool {
        self.anim_active
            || if self.brain.is_open() {
                (self.visibility - 1.0).abs() > 0.002
            } else {
                self.visibility > 0.002
            }
    }

    fn requires_composition(&self) -> bool {
        self.brain.is_open() || self.visibility > 0.01
    }

    fn backdrop_blur_sigma(&self) -> f32 {
        if self.brain.is_open() || self.visibility > 0.01 {
            BACKDROP_BLUR_SIGMA
        } else {
            0.0
        }
    }

    fn backdrop_regions(
        &self,
        display: (f32, f32),
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) -> Vec<BackdropRegion> {
        if !self.brain.is_open() && self.visibility <= 0.01 {
            return Vec::new();
        }
        let panel = Self::panel_rect(display, self.brain.filtered().len(), self.visibility);
        vec![BackdropRegion {
            x: panel.x,
            y: panel.y,
            w: panel.w,
            h: panel.h,
        }]
    }

    fn liquid_glass_regions(
        &self,
        display: (f32, f32),
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) -> Vec<LiquidGlassRegion> {
        if !self.brain.is_open() && self.visibility <= 0.01 {
            return Vec::new();
        }
        let panel = Self::panel_rect(display, self.brain.filtered().len(), self.visibility);
        vec![LiquidGlassRegion {
            bounds: BackdropRegion {
                x: panel.x,
                y: panel.y,
                w: panel.w,
                h: panel.h,
            },
            corner_radius: PANEL_RADIUS,
            opacity: self.visibility,
            shadow_alpha: 0.20,
            shadow_blur: 18.0,
            shadow_offset_y: 9.0,
        }]
    }
}

fn render_icon(frame: &mut Frame, icon: Option<*mut c_void>, progress: f32) {
    let size = ICON_SIZE * (0.92 + progress.clamp(0.0, 1.0) * 0.08);
    frame.column_ex(
        &LayoutOpts {
            width: ICON_SIZE,
            height: ICON_SIZE,
            cross: Align::Center,
            ..Default::default()
        },
        |frame| match icon {
            Some(pointer) => unsafe {
                frame.image(pointer as *mut lens::sys::flux_image, size, size);
            },
            None => {
                frame.column_ex(
                    &LayoutOpts {
                        width: size,
                        height: size,
                        bg: Color::rgba(78, 88, 120, alpha(230, progress)),
                        radius: 9.0,
                        cross: Align::Center,
                        ..Default::default()
                    },
                    |frame| {
                        frame.spacer(8.0);
                        frame.icon(Icon::FileText, 20.0);
                    },
                );
            }
        },
    );
}

fn contains(rect: Rect, x: f32, y: f32) -> bool {
    x >= rect.x && y >= rect.y && x < rect.x + rect.w && y < rect.y + rect.h
}

fn alpha(base: u8, progress: f32) -> u8 {
    (base as f32 * progress.clamp(0.0, 1.0)).round() as u8
}

fn with_progress(color: Color, progress: f32) -> Color {
    let (_, _, _, opacity) = color.components();
    color.with_alpha(alpha(opacity, progress))
}

fn faded_theme(theme: Theme, progress: f32) -> Theme {
    let fade = |color: Color| with_progress(color, progress);
    theme
        .with_fg(fade(theme.fg()))
        .with_accent(fade(theme.accent()))
        .with_border(fade(theme.border()))
        .with_hover(fade(theme.hover()))
        .with_active(fade(theme.active()))
        .with_disabled(fade(theme.disabled()))
        .with_error(fade(theme.error()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, name: &str) -> Entry {
        Entry {
            id: id.into(),
            name: name.into(),
            ..Entry::default()
        }
    }

    #[test]
    fn result_window_keeps_selection_visible() {
        assert_eq!(Prism::visible_range(3, 2, 6), 0..3);
        assert_eq!(Prism::visible_range(10, 0, 6), 0..6);
        assert_eq!(Prism::visible_range(10, 6, 6), 1..7);
        assert_eq!(Prism::visible_range(10, 9, 6), 4..10);
    }

    #[test]
    fn panel_stays_inside_small_outputs() {
        let panel = Prism::panel_rect((320.0, 240.0), 10, 1.0);
        assert!(panel.x >= 0.0);
        assert!(panel.y >= 0.0);
        assert!(panel.x + panel.w <= 320.0);
        assert!(panel.y + panel.h <= 240.0);

        let tiny = Prism::panel_rect((160.0, 100.0), 10, 1.0);
        assert!(tiny.x >= 0.0);
        assert!(tiny.y >= 0.0);
        assert!(tiny.x + tiny.w <= 160.0);
        assert!(tiny.y + tiny.h <= 100.0);
        assert_eq!(Prism::result_capacity((160.0, 100.0)), 0);
    }

    #[test]
    fn open_panel_is_one_analytic_glass_body() {
        let mut prism = Prism::new();
        prism.toggle_prism(&mut ChromeEvents::default());
        prism.visibility = 0.75;
        let display = (1280.0, 720.0);
        let workspaces = WorkspaceSnapshot {
            outputs: Vec::new(),
        };

        let backdrop = prism.backdrop_regions(display, &[], &workspaces);
        let glass = prism.liquid_glass_regions(display, &[], &workspaces);
        assert_eq!(backdrop.len(), 1);
        assert_eq!(glass.len(), 1);
        assert_eq!(glass[0].bounds, backdrop[0]);
        assert_eq!(glass[0].corner_radius, PANEL_RADIUS);
        assert_eq!(glass[0].opacity, 0.75);
    }

    #[test]
    fn toggle_and_escape_control_prism_only() {
        let mut prism = Prism::new();
        prism.update_app_catalog(&AppCatalog {
            apps: vec![entry("alpha.desktop", "Alpha")],
            ..AppCatalog::default()
        });
        prism.toggle_prism(&mut ChromeEvents::default());
        assert!(prism.prism_active());
        prism.key_char(
            &KeyChar {
                keysym: b'x' as u32,
                ch: Some('x'),
                mods: aegis_core::input::Mods::NONE,
            },
            &mut ChromeEvents::default(),
        );
        assert_eq!(prism.brain.query(), "x");
        prism.key_char(
            &KeyChar {
                keysym: aegis_core::input::XKB_KEY_Escape,
                ch: None,
                mods: aegis_core::input::Mods::NONE,
            },
            &mut ChromeEvents::default(),
        );
        assert!(!prism.prism_active());
        assert!(prism.brain.query().is_empty());
    }

    #[test]
    fn enter_emits_selected_application() {
        let mut prism = Prism::new();
        prism.update_app_catalog(&AppCatalog {
            apps: vec![entry("alpha.desktop", "Alpha")],
            ..AppCatalog::default()
        });
        prism.toggle_prism(&mut ChromeEvents::default());
        let mut events = ChromeEvents::default();
        prism.key_char(
            &KeyChar {
                keysym: aegis_core::input::XKB_KEY_Return,
                ch: None,
                mods: aegis_core::input::Mods::NONE,
            },
            &mut events,
        );
        assert_eq!(
            events.spawn.as_ref().map(|entry| entry.id.as_str()),
            Some("alpha.desktop")
        );
        assert!(!prism.prism_active());
    }
}
