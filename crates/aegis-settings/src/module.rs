//! Settings-module contract and registry for the System Settings host.
//!
//! The contract deliberately separates module discovery and lifecycle from
//! the way modules are linked. Modules are statically registered today, so
//! the application does not expose Rust's unstable dynamic-library ABI. A
//! future process-isolated loader can preserve the same metadata and state
//! model without changing the System Settings navigation contract.

use aegis_model::settings::{SettingsAction, SettingsSnapshot};
use aegis_shell::{Localizer, Message};
use lens::{Frame, Icon};

/// Stable identifier for a settings module and for deep-link routing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ModuleId(&'static str);

impl ModuleId {
    pub const fn new(id: &'static str) -> Self {
        Self(id)
    }

    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

/// Broad navigation group. Categories are presentation metadata, not owners
/// of settings or persistence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModuleCategory {
    Hardware,
    Personalization,
    System,
}

/// Whether edits take effect as the user changes a control or only after an
/// explicit apply operation inside the module.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyPolicy {
    Instant,
    Explicit,
}

/// Whether the module's authoritative backend is usable in this build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModuleAvailability {
    Available,
    BackendUnavailable,
}

/// Immutable discovery metadata owned by one module.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModuleMetadata {
    pub id: ModuleId,
    pub title: Message,
    pub icon: Icon,
    pub category: ModuleCategory,
    pub keywords: &'static [&'static str],
    pub apply_policy: ApplyPolicy,
    pub availability: ModuleAvailability,
}

/// Typed intents emitted by settings pages. The host decides whether those
/// intents travel through in-process compositor chrome or the public IPC.
/// Modules never persist configuration or call either transport directly.
#[derive(Debug, Default)]
pub struct ModuleEvents {
    pub actions: Vec<SettingsAction>,
}

/// One independently stateful settings page hosted by System Settings.
pub trait SettingsModule {
    fn metadata(&self) -> ModuleMetadata;

    fn render(&mut self, frame: &mut Frame, i18n: &Localizer, out: &mut ModuleEvents);

    /// Replace the module's authoritative presentation snapshot. Modules keep
    /// local editor state only while it is dirty; the implementation decides
    /// how a newer snapshot is reconciled with that draft.
    fn update_settings(&mut self, snapshot: &SettingsSnapshot);
}

/// Ordered module registry used for discovery, navigation, and deep links.
#[derive(Default)]
pub struct ModuleRegistry {
    modules: Vec<Box<dyn SettingsModule>>,
}

impl ModuleRegistry {
    pub fn register(&mut self, module: impl SettingsModule + 'static) {
        let id = module.metadata().id;
        assert!(
            self.modules
                .iter()
                .all(|registered| registered.metadata().id != id),
            "duplicate settings module id: {}",
            id.as_str()
        );
        self.modules.push(Box::new(module));
    }

    pub fn metadata(&self) -> impl Iterator<Item = ModuleMetadata> + '_ {
        self.modules.iter().map(|module| module.metadata())
    }

    pub fn contains(&self, id: ModuleId) -> bool {
        self.modules.iter().any(|module| module.metadata().id == id)
    }

    /// Resolve a deep-link segment without leaking module implementation
    /// types into the host.
    pub fn resolve(&self, id: &str) -> Option<ModuleId> {
        self.metadata()
            .find(|module| module.id.as_str() == id)
            .map(|module| module.id)
    }

    pub fn render(
        &mut self,
        id: ModuleId,
        frame: &mut Frame,
        i18n: &Localizer,
        out: &mut ModuleEvents,
    ) -> bool {
        let Some(module) = self
            .modules
            .iter_mut()
            .find(|module| module.metadata().id == id)
        else {
            return false;
        };
        module.render(frame, i18n, out);
        true
    }

    pub fn update_settings(&mut self, snapshot: &SettingsSnapshot) {
        for module in &mut self.modules {
            module.update_settings(snapshot);
        }
    }
}
