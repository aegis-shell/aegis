//! Shared presentation primitive for compositor-owned modal applications.
//!
//! The helper owns only layout and material policy. Individual chrome
//! components retain their open state, typed intents, and authoritative
//! snapshots.

use aegis_design::materials::{chrome_place, surface_layout};
use aegis_design::{Design, themes};
use lens::{Align, Frame, Icon, Input, LayoutOpts, Rect};

use crate::{BackdropRegion, Localizer, Message, Reserved};

const APP_MARGIN: f32 = 24.0;
const APP_RADIUS: f32 = 24.0;
const BACKDROP_BLUR_SIGMA: f32 = 18.0;

/// Stable presentation metadata for one compositor-owned modal application.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModalApplicationSpec {
    pub scrim_id: &'static str,
    pub panel_id: &'static str,
    pub scroll_id: &'static str,
    pub title: Message,
    pub icon: Icon,
    pub max_width: f32,
    pub max_height: f32,
}

impl ModalApplicationSpec {
    /// Resolve a centered application rectangle inside shell-reserved edges.
    pub fn bounds(self, display: (f32, f32), reserved: Reserved) -> Rect {
        let left = reserved.left.max(0) as f32;
        let top = reserved.top.max(0) as f32;
        let right = reserved.right.max(0) as f32;
        let bottom = reserved.bottom.max(0) as f32;
        let usable_w = (display.0 - left - right).max(1.0);
        let usable_h = (display.1 - top - bottom).max(1.0);
        let width = self.max_width.min((usable_w - APP_MARGIN * 2.0).max(240.0));
        let height = self
            .max_height
            .min((usable_h - APP_MARGIN * 2.0).max(300.0));
        Rect {
            x: left + ((usable_w - width) * 0.5).max(0.0),
            y: top + ((usable_h - height) * 0.5).max(0.0),
            w: width.min(usable_w),
            h: height.min(usable_h),
        }
    }

    /// Draw one modal application. Returns `true` when its close button or
    /// click-away behavior requests dismissal.
    pub fn render(
        self,
        frame: &mut Frame,
        input: &Input,
        reserved: Reserved,
        i18n: &Localizer,
        design: &Design,
        mut content: impl FnMut(&mut Frame),
    ) -> bool {
        let raw = input.as_raw();
        let display = (raw.display_size.x, raw.display_size.y);
        let bounds = self.bounds(display, reserved);
        frame.place(
            self.scrim_id,
            &chrome_place(
                Rect {
                    x: 0.0,
                    y: 0.0,
                    w: display.0,
                    h: display.1,
                },
                LayoutOpts {
                    bg: design.colors.scrim,
                    ..surface_layout()
                },
            ),
            |_| {},
        );

        let original_theme = frame.theme();
        frame.set_theme(themes::application(design));
        let mut close = false;
        frame.place(
            self.panel_id,
            &chrome_place(
                bounds,
                LayoutOpts {
                    bg: design.colors.application_surface.with_alpha(238),
                    border: design.colors.application_border,
                    border_width: 1.0,
                    radius: APP_RADIUS,
                    pad: 0.0,
                    ..surface_layout()
                },
            ),
            |frame| {
                frame.column_ex(
                    &LayoutOpts {
                        width: bounds.w,
                        height: bounds.h,
                        gap: 12.0,
                        pad: 22.0,
                        cross: Align::Stretch,
                        ..Default::default()
                    },
                    |frame| {
                        frame.row_ex(
                            &LayoutOpts {
                                height: 48.0,
                                gap: 12.0,
                                cross: Align::Center,
                                ..Default::default()
                            },
                            |frame| {
                                frame.size_next(36.0, 36.0);
                                frame.icon(self.icon, 28.0);
                                frame.column_ex(
                                    &LayoutOpts {
                                        gap: 1.0,
                                        cross: Align::Start,
                                        ..Default::default()
                                    },
                                    |frame| {
                                        frame.heading(i18n.text(self.title), 2);
                                        frame.label_sized(
                                            i18n.text(Message::BuiltInSystemApp),
                                            11.0,
                                        );
                                    },
                                );
                                frame.flex(1.0);
                                frame.spacer(0.0);
                                frame.size_next(34.0, 30.0);
                                close = frame.icon_button(Icon::X);
                            },
                        );
                        frame.separator();
                        frame.flex(1.0);
                        frame.scroll(self.scroll_id, |frame| {
                            frame.column_ex(
                                &LayoutOpts {
                                    gap: 12.0,
                                    cross: Align::Stretch,
                                    ..Default::default()
                                },
                                |frame| content(frame),
                            );
                        });
                    },
                );
            },
        );
        frame.set_theme(original_theme);

        let left_pressed = raw.mouse_pressed.first().copied().unwrap_or(false);
        close || (left_pressed && !contains(bounds, raw.cursor.x, raw.cursor.y))
    }

    pub fn backdrop_blur_sigma(self) -> f32 {
        BACKDROP_BLUR_SIGMA
    }

    /// Approximate the rounded panel with two rectangles for shared backdrop
    /// capture, avoiding a full-output blur request.
    pub fn backdrop_regions(self, display: (f32, f32), reserved: Reserved) -> Vec<BackdropRegion> {
        let panel = self.bounds(display, reserved);
        vec![
            BackdropRegion {
                x: panel.x + APP_RADIUS,
                y: panel.y,
                w: (panel.w - APP_RADIUS * 2.0).max(0.0),
                h: panel.h,
            },
            BackdropRegion {
                x: panel.x,
                y: panel.y + APP_RADIUS,
                w: panel.w,
                h: (panel.h - APP_RADIUS * 2.0).max(0.0),
            },
        ]
    }
}

fn contains(rect: Rect, x: f32, y: f32) -> bool {
    x >= rect.x && y >= rect.y && x < rect.x + rect.w && y < rect.y + rect.h
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPEC: ModalApplicationSpec = ModalApplicationSpec {
        scrim_id: "test-scrim",
        panel_id: "test-panel",
        scroll_id: "test-scroll",
        title: Message::StatusAndControls,
        icon: Icon::Sliders,
        max_width: 860.0,
        max_height: 590.0,
    };

    #[test]
    fn bounds_stay_inside_small_outputs_and_reserved_edges() {
        let bounds = SPEC.bounds(
            (320.0, 480.0),
            Reserved {
                top: 32,
                ..Reserved::default()
            },
        );
        assert!(bounds.x >= 0.0 && bounds.y >= 32.0);
        assert!(bounds.x + bounds.w <= 320.0);
        assert!(bounds.y + bounds.h <= 480.0);
    }

    #[test]
    fn backdrop_regions_cover_only_the_panel_cross() {
        let bounds = SPEC.bounds((1920.0, 1080.0), Reserved::default());
        let regions = SPEC.backdrop_regions((1920.0, 1080.0), Reserved::default());
        assert_eq!(regions.len(), 2);
        assert!(regions.iter().all(|region| region.w <= bounds.w));
        assert!(regions.iter().all(|region| region.h <= bounds.h));
    }
}
