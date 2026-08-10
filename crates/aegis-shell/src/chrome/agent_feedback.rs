//! Trusted visual feedback for input applied by an Agent Interaction Domain.
//!
//! The physical user's XDG cursor remains untouched. This component draws a
//! separate crosshair, click pulse, movement trail, and operation label over a
//! read-only human mirror. If the target is not visible, it draws a compact
//! background-operation pill instead. Because this is Shell chrome, directed
//! Interaction Domain capture never includes it and an Agent cannot steer from its own
//! feedback layer.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use aegis_design::Design;
use aegis_design::materials::{chrome_place, surface_layout};
use aegis_model::Point;
use aegis_model::interaction_domain::{
    InteractionDomainId, InteractionDomainSnapshot, InteractionDomainState,
};
use aegis_model::window::{Window, WindowId};
use aegis_model::workspace::WorkspaceSnapshot;
use lens::{Align, Color, Frame, Input, LayoutOpts, Rect};

use crate::{
    AgentActivity, AgentInputKind, Chrome, ChromeEvents, ChromeUpdate, HUD_HEIGHT, Localizer,
    Message, ellipsize,
};

const HOLD_FOR: Duration = Duration::from_secs(4);
const FADE_FOR: Duration = Duration::from_secs(2);
const VISIBLE_FOR: Duration = Duration::from_secs(6);
const CLICK_PULSE_FOR: Duration = Duration::from_millis(650);
const TRAIL_FOR: Duration = Duration::from_millis(360);
const MARKER_DIAMETER: f32 = 24.0;
const LABEL_HEIGHT: f32 = 28.0;
const BACKGROUND_HEIGHT: f32 = 34.0;

/// Non-interactive, compositor-owned projection of Agent input activity.
pub struct AgentFeedback {
    interaction_domains: InteractionDomainSnapshot,
    activity: BTreeMap<InteractionDomainId, VisualActivity>,
    /// The design snapshot the feedback layer paints from, from
    /// [`ChromeUpdate::Appearance`]. Seeded on registration by
    /// [`crate::Shell::add`] and refreshed when the desktop color scheme
    /// changes; defaults to the dark appearance until the first update arrives.
    design: Design,
}

#[derive(Debug, Clone)]
struct VisualActivity {
    latest: AgentActivity,
    latest_at: Instant,
    pointer_window: Option<WindowId>,
    pointer_position: Option<Point>,
    previous_pointer: Option<Point>,
    pointer_at: Option<Instant>,
    click_pulse: Option<ClickPulse>,
}

#[derive(Debug, Clone, Copy)]
struct ClickPulse {
    position: Point,
    at: Instant,
}

impl AgentFeedback {
    /// Construct an empty feedback layer.
    #[must_use]
    pub fn new() -> Self {
        Self {
            interaction_domains: aegis_model::interaction_domain::InteractionDomainModel::new()
                .snapshot(),
            activity: BTreeMap::new(),
            design: Design::dark(),
        }
    }

    #[cfg(test)]
    fn update_agent_activity(&mut self, activity: &AgentActivity) {
        <Self as Chrome>::update(self, ChromeUpdate::AgentActivity(activity));
    }
}

impl Default for AgentFeedback {
    fn default() -> Self {
        Self::new()
    }
}

impl Chrome for AgentFeedback {
    fn render(
        &mut self,
        f: &mut Frame,
        input: &Input,
        windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
        i18n: &Localizer,
        _out: &mut ChromeEvents,
    ) {
        let now = Instant::now();
        let live_interaction_domains = self
            .interaction_domains
            .interaction_domains
            .iter()
            .filter(|interaction_domain| {
                interaction_domain.state != InteractionDomainState::Revoked
            })
            .map(|interaction_domain| interaction_domain.id)
            .collect::<std::collections::BTreeSet<_>>();
        self.activity.retain(|interaction_domain, activity| {
            live_interaction_domains.contains(interaction_domain)
                && now.saturating_duration_since(activity.latest_at) < VISIBLE_FOR
        });

        let raw = input.as_raw();
        let display = (raw.display_size.x.max(1.0), raw.display_size.y.max(1.0));
        let interaction_domains = &self.interaction_domains;
        let design = &self.design;
        let mut background = Vec::new();
        for (interaction_domain, activity) in &self.activity {
            let interaction_domain_state = interaction_domains
                .interaction_domains
                .iter()
                .find(|candidate| candidate.id == *interaction_domain)
                .map(|candidate| candidate.state)
                .unwrap_or(InteractionDomainState::Revoked);
            let age = now.saturating_duration_since(activity.latest_at);
            let alpha = activity_alpha(age, interaction_domain_state);
            let projected = activity
                .pointer_window
                .zip(activity.pointer_position)
                .filter(|(window, position)| {
                    windows.iter().any(|candidate| {
                        candidate.id == *window
                            && candidate.read_only
                            && !candidate.minimized
                            && window_contains(candidate, *position)
                    }) && point_in_display(*position, display)
                });

            if let Some((_, position)) = projected {
                render_pointer_feedback(
                    f,
                    *interaction_domain,
                    activity,
                    position,
                    display,
                    interaction_domain_state,
                    alpha,
                    now,
                    i18n,
                    design,
                );
            } else {
                background.push((
                    *interaction_domain,
                    activity,
                    interaction_domain_state,
                    alpha,
                ));
            }
        }

        for (index, (interaction_domain, activity, interaction_domain_state, alpha)) in
            background.into_iter().enumerate()
        {
            render_background_activity(
                f,
                interaction_domain,
                activity,
                interaction_domain_state,
                alpha,
                display,
                index,
                i18n,
                design,
            );
        }
    }

    fn requires_composition(&self) -> bool {
        let now = Instant::now();
        self.activity.iter().any(|(interaction_domain, activity)| {
            now.saturating_duration_since(activity.latest_at) < VISIBLE_FOR
                && self
                    .interaction_domains
                    .interaction_domains
                    .iter()
                    .any(|candidate| {
                        candidate.id == *interaction_domain
                            && candidate.state != InteractionDomainState::Revoked
                    })
        })
    }

    fn update(&mut self, update: ChromeUpdate<'_>) {
        match update {
            ChromeUpdate::InteractionDomains(snapshot) => {
                self.interaction_domains = snapshot.clone();
                self.activity.retain(|interaction_domain, _| {
                    snapshot.interaction_domains.iter().any(|candidate| {
                        candidate.id == *interaction_domain
                            && candidate.state != InteractionDomainState::Revoked
                    })
                });
            }
            ChromeUpdate::Appearance(design) => self.design = *design,
            ChromeUpdate::AgentActivity(activity) => {
                let now = Instant::now();
                match self.activity.get_mut(&activity.interaction_domain) {
                    Some(state) => {
                        if activity.sequence <= state.latest.sequence {
                            return;
                        }
                        if let Some(position) = activity.position {
                            state.previous_pointer = (state.pointer_window
                                == Some(activity.window))
                            .then_some(state.pointer_position)
                            .flatten();
                            state.pointer_window = Some(activity.window);
                            state.pointer_position = Some(position);
                            state.pointer_at = Some(now);
                            if matches!(activity.kind, AgentInputKind::Click { .. }) {
                                state.click_pulse = Some(ClickPulse { position, at: now });
                            }
                        } else if state.pointer_window != Some(activity.window) {
                            state.pointer_window = None;
                            state.pointer_position = None;
                            state.previous_pointer = None;
                            state.pointer_at = None;
                        }
                        state.latest = activity.clone();
                        state.latest_at = now;
                    }
                    None => {
                        self.activity.insert(
                            activity.interaction_domain,
                            VisualActivity {
                                latest: activity.clone(),
                                latest_at: now,
                                pointer_window: activity.position.map(|_| activity.window),
                                pointer_position: activity.position,
                                previous_pointer: None,
                                pointer_at: activity.position.map(|_| now),
                                click_pulse: activity.position.and_then(|position| {
                                    matches!(activity.kind, AgentInputKind::Click { .. })
                                        .then_some(ClickPulse { position, at: now })
                                }),
                            },
                        );
                    }
                }
            }
            _ => {}
        }
    }

    fn anim_pending(&self) -> bool {
        self.activity.values().any(|activity| {
            Instant::now().saturating_duration_since(activity.latest_at) < VISIBLE_FOR
        })
    }
}

#[allow(clippy::too_many_arguments)]
fn render_pointer_feedback(
    f: &mut Frame,
    interaction_domain: InteractionDomainId,
    activity: &VisualActivity,
    position: Point,
    display: (f32, f32),
    interaction_domain_state: InteractionDomainState,
    alpha: u8,
    now: Instant,
    i18n: &Localizer,
    design: &Design,
) {
    let accent = interaction_domain_color(interaction_domain).with_alpha(alpha);
    if let (Some(previous), Some(pointer_at)) = (activity.previous_pointer, activity.pointer_at)
        && now.saturating_duration_since(pointer_at) < TRAIL_FOR
        && previous != position
    {
        for (index, amount) in [0.2_f32, 0.45, 0.7].into_iter().enumerate() {
            let x = previous.x as f32 + (position.x - previous.x) as f32 * amount;
            let y = previous.y as f32 + (position.y - previous.y) as f32 * amount;
            let size = 4.0 + index as f32 * 1.5;
            render_shape(
                f,
                &format!("aegis-agent-trail-{}-{index}", interaction_domain.0),
                Rect {
                    x: x - size * 0.5,
                    y: y - size * 0.5,
                    w: size,
                    h: size,
                },
                accent.with_alpha(alpha.saturating_sub((2 - index) as u8 * 42)),
                Color::TRANSPARENT,
                0.0,
                size * 0.5,
            );
        }
    }

    if let Some(pulse) = activity.click_pulse {
        let pulse_age = now.saturating_duration_since(pulse.at);
        if pulse_age < CLICK_PULSE_FOR && pulse.position == position {
            let progress = pulse_age.as_secs_f32() / CLICK_PULSE_FOR.as_secs_f32();
            let diameter = MARKER_DIAMETER + 10.0 + 24.0 * progress;
            let pulse_alpha = ((1.0 - progress) * f32::from(alpha)).round() as u8;
            render_shape(
                f,
                &format!("aegis-agent-click-pulse-{}", interaction_domain.0),
                centered_rect(position, diameter),
                Color::TRANSPARENT,
                interaction_domain_color(interaction_domain).with_alpha(pulse_alpha),
                2.0,
                diameter * 0.5,
            );
        }
    }

    render_shape(
        f,
        &format!("aegis-agent-marker-{}", interaction_domain.0),
        centered_rect(position, MARKER_DIAMETER),
        design
            .colors
            .application_surface
            .with_alpha(scaled_alpha(alpha, 3, 4)),
        accent,
        2.0,
        MARKER_DIAMETER * 0.5,
    );
    render_shape(
        f,
        &format!("aegis-agent-marker-center-{}", interaction_domain.0),
        centered_rect(position, 6.0),
        accent,
        Color::TRANSPARENT,
        0.0,
        3.0,
    );
    for (suffix, rect) in marker_ticks(position).into_iter() {
        render_shape(
            f,
            &format!("aegis-agent-marker-{suffix}-{}", interaction_domain.0),
            rect,
            accent,
            Color::TRANSPARENT,
            0.0,
            1.0,
        );
    }

    let label = activity_label(&activity.latest, interaction_domain_state, i18n, true);
    let measured = f.measure_text(&label, 11.0).width;
    let width = (measured + 20.0)
        .clamp(128.0, 290.0)
        .min((display.0 - 16.0).max(1.0));
    let label_rect = marker_label_rect(position, width, display);
    let label = ellipsize(f, &label, 11.0, (label_rect.w - 14.0).max(0.0));
    f.place(
        &format!("aegis-agent-label-{}", interaction_domain.0),
        &chrome_place(
            label_rect,
            LayoutOpts {
                bg: design
                    .colors
                    .application_surface
                    .with_alpha(scaled_alpha(alpha, 9, 10)),
                border: accent,
                border_width: 1.0,
                radius: LABEL_HEIGHT * 0.5,
                ..surface_layout()
            },
        ),
        |f| {
            f.column_ex(
                &LayoutOpts {
                    width: label_rect.w,
                    height: label_rect.h,
                    pad: 7.0,
                    cross: Align::Center,
                    ..Default::default()
                },
                |f| f.label_compact_sized(&label, 11.0),
            );
        },
    );
}

#[allow(clippy::too_many_arguments)]
fn render_background_activity(
    f: &mut Frame,
    interaction_domain: InteractionDomainId,
    activity: &VisualActivity,
    interaction_domain_state: InteractionDomainState,
    alpha: u8,
    display: (f32, f32),
    index: usize,
    i18n: &Localizer,
    design: &Design,
) {
    let label = activity_label(&activity.latest, interaction_domain_state, i18n, false);
    let measured = f.measure_text(&label, 11.0).width;
    let width = (measured + 28.0)
        .clamp(190.0, 360.0)
        .min((display.0 - 16.0).max(1.0));
    let rect = Rect {
        x: ((display.0 - width) * 0.5).max(8.0),
        y: HUD_HEIGHT + 10.0 + index as f32 * (BACKGROUND_HEIGHT + 7.0),
        w: width,
        h: BACKGROUND_HEIGHT,
    };
    let label = ellipsize(f, &label, 11.0, (rect.w - 31.0).max(0.0));
    let accent = interaction_domain_color(interaction_domain).with_alpha(alpha);
    f.place(
        &format!("aegis-agent-background-{}", interaction_domain.0),
        &chrome_place(
            rect,
            LayoutOpts {
                bg: design
                    .colors
                    .application_surface
                    .with_alpha(scaled_alpha(alpha, 9, 10)),
                border: accent,
                border_width: 1.0,
                radius: BACKGROUND_HEIGHT * 0.5,
                ..surface_layout()
            },
        ),
        |f| {
            f.row_ex(
                &LayoutOpts {
                    width: rect.w,
                    height: rect.h,
                    gap: 8.0,
                    pad: 8.0,
                    cross: Align::Center,
                    ..Default::default()
                },
                |f| {
                    f.column_ex(
                        &LayoutOpts {
                            width: 7.0,
                            height: 7.0,
                            bg: accent,
                            radius: 3.5,
                            ..Default::default()
                        },
                        |_| {},
                    );
                    f.label_compact_sized(&label, 11.0);
                },
            );
        },
    );
}

fn activity_label(
    activity: &AgentActivity,
    state: InteractionDomainState,
    i18n: &Localizer,
    pointer_visible: bool,
) -> String {
    let interaction_domain = &activity.interaction_domain_label;
    let operation = operation_label(activity.kind, i18n);
    let state_suffix = if state == InteractionDomainState::Paused {
        format!(" · {}", i18n.text(Message::InteractionDomainPaused))
    } else {
        String::new()
    };
    if pointer_visible {
        format!(
            "{} · {interaction_domain} · {operation}{state_suffix}",
            i18n.text(Message::AgentBadge)
        )
    } else {
        format!(
            "{interaction_domain} · {} · {operation}{state_suffix}",
            i18n.text(Message::AgentOperating)
        )
    }
}

fn operation_label(kind: AgentInputKind, i18n: &Localizer) -> &'static str {
    match kind {
        AgentInputKind::PointerMove => i18n.text(Message::AgentPointerMove),
        AgentInputKind::Click { button: 0x111 } => i18n.text(Message::AgentRightClick),
        AgentInputKind::Click { button: 0x112 } => i18n.text(Message::AgentMiddleClick),
        AgentInputKind::Click { .. } => i18n.text(Message::AgentClick),
        AgentInputKind::Scroll { dx, dy } if dy.abs() >= dx.abs() && dy < 0.0 => {
            i18n.text(Message::AgentScrollUp)
        }
        AgentInputKind::Scroll { dx, dy } if dy.abs() >= dx.abs() => {
            i18n.text(Message::AgentScrollDown)
        }
        AgentInputKind::Scroll { dx, .. } if dx < 0.0 => i18n.text(Message::AgentScrollLeft),
        AgentInputKind::Scroll { .. } => i18n.text(Message::AgentScrollRight),
        AgentInputKind::Keyboard => i18n.text(Message::AgentKeyboard),
    }
}

fn activity_alpha(age: Duration, state: InteractionDomainState) -> u8 {
    let state_alpha = if state == InteractionDomainState::Paused {
        150.0
    } else {
        255.0
    };
    if age <= HOLD_FOR {
        return state_alpha as u8;
    }
    let fade = age.saturating_sub(HOLD_FOR).as_secs_f32() / FADE_FOR.as_secs_f32();
    (state_alpha * (1.0 - fade.clamp(0.0, 1.0))).round() as u8
}

fn scaled_alpha(alpha: u8, numerator: u16, denominator: u16) -> u8 {
    let scaled = u16::from(alpha).saturating_mul(numerator) / denominator.max(1);
    u8::try_from(scaled.min(255)).unwrap_or(255)
}

pub(super) fn interaction_domain_color(interaction_domain: InteractionDomainId) -> Color {
    // Intentional content colors: per-domain identity hues that distinguish
    // concurrent Interaction Domains. They are content styling, independent
    // of the desktop color scheme.
    const PALETTE: [(u8, u8, u8); 6] = [
        (92, 214, 255),
        (255, 184, 76),
        (225, 113, 255),
        (91, 224, 151),
        (255, 111, 140),
        (139, 142, 255),
    ];
    let index = usize::try_from(interaction_domain.0 % PALETTE.len() as u64).unwrap_or(0);
    let (r, g, b) = PALETTE[index];
    Color::rgba(r, g, b, 255)
}

fn window_contains(window: &Window, position: Point) -> bool {
    window.size.w > 0
        && window.size.h > 0
        && position.x >= window.position.x
        && position.y >= window.position.y
        && position.x < window.position.x.saturating_add(window.size.w)
        && position.y < window.position.y.saturating_add(window.size.h)
}

fn point_in_display(position: Point, display: (f32, f32)) -> bool {
    position.x >= 0
        && position.y >= 0
        && (position.x as f32) < display.0
        && (position.y as f32) < display.1
}

fn centered_rect(position: Point, diameter: f32) -> Rect {
    Rect {
        x: position.x as f32 - diameter * 0.5,
        y: position.y as f32 - diameter * 0.5,
        w: diameter,
        h: diameter,
    }
}

fn marker_ticks(position: Point) -> [(&'static str, Rect); 4] {
    let x = position.x as f32;
    let y = position.y as f32;
    [
        (
            "north",
            Rect {
                x: x - 1.0,
                y: y - 16.0,
                w: 2.0,
                h: 7.0,
            },
        ),
        (
            "south",
            Rect {
                x: x - 1.0,
                y: y + 9.0,
                w: 2.0,
                h: 7.0,
            },
        ),
        (
            "west",
            Rect {
                x: x - 16.0,
                y: y - 1.0,
                w: 7.0,
                h: 2.0,
            },
        ),
        (
            "east",
            Rect {
                x: x + 9.0,
                y: y - 1.0,
                w: 7.0,
                h: 2.0,
            },
        ),
    ]
}

fn marker_label_rect(position: Point, width: f32, display: (f32, f32)) -> Rect {
    let right = position.x as f32 + 20.0;
    let x = if right + width <= display.0 - 8.0 {
        right
    } else {
        (position.x as f32 - width - 20.0).max(8.0)
    };
    let below = position.y as f32 + 18.0;
    let y = if below + LABEL_HEIGHT <= display.1 - 8.0 {
        below.max(HUD_HEIGHT + 4.0)
    } else {
        (position.y as f32 - LABEL_HEIGHT - 18.0).max(HUD_HEIGHT + 4.0)
    };
    Rect {
        x,
        y,
        w: width,
        h: LABEL_HEIGHT,
    }
}

fn render_shape(
    f: &mut Frame,
    id: &str,
    rect: Rect,
    background: Color,
    border: Color,
    border_width: f32,
    radius: f32,
) {
    f.place(
        id,
        &chrome_place(
            rect,
            LayoutOpts {
                bg: background,
                border,
                border_width,
                radius,
                ..surface_layout()
            },
        ),
        |f| {
            f.column_ex(
                &LayoutOpts {
                    width: rect.w,
                    height: rect.h,
                    ..Default::default()
                },
                |_| {},
            );
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn activity(sequence: u64, kind: AgentInputKind, position: Option<Point>) -> AgentActivity {
        AgentActivity {
            sequence,
            interaction_domain: InteractionDomainId(7),
            interaction_domain_label: "Fuji".into(),
            window: WindowId(42),
            position,
            kind,
        }
    }

    #[test]
    fn keyboard_activity_keeps_same_window_pointer_without_exposing_a_key() {
        let mut feedback = AgentFeedback::new();
        feedback.update_agent_activity(&activity(
            1,
            AgentInputKind::PointerMove,
            Some(Point { x: 120, y: 80 }),
        ));
        feedback.update_agent_activity(&activity(2, AgentInputKind::Keyboard, None));

        let visual = feedback
            .activity
            .get(&InteractionDomainId(7))
            .expect("activity");
        assert_eq!(visual.pointer_position, Some(Point { x: 120, y: 80 }));
        assert_eq!(visual.latest.kind, AgentInputKind::Keyboard);
        assert_eq!(
            operation_label(visual.latest.kind, &Localizer::new("en-US")),
            "Keyboard"
        );
    }

    #[test]
    fn stale_activity_cannot_rewind_visual_state() {
        let mut feedback = AgentFeedback::new();
        feedback.update_agent_activity(&activity(2, AgentInputKind::Keyboard, None));
        feedback.update_agent_activity(&activity(
            1,
            AgentInputKind::Click { button: 0x110 },
            Some(Point { x: 1, y: 2 }),
        ));
        let visual = feedback
            .activity
            .get(&InteractionDomainId(7))
            .expect("activity");
        assert_eq!(visual.latest.sequence, 2);
        assert_eq!(visual.pointer_position, None);
    }

    #[test]
    fn marker_projects_only_inside_a_read_only_human_mirror() {
        let mut window = Window::new(WindowId(42));
        window.position = Point { x: 20, y: 30 };
        window.size = aegis_model::Size { w: 100, h: 80 };
        assert!(window_contains(&window, Point { x: 25, y: 35 }));
        assert!(!window.read_only);
        window.read_only = true;
        assert!(window.read_only && window_contains(&window, Point { x: 25, y: 35 }));
        assert!(!window_contains(&window, Point { x: 120, y: 35 }));
    }

    #[test]
    fn labels_are_localized_and_unicode_safe() {
        let zh = Localizer::new("zh-CN");
        assert_eq!(operation_label(AgentInputKind::Keyboard, &zh), "键盘输入");
        assert_eq!(
            operation_label(AgentInputKind::Click { button: 0x111 }, &zh),
            "右键点击"
        );
        assert_eq!(crate::truncate("智能体正在操作", 5), "智能体正…");
    }
}
