//! Module contract and built-in pages for the standalone System Settings app.
//!
//! Settings pages own presentation and draft state. The application host owns
//! navigation and routes typed intents to the authoritative service.

pub mod module;

mod modules;
mod ui;

use lens::Icon;

use aegis_shell::Message;
use module::{
    ApplyPolicy, ModuleAvailability, ModuleCategory, ModuleId, ModuleMetadata, ModuleRegistry,
};
use modules::{AppearanceModule, DisplayModule, PowerModule, TouchpadModule, UnavailableModule};

/// Construct the built-in module set in stable navigation order.
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
    modules.register(AppearanceModule::new());
    modules.register(PowerModule::new());
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_modules_have_stable_routes() {
        let modules = builtin_settings_modules();
        assert_eq!(
            modules
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
            modules
                .metadata()
                .filter(|module| module.availability == ModuleAvailability::Available)
                .map(|module| module.id.as_str())
                .collect::<Vec<_>>(),
            vec!["display", "touchpad", "appearance", "power"]
        );
    }
}
