//! Low-battery warning alerts.
//!
//! Unlike the picks and prompts there is no IPC requester: the compositor
//! itself opens the alert when a configured `[battery]` threshold fires, and
//! dismissal travels no reply. All policy — once per threshold per discharge
//! cycle, recovery rearm, charge reset — lives in the tested latch
//! (`tessera_model::system::BatteryWarningLatches`); this module only gates and
//! forwards.

use super::*;

impl CompositorRuntime {
    /// Evaluate the latest battery sample against the live `[battery]`
    /// thresholds and open the alert chrome on a fresh crossing.
    ///
    /// Skipped without latching while the session is locked or inactive, or
    /// while another modal owns the chrome layer (the same predicates the
    /// pick/prompt control paths enforce, plus the exclusive keyboard
    /// surfaces); the status cadence retries a skipped warning on the next
    /// sample. An already-open alert is not a gate: a deeper crossing
    /// escalates it in place to the critical wording.
    pub(super) fn poll_battery_warning(&mut self) {
        let Some(battery) = self.system_status.battery else {
            return;
        };
        if self.server.session_locked() || !self.host.is_active() {
            return;
        }
        if self.pending_pick.is_some()
            || self.pending_app_pick.is_some()
            || self.pending_secret_prompt.is_some()
            || self.pending_confirm_pick.is_some()
            || self.pending_capability_pick.is_some()
            || self.shell.screenshot_active()
            || self.shell.command_panel_active()
            || self.shell.window_switcher_active()
        {
            return;
        }
        let thresholds = self
            .config
            .as_ref()
            .map(|config| config.battery.warn_at.as_slice())
            .unwrap_or(&[]);
        if let Some(threshold) =
            self.battery_latches
                .poll(battery.percent, battery.charging, thresholds)
        {
            let critical = thresholds.iter().min().copied() == Some(threshold);
            self.shell
                .start_battery_alert(tessera_shell::BatteryAlertParams {
                    percent: battery.percent,
                    critical,
                });
            // Opening the alert is a shell mutation outside the signed paths;
            // without damage the tick that carried an unchanged sample would
            // present nothing.
            self.damage.chrome_dirty = true;
        }
    }
}
