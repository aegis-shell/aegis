//! The application launcher chrome: a centered overlay backed by the pure
//! [`ass_core::launcher::Launcher`] state machine.
//!
//! The component is a thin flux-ui adapter: it forwards mouse clicks and
//! resolved key events to the brain, and renders whatever the brain's query,
//! filter, and selection produce. All search/selection logic lives in
//! `ass-core` (unit-tested there, no flux-ui dependency). See ADR-0022.
//!
//! Interaction: click the top-center "Apps" toggle to open, or open via a
//! future hotkey. While open the launcher captures the keyboard: type to
//! filter, Up/Down to move the selection, Enter to launch, Escape to close,
//! Backspace to delete. Mouse clicks on rows still work alongside.

use flux_ui::{Align, Color, Frame, Input, OverlayOpts, Rect};

use crate::{Chrome, ChromeEvents};
use ass_core::app::Entry;
use ass_core::input::{key_action, KeyChar};
use ass_core::launcher::{Launch, Launcher as Brain};
use ass_core::window::Window;

/// Cap on rows rendered in one frame. The brain keeps the full filtered list;
/// only the first `MAX_ROWS` are drawn. Scrolling is deferred (ADR-0022).
const MAX_ROWS: usize = 24;

/// The application launcher chrome component.
///
/// Wraps [`Brain`]; all state (open/query/selection) lives in the brain.
pub struct Launcher {
    brain: Brain,
}

impl Launcher {
    /// Construct with the launchable entries the binary enumerated.
    pub fn new(apps: Vec<Entry>) -> Launcher {
        Launcher {
            brain: Brain::new(apps),
        }
    }
}

impl Chrome for Launcher {
    fn render(&mut self, f: &mut Frame, input: &Input, windows: &[Window], out: &mut ChromeEvents) {
        let disp = input.as_raw().display_size;

        // Refresh the brain's view of which apps are already running, so it
        // can mark them and focus them instead of spawning a duplicate. Built
        // from the server's live toplevel snapshot (Window.app_id ↔ surface
        // id); empty when nothing matches.
        let running: Vec<(String, usize)> = windows
            .iter()
            .filter_map(|w| w.app_id.as_ref().map(|a| (a.clone(), w.id)))
            .collect();
        self.brain.set_running(running);

        if !self.brain.is_open() {
            // Collapsed: a small centered toggle at the top edge.
            let (bw, bh) = (120.0, 28.0);
            let rect = Rect {
                x: (disp.x - bw) * 0.5,
                y: 8.0,
                w: bw,
                h: bh,
            };
            f.overlay("ass-launcher-toggle", rect, &toggle_opts(), |f| {
                if f.button("Apps") {
                    self.brain.open();
                }
            });
            return;
        }

        // Expanded. Snapshot the brain's immutable view into owned values so
        // the overlay closure can later call &mut brain methods (launch) and
        // mutate `out` without holding a borrow across the mutation.
        let query = self.brain.query().to_string();
        let filtered: Vec<usize> = self.brain.filtered();
        let selection = self.brain.selection();
        let total = self.brain.apps().len();
        let shown = filtered.len().min(MAX_ROWS);
        let more = filtered.len().saturating_sub(shown);
        let rows: Vec<(usize, String, bool)> = filtered
            .iter()
            .enumerate()
            .take(MAX_ROWS)
            .map(|(fpos, &app_idx)| {
                let e = &self.brain.apps()[app_idx];
                let label = if e.summary().is_empty() {
                    e.name.clone()
                } else {
                    format!("{}  —  {}", e.name, e.summary())
                };
                (fpos, label, self.brain.is_running(app_idx))
            })
            .collect();

        let pw = (disp.x * 0.5).min(560.0);
        let ph = (disp.y * 0.72).min(520.0);
        let rect = Rect {
            x: (disp.x - pw) * 0.5,
            y: (disp.y - ph) * 0.5,
            w: pw,
            h: ph,
        };
        f.overlay("ass-launcher", rect, &panel_opts(), |f| {
            f.title("Applications");
            f.label_sized(&format!("Search: {query}_"), 14.0);
            f.label_sized(
                &format!("{total} apps · {shown} match{}", if more > 0 { format!(", {more} more") } else { String::new() }),
                11.0,
            );
            f.separator();
            for (fpos, label, running) in &rows {
                // "▸" marks the keyboard selection; "●" marks a running
                // instance (clicking or Enter will focus it, not spawn).
                let mark = match (*fpos == selection, *running) {
                    (true, true) => "▸● ",
                    (true, false) => "▸  ",
                    (false, true) => "  ● ",
                    (false, false) => "    ",
                };
                if f.button(&format!("{mark}{label}")) {
                    Self::emit(self.brain.launch_filtered(*fpos), out);
                }
            }
            f.separator();
            if f.button("Close") {
                self.brain.close();
            }
        });
    }

    /// Route a launch outcome into the chrome intent sink: spawn a new
    /// instance, or focus an already-running one through the existing
    /// `clicked` → `Server::focus_surface_by_id` path.
    fn emit(outcome: Option<Launch>, out: &mut ChromeEvents) {
        match outcome {
            Some(Launch::Spawn(entry)) => out.spawn = Some(entry),
            Some(Launch::Focus(surface_id)) => out.clicked = Some(surface_id),
            None => {}
        }
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

fn toggle_opts() -> OverlayOpts {
    OverlayOpts {
        bg: Color::rgba(28, 30, 44, 220),
        border: Color::rgba(60, 64, 84, 255),
        border_width: 1.0,
        radius: 14.0,
        pad: 4.0,
        cross: Align::Center,
        ..Default::default()
    }
}

fn panel_opts() -> OverlayOpts {
    OverlayOpts {
        bg: Color::rgba(24, 26, 38, 240),
        border: Color::rgba(60, 64, 84, 255),
        border_width: 1.0,
        radius: 16.0,
        pad: 12.0,
        cross: Align::Center,
        ..Default::default()
    }
}
