//! The `bsod` stop-screen composition.
//!
//! A nostalgic full-screen stop page as a lock screen: flat signature blue,
//! a large sad face, a left-aligned headline, and the authentication state
//! woven into the page's own voice — there is no input box. Typed characters
//! advance the percentage counter (three percent each), the counter itself
//! narrates verification, and a rejected attempt becomes a stop-code update
//! in the support block. A hand-rolled QR module (see [`qr`]) carries the
//! easter egg where the classic page shows its support code.

use std::time::{SystemTime, UNIX_EPOCH};

use aegis_lock::{LockState, bsod_layout};
use flux::Canvas;
use lens::{Align, Color, Rect};

use crate::render::{LockBackground, LockVisual};
use crate::style::common::{
    FramePresentation, StylePainter, aligned_layer, credential_label, keyboard_status, localized,
    lock_theme,
};
use crate::style::qr;

/// Signature stop-screen blue (`#0078D7`). The composition always paints
/// this color regardless of `[lock_screen.background]`.
const BSOD_BLUE: [u8; 3] = [0, 120, 215];
/// Progress granted per typed character.
const PERCENT_PER_CHARACTER: u32 = 3;
/// Idle counter cycle length in seconds.
const IDLE_CYCLE: f32 = 9.0;
/// Quiet-zone width in modules around the painted QR.
const QR_QUIET_ZONE: f32 = 2.0;

pub struct BsodPainter {
    pub visual: LockVisual,
}

impl StylePainter for BsodPainter {
    fn paint_background(
        &self,
        canvas: &Canvas,
        _device: &flux::Device,
        _background: &mut LockBackground,
        output: (u32, u32),
        _dim: f32,
    ) {
        // The stop-screen composition is a complete visual identity: it
        // always paints its signature blue and ignores the configured
        // artwork, solid color, and scrim so the effect can never be diluted
        // into a hybrid.
        canvas.fill_rect(
            0.0,
            0.0,
            output.0 as f32,
            output.1 as f32,
            flux::rgba(BSOD_BLUE[0], BSOD_BLUE[1], BSOD_BLUE[2], 255),
        );
    }

    fn paint_materials(&self, canvas: &Canvas, frame: &FramePresentation<'_>) {
        let layout = bsod_layout(frame.logical.0 as f32, frame.logical.1 as f32);
        let progress = frame.progress.clamp(0.0, 1.0);
        // The QR module grid is the only flux shape: identity chrome is text.
        paint_qr(canvas, &layout, frame.scale, progress);
    }

    fn paint_clock(
        &self,
        ui: &mut lens::Frame,
        frame: &FramePresentation<'_>,
        clock: &str,
        _date: &str,
    ) {
        let layout = bsod_layout(frame.logical.0 as f32, frame.logical.1 as f32);
        ui.set_theme(lock_theme(
            &self.visual.design,
            Color::rgba(255, 255, 255, 255),
            242,
        ));
        ui.place(
            "bsod-clock",
            &aegis_design::materials::chrome_place(
                Rect {
                    x: layout.clock_x,
                    y: layout.clock_y,
                    w: layout.clock_width,
                    h: layout.clock_size * 1.4,
                },
                aligned_layer(Align::End),
            ),
            |ui| ui.label_compact_sized(clock, layout.clock_size),
        );
    }

    fn paint_identity(&self, ui: &mut lens::Frame, frame: &FramePresentation<'_>) {
        let layout = bsod_layout(frame.logical.0 as f32, frame.logical.1 as f32);
        let alpha = (255.0 * frame.progress) as u8;
        let white = lock_theme(&self.visual.design, Color::rgba(255, 255, 255, 255), alpha);
        let copy = BsodCopy::for_state(frame.state);
        let counter = counter_value(frame.state);

        // Sad face. Rendered as text so it inherits the platform font
        // exactly the way the classic page did.
        ui.set_theme(white);
        ui.place(
            "bsod-face",
            &aegis_design::materials::chrome_place(
                Rect {
                    x: layout.margin_x,
                    y: layout.face_y,
                    w: layout.face_size * 2.0,
                    h: layout.face_size * 1.2,
                },
                aligned_layer(Align::Start),
            ),
            |ui| ui.label_compact_sized(copy.face, layout.face_size),
        );

        // Headline.
        ui.place(
            "bsod-headline",
            &aegis_design::materials::chrome_place(
                Rect {
                    x: layout.margin_x + frame.feedback_offset,
                    y: layout.headline_y,
                    w: layout.copy_width,
                    h: layout.headline_size * 3.0,
                },
                aligned_layer(Align::Start),
            ),
            |ui| {
                ui.label_wrapped_sized(&copy.headline, layout.headline_size, layout.copy_width);
            },
        );

        // The counter line narrates typing, verification, and rejection in
        // the page's own voice — no input box, no separate status row.
        let counter_color = if frame.state.rejected() {
            Color::rgba(255, 176, 176, 255)
        } else {
            Color::rgba(255, 255, 255, 255)
        };
        ui.set_theme(lock_theme(&self.visual.design, counter_color, alpha));
        ui.place(
            "bsod-counter",
            &aegis_design::materials::chrome_place(
                Rect {
                    x: layout.margin_x + frame.feedback_offset,
                    y: layout.counter_y,
                    w: layout.copy_width,
                    h: layout.counter_size * 1.4,
                },
                aligned_layer(Align::Start),
            ),
            |ui| {
                ui.label_compact_sized(
                    &counter_line(&copy, counter, frame.state),
                    layout.counter_size,
                );
            },
        );

        // Typed characters echo as narrow marks directly under the counter,
        // tying the input to the progress it produced.
        let marks = if frame.state.password_len() > 0 {
            "| ".repeat(frame.state.password_len())
                .trim_end()
                .to_owned()
        } else {
            String::new()
        };
        if !marks.is_empty() {
            ui.place(
                "bsod-marks",
                &aegis_design::materials::chrome_place(
                    Rect {
                        x: layout.margin_x,
                        y: layout.marks_y,
                        w: layout.copy_width,
                        h: layout.counter_size * 1.2,
                    },
                    aligned_layer(Align::Start),
                ),
                |ui| {
                    ui.label_compact_sized(
                        &credential_label(&marks, frame.state.credential_revision()),
                        layout.counter_size,
                    );
                },
            );
        }

        // Support block pinned lower-left, beside the QR.
        ui.set_theme(white);
        let support_lines = copy.support_lines();
        let mut support_y = layout.support_y;
        for (index, line) in support_lines.iter().enumerate() {
            ui.place(
                &format!("bsod-support-{index}"),
                &aegis_design::materials::chrome_place(
                    Rect {
                        x: layout.support_x,
                        y: support_y,
                        w: layout.support_width,
                        h: layout.support_size * 1.5,
                    },
                    aligned_layer(Align::Start),
                ),
                |ui| ui.label_compact_sized(line, layout.support_size),
            );
            support_y += layout.support_size * 1.5;
        }
        if let Some(keyboard) = keyboard_status(frame.state) {
            ui.place(
                "bsod-keyboard",
                &aegis_design::materials::chrome_place(
                    Rect {
                        x: layout.support_x,
                        y: support_y,
                        w: layout.support_width,
                        h: layout.support_size * 1.5,
                    },
                    aligned_layer(Align::Start),
                ),
                |ui| {
                    ui.set_theme(lock_theme(
                        &self.visual.design,
                        Color::rgba(255, 255, 255, 255),
                        170,
                    ));
                    ui.label_compact_sized(&keyboard, layout.support_size);
                },
            );
        }
    }

    fn animates_while_engaged(&self, state: &LockState) -> bool {
        !self.visual.reduced_motion && state.password_len() == 0 && !state.validation_pending()
    }
}

/// Percentage value shown by the counter.
///
/// The number is the authentication story itself: typing advances it by
/// three percent per character, verification holds it while the
/// authenticator works, and rejection hands it back to the idle cycle.
pub fn counter_value(state: &LockState) -> u32 {
    if state.validation_pending() {
        // Freeze at the typed progress while the authenticator works; the
        // counter never claims completion it cannot guarantee.
        typed_progress(state)
    } else if state.rejected() {
        // A rejected attempt restarts the loop from the top.
        0
    } else if state.password_len() > 0 {
        typed_progress(state)
    } else {
        idle_cycle_value()
    }
}

/// Three percent per typed character, saturating just below completion.
fn typed_progress(state: &LockState) -> u32 {
    (state.password_len() as u32 * PERCENT_PER_CHARACTER).min(99)
}

fn idle_cycle_value() -> u32 {
    // The idle loop rides the wall clock: a monotonic `Instant` is only
    // meaningful against a stored baseline, and the renderer is stateless
    // across frames.
    let phase = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f32() % IDLE_CYCLE)
        / IDLE_CYCLE;
    let eased = 0.5 - (phase * std::f32::consts::PI).cos() * 0.5;
    (eased * 100.0).round() as u32
}

/// All stop-screen copy for one authentication state.
struct BsodCopy {
    face: &'static str,
    headline: String,
    counter_lead: String,
    counter_idle: String,
    support_intro: String,
    stop_code: String,
}

impl BsodCopy {
    fn for_state(state: &LockState) -> Self {
        let (headline, stop_code) = if state.rejected() {
            (
                localized(
                    "Your PC ran into a problem and needs to restart. Just kidding — the password was wrong.",
                    "你的电脑遇到问题,需要重新启动。开个玩笑——密码不对。",
                ),
                localized("CREDENTIAL_MISMATCH", "凭据不匹配"),
            )
        } else if state.message().is_some() {
            (
                localized(
                    "Your session ran into a problem it cannot fix alone. Authentication is unavailable.",
                    "你的会话遇到了无法独自修复的问题。认证服务不可用。",
                ),
                localized("AUTH_SERVICE_UNAVAILABLE", "认证服务不可用"),
            )
        } else {
            (
                localized(
                    "Your session has been locked. Type your password to continue.",
                    "你的会话已锁定。输入密码以继续。",
                ),
                localized("SESSION_LOCKED", "会话已锁定"),
            )
        };
        Self {
            face: ":(",
            headline,
            counter_lead: localized("Collecting keystrokes, and then", "正在收集按键,然后"),
            counter_idle: localized("complete", "完成"),
            support_intro: localized(
                "For more information about this issue, scan the code:",
                "有关此问题的详细信息,请扫描此码:",
            ),
            stop_code,
        }
    }

    fn support_lines(&self) -> Vec<String> {
        vec![
            self.support_intro.clone(),
            format!("Stop code: {}", self.stop_code),
        ]
    }
}

fn counter_line(copy: &BsodCopy, counter: u32, state: &LockState) -> String {
    if state.validation_pending() {
        localized("Verifying your identity…", "正在验证你的身份…")
    } else if state.rejected() {
        localized(
            "Restarting the collection: 0% complete",
            "重新开始收集:0% 完成",
        )
    } else {
        format!("{} {}% {}", copy.counter_lead, counter, copy.counter_idle)
    }
}

/// Paint the easter-egg QR module grid beside the support block.
fn paint_qr(canvas: &Canvas, layout: &aegis_lock::BsodLayout, scale: f32, alpha_progress: f32) {
    let payload = localized("Try turning it off and on again :)", "试试关机再开机 :)");
    let Ok(matrix) = qr::encode(&payload) else {
        return;
    };
    let module = layout.qr_size / qr::MODULES as f32;
    let origin_x = (layout.qr_x - QR_QUIET_ZONE * module) * scale;
    let origin_y = (layout.qr_y - QR_QUIET_ZONE * module) * scale;
    let quiet = QR_QUIET_ZONE * module * scale;
    let alpha = (alpha_progress.clamp(0.0, 1.0) * 255.0) as u8;
    // Quiet zone first so dark modules sit on a clean light field.
    canvas.fill_rect(
        origin_x,
        origin_y,
        layout.qr_size * scale + quiet * 2.0,
        layout.qr_size * scale + quiet * 2.0,
        flux::rgba(255, 255, 255, alpha),
    );
    for y in 0..qr::MODULES {
        for x in 0..qr::MODULES {
            if !matrix[y * qr::MODULES + x] {
                continue;
            }
            canvas.fill_rect(
                origin_x + quiet + x as f32 * module * scale,
                origin_y + quiet + y as f32 * module * scale,
                module * scale,
                module * scale,
                flux::rgba(BSOD_BLUE[0], BSOD_BLUE[1], BSOD_BLUE[2], alpha),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aegis_lock::{AuthResult, LockAction, LockState};
    use std::time::Instant;

    #[test]
    fn counter_grants_three_percent_per_character() {
        let now = Instant::now();
        let mut lock = LockState::new(now);
        assert_eq!(counter_value(&lock), idle_cycle_value());
        assert!(lock.type_text("abcde", now));
        assert_eq!(counter_value(&lock), 15);
        // Long credentials saturate below completion.
        let mut long = LockState::new(now);
        assert!(long.type_text(&"x".repeat(40), now));
        assert_eq!(typed_progress(&long), 99);
    }

    #[test]
    fn counter_holds_typed_progress_while_verifying() {
        let now = Instant::now();
        let mut lock = LockState::new(now);
        assert!(lock.type_text("secret", now));
        assert!(matches!(lock.submit(now), LockAction::Authenticate(_)));
        let held = counter_value(&lock);
        assert_eq!(held, 18);
    }

    #[test]
    fn rejection_restarts_the_collection_story() {
        let now = Instant::now();
        let mut lock = LockState::new(now);
        assert!(lock.type_text("wrong", now));
        assert!(matches!(lock.submit(now), LockAction::Authenticate(_)));
        assert!(matches!(
            lock.authentication_finished(
                AuthResult::Rejected {
                    message: "Incorrect password".into()
                },
                now
            ),
            LockAction::None
        ));
        assert!(lock.rejected());
        assert_eq!(counter_value(&lock), 0);
        let copy = BsodCopy::for_state(&lock);
        assert!(
            copy.stop_code == "CREDENTIAL_MISMATCH" || copy.stop_code == "凭据不匹配",
            "unexpected stop code {}",
            copy.stop_code
        );
        assert!(copy.headline.contains("password") || copy.headline.contains("密码"));
    }

    #[test]
    fn unavailable_authentication_gets_its_own_stop_code() {
        let now = Instant::now();
        let mut lock = LockState::new(now);
        assert!(lock.type_text("x", now));
        assert!(matches!(lock.submit(now), LockAction::Authenticate(_)));
        assert!(matches!(
            lock.authentication_finished(
                AuthResult::Unavailable {
                    message: "no PAM".into()
                },
                now
            ),
            LockAction::None
        ));
        let copy = BsodCopy::for_state(&lock);
        assert!(copy.stop_code == "AUTH_SERVICE_UNAVAILABLE" || copy.stop_code == "认证服务不可用");
    }

    #[test]
    fn qr_payloads_stay_within_capacity() {
        assert!(qr::encode("Try turning it off and on again :)").is_ok());
        assert!(qr::encode("试试关机再开机 :)").is_ok());
    }
}
