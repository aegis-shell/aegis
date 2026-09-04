//! The `centered` composition: a conventional centered identity column.
//!
//! Clock and credentials share a portrait-friendly column with a persona
//! avatar (image, 3D model, or initial fallback) above a rounded credential
//! field.

use tessera_config::LockScreenStyle;
use tessera_design::AvatarRole;
use tessera_design::materials::chrome_place;
use tessera_lock::lock_layout_for;
use flux::{Canvas, GradientStop};
use lens::{Align, LayoutOpts, Rect};

use crate::profile::Profile;
use crate::render::{LockBackground, LockVisual};
use crate::style::common::{
    FramePresentation, StylePainter, aligned_layer, centered_layer, credential_label,
    keyboard_status, localized, localized_ref, lock_theme, palette_foreground, palette_muted,
};

pub struct CenteredPainter {
    pub visual: LockVisual,
}

impl StylePainter for CenteredPainter {
    fn paint_background(
        &self,
        canvas: &Canvas,
        device: &flux::Device,
        background: &mut LockBackground,
        output: (u32, u32),
        dim: f32,
    ) {
        crate::style::paint_artwork_background(
            canvas,
            device,
            background,
            output,
            LockScreenStyle::Centered,
            dim,
        );
    }

    fn paint_materials(&self, canvas: &Canvas, frame: &FramePresentation<'_>) {
        let layout = lock_layout_for(
            LockScreenStyle::Centered,
            frame.logical.0 as f32,
            frame.logical.1 as f32,
        );
        let progress = frame.progress.clamp(0.0, 1.0);
        if frame.state.presentation() == tessera_lock::PresentationMode::Ambient && progress <= 0.02 {
            return;
        }
        let avatar_style = self.visual.design.avatars.for_role(AvatarRole::LockHero);
        let avatar_x = layout.avatar_x * frame.scale;
        let avatar_y = (layout.avatar_y + (1.0 - progress) * 18.0) * frame.scale;
        let avatar_size = layout.avatar_size * frame.scale;
        match frame.avatar_status {
            crate::render::AvatarStatus::Image | crate::render::AvatarStatus::Animated3d { .. } => {
                // GPU-rendered VRM frames stay square internally; the
                // analytic rounded-image clip keeps every source a perfect
                // disc without a readback/re-upload on each animation frame.
                if let Some(avatar) = frame.avatar {
                    canvas.draw_image_rrect(
                        avatar,
                        avatar_x,
                        avatar_y,
                        avatar_size,
                        avatar_size,
                        avatar_size * 0.5,
                    );
                }
            }
            crate::render::AvatarStatus::Fallback => {
                let [red, green, blue] = self.visual.palette.avatar_fill;
                canvas.fill_rrect(
                    avatar_x,
                    avatar_y,
                    avatar_size,
                    avatar_size,
                    avatar_size * 0.5,
                    flux::rgba(red, green, blue, (255.0 * progress) as u8),
                );
            }
        }
        // A hairline frames both real avatars and the flat initial fallback.
        // It must remain a stroke: filling this shape is what washed the old
        // blue fallback toward white.
        let (ring_red, ring_green, ring_blue, ring_alpha) = avatar_style.ring.components();
        canvas.stroke_rrect(
            avatar_x,
            avatar_y,
            avatar_size,
            avatar_size,
            avatar_size * 0.5,
            flux::rgba(
                ring_red,
                ring_green,
                ring_blue,
                (ring_alpha as f32 * progress).round() as u8,
            ),
            avatar_style.ring_width * frame.scale,
        );

        // Rounded credential field.
        let field_x = (layout.field_x + frame.feedback_offset) * frame.scale;
        let field_y = (layout.field_y + (1.0 - progress) * 22.0) * frame.scale;
        let field_w = layout.field_width * frame.scale;
        let field_h = layout.field_height * frame.scale;
        let (error_red, error_green, error_blue, _) =
            self.visual.design.colors.critical.components();
        canvas.fill_rrect(
            field_x,
            field_y,
            field_w,
            field_h,
            10.0 * frame.scale,
            if frame.state.rejected() {
                flux::rgba(38, 8, 14, (174.0 * progress) as u8)
            } else {
                flux::rgba(4, 8, 16, (142.0 * progress) as u8)
            },
        );
        canvas.stroke_rrect(
            field_x,
            field_y,
            field_w,
            field_h,
            10.0 * frame.scale,
            if frame.state.rejected() {
                flux::rgba(error_red, error_green, error_blue, (238.0 * progress) as u8)
            } else if frame.state.validation_pending() {
                let (red, green, blue, _) = self.visual.design.colors.validation.components();
                flux::rgba(red, green, blue, (168.0 * progress) as u8)
            } else {
                flux::rgba(255, 255, 255, (62.0 * progress) as u8)
            },
            frame.scale,
        );
    }

    fn paint_clock(
        &self,
        ui: &mut lens::Frame,
        frame: &FramePresentation<'_>,
        clock: &str,
        date: &str,
    ) {
        let layout = lock_layout_for(
            LockScreenStyle::Centered,
            frame.logical.0 as f32,
            frame.logical.1 as f32,
        );
        let alignment = Align::Center;
        ui.place(
            "lock-clock",
            &chrome_place(
                Rect {
                    x: layout.clock_x,
                    y: layout.clock_y,
                    w: layout.clock_width,
                    h: layout.clock_size + 12.0,
                },
                aligned_layer(alignment),
            ),
            |ui| ui.label_compact_sized(clock, layout.clock_size),
        );
        ui.set_theme(lock_theme(
            &self.visual.design,
            palette_muted(self.visual.palette),
            255,
        ));
        ui.place(
            "lock-date",
            &chrome_place(
                Rect {
                    x: layout.clock_x,
                    y: layout.clock_y + layout.clock_size + 8.0,
                    w: layout.clock_width,
                    h: 28.0,
                },
                aligned_layer(alignment),
            ),
            |ui| {
                ui.label_compact_sized(date, if layout.height < 650.0 { 15.0 } else { 18.0 });
            },
        );
    }

    fn paint_identity(&self, ui: &mut lens::Frame, frame: &FramePresentation<'_>) {
        let layout = lock_layout_for(
            LockScreenStyle::Centered,
            frame.logical.0 as f32,
            frame.logical.1 as f32,
        );
        let state = frame.state;
        let profile: &Profile = frame.profile;
        let alpha = (255.0 * frame.progress) as u8;
        let shifted_avatar_y = layout.avatar_y + (1.0 - frame.progress) * 18.0;
        if frame.avatar_status == crate::render::AvatarStatus::Fallback {
            let avatar_style = self.visual.design.avatars.for_role(AvatarRole::LockHero);
            ui.set_theme(lock_theme(
                &self.visual.design,
                self.visual.palette.avatar_foreground,
                alpha,
            ));
            ui.place(
                "lock-avatar-label",
                &chrome_place(
                    Rect {
                        x: layout.avatar_x,
                        y: shifted_avatar_y,
                        w: layout.avatar_size,
                        h: layout.avatar_size,
                    },
                    centered_layer(),
                ),
                |ui| {
                    ui.row_ex(
                        &LayoutOpts {
                            width: layout.avatar_size,
                            height: layout.avatar_size,
                            pad: 0.0,
                            cross: Align::Center,
                            ..Default::default()
                        },
                        |ui| {
                            ui.flex(1.0);
                            ui.spacer(0.0);
                            ui.label_compact_sized(
                                &profile.initials,
                                layout.avatar_size * avatar_style.initials_scale,
                            );
                            ui.flex(1.0);
                            ui.spacer(0.0);
                        },
                    );
                },
            );
        }
        ui.set_theme(lock_theme(
            &self.visual.design,
            palette_foreground(self.visual.palette),
            alpha,
        ));
        let name_x = (layout.width - 520.0) * 0.5;
        let name_y = shifted_avatar_y + layout.avatar_size + 16.0;
        ui.place(
            "lock-display-name",
            &chrome_place(
                Rect {
                    x: name_x,
                    y: name_y,
                    w: 520.0,
                    h: 30.0,
                },
                aligned_layer(Align::Center),
            ),
            |ui| ui.label_compact_sized(&profile.display_name, 19.0),
        );

        let field_y = layout.field_y + (1.0 - frame.progress) * 22.0;
        let field_x = layout.field_x + frame.feedback_offset;
        let dots = if state.password_len() == 0 {
            localized("Enter password", "输入密码")
        } else {
            let visible = state.password_len().min(18);
            format!(
                "{}{}",
                "•".repeat(visible),
                if state.password_len() > visible {
                    "…"
                } else {
                    ""
                }
            )
        };
        let credential = credential_label(&dots, state.credential_revision());
        let muted = state.password_len() == 0 && !state.rejected() && !state.validation_pending();
        let credential_color = if state.rejected() {
            self.visual.design.colors.critical
        } else if state.validation_pending() {
            self.visual.design.colors.validation.with_alpha(224)
        } else if muted {
            palette_muted(self.visual.palette)
        } else {
            palette_foreground(self.visual.palette)
        };
        ui.set_theme(lock_theme(&self.visual.design, credential_color, alpha));
        ui.place(
            "lock-password-content",
            &chrome_place(
                Rect {
                    x: field_x,
                    y: field_y,
                    w: layout.field_width,
                    h: layout.field_height,
                },
                LayoutOpts {
                    pad: 0.0,
                    cross: Align::Stretch,
                    ..tessera_design::materials::surface_layout()
                },
            ),
            |ui| {
                ui.row_ex(
                    &LayoutOpts {
                        width: layout.field_width,
                        height: layout.field_height,
                        pad: 15.0,
                        cross: Align::Center,
                        ..Default::default()
                    },
                    |ui| {
                        ui.label_compact_sized(&credential, 14.0);
                    },
                );
            },
        );

        let status = if state.rejected() {
            None
        } else if let Some(message) = state.message() {
            Some((message.to_owned(), true))
        } else if state.validation_pending() {
            Some((localized_ref("Checking…", "正在验证…").to_owned(), false))
        } else {
            keyboard_status(state).map(|status| (status, false))
        };
        if let Some((message, error)) = status {
            let color = if error {
                lens::Color::rgba(255, 174, 168, 255)
            } else {
                palette_muted(self.visual.palette)
            };
            ui.set_theme(lock_theme(&self.visual.design, color, alpha));
            ui.place(
                "lock-status",
                &chrome_place(
                    Rect {
                        x: (layout.width - 520.0) * 0.5,
                        y: field_y + layout.field_height + 12.0,
                        w: 520.0,
                        h: 24.0,
                    },
                    aligned_layer(Align::Center),
                ),
                |ui| ui.label_compact_sized(&message, 12.0),
            );
        }
    }
}

/// Scrim gradient for the shared artwork painter.
pub(crate) fn centered_scrim_stops() -> [GradientStop; 3] {
    [
        GradientStop::new(0.0, flux::rgba(2, 5, 12, 54)),
        GradientStop::new(0.55, flux::rgba(2, 5, 12, 10)),
        GradientStop::new(1.0, flux::rgba(2, 5, 12, 110)),
    ]
}
