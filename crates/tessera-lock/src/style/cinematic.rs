//! The `cinematic` composition: full-bleed artwork, peripheral clock, and
//! a lower-right typographic credential rail.

use tessera_config::LockScreenStyle;
use tessera_design::materials::chrome_place;
use tessera_lock::lock_layout_for;
use flux::{Canvas, GradientStop};
use lens::{Align, Color, Rect};

use crate::profile::Profile;
use crate::render::{LockBackground, LockVisual};
use crate::style::common::{
    FramePresentation, StylePainter, aligned_layer, cinematic_password_marks, credential_label,
    keyboard_status, localized, localized_ref, lock_theme, palette_foreground, palette_muted,
};

pub struct CinematicPainter {
    pub visual: LockVisual,
}

impl StylePainter for CinematicPainter {
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
            LockScreenStyle::Cinematic,
            dim,
        );
    }

    fn paint_materials(&self, canvas: &Canvas, frame: &FramePresentation<'_>) {
        let layout = lock_layout_for(
            LockScreenStyle::Cinematic,
            frame.logical.0 as f32,
            frame.logical.1 as f32,
        );
        let progress = frame.progress.clamp(0.0, 1.0);
        if frame.state.presentation() == tessera_lock::PresentationMode::Ambient && progress <= 0.02 {
            return;
        }
        let field_x = (layout.field_x + frame.feedback_offset) * frame.scale;
        let field_y = (layout.field_y + (1.0 - progress) * 22.0) * frame.scale;
        let field_w = layout.field_width * frame.scale;
        let field_h = layout.field_height * frame.scale;
        let state = frame.state;

        let (critical_red, critical_green, critical_blue, _) =
            self.visual.design.colors.critical.components();
        let (validation_red, validation_green, validation_blue, _) =
            self.visual.design.colors.validation.components();
        let ([red, green, blue], rail_alpha) = if state.rejected() {
            ([critical_red, critical_green, critical_blue], 218.0)
        } else if state.validation_pending() {
            ([validation_red, validation_green, validation_blue], 132.0)
        } else if state.password_len() > 0 {
            ([245, 247, 252], 132.0)
        } else {
            ([245, 247, 252], 82.0)
        };
        canvas.fill_rect(
            field_x,
            field_y + field_h - 1.5 * frame.scale,
            field_w,
            1.5 * frame.scale,
            flux::rgba(red, green, blue, (rail_alpha * progress) as u8),
        );
        if state.validation_pending()
            && !self.visual.reduced_motion
            && let Some(sweep) = state.validation_feedback_progress(frame.now)
        {
            draw_validation_sweep(
                canvas,
                field_x,
                field_y,
                field_w,
                field_h,
                frame.scale,
                sweep,
                progress,
                self.visual.design.colors.validation,
            );
        }
    }

    fn paint_clock(
        &self,
        ui: &mut lens::Frame,
        frame: &FramePresentation<'_>,
        clock: &str,
        date: &str,
    ) {
        let layout = lock_layout_for(
            LockScreenStyle::Cinematic,
            frame.logical.0 as f32,
            frame.logical.1 as f32,
        );
        let alignment = Align::End;
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
            |ui| ui.label_compact_sized(date, 13.0),
        );
    }

    fn paint_identity(&self, ui: &mut lens::Frame, frame: &FramePresentation<'_>) {
        let layout = lock_layout_for(
            LockScreenStyle::Cinematic,
            frame.logical.0 as f32,
            frame.logical.1 as f32,
        );
        let state = frame.state;
        let profile: &Profile = frame.profile;
        let alpha = (255.0 * frame.progress) as u8;

        ui.set_theme(lock_theme(
            &self.visual.design,
            palette_foreground(self.visual.palette),
            alpha,
        ));
        let name_x = layout.field_x;
        let name_y = layout.field_y - 50.0;
        let name_width = layout.field_width * 0.64;
        let display_name = profile.display_name.to_uppercase();
        ui.place(
            "lock-display-name",
            &chrome_place(
                Rect {
                    x: name_x,
                    y: name_y,
                    w: name_width,
                    h: 32.0,
                },
                aligned_layer(Align::Start),
            ),
            |ui| {
                // Keep the cinematic profile quiet and precise. Lens titles
                // are deliberately bold; the regular compact run gives this
                // line the lighter stroke requested by the composition.
                ui.label_compact_sized(&display_name, 24.0);
            },
        );

        if let Some(keyboard) = keyboard_status(state) {
            ui.set_theme(lock_theme(
                &self.visual.design,
                palette_muted(self.visual.palette),
                alpha,
            ));
            let indicator_width = layout.field_width * 0.32;
            ui.place(
                "lock-keyboard-status",
                &chrome_place(
                    Rect {
                        x: layout.field_x + layout.field_width - indicator_width,
                        // Both boxes finish on the same line even though
                        // their type sizes differ.
                        y: name_y + 32.0 - 17.0,
                        w: indicator_width,
                        h: 17.0,
                    },
                    aligned_layer(Align::End),
                ),
                |ui| ui.label_compact_sized(&keyboard, 11.0),
            );
        }

        let field_y = layout.field_y + (1.0 - frame.progress) * 22.0;
        let field_x = layout.field_x + frame.feedback_offset;
        let dots = cinematic_password_marks(state.password_len());
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
                lens::LayoutOpts {
                    pad: 0.0,
                    cross: Align::Stretch,
                    ..tessera_design::materials::surface_layout()
                },
            ),
            |ui| {
                ui.row_ex(
                    &lens::LayoutOpts {
                        width: layout.field_width,
                        height: layout.field_height,
                        pad: 0.0,
                        cross: Align::Center,
                        ..Default::default()
                    },
                    |ui| {
                        ui.label_compact_sized(&credential, 12.0);
                    },
                );
            },
        );

        // Cinematic expresses rejection purely through the red rail; only
        // the service-unavailable failure gets a sentence.
        let status = if state.rejected() {
            None
        } else {
            state.message().map(|message| (message.to_owned(), true))
        };
        if let Some((message, error)) = status {
            let color = if error {
                Color::rgba(255, 174, 168, 255)
            } else {
                palette_muted(self.visual.palette)
            };
            ui.set_theme(lock_theme(&self.visual.design, color, alpha));
            ui.place(
                "lock-status",
                &chrome_place(
                    Rect {
                        x: field_x,
                        y: field_y - 48.0,
                        w: layout.field_width,
                        h: 24.0,
                    },
                    aligned_layer(Align::End),
                ),
                |ui| ui.label_compact_sized(&message, 12.0),
            );
        }
        let _ = localized_ref("", "");
        let _ = localized("", "");
    }
}

/// Cool light sweep traveling along the credential rail while PAM runs.
#[allow(clippy::too_many_arguments)]
fn draw_validation_sweep(
    canvas: &Canvas,
    field_x: f32,
    field_y: f32,
    field_w: f32,
    field_h: f32,
    scale: f32,
    progress: f32,
    alpha: f32,
    validation: Color,
) {
    let sweep_w = (field_w * 0.28).clamp(72.0 * scale, 144.0 * scale);
    let sweep_x = field_x - sweep_w + (field_w + sweep_w) * progress.clamp(0.0, 1.0);
    let rail_y = field_y + field_h - 1.5 * scale;
    let (validation_red, validation_green, validation_blue, _) = validation.components();
    canvas.save();
    canvas.clip_rect(field_x, rail_y - 5.0 * scale, field_w, 10.0 * scale);
    canvas.fill_rect_linear_gradient(
        (sweep_x, rail_y - 4.0 * scale, sweep_w, 8.0 * scale),
        (sweep_x, rail_y),
        (sweep_x + sweep_w, rail_y),
        &[
            GradientStop::new(0.0, flux::rgba(150, 210, 255, 0)),
            GradientStop::new(
                0.5,
                flux::rgba(
                    validation_red,
                    validation_green,
                    validation_blue,
                    (112.0 * alpha) as u8,
                ),
            ),
            GradientStop::new(1.0, flux::rgba(150, 210, 255, 0)),
        ],
    );
    canvas.fill_rect_linear_gradient(
        (sweep_x, rail_y, sweep_w, 1.5 * scale),
        (sweep_x, rail_y),
        (sweep_x + sweep_w, rail_y),
        &[
            GradientStop::new(0.0, flux::rgba(210, 238, 255, 0)),
            GradientStop::new(0.5, flux::rgba(226, 245, 255, (250.0 * alpha) as u8)),
            GradientStop::new(1.0, flux::rgba(210, 238, 255, 0)),
        ],
    );
    canvas.restore();
}

/// Scrim gradient for the shared artwork painter.
pub(crate) fn cinematic_scrim_stops() -> [GradientStop; 3] {
    [
        GradientStop::new(0.0, flux::rgba(2, 4, 9, 6)),
        GradientStop::new(0.58, flux::rgba(2, 4, 9, 34)),
        GradientStop::new(1.0, flux::rgba(2, 4, 9, 176)),
    ]
}
