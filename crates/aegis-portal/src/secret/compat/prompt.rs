//! `org.freedesktop.Secret.Prompt`: the unlock prompt handle.
//!
//! The prompt object is registered for spec shape; `Service::Unlock`
//! completes it after the compositor-chrome password interaction finishes
//! (see `service.rs`). The methods stay log-only.

use zbus::zvariant::Value;

/// The served prompt object.
pub(crate) struct PromptIface {
    pub(crate) id: String,
}

#[zbus::interface(name = "org.freedesktop.Secret.Prompt")]
impl PromptIface {
    async fn prompt(&self, window_id: &str) -> zbus::fdo::Result<()> {
        log::info!(
            "portal: secrets prompt {} called with window_id '{window_id}' (no prompter yet)",
            self.id
        );
        Ok(())
    }

    async fn dismiss(&self) -> zbus::fdo::Result<()> {
        log::info!("portal: secrets prompt {} dismissed", self.id);
        Ok(())
    }

    /// Declared for introspection; the compat unlock flow emits it through
    /// the blocking connection instead of this generated helper.
    #[zbus(signal)]
    pub async fn completed(
        emitter: &zbus::object_server::SignalEmitter<'_>,
        dismissed: bool,
        result: Value<'_>,
    ) -> zbus::Result<()>;
}
