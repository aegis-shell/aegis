//! The compositor-owned Control Center host.
//!
//! The host owns navigation and modal presentation. Persistent setting pages
//! are independent modules, while instant controls and Realm lifecycle
//! management use explicit non-settings routes. The UI emits typed intents;
//! it never probes the host or invokes host commands itself.

pub mod module;

mod modules;
mod quick_settings;
mod realm_manager;
mod ui;

use std::ffi::c_void;

use ass_core::app::BuiltInApplication;
use ass_core::input::{KeyAction, KeyChar, key_action};
use ass_core::realm::RealmSnapshot;
use ass_core::settings::{SettingsAction, SettingsSnapshot};
use ass_core::window::Window;
use ass_core::workspace::WorkspaceSnapshot;
use ass_design::{Design, themes};
use lens::{Align, Color, Frame, Icon, Input, LayoutOpts, OverlayOpts, Rect};

use ass_shell::{
    AppCatalog, BackdropRegion, Chrome, ChromeEvents, CursorShape, IconSet, Localizer, Message,
    Reserved, SystemStatus,
};

use module::{
    ApplyPolicy, ModuleAvailability, ModuleCategory, ModuleEvents, ModuleId, ModuleMetadata,
    ModuleRegistry,
};
#[cfg(test)]
use modules::{DISPLAY_MODULE_ID, TOUCHPAD_MODULE_ID};
use modules::{DisplayModule, TouchpadModule, UnavailableModule};
use quick_settings::QuickSettings;
use realm_manager::RealmManager;

const APP_MAX_W: f32 = 860.0;
const APP_MAX_H: f32 = 590.0;
const APP_MARGIN: f32 = 24.0;
const APP_RADIUS: f32 = 24.0;
const BACKDROP_BLUR_SIGMA: f32 = 18.0;

/// Construct the built-in module set in stable navigation order. Both the
/// compositor compatibility host and the standalone app use this registry.
pub fn builtin_settings_modules() -> ModuleRegistry {
    let mut modules = ModuleRegistry::default();
    modules.register(DisplayModule::new());
    modules.register(UnavailableModule::new(
        ModuleMetadata {
            id: ModuleId::new("mouse"),
            title: Message::Mouse,
            icon: Icon::MousePointer,
            category: ModuleCategory::Hardware,
            keywords: &["mouse", "pointer", "acceleration", "buttons", "wheel"],
            apply_policy: ApplyPolicy::Instant,
            availability: ModuleAvailability::BackendUnavailable,
        },
        Message::MouseDescription,
    ));
    modules.register(TouchpadModule::new());
    modules.register(UnavailableModule::new(
        ModuleMetadata {
            id: ModuleId::new("keyboard"),
            title: Message::Keyboard,
            icon: Icon::Type,
            category: ModuleCategory::Hardware,
            keywords: &["keyboard", "layout", "repeat", "compose", "shortcuts"],
            apply_policy: ApplyPolicy::Explicit,
            availability: ModuleAvailability::BackendUnavailable,
        },
        Message::KeyboardDescription,
    ));
    modules.register(UnavailableModule::new(
        ModuleMetadata {
            id: ModuleId::new("appearance"),
            title: Message::Appearance,
            icon: Icon::PenTool,
            category: ModuleCategory::Personalization,
            keywords: &["theme", "appearance", "icons", "fonts", "cursor", "motion"],
            apply_policy: ApplyPolicy::Explicit,
            availability: ModuleAvailability::BackendUnavailable,
        },
        Message::AppearanceDescription,
    ));
    modules.register(UnavailableModule::new(
        ModuleMetadata {
            id: ModuleId::new("power"),
            title: Message::PowerManagement,
            icon: Icon::Zap,
            category: ModuleCategory::System,
            keywords: &["power", "battery", "idle", "suspend", "profile"],
            apply_policy: ApplyPolicy::Instant,
            availability: ModuleAvailability::BackendUnavailable,
        },
        Message::PowerManagementDescription,
    ));
    modules.register(UnavailableModule::new(
        ModuleMetadata {
            id: ModuleId::new("users"),
            title: Message::UserAccounts,
            icon: Icon::Users,
            category: ModuleCategory::System,
            keywords: &["users", "accounts", "password", "avatar", "administrator"],
            apply_policy: ApplyPolicy::Explicit,
            availability: ModuleAvailability::BackendUnavailable,
        },
        Message::UserAccountsDescription,
    ));
    modules.register(UnavailableModule::new(
        ModuleMetadata {
            id: ModuleId::new("window-rules"),
            title: Message::WindowRules,
            icon: Icon::FileText,
            category: ModuleCategory::System,
            keywords: &["window", "rules", "application", "workspace", "floating"],
            apply_policy: ApplyPolicy::Explicit,
            availability: ModuleAvailability::BackendUnavailable,
        },
        Message::WindowRulesDescription,
    ));
    modules
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Route {
    QuickSettings,
    Settings(ModuleId),
    RealmManager,
}

/// Trusted host for built-in settings modules and system-management routes.
pub struct ControlCenter {
    open: bool,
    icons: IconSet,
    modal_reserved: Reserved,
    route: Route,
    modules: ModuleRegistry,
    quick_settings: QuickSettings,
    realm_manager: RealmManager,
}

impl ControlCenter {
    /// Construct a closed Control Center. Decoded icons and authoritative
    /// snapshots arrive through the [`Chrome`] update hooks.
    pub fn new() -> Self {
        Self {
            open: false,
            icons: IconSet::default(),
            modal_reserved: Reserved::default(),
            route: Route::QuickSettings,
            modules: builtin_settings_modules(),
            quick_settings: QuickSettings::new(),
            realm_manager: RealmManager::new(),
        }
    }

    fn bounds(&self, display: (f32, f32)) -> Rect {
        let left = self.modal_reserved.left.max(0) as f32;
        let top = self.modal_reserved.top.max(0) as f32;
        let right = self.modal_reserved.right.max(0) as f32;
        let bottom = self.modal_reserved.bottom.max(0) as f32;
        let usable_w = (display.0 - left - right).max(1.0);
        let usable_h = (display.1 - top - bottom).max(1.0);
        let w = APP_MAX_W.min((usable_w - APP_MARGIN * 2.0).max(240.0));
        let h = APP_MAX_H.min((usable_h - APP_MARGIN * 2.0).max(300.0));
        Rect {
            x: left + ((usable_w - w) * 0.5).max(0.0),
            y: top + ((usable_h - h) * 0.5).max(0.0),
            w: w.min(usable_w),
            h: h.min(usable_h),
        }
    }

    fn app_icon(&self) -> Option<*mut c_void> {
        self.icons
            .get("ass-control-center")
            .or_else(|| self.icons.get("ass-hud:preferences-system-symbolic"))
    }

    fn render_header(&self, frame: &mut Frame, i18n: &Localizer) -> bool {
        let mut close = false;
        frame.row_ex(
            &LayoutOpts {
                height: 48.0,
                gap: 12.0,
                cross: Align::Center,
                ..Default::default()
            },
            |frame| {
                frame.size_next(36.0, 36.0);
                match self.app_icon() {
                    Some(icon) => unsafe {
                        frame.image(icon as *mut lens::sys::flux_image, 32.0, 32.0)
                    },
                    None => frame.icon(Icon::Settings, 28.0),
                }
                frame.column_ex(
                    &LayoutOpts {
                        gap: 1.0,
                        cross: Align::Start,
                        ..Default::default()
                    },
                    |frame| {
                        frame.heading(i18n.text(Message::ControlCenter), 2);
                        frame.label_sized(i18n.text(Message::BuiltInSystemApp), 11.0);
                    },
                );
                frame.flex(1.0);
                frame.spacer(0.0);
                frame.size_next(34.0, 30.0);
                close = frame.icon_button(Icon::X);
            },
        );
        close
    }

    fn navigation_items(&self) -> Vec<(Route, Icon, Message)> {
        let mut items = vec![(Route::QuickSettings, Icon::Sliders, Message::QuickSettings)];
        items.extend(
            self.modules
                .metadata()
                .map(|module| (Route::Settings(module.id), module.icon, module.title)),
        );
        items.push((Route::RealmManager, Icon::Grid, Message::AiWorkspaces));
        items
    }

    fn render_navigation(&mut self, frame: &mut Frame, i18n: &Localizer) {
        for (route, icon, title) in self.navigation_items() {
            if frame.selectable_icon(icon, i18n.text(title), self.route == route) {
                self.route = route;
            }
        }
    }

    fn render_compact_navigation(&mut self, frame: &mut Frame, i18n: &Localizer) {
        let items = self.navigation_items();
        for entries in items.chunks(2) {
            frame.row_ex(
                &LayoutOpts {
                    gap: 4.0,
                    cross: Align::Stretch,
                    ..Default::default()
                },
                |frame| {
                    for (route, _, title) in entries.iter().copied() {
                        frame.flex(1.0);
                        if frame.selectable(i18n.text(title), self.route == route) {
                            self.route = route;
                        }
                    }
                    if entries.len() == 1 {
                        frame.flex(1.0);
                        frame.spacer(0.0);
                    }
                },
            );
        }
    }

    fn render_page(&mut self, frame: &mut Frame, i18n: &Localizer, out: &mut ChromeEvents) {
        match self.route {
            Route::QuickSettings => {
                if self.quick_settings.render(frame, i18n, out) {
                    self.open = false;
                }
            }
            Route::Settings(id) => {
                let mut events = ModuleEvents::default();
                if !self.modules.render(id, frame, i18n, &mut events) {
                    self.route = Route::QuickSettings;
                }
                for action in events.actions {
                    out.system_actions.push(match action {
                        SettingsAction::SetTouchpad { config } => {
                            ass_shell::SystemAction::SetTouchpad(config)
                        }
                        SettingsAction::SetDisplay { settings } => {
                            ass_shell::SystemAction::SetDisplay(settings)
                        }
                    });
                }
            }
            Route::RealmManager => self.realm_manager.render(frame, i18n, out),
        }
    }
}

impl Default for ControlCenter {
    fn default() -> Self {
        Self::new()
    }
}

impl Chrome for ControlCenter {
    fn render(
        &mut self,
        frame: &mut Frame,
        input: &Input,
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
        i18n: &Localizer,
        out: &mut ChromeEvents,
    ) {
        if !self.open {
            return;
        }

        let raw = input.as_raw();
        let display = (raw.display_size.x, raw.display_size.y);
        let bounds = self.bounds(display);
        frame.layer(
            "ass-control-center-scrim",
            Rect {
                x: 0.0,
                y: 0.0,
                w: display.0,
                h: display.1,
            },
            &OverlayOpts {
                bg: Color::rgba(8, 10, 18, 118),
                ..Default::default()
            },
            |_| {},
        );

        let original_theme = frame.theme();
        frame.set_theme(themes::application(&Design::dark()));
        let mut close = false;
        frame.layer(
            "ass-control-center-app",
            bounds,
            &OverlayOpts {
                bg: Color::rgba(25, 28, 40, 238),
                border: Color::rgba(255, 255, 255, 48),
                border_width: 1.0,
                radius: APP_RADIUS,
                pad: 0.0,
                ..Default::default()
            },
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
                        close = self.render_header(frame, i18n);
                        frame.separator();
                        frame.flex(1.0);
                        if bounds.w >= 640.0 {
                            frame.row_ex(
                                &LayoutOpts {
                                    flex: 1.0,
                                    gap: 18.0,
                                    cross: Align::Stretch,
                                    ..Default::default()
                                },
                                |frame| {
                                    frame.column_ex(
                                        &LayoutOpts {
                                            width: 184.0,
                                            gap: 5.0,
                                            pad: 8.0,
                                            cross: Align::Stretch,
                                            bg: Color::rgba(255, 255, 255, 10),
                                            radius: 14.0,
                                            ..Default::default()
                                        },
                                        |frame| self.render_navigation(frame, i18n),
                                    );
                                    frame.flex(1.0);
                                    frame.scroll("ass-control-center-page", |frame| {
                                        frame.column_ex(
                                            &LayoutOpts {
                                                gap: 12.0,
                                                cross: Align::Stretch,
                                                ..Default::default()
                                            },
                                            |frame| self.render_page(frame, i18n, out),
                                        );
                                    });
                                },
                            );
                        } else {
                            self.render_compact_navigation(frame, i18n);
                            frame.flex(1.0);
                            frame.scroll("ass-control-center-narrow-page", |frame| {
                                frame.column_ex(
                                    &LayoutOpts {
                                        gap: 12.0,
                                        cross: Align::Stretch,
                                        ..Default::default()
                                    },
                                    |frame| self.render_page(frame, i18n, out),
                                );
                            });
                        }
                    },
                );
            },
        );
        frame.set_theme(original_theme);

        let left_pressed = raw.mouse_pressed.first().copied().unwrap_or(false);
        let outside = !contains(bounds, raw.cursor.x, raw.cursor.y);
        if close || (left_pressed && outside) {
            self.open = false;
        }
    }

    fn captures_keyboard(&self) -> bool {
        self.open
    }

    fn key_char(&mut self, key: &KeyChar, _out: &mut ChromeEvents) {
        if self.open && matches!(key_action(key.keysym, key.ch), KeyAction::Escape) {
            self.open = false;
        }
    }

    fn open_builtin(&mut self, app: BuiltInApplication) {
        match app {
            BuiltInApplication::ControlCenter => self.open = true,
            BuiltInApplication::AiWorkspaces => {
                self.route = Route::RealmManager;
                self.open = true;
            }
            BuiltInApplication::ScreenshotSelector => {}
        }
    }

    fn update_system_status(&mut self, status: &SystemStatus) {
        self.quick_settings.update_system_status(status);
        self.modules.update_settings(&SettingsSnapshot {
            revision: 0,
            touchpad: status.touchpad.clone(),
            display: status.display.clone(),
        });
    }

    fn update_realms(&mut self, snapshot: &RealmSnapshot) {
        self.realm_manager.update(snapshot);
    }

    fn update_app_catalog(&mut self, catalog: &AppCatalog) {
        self.icons = catalog.icons.clone();
    }

    fn captures_pointer(
        &self,
        _x: f32,
        _y: f32,
        _display: (f32, f32),
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) -> bool {
        self.open
    }

    fn cursor_shape_at(
        &self,
        _x: f32,
        _y: f32,
        _display: (f32, f32),
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) -> Option<CursorShape> {
        Some(CursorShape::Pointer)
    }

    fn modal_active(&self) -> bool {
        self.open
    }

    fn visible_during_modal(&self) -> bool {
        true
    }

    fn set_modal_reserved(&mut self, reserved: Reserved) {
        self.modal_reserved = reserved;
    }

    fn backdrop_blur_sigma(&self) -> f32 {
        if self.open { BACKDROP_BLUR_SIGMA } else { 0.0 }
    }

    fn backdrop_regions(
        &self,
        display: (f32, f32),
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) -> Vec<BackdropRegion> {
        if !self.open {
            return Vec::new();
        }
        let panel = self.bounds(display);
        let radius = APP_RADIUS;
        vec![
            BackdropRegion {
                x: panel.x + radius,
                y: panel.y,
                w: (panel.w - radius * 2.0).max(0.0),
                h: panel.h,
            },
            BackdropRegion {
                x: panel.x,
                y: panel.y + radius,
                w: panel.w,
                h: (panel.h - radius * 2.0).max(0.0),
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

    #[test]
    fn app_window_stays_inside_small_outputs() {
        let center = ControlCenter::new();
        let bounds = center.bounds((320.0, 480.0));
        assert!(bounds.x >= 0.0 && bounds.y >= 0.0);
        assert!(bounds.x + bounds.w <= 320.0);
        assert!(bounds.y + bounds.h <= 480.0);
    }

    #[test]
    fn ai_workspace_route_opens_the_manager() {
        let mut center = ControlCenter::new();
        center.open_builtin(BuiltInApplication::AiWorkspaces);
        assert!(center.open);
        assert_eq!(center.route, Route::RealmManager);
    }

    #[test]
    fn built_in_settings_modules_have_stable_routes() {
        let center = ControlCenter::new();
        assert!(center.modules.contains(DISPLAY_MODULE_ID));
        assert!(center.modules.contains(TOUCHPAD_MODULE_ID));
        assert_eq!(
            center
                .modules
                .metadata()
                .map(|module| module.id.as_str())
                .collect::<Vec<_>>(),
            vec![
                "display",
                "mouse",
                "touchpad",
                "keyboard",
                "appearance",
                "power",
                "users",
                "window-rules",
            ]
        );
        assert_eq!(
            center
                .modules
                .metadata()
                .filter(|module| module.availability == ModuleAvailability::Available)
                .map(|module| module.id.as_str())
                .collect::<Vec<_>>(),
            vec!["display", "touchpad"]
        );
    }
}
