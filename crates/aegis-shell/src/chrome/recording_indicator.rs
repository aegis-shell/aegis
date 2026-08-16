//! Persistent, non-interactive screen-recording indicator (ADR-0128).
//!
//! While at least one compositor-owned capture stream is live, a compact
//! pill rides above the desktop: a recording marker (the design's critical
//! emphasis color) plus a localized label carrying the live stream count.
//! The pill reserves no space, accepts no input (clicks fall through), and
//! stays up while any stream lives — including over fullscreen windows,
//! where the status HUD deliberately steps aside. It renders into the
//! ordinary desktop composite, so it is visible inside the recording
//! itself — the same self-reference GNOME's indicator has.
//!
//! The data flow is the shell's usual push model: the runtime mirrors the
//! live stream count into [`crate::SystemStatus::capture_streams`] and
//! pushes the snapshot through [`ChromeUpdate::SystemStatus`]; this
//! component retains the count and paints from it.

use aegis_design::Design;
use aegis_design::materials::{chrome_place, surface_layout};
use aegis_model::window::Window;
use aegis_model::workspace::WorkspaceSnapshot;
use lens::{Align, Frame, Input, LayoutOpts, Rect};

use crate::{Chrome, ChromeEvents, ChromeUpdate, HUD_HEIGHT, Localizer};

const PILL_HEIGHT: f32 = 34.0;
const PILL_PAD: f32 = 12.0;
const PILL_GAP: f32 = 8.0;
const DOT_DIAMETER: f32 = 8.0;
/// Same resting band the Agent background-operation pill uses: just below
/// the top chip row, horizontally centered.
const PILL_TOP: f32 = HUD_HEIGHT + 10.0;
const PILL_SIDE: f32 = 8.0;

/// Trusted, non-interactive recording indicator. The count it paints is
/// compositor-owned; no interaction of any kind is wired to it.
pub struct RecordingIndicator {
    /// Live capture streams, mirrored from the pushed
    /// [`crate::SystemStatus`].
    capture_streams: u32,
    /// The design snapshot the pill paints from, from
    /// [`ChromeUpdate::Appearance`]. Seeded on registration by
    /// [`crate::Shell::add`] and refreshed when the desktop color scheme
    /// changes; defaults to the dark appearance until the first update arrives.
    design: Design,
}

impl RecordingIndicator {
    pub fn new() -> Self {
        Self {
            capture_streams: 0,
            design: Design::dark(),
        }
    }

    /// Whether the indicator is currently shown (at least one live stream).
    fn visible(&self) -> bool {
        self.capture_streams > 0
    }

    /// The label painted this frame: the localized recording marker with
    /// the live stream count.
    fn label(&self, i18n: &Localizer) -> String {
        i18n.recording_stream_count(self.capture_streams)
    }

    /// The pill rectangle for one label, centered below the top chip row.
    fn pill_rect(&self, frame: &mut Frame, display: (f32, f32), i18n: &Localizer) -> Rect {
        let label = self.label(i18n);
        let measured = frame
            .measure_text(&label, self.design.typography.footnote)
            .width;
        let width = (measured + DOT_DIAMETER + PILL_GAP + PILL_PAD * 2.0)
            .min((display.0 - PILL_SIDE * 2.0).max(1.0));
        Rect {
            x: ((display.0 - width) * 0.5).max(PILL_SIDE),
            y: PILL_TOP,
            w: width,
            h: PILL_HEIGHT,
        }
    }
}

impl Default for RecordingIndicator {
    fn default() -> Self {
        Self::new()
    }
}

impl Chrome for RecordingIndicator {
    fn render(
        &mut self,
        frame: &mut Frame,
        input: &Input,
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
        i18n: &Localizer,
        _out: &mut ChromeEvents,
    ) {
        if !self.visible() {
            return;
        }
        let raw = input.as_raw();
        let display = (raw.display_size.x.max(1.0), raw.display_size.y.max(1.0));
        let rect = self.pill_rect(frame, display, i18n);
        let label = self.label(i18n);
        let design = &self.design;
        frame.place(
            "aegis-recording-indicator",
            &chrome_place(
                rect,
                LayoutOpts {
                    bg: design.colors.application_surface,
                    border: design.colors.application_border,
                    border_width: 1.0,
                    radius: PILL_HEIGHT * 0.5,
                    ..surface_layout()
                },
            ),
            |frame| {
                frame.row_ex(
                    &LayoutOpts {
                        width: rect.w,
                        height: rect.h,
                        gap: PILL_GAP,
                        pad: PILL_PAD,
                        cross: Align::Center,
                        ..Default::default()
                    },
                    |frame| {
                        // The recording marker: a filled dot in the design's
                        // critical emphasis color, followed by the count label.
                        frame.column_ex(
                            &LayoutOpts {
                                width: DOT_DIAMETER,
                                height: DOT_DIAMETER,
                                bg: design.colors.critical,
                                radius: DOT_DIAMETER * 0.5,
                                ..Default::default()
                            },
                            |_| {},
                        );
                        frame.label_compact_sized(&label, design.typography.footnote);
                    },
                );
            },
        );
    }

    fn update(&mut self, update: ChromeUpdate<'_>) {
        match update {
            ChromeUpdate::SystemStatus(status) => {
                self.capture_streams = status.capture_streams;
            }
            ChromeUpdate::Appearance(design) => self.design = *design,
            _ => {}
        }
    }

    fn requires_composition(&self) -> bool {
        self.visible()
    }

    fn persistent_decoration(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SystemStatus;

    fn status_with_streams(capture_streams: u32) -> SystemStatus {
        SystemStatus {
            capture_streams,
            ..SystemStatus::default()
        }
    }

    #[test]
    fn hidden_without_streams_and_visible_from_the_first_one() {
        let mut indicator = RecordingIndicator::new();
        assert!(!indicator.visible());
        assert!(!indicator.requires_composition());

        indicator.update(ChromeUpdate::SystemStatus(&status_with_streams(1)));
        assert!(indicator.visible());
        assert!(indicator.requires_composition());

        indicator.update(ChromeUpdate::SystemStatus(&status_with_streams(3)));
        assert!(indicator.visible());

        indicator.update(ChromeUpdate::SystemStatus(&status_with_streams(0)));
        assert!(!indicator.visible());
        assert!(!indicator.requires_composition());
    }

    #[test]
    fn label_carries_the_live_stream_count() {
        let mut indicator = RecordingIndicator::new();
        let i18n = Localizer::new("en-US");
        indicator.update(ChromeUpdate::SystemStatus(&status_with_streams(1)));
        assert_eq!(indicator.label(&i18n), "Screen recording · 1");
        indicator.update(ChromeUpdate::SystemStatus(&status_with_streams(2)));
        assert_eq!(indicator.label(&i18n), "Screen recording · 2");

        let zh = Localizer::new("zh-CN");
        assert_eq!(indicator.label(&zh), "屏幕录制 · 2");
    }
}
