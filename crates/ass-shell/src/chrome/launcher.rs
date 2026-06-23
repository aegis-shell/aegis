//! The application launcher chrome: a full-screen, macOS Launchpad-style grid
//! of every enumerated `.desktop` entry, backed by the pure
//! [`ass_core::launcher::Launcher`] state machine.
//!
//! The component is a thin lens adapter: it forwards mouse clicks and resolved
//! key events to the brain, and renders whatever the brain's query, filter, and
//! selection produce. All search/selection logic lives in `ass-core` (unit-
//! tested there, no lens dependency). See ADR-0022.
//!
//! There is no toggle button of its own any more — the dock's Launchpad tile
//! opens it (via [`crate::ChromeEvents::toggle_launcher`]); a Super tap and the
//! configured hotkey still toggle it too. While open the launcher fills the
//! screen and captures the keyboard: type to filter, arrows to move the
//! selection, Enter to launch, Escape to close. Clicking an icon launches it.

use std::collections::HashMap;
use std::ffi::c_void;

use lens::{Align, Color, Frame, Icon, Input, LayoutOpts, OverlayOpts, Rect};

use crate::{Chrome, ChromeEvents};
use ass_core::app::Entry;
use ass_core::input::{key_action, KeyChar};
use ass_core::launcher::{Launch, Launcher as Brain};
use ass_core::window::Window;

/// Cap on the cells rendered in one frame. The brain keeps the full filtered
/// list; only the first `MAX_CELLS` are drawn into the grid.
const MAX_CELLS: usize = 200;
/// Width of one grid cell (icon + label) in logical px.
const CELL_W: f32 = 132.0;
/// Height of one grid cell.
const CELL_H: f32 = 116.0;
/// Icon side length within a cell.
const CELL_ICON: f32 = 72.0;
/// Outer padding around the grid.
const GRID_PAD: f32 = 56.0;

/// The application launcher chrome component.
///
/// Wraps [`Brain`]; all state (open/query/selection) lives in the brain. Holds
/// a borrowed icon map so grid cells can show real app icons.
pub struct Launcher {
    brain: Brain,
    /// `app_id`/icon-name (lowercased) → borrowed icon texture pointer. Shared
    /// with the dock; the binary's `IconCache` owns the textures.
    icons: HashMap<String, *mut c_void>,
}

impl Launcher {
    /// Construct with the launchable entries the binary enumerated, no icons.
    pub fn new(apps: Vec<Entry>) -> Launcher {
        Launcher {
            brain: Brain::new(apps),
            icons: HashMap::new(),
        }
    }

    /// Construct with the entries and a borrowed icon map (`app_id` →
    /// `flux_image` pointer erased to `c_void`). The caller retains ownership
    /// of the textures, which must outlive the launcher.
    pub fn with_icons(apps: Vec<Entry>, icons: HashMap<String, *mut c_void>) -> Launcher {
        Launcher {
            brain: Brain::new(apps),
            icons,
        }
    }

    /// Resolve an entry's icon texture from the borrowed map, trying the same
    /// ids the icon cache files textures under (StartupWMClass, desktop-id
    /// stem, icon name), all lowercased. `None` falls back to a glyph.
    fn entry_icon(&self, e: &Entry) -> Option<*mut c_void> {
        let get = |k: &str| {
            let k = k.to_ascii_lowercase();
            if k.is_empty() {
                None
            } else {
                self.icons.get(&k).copied()
            }
        };
        if let Some(wm) = &e.startup_wm_class {
            if let Some(p) = get(wm) {
                return Some(p);
            }
        }
        if let Some(p) = get(e.id.strip_suffix(".desktop").unwrap_or(&e.id)) {
            return Some(p);
        }
        e.icon.as_deref().and_then(|ic| get(ic))
    }

    /// Route a launch outcome into the chrome intent sink: spawn a new
    /// instance, or focus an already-running one through the existing
    /// `clicked` → `Server::focus_surface_by_id` path.
    fn emit(outcome: Option<Launch>, out: &mut ChromeEvents) {
        match outcome {
            Some(Launch::Spawn(entry)) => out.spawn = Some(*entry),
            Some(Launch::Focus(surface_id)) => out.clicked = Some(surface_id),
            None => {}
        }
    }
}

/// One resolved grid cell for the current frame: filtered position, label,
/// running flag, selection highlight, and the icon texture (if any).
struct Cell {
    fpos: usize,
    label: String,
    running: bool,
    selected: bool,
    icon: Option<*mut c_void>,
}

impl Chrome for Launcher {
    fn render(
        &mut self,
        f: &mut Frame,
        input: &Input,
        windows: &[Window],
        _workspaces: &crate::WorkspaceSnapshot,
        out: &mut ChromeEvents,
    ) {
        let disp = input.as_raw().display_size;

        // Refresh the brain's view of which apps are already running, so it can
        // mark them and focus them instead of spawning a duplicate.
        let running: Vec<(String, ass_core::window::WindowId)> = windows
            .iter()
            .filter_map(|w| w.app_id.as_ref().map(|a| (a.clone(), w.id)))
            .collect();
        self.brain.set_running(running);

        // Collapsed: render nothing. The dock's Launchpad tile opens it.
        if !self.brain.is_open() {
            return;
        }

        // Snapshot the brain's immutable view into owned values so the grid
        // closure can later call &mut brain methods (launch) without holding a
        // borrow across the mutation.
        let query = self.brain.query().to_string();
        let filtered: Vec<usize> = self.brain.filtered();
        let selection = self.brain.selection();
        let total = self.brain.apps().len();
        let shown = filtered.len().min(MAX_CELLS);
        let more = filtered.len().saturating_sub(shown);
        let cells: Vec<Cell> = filtered
            .iter()
            .enumerate()
            .take(MAX_CELLS)
            .map(|(fpos, &app_idx)| {
                let e = &self.brain.apps()[app_idx];
                Cell {
                    fpos,
                    label: e.name.clone(),
                    running: self.brain.is_running(app_idx),
                    selected: fpos == selection,
                    icon: self.entry_icon(e),
                }
            })
            .collect();

        // Columns that fit the screen width; the grid scrolls vertically.
        let cols = (((disp.x - 2.0 * GRID_PAD) / CELL_W).floor() as usize).max(1);

        let full = Rect {
            x: 0.0,
            y: 0.0,
            w: disp.x,
            h: disp.y,
        };
        f.layer("ass-launcher", full, &backdrop_opts(), |f| {
            // Centered search header.
            f.title("Applications");
            f.label_sized(&format!("Search: {query}_"), 16.0);
            f.label_sized(
                &format!(
                    "{total} apps · {shown} shown{}",
                    if more > 0 {
                        format!(" · {more} more")
                    } else {
                        String::new()
                    }
                ),
                11.0,
            );
            f.spacer(12.0);

            // The scrollable icon grid: rows of `cols` cells.
            f.scroll("ass-launcher-grid", |f| {
                let row = LayoutOpts {
                    gap: 8.0,
                    cross: Align::Center,
                    ..Default::default()
                };
                let cell = LayoutOpts {
                    gap: 6.0,
                    cross: Align::Center,
                    ..Default::default()
                };
                for chunk in cells.chunks(cols) {
                    f.row_ex(&row, |f| {
                        for c in chunk {
                            f.size_next(CELL_W, CELL_H);
                            f.column_ex(&cell, |f| {
                                f.size_next(CELL_ICON, CELL_ICON);
                                let clicked = match c.icon {
                                    Some(ptr) => unsafe {
                                        f.image_button_active(
                                            ptr as *mut lens::sys::flux_image,
                                            c.selected,
                                        )
                                    },
                                    None => f.icon_button_active(Icon::FileText, c.selected),
                                };
                                // "● " marks a running app; the label is its name.
                                let label = if c.running {
                                    format!("● {}", c.label)
                                } else {
                                    c.label.clone()
                                };
                                f.label_sized(&label, 12.0);
                                if clicked {
                                    Self::emit(self.brain.launch_filtered(c.fpos), out);
                                }
                            });
                        }
                    });
                }
            });
        });
    }

    fn captures_keyboard(&self) -> bool {
        self.brain.is_open()
    }

    fn key_char(&mut self, kc: &KeyChar, out: &mut ChromeEvents) {
        let action = key_action(kc.keysym, kc.ch);
        Self::emit(self.brain.handle(action), out);
    }

    fn toggle(&mut self, _out: &mut ChromeEvents) {
        self.brain.toggle();
    }
}

/// The full-screen launcher backdrop: a dim, near-opaque wash over the desktop,
/// content laid out top-center.
fn backdrop_opts() -> OverlayOpts {
    OverlayOpts {
        bg: Color::rgba(18, 20, 30, 240),
        border: Color::TRANSPARENT,
        border_width: 0.0,
        radius: 0.0,
        pad: GRID_PAD,
        cross: Align::Center,
        ..Default::default()
    }
}
