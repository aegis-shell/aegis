//! The agent capability-borrowing consent dialog: a modal, centered panel
//! listing the requested capability families with one checkbox each, so the
//! user approves a subset instead of an all-or-nothing Allow/Deny
//! (ADR-0088 agent pairing). A family row expands to its individual
//! operations for fine-grained review; operations whose first use is always
//! re-confirmed interactively carry a † marker, spelled out once in the
//! legend below the list.
//!
//! The flow mirrors the confirmation dialog: [`ChromeCommand::StartCapabilityPick`]
//! opens the panel, and the user's answer travels back through
//! [`ChromeEvents::capability_pick_answered`] (`approved: Some(keys)` = the
//! checked operations the user allowed, `approved: None` = denied). Ordinary
//! modal chrome over the live scene: no freeze, no screen-content capture.

use std::collections::BTreeSet;

use lens::{Align, Color, Frame, Input, LayoutOpts, Rect};

use crate::{
    BackdropRegion, Chrome, ChromeCommand, ChromeEvents, ChromeUpdate, CursorShape,
    LiquidGlassRegion, Localizer, Reserved, ellipsize,
};
use aegis_design::{Design, GlassRole, materials, themes};
use aegis_model::input::{KeyAction, KeyChar, key_action};
use aegis_model::window::Window;

const PANEL_W: f32 = 460.0;
const PANEL_PAD: f32 = 16.0;
const TITLE_H: f32 = 24.0;
const WARNING_H: f32 = 24.0;
const ROW_H: f32 = 26.0;
const MAX_VISIBLE_ROWS: usize = 14;
const LEGEND_H: f32 = 18.0;
const CHEVRON_W: f32 = 18.0;
const MEMBER_INDENT: f32 = 18.0;
const CHECK: f32 = 15.0;
const BUTTON_H: f32 = 30.0;
const BUTTON_W: f32 = 96.0;
const BACKDROP_BLUR_SIGMA: f32 = 18.0;
/// Rows scrolled per wheel detent over the list.
const WHEEL_ROWS: f32 = 3.0;

/// Marks operations whose first use is confirmed again interactively
/// (ADR-0088); the row shows the dagger, the legend explains it once.
const GATED_MARK: &str = "†";
const GATED_LEGEND: &str = "† confirmed again on first use";

/// Parameters of one capability-borrowing checklist, mapped from the agent
/// pairing request by the compositor runtime.
#[derive(Debug, Clone)]
pub struct CapabilityPickParams {
    /// Dialog heading (e.g. "Codex wants to borrow desktop capabilities").
    pub title: String,
    /// Look-alike installation warning (ADR-0088 TOFU continuity), shown as
    /// a highlighted row under the title when present.
    pub warning: Option<String>,
    /// One row per requested capability family, in display order.
    pub families: Vec<CapabilityFamily>,
}

/// One checkable capability family row; its members are the expandable
/// per-operation detail.
#[derive(Debug, Clone)]
pub struct CapabilityFamily {
    /// Stable machine key, unique within one checklist.
    pub key: String,
    /// Human-readable family description (e.g. "Control windows").
    pub label: String,
    /// The requested operations in this family, in display order.
    pub members: Vec<CapabilityGroup>,
}

/// One checkable operation row inside a family.
#[derive(Debug, Clone)]
pub struct CapabilityGroup {
    /// Stable machine key the runtime maps back to an operation family.
    pub key: String,
    /// Human-readable capability description (e.g. "Focus windows").
    pub label: String,
    /// High-risk operation: first use is confirmed again interactively.
    pub gated: bool,
    /// Initially checked.
    pub enabled: bool,
}

/// The user's answer: the checked operation keys on Allow, or `None` on
/// Deny, `Escape`, or the compositor panic chord.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityPickResult {
    pub approved: Option<Vec<String>>,
}

/// The resolved geometry of the panel for one frame.
#[derive(Debug, Clone)]
struct PromptLayout {
    panel: Rect,
    title: Rect,
    warning: Option<Rect>,
    list: Rect,
    legend: Option<Rect>,
    deny: Rect,
    allow: Rect,
    visible_rows: usize,
}

impl PromptLayout {
    fn for_display(
        display: (f32, f32),
        reserved: Reserved,
        row_count: usize,
        family_count: usize,
        has_warning: bool,
        has_gated: bool,
    ) -> PromptLayout {
        let left = reserved.left.max(0) as f32;
        let top = reserved.top.max(0) as f32;
        let usable_w = (display.0 - left - reserved.right.max(0) as f32).max(1.0);
        let usable_h = (display.1 - top - reserved.bottom.max(0) as f32).max(1.0);

        let panel_w = PANEL_W.min((usable_w - 32.0).max(240.0));
        let warning_block = if has_warning { WARNING_H + 4.0 } else { 0.0 };
        let legend_block = if has_gated { LEGEND_H + 6.0 } else { 0.0 };
        let fixed =
            PANEL_PAD + TITLE_H + warning_block + 10.0 + legend_block + 10.0 + BUTTON_H + PANEL_PAD;
        let max_h = (usable_h - 32.0).max(160.0);
        // The list shows a window over the rows: capped by the display
        // height, by MAX_VISIBLE_ROWS, and by the content itself.
        let row_cap =
            (((max_h - fixed).max(ROW_H) / ROW_H).floor() as usize).clamp(1, MAX_VISIBLE_ROWS);
        let visible_rows = row_cap.min(row_count.max(1));
        let list_h = visible_rows as f32 * ROW_H;
        let panel_h = fixed + list_h;
        // Anchor the panel at the collapsed (families-only) size, so
        // expanding a family grows the panel downward instead of moving the
        // rows out from under the cursor; clamp the bottom into view.
        let anchor_h = fixed + row_cap.min(family_count.max(1)) as f32 * ROW_H;
        let panel_y = (top + ((usable_h - anchor_h) * 0.5).max(0.0))
            .min(top + usable_h - panel_h)
            .max(top);
        let panel = Rect {
            x: left + ((usable_w - panel_w) * 0.5).max(0.0),
            y: panel_y,
            w: panel_w,
            h: panel_h,
        };

        let inner_x = panel.x + PANEL_PAD;
        let inner_w = panel.w - 2.0 * PANEL_PAD;
        let title = Rect {
            x: inner_x,
            y: panel.y + PANEL_PAD,
            w: inner_w,
            h: TITLE_H,
        };
        let warning = has_warning.then_some(Rect {
            x: inner_x,
            y: title.y + title.h + 4.0,
            w: inner_w,
            h: WARNING_H,
        });
        let list_y = warning.map(|r| r.y + r.h).unwrap_or(title.y + title.h) + 10.0;
        let list = Rect {
            x: inner_x,
            y: list_y,
            w: inner_w,
            h: list_h,
        };
        let legend = has_gated.then_some(Rect {
            x: inner_x,
            y: list.y + list.h + 6.0,
            w: inner_w,
            h: LEGEND_H,
        });
        let buttons_y = panel.y + panel.h - PANEL_PAD - BUTTON_H;
        let allow = Rect {
            x: panel.x + panel.w - PANEL_PAD - BUTTON_W,
            y: buttons_y,
            w: BUTTON_W,
            h: BUTTON_H,
        };
        let deny = Rect {
            x: allow.x - BUTTON_W - 8.0,
            y: buttons_y,
            w: BUTTON_W,
            h: BUTTON_H,
        };
        PromptLayout {
            panel,
            title,
            warning,
            list,
            legend,
            deny,
            allow,
            visible_rows,
        }
    }
}

/// One row of the flattened display list: a family header, or one member
/// operation of an expanded family.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DisplayRow {
    Family(usize),
    Member(usize, usize),
}

/// The checkbox state of a family row, derived from its members.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FamilyCheck {
    None,
    Partial,
    All,
}

fn family_check(family: &CapabilityFamily) -> FamilyCheck {
    let enabled = family
        .members
        .iter()
        .filter(|member| member.enabled)
        .count();
    if enabled == 0 {
        FamilyCheck::None
    } else if enabled == family.members.len() {
        FamilyCheck::All
    } else {
        FamilyCheck::Partial
    }
}

/// The hit result of one row press.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RowHit {
    /// The family checkbox: enable/disable every member at once.
    FamilyToggle(usize),
    /// Anywhere else on the family row: expand/collapse the member detail.
    FamilyExpand(usize),
    /// A member row: toggle that one operation.
    Member(usize, usize),
}

/// The capability-checklist chrome component. Inert until the runtime opens
/// it with [`ChromeCommand::StartCapabilityPick`].
pub struct CapabilityPrompt {
    active: bool,
    title: String,
    warning: Option<String>,
    families: Vec<CapabilityFamily>,
    /// Keys of the families currently expanded to their member detail.
    expanded: BTreeSet<String>,
    scroll_row: usize,
    wheel: f32,
    modal_reserved: Reserved,
    /// The design snapshot the prompt paints from, from
    /// [`ChromeUpdate::Appearance`]. Seeded on registration by
    /// [`crate::Shell::add`] and refreshed when the desktop color scheme
    /// changes; defaults to the dark appearance until the first update arrives.
    design: Design,
}

impl CapabilityPrompt {
    pub fn new() -> CapabilityPrompt {
        CapabilityPrompt {
            active: false,
            title: String::new(),
            warning: None,
            families: Vec::new(),
            expanded: BTreeSet::new(),
            scroll_row: 0,
            wheel: 0.0,
            modal_reserved: Reserved::default(),
            design: Design::dark(),
        }
    }

    fn has_gated(&self) -> bool {
        self.families
            .iter()
            .any(|family| family.members.iter().any(|member| member.gated))
    }

    /// The flattened display list: family headers, with member rows right
    /// after their family when expanded.
    fn display_rows(&self) -> Vec<DisplayRow> {
        let mut rows = Vec::new();
        for (index, family) in self.families.iter().enumerate() {
            rows.push(DisplayRow::Family(index));
            if self.expanded.contains(&family.key) {
                rows.extend(
                    (0..family.members.len()).map(|member| DisplayRow::Member(index, member)),
                );
            }
        }
        rows
    }

    fn layout(&self, display: (f32, f32)) -> PromptLayout {
        PromptLayout::for_display(
            display,
            self.modal_reserved,
            self.display_rows().len(),
            self.families.len(),
            self.warning.is_some(),
            self.has_gated(),
        )
    }

    /// The first visible row, clamped against the current content length.
    fn scroll_window(&self, row_count: usize, visible_rows: usize) -> usize {
        self.scroll_row.min(row_count.saturating_sub(visible_rows))
    }

    /// Answer the dialog and close.
    fn answer(&mut self, approved: Option<Vec<String>>, out: &mut ChromeEvents) {
        out.capability_pick_answered = Some(CapabilityPickResult { approved });
        self.active = false;
    }

    fn start_capability_pick(&mut self, params: CapabilityPickParams) {
        self.title = params.title;
        self.warning = params.warning;
        self.families = params.families;
        self.expanded = BTreeSet::new();
        self.scroll_row = 0;
        self.wheel = 0.0;
        self.active = true;
    }

    /// Allow the currently checked operations and close.
    fn allow(&mut self, out: &mut ChromeEvents) {
        let keys = self
            .families
            .iter()
            .flat_map(|family| &family.members)
            .filter(|member| member.enabled)
            .map(|member| member.key.clone())
            .collect();
        self.answer(Some(keys), out);
    }

    /// Map one panel-space point to the row interaction under it.
    fn hit_row(&self, layout: &PromptLayout, x: f32, y: f32) -> Option<RowHit> {
        let rows = self.display_rows();
        let scroll = self.scroll_window(rows.len(), layout.visible_rows);
        for pos in 0..layout.visible_rows {
            let Some(row) = rows.get(scroll + pos) else {
                break;
            };
            let rect = row_rect(layout.list, pos);
            if !contains(rect, x, y) {
                continue;
            }
            return Some(match *row {
                DisplayRow::Family(family) => {
                    if contains(check_rect(rect, 0.0), x, y) {
                        RowHit::FamilyToggle(family)
                    } else {
                        RowHit::FamilyExpand(family)
                    }
                }
                DisplayRow::Member(family, member) => RowHit::Member(family, member),
            });
        }
        None
    }

    fn apply_row_hit(&mut self, hit: RowHit) {
        match hit {
            RowHit::FamilyToggle(index) => {
                if let Some(family) = self.families.get_mut(index) {
                    let enable = family_check(family) != FamilyCheck::All;
                    for member in &mut family.members {
                        member.enabled = enable;
                    }
                }
            }
            RowHit::FamilyExpand(index) => {
                if let Some(family) = self.families.get(index) {
                    let key = family.key.clone();
                    if !self.expanded.remove(&key) {
                        self.expanded.insert(key);
                    }
                }
            }
            RowHit::Member(family, member) => {
                if let Some(member) = self
                    .families
                    .get_mut(family)
                    .and_then(|family| family.members.get_mut(member))
                {
                    member.enabled = !member.enabled;
                }
            }
        }
    }

    /// Handle one primary-button press at output-space `(x, y)`: toggles a
    /// row or answers on the buttons. Clicks outside the panel are ignored:
    /// a capability grant must be a deliberate choice, never an accidental
    /// click.
    fn press_at(&mut self, x: f32, y: f32, display: (f32, f32), out: &mut ChromeEvents) {
        let layout = self.layout(display);
        if contains(layout.deny, x, y) {
            self.answer(None, out);
            return;
        }
        if contains(layout.allow, x, y) {
            self.allow(out);
            return;
        }
        if !contains(layout.panel, x, y) {
            return;
        }
        if let Some(hit) = self.hit_row(&layout, x, y) {
            self.apply_row_hit(hit);
        }
    }
}

impl Default for CapabilityPrompt {
    fn default() -> Self {
        CapabilityPrompt::new()
    }
}

impl Chrome for CapabilityPrompt {
    fn render(
        &mut self,
        frame: &mut Frame,
        input: &Input,
        _windows: &[Window],
        _workspaces: &crate::WorkspaceSnapshot,
        _i18n: &Localizer,
        out: &mut ChromeEvents,
    ) {
        if !self.active {
            return;
        }
        let raw = input.as_raw();
        let display = (raw.display_size.x, raw.display_size.y);
        let cursor = raw.cursor;
        let pressed = raw.mouse_pressed.first().copied().unwrap_or(false);
        let design = self.design;
        let layout = self.layout(display);

        frame.place(
            "aegis-capability-prompt-scrim",
            &materials::chrome_place(
                Rect {
                    x: 0.0,
                    y: 0.0,
                    w: display.0,
                    h: display.1,
                },
                LayoutOpts {
                    bg: design.colors.scrim,
                    ..materials::surface_layout()
                },
            ),
            |_| {},
        );

        let original_theme = frame.theme();
        frame.set_theme(themes::application(&design));

        // Minimal foreground tint only. The compositor-owned analytic pass
        // supplies the body, refraction, rim light, and shadow.
        frame.place(
            "aegis-capability-prompt-panel",
            &materials::chrome_place(layout.panel, materials::glass_panel(&design)),
            |_| {},
        );

        let title = ellipsize(
            frame,
            &self.title,
            design.typography.headline,
            layout.title.w,
        );
        frame.place(
            "aegis-capability-prompt-title",
            &materials::chrome_place(layout.title, materials::transparent()),
            |frame| {
                frame.row_ex(&stretch(layout.title), |frame| {
                    frame.label_compact_sized(&title, design.typography.headline);
                });
            },
        );

        if let (Some(warning), Some(rect)) = (&self.warning, layout.warning) {
            let warning = ellipsize(
                frame,
                &format!("Warning: {warning}"),
                design.typography.label,
                (rect.w - 12.0).max(0.0),
            );
            frame.place(
                "aegis-capability-prompt-warning",
                &materials::chrome_place(
                    rect,
                    LayoutOpts {
                        bg: design.colors.application_hover,
                        border: design.colors.application_border,
                        border_width: design.strokes.hairline,
                        radius: design.radii.control,
                        pad: 0.0,
                        ..materials::surface_layout()
                    },
                ),
                |frame| {
                    frame.row_ex(&stretch_pad(rect), |frame| {
                        frame.label_compact_sized(&warning, design.typography.label);
                    });
                },
            );
        }

        // The checklist: a sliding window over the flattened display rows.
        let rows = self.display_rows();
        let scroll = self.scroll_window(rows.len(), layout.visible_rows);
        for pos in 0..layout.visible_rows {
            let Some(row) = rows.get(scroll + pos).copied() else {
                break;
            };
            let rect = row_rect(layout.list, pos);
            let hovered = contains(rect, cursor.x, cursor.y);
            let (indent, label, gated, check) = match row {
                DisplayRow::Family(index) => {
                    let family = &self.families[index];
                    (
                        0.0,
                        family.label.clone(),
                        family.members.iter().any(|member| member.gated),
                        match family_check(family) {
                            FamilyCheck::All => Some("✓"),
                            FamilyCheck::Partial => Some("–"),
                            FamilyCheck::None => None,
                        },
                    )
                }
                DisplayRow::Member(family, member) => {
                    let member = &self.families[family].members[member];
                    (
                        MEMBER_INDENT,
                        member.label.clone(),
                        member.gated,
                        if member.enabled { Some("✓") } else { None },
                    )
                }
            };
            frame.place(
                &format!("aegis-capability-prompt-row-{pos}"),
                &materials::chrome_place(
                    rect,
                    if hovered {
                        materials::glass_focus(&design, false)
                    } else {
                        LayoutOpts {
                            bg: Color::TRANSPARENT,
                            radius: design.radii.control,
                            pad: 0.0,
                            ..materials::surface_layout()
                        }
                    },
                ),
                |_| {},
            );
            if let DisplayRow::Family(index) = row {
                let chevron = if self.expanded.contains(&self.families[index].key) {
                    "▾"
                } else {
                    "▸"
                };
                frame.place(
                    &format!("aegis-capability-prompt-chevron-{pos}"),
                    &materials::chrome_place(
                        chevron_rect(rect),
                        LayoutOpts {
                            pad: 0.0,
                            ..materials::transparent()
                        },
                    ),
                    |frame| {
                        let rect = chevron_rect(rect);
                        frame.centered(rect.w, rect.h, |frame| {
                            frame.label_compact_sized(chevron, design.typography.footnote);
                        });
                    },
                );
            }
            let check_rect = check_rect(rect, indent);
            let checked = check.is_some();
            frame.place(
                &format!("aegis-capability-prompt-check-{pos}"),
                &materials::chrome_place(
                    check_rect,
                    LayoutOpts {
                        bg: if checked {
                            design.colors.application_accent
                        } else {
                            design.colors.card_surface
                        },
                        border: design.colors.application_border,
                        border_width: design.strokes.hairline,
                        radius: design.radii.control,
                        pad: 0.0,
                        ..materials::surface_layout()
                    },
                ),
                |frame| {
                    if let Some(glyph) = check {
                        frame.centered(check_rect.w, check_rect.h, |frame| {
                            frame.label_compact_sized(glyph, design.typography.label);
                        });
                    }
                },
            );
            let text = label_rect(rect, indent);
            let gated_width = if gated {
                frame
                    .measure_text(GATED_MARK, design.typography.footnote)
                    .width
                    + 10.0
            } else {
                0.0
            };
            let label = ellipsize(
                frame,
                &label,
                design.typography.body,
                (text.w - gated_width - 6.0).max(0.0),
            );
            frame.place(
                &format!("aegis-capability-prompt-label-{pos}"),
                &materials::chrome_place(text, materials::transparent()),
                |frame| {
                    frame.row_ex(&stretch_gap(text), |frame| {
                        frame.label_compact_sized(&label, design.typography.body);
                        if gated {
                            frame.label_compact_sized(GATED_MARK, design.typography.footnote);
                        }
                    });
                },
            );
        }

        if let Some(legend) = layout.legend {
            frame.place(
                "aegis-capability-prompt-legend",
                &materials::chrome_place(legend, materials::transparent()),
                |frame| {
                    frame.row_ex(&stretch(legend), |frame| {
                        frame.label_compact_sized(GATED_LEGEND, design.typography.footnote);
                    });
                },
            );
        }

        let deny_hovered = contains(layout.deny, cursor.x, cursor.y);
        frame.place(
            "aegis-capability-prompt-deny",
            &materials::chrome_place(
                layout.deny,
                LayoutOpts {
                    bg: if deny_hovered {
                        design.colors.application_hover
                    } else {
                        design.colors.card_surface
                    },
                    radius: design.radii.control,
                    pad: 0.0,
                    ..materials::surface_layout()
                },
            ),
            |frame| {
                frame.centered(layout.deny.w, layout.deny.h, |frame| {
                    frame.label_compact_sized("Deny", design.typography.body);
                });
            },
        );
        frame.place(
            "aegis-capability-prompt-allow",
            &materials::chrome_place(
                layout.allow,
                LayoutOpts {
                    bg: design.colors.application_accent,
                    radius: design.radii.control,
                    pad: 0.0,
                    ..materials::surface_layout()
                },
            ),
            |frame| {
                frame.centered(layout.allow.w, layout.allow.h, |frame| {
                    frame.label_compact_sized("Allow", design.typography.body);
                });
            },
        );

        frame.set_theme(original_theme);

        if pressed {
            self.press_at(cursor.x, cursor.y, display, out);
        }

        // Shell input carries lens-convention scroll deltas: scrolling down
        // is negative. Negate so wheel-down advances the list downward.
        let wheel = -(raw.scroll_y * WHEEL_ROWS + raw.scroll_pixels_y / ROW_H);
        if contains(layout.list, cursor.x, cursor.y) && wheel != 0.0 {
            self.wheel += wheel;
            let steps = self.wheel.trunc() as i32;
            self.wheel -= steps as f32;
            if steps != 0 {
                let max_scroll = rows.len().saturating_sub(layout.visible_rows);
                self.scroll_row =
                    (self.scroll_row as i32 + steps).clamp(0, max_scroll as i32) as usize;
            }
        }
    }

    fn captures_keyboard(&self) -> bool {
        self.active
    }

    fn captures_pointer(
        &self,
        _x: f32,
        _y: f32,
        _display: (f32, f32),
        _windows: &[Window],
        _workspaces: &crate::WorkspaceSnapshot,
    ) -> bool {
        self.active
    }

    fn modal_active(&self) -> bool {
        self.active
    }

    // A pending consent owns the complete chrome band: the Dock, HUD, and
    // toasts stay suppressed until the prompt is answered.
    fn exclusive_presentation_active(&self) -> bool {
        self.active
    }

    fn visible_during_modal(&self) -> bool {
        true
    }

    fn requires_composition(&self) -> bool {
        self.active
    }

    fn cursor_shape_at(
        &self,
        x: f32,
        y: f32,
        display: (f32, f32),
        _windows: &[Window],
        _workspaces: &crate::WorkspaceSnapshot,
    ) -> Option<CursorShape> {
        if !self.active {
            return None;
        }
        let layout = self.layout(display);
        let over_row = self.hit_row(&layout, x, y).is_some();
        Some(
            if over_row || contains(layout.allow, x, y) || contains(layout.deny, x, y) {
                CursorShape::Pointer
            } else {
                CursorShape::Default
            },
        )
    }

    fn update(&mut self, update: ChromeUpdate<'_>) {
        match update {
            ChromeUpdate::ModalReserved(reserved) => self.modal_reserved = reserved,
            ChromeUpdate::Appearance(design) => self.design = *design,
            _ => {}
        }
    }

    fn key_char(&mut self, key: &KeyChar, out: &mut ChromeEvents) {
        if !self.active {
            return;
        }
        match key_action(key.keysym, key.ch) {
            KeyAction::Enter => self.allow(out),
            KeyAction::Escape => self.answer(None, out),
            _ => {}
        }
    }

    fn command(&mut self, command: &ChromeCommand<'_>, out: &mut ChromeEvents) {
        match command {
            ChromeCommand::StartCapabilityPick(params) => {
                self.start_capability_pick((**params).clone());
            }
            ChromeCommand::CancelCapabilityPick if self.active => self.active = false,
            ChromeCommand::DismissModal if self.active => self.answer(None, out),
            _ => {}
        }
    }

    fn capability_pick_active(&self) -> bool {
        self.active
    }

    fn backdrop_blur_sigma(&self) -> f32 {
        if self.active {
            BACKDROP_BLUR_SIGMA
        } else {
            0.0
        }
    }

    fn backdrop_regions(
        &self,
        display: (f32, f32),
        _windows: &[Window],
        _workspaces: &crate::WorkspaceSnapshot,
    ) -> Vec<BackdropRegion> {
        if !self.active {
            return Vec::new();
        }
        let layout = self.layout(display);
        // One region exactly matching the glass body below: the runtime drops
        // it from the rectangular frost set, so the analytic pass alone owns
        // the rounded panel.
        vec![BackdropRegion::from(layout.panel)]
    }

    fn liquid_glass_regions(
        &self,
        display: (f32, f32),
        _windows: &[Window],
        _workspaces: &crate::WorkspaceSnapshot,
    ) -> Vec<LiquidGlassRegion> {
        if !self.active {
            return Vec::new();
        }
        let layout = self.layout(display);
        vec![LiquidGlassRegion::from_role(
            &self.design,
            GlassRole::ProminentPanel,
            BackdropRegion::from(layout.panel),
            self.design.radii.glass_panel,
            1.0,
        )]
    }
}

fn contains(rect: Rect, x: f32, y: f32) -> bool {
    x >= rect.x && y >= rect.y && x < rect.x + rect.w && y < rect.y + rect.h
}

/// The `pos`-th visible row inside the list window.
fn row_rect(list: Rect, pos: usize) -> Rect {
    Rect {
        x: list.x,
        y: list.y + pos as f32 * ROW_H,
        w: list.w,
        h: ROW_H,
    }
}

/// The disclosure-chevron strip of a family row.
fn chevron_rect(row: Rect) -> Rect {
    Rect {
        x: row.x + 4.0,
        y: row.y,
        w: CHEVRON_W - 4.0,
        h: ROW_H,
    }
}

/// The checkbox square of one row; member rows are indented under their
/// family's label.
fn check_rect(row: Rect, indent: f32) -> Rect {
    Rect {
        x: row.x + CHEVRON_W + 4.0 + indent,
        y: row.y + (ROW_H - CHECK) * 0.5,
        w: CHECK,
        h: CHECK,
    }
}

/// The label strip of one row, right of its checkbox.
fn label_rect(row: Rect, indent: f32) -> Rect {
    let check = check_rect(row, indent);
    Rect {
        x: check.x + CHECK + 8.0,
        y: row.y,
        w: (row.x + row.w - check.x - CHECK - 14.0).max(0.0),
        h: ROW_H,
    }
}

fn stretch(rect: Rect) -> LayoutOpts {
    LayoutOpts {
        width: rect.w,
        height: rect.h,
        cross: Align::Center,
        ..Default::default()
    }
}

fn stretch_pad(rect: Rect) -> LayoutOpts {
    LayoutOpts {
        width: rect.w,
        height: rect.h,
        cross: Align::Center,
        pad: 6.0,
        ..Default::default()
    }
}

fn stretch_gap(rect: Rect) -> LayoutOpts {
    LayoutOpts {
        width: rect.w,
        height: rect.h,
        cross: Align::Center,
        gap: 6.0,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(key: &str, label: &str, gated: bool) -> CapabilityGroup {
        CapabilityGroup {
            key: key.to_string(),
            label: label.to_string(),
            gated,
            enabled: true,
        }
    }

    fn params() -> CapabilityPickParams {
        CapabilityPickParams {
            title: "Codex wants to borrow desktop capabilities".to_string(),
            warning: Some(
                "A different installation already registered under this name.".to_string(),
            ),
            families: vec![
                CapabilityFamily {
                    key: "windows".to_string(),
                    label: "Control windows".to_string(),
                    members: vec![
                        member("Focus", "Focus windows", false),
                        member("Close", "Close windows", true),
                    ],
                },
                CapabilityFamily {
                    key: "capture".to_string(),
                    label: "Capture window contents".to_string(),
                    members: vec![member("CaptureWindow", "Capture window contents", true)],
                },
            ],
        }
    }

    fn many_families(count: usize) -> CapabilityPickParams {
        CapabilityPickParams {
            families: (0..count)
                .map(|index| CapabilityFamily {
                    key: format!("family-{index}"),
                    label: format!("Family {index}"),
                    members: vec![member(
                        &format!("Op{index}"),
                        &format!("Operation {index}"),
                        false,
                    )],
                })
                .collect(),
            ..params()
        }
    }

    /// The center of the `pos`-th visible row.
    fn row_center(layout: &PromptLayout, pos: usize) -> (f32, f32) {
        let row = row_rect(layout.list, pos);
        (row.x + row.w * 0.5, row.y + row.h * 0.5)
    }

    /// The center of the `pos`-th visible row's checkbox (family rows).
    fn check_center(layout: &PromptLayout, pos: usize) -> (f32, f32) {
        let check = check_rect(row_rect(layout.list, pos), 0.0);
        (check.x + check.w * 0.5, check.y + check.h * 0.5)
    }

    #[test]
    fn clicking_a_family_row_expands_and_collapses_it() {
        let mut prompt = CapabilityPrompt::new();
        prompt.start_capability_pick(params());
        let display = (1280.0, 800.0);
        let layout = prompt.layout(display);
        let (x, y) = row_center(&layout, 0);
        let mut out = ChromeEvents::default();
        prompt.press_at(x, y, display, &mut out);
        assert!(prompt.expanded.contains("windows"));
        assert_eq!(prompt.display_rows().len(), 4);
        assert!(out.capability_pick_answered.is_none());
        prompt.press_at(x, y, display, &mut out);
        assert!(prompt.expanded.is_empty());
        assert_eq!(prompt.display_rows().len(), 2);
    }

    #[test]
    fn expanding_grows_the_panel_downward_without_moving_the_rows() {
        let mut prompt = CapabilityPrompt::new();
        prompt.start_capability_pick(params());
        let display = (1280.0, 800.0);
        let before = prompt.layout(display);
        prompt.expanded.insert("windows".to_string());
        let after = prompt.layout(display);
        assert_eq!(before.panel.y, after.panel.y);
        assert_eq!(before.list.y, after.list.y);
        assert!(after.panel.h > before.panel.h);
    }

    #[test]
    fn clicking_a_family_checkbox_toggles_every_member() {
        let mut prompt = CapabilityPrompt::new();
        prompt.start_capability_pick(params());
        let display = (1280.0, 800.0);
        let layout = prompt.layout(display);
        let (x, y) = check_center(&layout, 0);
        let mut out = ChromeEvents::default();
        prompt.press_at(x, y, display, &mut out);
        assert!(prompt.families[0].members.iter().all(|m| !m.enabled));
        // The family stays collapsed; the sibling family is untouched.
        assert!(prompt.expanded.is_empty());
        assert!(prompt.families[1].members.iter().all(|m| m.enabled));
        prompt.press_at(x, y, display, &mut out);
        assert!(prompt.families[0].members.iter().all(|m| m.enabled));
    }

    #[test]
    fn clicking_a_member_row_toggles_only_it() {
        let mut prompt = CapabilityPrompt::new();
        prompt.start_capability_pick(params());
        prompt.expanded.insert("windows".to_string());
        let display = (1280.0, 800.0);
        let layout = prompt.layout(display);
        // Rows: family 0, member Focus, member Close, family 1.
        let (x, y) = row_center(&layout, 1);
        let mut out = ChromeEvents::default();
        prompt.press_at(x, y, display, &mut out);
        assert!(!prompt.families[0].members[0].enabled);
        assert!(prompt.families[0].members[1].enabled);
    }

    #[test]
    fn family_checkbox_state_tracks_the_members() {
        let mut prompt = CapabilityPrompt::new();
        prompt.start_capability_pick(params());
        assert_eq!(family_check(&prompt.families[0]), FamilyCheck::All);
        prompt.families[0].members[0].enabled = false;
        assert_eq!(family_check(&prompt.families[0]), FamilyCheck::Partial);
        prompt.families[0].members[1].enabled = false;
        assert_eq!(family_check(&prompt.families[0]), FamilyCheck::None);
    }

    #[test]
    fn allow_flattens_the_checked_member_keys() {
        let mut prompt = CapabilityPrompt::new();
        prompt.start_capability_pick(params());
        prompt.families[0].members[0].enabled = false;
        let mut out = ChromeEvents::default();
        prompt.allow(&mut out);
        assert_eq!(
            out.capability_pick_answered,
            Some(CapabilityPickResult {
                approved: Some(vec!["Close".to_string(), "CaptureWindow".to_string()]),
            })
        );
        assert!(!prompt.capability_pick_active());
    }

    #[test]
    fn escape_denies() {
        let mut prompt = CapabilityPrompt::new();
        prompt.start_capability_pick(params());
        let mut out = ChromeEvents::default();
        prompt.key_char(
            &KeyChar {
                keysym: aegis_model::input::XKB_KEY_Escape,
                ch: None,
                mods: aegis_model::input::Mods::NONE,
            },
            &mut out,
        );
        assert_eq!(
            out.capability_pick_answered,
            Some(CapabilityPickResult { approved: None })
        );
        assert!(!prompt.capability_pick_active());
    }

    #[test]
    fn clicking_outside_keeps_the_prompt_open() {
        let mut prompt = CapabilityPrompt::new();
        prompt.start_capability_pick(params());
        let mut out = ChromeEvents::default();
        prompt.press_at(4.0, 4.0, (1280.0, 800.0), &mut out);
        assert!(out.capability_pick_answered.is_none());
        assert!(prompt.capability_pick_active());
    }

    #[test]
    fn the_panic_chord_command_denies() {
        let mut prompt = CapabilityPrompt::new();
        prompt.start_capability_pick(params());
        let mut out = ChromeEvents::default();
        prompt.command(&ChromeCommand::DismissModal, &mut out);
        assert_eq!(
            out.capability_pick_answered,
            Some(CapabilityPickResult { approved: None })
        );
        assert!(!prompt.capability_pick_active());
    }

    #[test]
    fn deny_button_denies() {
        let mut prompt = CapabilityPrompt::new();
        prompt.start_capability_pick(params());
        let display = (1280.0, 800.0);
        let layout = prompt.layout(display);
        let mut out = ChromeEvents::default();
        prompt.press_at(layout.deny.x + 4.0, layout.deny.y + 4.0, display, &mut out);
        assert_eq!(
            out.capability_pick_answered,
            Some(CapabilityPickResult { approved: None })
        );
    }

    #[test]
    fn the_legend_row_exists_exactly_when_a_member_is_gated() {
        let mut prompt = CapabilityPrompt::new();
        prompt.start_capability_pick(params());
        assert!(prompt.layout((1280.0, 800.0)).legend.is_some());

        let mut families = params().families;
        for family in &mut families {
            for member in &mut family.members {
                member.gated = false;
            }
        }
        let mut prompt = CapabilityPrompt::new();
        prompt.start_capability_pick(CapabilityPickParams {
            families,
            ..params()
        });
        assert!(prompt.layout((1280.0, 800.0)).legend.is_none());
    }

    #[test]
    fn a_warning_lays_out_its_own_row() {
        let mut prompt = CapabilityPrompt::new();
        prompt.start_capability_pick(params());
        assert!(prompt.layout((1280.0, 800.0)).warning.is_some());

        let mut prompt = CapabilityPrompt::new();
        prompt.start_capability_pick(CapabilityPickParams {
            warning: None,
            ..params()
        });
        assert!(prompt.layout((1280.0, 800.0)).warning.is_none());
    }

    #[test]
    fn the_list_window_stays_inside_the_display() {
        let mut prompt = CapabilityPrompt::new();
        prompt.start_capability_pick(many_families(40));
        // A short display caps the window below MAX_VISIBLE_ROWS.
        let layout = prompt.layout((1280.0, 480.0));
        assert!(layout.visible_rows < 40);
        assert!(layout.panel.h <= 480.0 - 32.0 + 0.01);
        // A tall display caps the window at MAX_VISIBLE_ROWS.
        let layout = prompt.layout((1280.0, 2000.0));
        assert_eq!(layout.visible_rows, MAX_VISIBLE_ROWS);
        // Content shorter than the window shrinks the list to fit.
        let mut prompt = CapabilityPrompt::new();
        prompt.start_capability_pick(params());
        let layout = prompt.layout((1280.0, 2000.0));
        assert_eq!(layout.visible_rows, 2);
    }

    #[test]
    fn the_scroll_window_clamps_against_the_content() {
        let mut prompt = CapabilityPrompt::new();
        prompt.start_capability_pick(many_families(40));
        prompt.scroll_row = 39;
        assert_eq!(prompt.scroll_window(40, 14), 26);
        assert_eq!(prompt.scroll_window(10, 14), 0);
    }

    #[test]
    fn the_active_panel_is_one_analytic_glass_body() {
        let mut prompt = CapabilityPrompt::new();
        let display = (1280.0, 800.0);
        let workspaces = crate::WorkspaceSnapshot {
            outputs: Vec::new(),
        };
        assert!(
            prompt
                .liquid_glass_regions(display, &[], &workspaces)
                .is_empty()
        );
        assert!(!prompt.exclusive_presentation_active());

        prompt.start_capability_pick(params());
        let backdrop = prompt.backdrop_regions(display, &[], &workspaces);
        let glass = prompt.liquid_glass_regions(display, &[], &workspaces);
        assert_eq!(backdrop.len(), 1);
        assert_eq!(glass.len(), 1);
        assert_eq!(glass[0].bounds, backdrop[0]);
        assert_eq!(glass[0].corner_radius, Design::dark().radii.glass_panel);
        assert_eq!(glass[0].opacity, 1.0);
        assert!(prompt.exclusive_presentation_active());
    }
}
