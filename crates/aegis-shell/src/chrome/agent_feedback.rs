//! Trusted visual feedback for input applied by an Agent Interaction Domain.
//!
//! The physical user's XDG cursor remains untouched. This component projects
//! each applied operation onto the human's read-only mirror as a
//! semi-transparent mask plus a text label naming the external operation, so
//! the observer still sees the window content underneath. An arrow-cursor
//! sprite marks the applied pointer position and a simplified mouse sprite
//! with the pressed button highlighted marks clicks and scrolls; both sprites
//! render below the mask and label. If the target is not visible, a compact
//! background-operation pill is drawn instead. Colors come from the shared
//! design tokens — operations are identified by the mask, sprite shape, and
//! text, never by per-domain hues. Because this is Shell chrome, directed
//! Interaction Domain capture never includes it and an Agent cannot steer from
//! its own feedback layer.
//!
//! The projection is window-scoped today: the feedback region is the mirror
//! window's rectangle. The [`OperationRegion`] seam keeps a future
//! workspace-scoped domain (an entire workspace handed to an Interaction
//! Domain) open without changing the mask, sprite, or label drawing.

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
const CLICK_GLYPH_FOR: Duration = Duration::from_millis(650);
const SCROLL_GLYPH_FOR: Duration = Duration::from_millis(450);
const LABEL_HEIGHT: f32 = 28.0;
const BACKGROUND_HEIGHT: f32 = 34.0;
const CURSOR_SIZE: f32 = 22.0;
/// Bibata `left_ptr` hotspot (55, 17) in its 256-unit view box.
const CURSOR_HOTSPOT: (f32, f32) = (55.0 / 256.0, 17.0 / 256.0);
const CLICK_SIZE: (f32, f32) = (21.0, 30.0);
/// The click point lands between the mouse buttons, not the body center.
const CLICK_ANCHOR: (f32, f32) = (0.5, 0.175);

// Arrow cursor: Bibata Modern Ice `left_ptr` (GPL-3.0), the same theme the
// compositor's software cursor embeds under `assets/cursors/`. The Agent
// pointer is an ordinary arrow by design — the mask and text carry the
// operation signal, not an exotic marker shape.
const CURSOR_SVG: &str = include_str!("../../assets/agent/cursor.svg");
// Simplified mouse glyphs authored for this component: one shared body with
// the left, right, or middle (wheel) button highlighted.
const MOUSE_LEFT_SVG: &str = include_str!("../../assets/agent/mouse-left.svg");
const MOUSE_RIGHT_SVG: &str = include_str!("../../assets/agent/mouse-right.svg");
const MOUSE_MIDDLE_SVG: &str = include_str!("../../assets/agent/mouse-middle.svg");

/// Where an applied Agent operation is projected for the human observer.
///
/// Window-scoped today: the region is the visible rectangle of the read-only
/// mirror of the Agent-controlled window. A future workspace-scoped domain
/// would resolve to a whole-output rectangle here; the mask, sprite, and
/// label rendering below is region-agnostic.
#[derive(Debug, Clone, Copy, PartialEq)]
enum OperationRegion {
    /// Mirror-window rectangle clipped to the display, with the corner
    /// radius that matches the window's presentation state.
    Window { rect: Rect, radius: f32 },
}

/// Raster sprites uploaded once from the embedded SVGs. `None` only when the
/// GPU upload fails (or in tests); the feedback then falls back to a plain
/// neutral dot so the applied position is still visible.
struct AgentSprites {
    cursor: flux::Image,
    click_left: flux::Image,
    click_right: flux::Image,
    click_middle: flux::Image,
}

impl AgentSprites {
    fn upload(device: &flux::Device) -> Option<Self> {
        Some(Self {
            cursor: upload_sprite(device, CURSOR_SVG, 96, 96)?,
            click_left: upload_sprite(device, MOUSE_LEFT_SVG, 84, 120)?,
            click_right: upload_sprite(device, MOUSE_RIGHT_SVG, 84, 120)?,
            click_middle: upload_sprite(device, MOUSE_MIDDLE_SVG, 84, 120)?,
        })
    }

    fn glyph(&self, glyph: PointerGlyph) -> &flux::Image {
        match glyph {
            PointerGlyph::Cursor => &self.cursor,
            PointerGlyph::ClickLeft => &self.click_left,
            PointerGlyph::ClickRight => &self.click_right,
            PointerGlyph::ClickMiddle => &self.click_middle,
        }
    }
}

/// Which pointer sprite an applied operation shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PointerGlyph {
    Cursor,
    ClickLeft,
    ClickRight,
    ClickMiddle,
}

/// Non-interactive, compositor-owned projection of Agent input activity.
pub struct AgentFeedback {
    interaction_domains: InteractionDomainSnapshot,
    activity: BTreeMap<InteractionDomainId, VisualActivity>,
    /// The design snapshot the feedback layer paints from, from
    /// [`ChromeUpdate::Appearance`]. Seeded on registration by
    /// [`crate::Shell::add`] and refreshed when the desktop color scheme
    /// changes; defaults to the dark appearance until the first update arrives.
    design: Design,
    sprites: Option<AgentSprites>,
}

#[derive(Debug, Clone)]
struct VisualActivity {
    latest: AgentActivity,
    latest_at: Instant,
    pointer_window: Option<WindowId>,
    pointer_position: Option<Point>,
    click_pulse: Option<ClickPulse>,
}

#[derive(Debug, Clone, Copy)]
struct ClickPulse {
    position: Point,
    button: u32,
    at: Instant,
}

impl AgentFeedback {
    /// Construct the feedback layer, uploading the pointer sprites through
    /// the composition root's flux device. The device is borrowed for the
    /// upload only; the root declares it before the shell and drops it after.
    #[must_use]
    pub fn new(device: &flux::Device) -> Self {
        Self::with_sprites(AgentSprites::upload(device))
    }

    fn with_sprites(sprites: Option<AgentSprites>) -> Self {
        Self {
            interaction_domains: aegis_model::interaction_domain::InteractionDomainModel::new()
                .snapshot(),
            activity: BTreeMap::new(),
            design: Design::dark(),
            sprites,
        }
    }

    #[cfg(test)]
    fn without_sprites() -> Self {
        Self::with_sprites(None)
    }

    #[cfg(test)]
    fn update_agent_activity(&mut self, activity: &AgentActivity) {
        <Self as Chrome>::update(self, ChromeUpdate::AgentActivity(activity));
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
        let sprites = self.sprites.as_ref();
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
                .filter(|(_, position)| point_in_display(*position, display))
                .and_then(|(window, position)| {
                    operation_region(windows, window, position, display)
                        .map(|region| (region, position))
                });

            if let Some((region, position)) = projected {
                render_pointer_feedback(
                    f,
                    *interaction_domain,
                    activity,
                    region,
                    position,
                    display,
                    interaction_domain_state,
                    alpha,
                    now,
                    i18n,
                    design,
                    sprites,
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
                            state.pointer_window = Some(activity.window);
                            state.pointer_position = Some(position);
                            if let AgentInputKind::Click { button } = activity.kind {
                                state.click_pulse = Some(ClickPulse {
                                    position,
                                    button,
                                    at: now,
                                });
                            }
                        } else if state.pointer_window != Some(activity.window) {
                            state.pointer_window = None;
                            state.pointer_position = None;
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
                                click_pulse: activity.position.and_then(|position| {
                                    if let AgentInputKind::Click { button } = activity.kind {
                                        Some(ClickPulse {
                                            position,
                                            button,
                                            at: now,
                                        })
                                    } else {
                                        None
                                    }
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

/// Resolve the feedback region for an applied pointer position: the visible
/// rectangle of the read-only mirror the position landed in.
fn operation_region(
    windows: &[Window],
    window: WindowId,
    position: Point,
    display: (f32, f32),
) -> Option<OperationRegion> {
    let window = windows.iter().find(|candidate| {
        candidate.id == window
            && candidate.read_only
            && !candidate.minimized
            && window_contains(candidate, position)
    })?;
    let left = (window.position.x as f32).max(0.0);
    let top = (window.position.y as f32).max(0.0);
    let right = (window.position.x as f32 + window.size.w as f32).min(display.0);
    let bottom = (window.position.y as f32 + window.size.h as f32).min(display.1);
    (right > left && bottom > top).then_some(OperationRegion::Window {
        rect: Rect {
            x: left,
            y: top,
            w: right - left,
            h: bottom - top,
        },
        radius: if window.state.fullscreen { 0.0 } else { 7.0 },
    })
}

#[allow(clippy::too_many_arguments)]
fn render_pointer_feedback(
    f: &mut Frame,
    interaction_domain: InteractionDomainId,
    activity: &VisualActivity,
    region: OperationRegion,
    position: Point,
    display: (f32, f32),
    interaction_domain_state: InteractionDomainState,
    alpha: u8,
    now: Instant,
    i18n: &Localizer,
    design: &Design,
    sprites: Option<&AgentSprites>,
) {
    // Pointer sprite first: it stays below the mask and label so the
    // semi-transparent overlay never hides where the operation landed.
    let glyph = pointer_glyph(activity, position, now);
    render_glyph(f, interaction_domain, glyph, position, alpha, sprites);

    // The mask marks the operated window as externally driven while keeping
    // its content readable.
    let OperationRegion::Window { rect, radius } = region;
    render_shape(
        f,
        &format!("aegis-agent-mask-{}", interaction_domain.0),
        rect,
        design.colors.scrim.with_alpha(scaled_alpha(alpha, 6, 17)),
        design
            .colors
            .application_border
            .with_alpha(scaled_alpha(alpha, 2, 3)),
        1.0,
        radius,
    );

    let label = activity_label(&activity.latest, interaction_domain_state, i18n, true);
    let measured = f.measure_text(&label, design.typography.footnote).width;
    let width = (measured + 20.0)
        .clamp(128.0, 290.0)
        .min((display.0 - 16.0).max(1.0));
    let label_rect = pointer_label_rect(position, width, display);
    let label = ellipsize(
        f,
        &label,
        design.typography.footnote,
        (label_rect.w - 14.0).max(0.0),
    );
    f.place(
        &format!("aegis-agent-label-{}", interaction_domain.0),
        &chrome_place(
            label_rect,
            LayoutOpts {
                bg: design
                    .colors
                    .application_surface
                    .with_alpha(scaled_alpha(alpha, 9, 10)),
                border: design
                    .colors
                    .application_border
                    .with_alpha(scaled_alpha(alpha, 3, 4)),
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
                |f| f.label_compact_sized(&label, design.typography.footnote),
            );
        },
    );
}

fn render_glyph(
    f: &mut Frame,
    interaction_domain: InteractionDomainId,
    glyph: PointerGlyph,
    position: Point,
    alpha: u8,
    sprites: Option<&AgentSprites>,
) {
    let id = format!("aegis-agent-glyph-{}", interaction_domain.0);
    let rect = glyph_rect(glyph, position);
    match sprites {
        Some(sprites) => {
            let image = sprites.glyph(glyph);
            f.place(
                &id,
                &chrome_place(rect, LayoutOpts::default()),
                |f| unsafe {
                    // SAFETY: the sprite textures are owned by this component and
                    // outlive the frame's `Ui::render`.
                    f.image_tinted(
                        image.as_raw() as *mut lens::sys::flux_image,
                        rect.w,
                        rect.h,
                        Color::rgba(255, 255, 255, alpha),
                    );
                },
            );
        }
        None => {
            render_shape(
                f,
                &id,
                centered_rect(position, 8.0),
                Color::rgba(244, 246, 252, alpha),
                Color::TRANSPARENT,
                0.0,
                4.0,
            );
        }
    }
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
    let measured = f.measure_text(&label, design.typography.footnote).width;
    let width = (measured + 28.0)
        .clamp(190.0, 360.0)
        .min((display.0 - 16.0).max(1.0));
    let rect = Rect {
        x: ((display.0 - width) * 0.5).max(8.0),
        y: HUD_HEIGHT + 10.0 + index as f32 * (BACKGROUND_HEIGHT + 7.0),
        w: width,
        h: BACKGROUND_HEIGHT,
    };
    let label = ellipsize(
        f,
        &label,
        design.typography.footnote,
        (rect.w - 31.0).max(0.0),
    );
    let border = design
        .colors
        .application_border
        .with_alpha(scaled_alpha(alpha, 1, 2));
    f.place(
        &format!("aegis-agent-background-{}", interaction_domain.0),
        &chrome_place(
            rect,
            LayoutOpts {
                bg: design
                    .colors
                    .application_surface
                    .with_alpha(scaled_alpha(alpha, 9, 10)),
                border,
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
                            bg: design.colors.menu_heading.with_alpha(alpha),
                            radius: 3.5,
                            ..Default::default()
                        },
                        |_| {},
                    );
                    f.label_compact_sized(&label, design.typography.footnote);
                },
            );
        },
    );
}

/// Which pointer sprite to show at the applied position: a brief mouse glyph
/// with the pressed button highlighted for clicks, the wheel for fresh
/// scrolls, and the plain arrow cursor otherwise.
fn pointer_glyph(activity: &VisualActivity, position: Point, now: Instant) -> PointerGlyph {
    if let Some(pulse) = activity.click_pulse
        && pulse.position == position
        && now.saturating_duration_since(pulse.at) < CLICK_GLYPH_FOR
    {
        return match pulse.button {
            0x111 => PointerGlyph::ClickRight,
            0x112 => PointerGlyph::ClickMiddle,
            _ => PointerGlyph::ClickLeft,
        };
    }
    if matches!(activity.latest.kind, AgentInputKind::Scroll { .. })
        && now.saturating_duration_since(activity.latest_at) < SCROLL_GLYPH_FOR
    {
        return PointerGlyph::ClickMiddle;
    }
    PointerGlyph::Cursor
}

fn glyph_rect(glyph: PointerGlyph, position: Point) -> Rect {
    match glyph {
        PointerGlyph::Cursor => Rect {
            x: position.x as f32 - CURSOR_SIZE * CURSOR_HOTSPOT.0,
            y: position.y as f32 - CURSOR_SIZE * CURSOR_HOTSPOT.1,
            w: CURSOR_SIZE,
            h: CURSOR_SIZE,
        },
        PointerGlyph::ClickLeft | PointerGlyph::ClickRight | PointerGlyph::ClickMiddle => Rect {
            x: position.x as f32 - CLICK_SIZE.0 * CLICK_ANCHOR.0,
            y: position.y as f32 - CLICK_SIZE.1 * CLICK_ANCHOR.1,
            w: CLICK_SIZE.0,
            h: CLICK_SIZE.1,
        },
    }
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

/// Rasterize an embedded SVG to premultiplied BGRA8 and upload it as one
/// sprite texture. Same path-only pipeline as the compositor's software
/// cursor: tiny-skia emits premultiplied RGBA8, flux samples premultiplied
/// BGRA8, so the red and blue channels swap on upload.
fn upload_sprite(device: &flux::Device, svg: &str, width: u32, height: u32) -> Option<flux::Image> {
    let tree = usvg::Tree::from_data(svg.as_bytes(), &usvg::Options::default()).ok()?;
    let mut pixmap = tiny_skia::Pixmap::new(width, height)?;
    let size = tree.size();
    let transform = tiny_skia::Transform::from_scale(
        width as f32 / size.width(),
        height as f32 / size.height(),
    );
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    let mut pixels = pixmap.take();
    for chunk in pixels.chunks_exact_mut(4) {
        chunk.swap(0, 2);
    }
    flux::Image::from_bytes(
        device,
        width,
        height,
        flux::Format::FLUX_FORMAT_BGRA8_UNORM,
        &pixels,
    )
    .ok()
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

fn pointer_label_rect(position: Point, width: f32, display: (f32, f32)) -> Rect {
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

    fn visual_of(activity: AgentActivity) -> VisualActivity {
        let now = Instant::now();
        VisualActivity {
            click_pulse: activity.position.and_then(|position| {
                if let AgentInputKind::Click { button } = activity.kind {
                    Some(ClickPulse {
                        position,
                        button,
                        at: now,
                    })
                } else {
                    None
                }
            }),
            latest_at: now,
            latest: activity,
            pointer_window: None,
            pointer_position: None,
        }
    }

    #[test]
    fn keyboard_activity_keeps_same_window_pointer_without_exposing_a_key() {
        let mut feedback = AgentFeedback::without_sprites();
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
        let mut feedback = AgentFeedback::without_sprites();
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
    fn region_projects_only_inside_a_read_only_human_mirror() {
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
    fn click_glyphs_highlight_the_pressed_button() {
        let position = Point { x: 50, y: 50 };
        let now = Instant::now();
        for (button, glyph) in [
            (0x110, PointerGlyph::ClickLeft),
            (0x111, PointerGlyph::ClickRight),
            (0x112, PointerGlyph::ClickMiddle),
        ] {
            let visual = visual_of(activity(
                1,
                AgentInputKind::Click { button },
                Some(position),
            ));
            assert_eq!(pointer_glyph(&visual, position, now), glyph);
        }
        // The glyph is transient: it falls back to the arrow cursor.
        let mut visual = visual_of(activity(
            1,
            AgentInputKind::Click { button: 0x110 },
            Some(position),
        ));
        visual.click_pulse = visual.click_pulse.map(|pulse| ClickPulse {
            at: pulse.at - CLICK_GLYPH_FOR,
            ..pulse
        });
        assert_eq!(
            pointer_glyph(&visual, position, Instant::now()),
            PointerGlyph::Cursor
        );
        // A click somewhere else is not where the pointer is now.
        let visual = visual_of(activity(
            2,
            AgentInputKind::Click { button: 0x110 },
            Some(Point { x: 10, y: 10 }),
        ));
        assert_eq!(pointer_glyph(&visual, position, now), PointerGlyph::Cursor);
    }

    #[test]
    fn fresh_scrolls_show_the_wheel_glyph() {
        let position = Point { x: 50, y: 50 };
        let visual = visual_of(activity(
            1,
            AgentInputKind::Scroll { dx: 0.0, dy: -1.0 },
            Some(position),
        ));
        assert_eq!(
            pointer_glyph(&visual, position, Instant::now()),
            PointerGlyph::ClickMiddle
        );
        let mut stale = visual.clone();
        stale.latest_at -= SCROLL_GLYPH_FOR;
        assert_eq!(
            pointer_glyph(&stale, position, Instant::now()),
            PointerGlyph::Cursor
        );
        let movement = visual_of(activity(2, AgentInputKind::PointerMove, Some(position)));
        assert_eq!(
            pointer_glyph(&movement, position, Instant::now()),
            PointerGlyph::Cursor
        );
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
