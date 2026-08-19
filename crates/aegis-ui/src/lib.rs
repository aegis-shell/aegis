//! Composite UI patterns, modal dialog scaffolding, settings controls, motion curves, and shared layout primitives for Aegis chrome.
//!
//! # Architecture & Boundary
//!
//! `aegis-ui` builds on top of `lens` (the immediate-mode UI engine) and `aegis-design`
//! (the product design tokens and materials). It provides reusable composite UI patterns
//! and enforces UI design consistency across Aegis compositor chrome components
//! without coupling to compositor server state.

#![forbid(unsafe_code)]

pub mod chip;
pub mod dialog;
pub mod geom;
pub mod menu;
pub mod motion;
pub mod picker;
pub mod settings;
pub mod shapes;

pub use chip::{
    DEFAULT_WORKSPACE_DOT_DIAMETER, chip_layout, chip_opts, hud_glyph_outline,
    hud_glyph_outline_params, hud_text_outline, hud_text_outline_params, place_chip,
    render_badge, workspace_dot_color, workspace_dot_intensity,
};
pub use dialog::{
    ActionButtonStyle, DEFAULT_BACKDROP_BLUR_SIGMA, DEFAULT_BUTTON_HEIGHT, DEFAULT_BUTTON_WIDTH,
    DEFAULT_MODAL_PAD, DEFAULT_MODAL_WIDTH, DEFAULT_TITLE_HEIGHT, GRANT_ACCENT_INDEX,
    GRANT_LABELS, is_cancel_key, is_confirm_key, place_modal_panel, place_modal_scrim,
    render_action_button, render_dialog_actions_two_button, render_dialog_title,
    render_grant_action_buttons,
};
pub use geom::{center_rect, contains, stretch, stretch_gap, stretch_pad, stretch_top};
pub use menu::{
    DEFAULT_MENU_HEADER_HEIGHT, DEFAULT_MENU_PAD, DEFAULT_MENU_ROW_HEIGHT,
    DEFAULT_MENU_SECTION_HEIGHT, DEFAULT_MENU_WIDTH, menu_item_layout, menu_panel_layout,
};
pub use motion::{ease_out_cubic, lerp, stagger};
pub use picker::{
    DEFAULT_DOUBLE_CLICK_TIMEOUT, DEFAULT_PICKER_ROW_HEIGHT, DEFAULT_WHEEL_SCROLL_ROWS,
    clamp_scroll_window, picker_row_layout,
};
pub use settings::{
    render_card, render_section_heading, render_setting_row, render_unavailable_row,
    section_heading_layout, settings_card_layout,
};
pub use shapes::{render_disc, render_dot, render_ring};
