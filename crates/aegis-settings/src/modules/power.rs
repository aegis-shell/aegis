use aegis_design::Design;
use aegis_model::settings::{IdleSettings, SettingsAction, SettingsSnapshot};
use aegis_shell::{Localizer, Message};
use lens::{Align, Frame, Icon, LayoutOpts};

use crate::module::{
    ApplyPolicy, ModuleAvailability, ModuleCategory, ModuleEvents, ModuleId, ModuleMetadata,
    SettingsModule,
};
use crate::ui::{section_heading_layout, settings_card_layout};

pub(crate) const POWER_MODULE_ID: ModuleId = ModuleId::new("power");

/// A transactional editor for the security-ordered inactivity policy.
pub(crate) struct PowerModule {
    authoritative: IdleSettings,
    draft: IdleSettings,
    dirty: bool,
    invalid: bool,
}

impl PowerModule {
    pub(crate) fn new() -> Self {
        let settings = IdleSettings::default();
        Self {
            authoritative: settings,
            draft: settings,
            dirty: false,
            invalid: false,
        }
    }

    fn reset_editor(&mut self) {
        self.draft = self.authoritative;
        self.dirty = false;
        self.invalid = false;
    }

    /// Preserve the security order when a timeout changes. The selected stage
    /// wins and later enabled stages move forward by at least one minute.
    fn normalize_after(&mut self, selected: usize) {
        let mut stages = [
            self.draft.dim_after_seconds,
            self.draft.lock_after_seconds,
            self.draft.display_off_after_seconds,
            self.draft.suspend_after_seconds,
        ];
        if selected != 0 && stages[selected] != 0 {
            let previous = stages[..selected]
                .iter()
                .rev()
                .copied()
                .find(|seconds| *seconds != 0)
                .unwrap_or(0);
            if stages[selected] <= previous {
                stages[selected] = previous
                    .saturating_add(60)
                    .min(IdleSettings::MAX_TIMEOUT_SECONDS);
            }
        }
        let mut previous = stages[selected];
        for stage in stages.iter_mut().skip(selected + 1) {
            if *stage != 0 && *stage <= previous {
                *stage = previous
                    .checked_add(60)
                    .filter(|seconds| *seconds <= IdleSettings::MAX_TIMEOUT_SECONDS)
                    .unwrap_or(0);
            }
            if *stage != 0 {
                previous = *stage;
            }
        }
        [
            self.draft.dim_after_seconds,
            self.draft.lock_after_seconds,
            self.draft.display_off_after_seconds,
            self.draft.suspend_after_seconds,
        ] = stages;
        self.invalid = self.draft.validate().is_err();
    }

    fn toggle_stage(&mut self, stage: usize, enabled: bool) {
        let defaults = IdleSettings::default();
        let mut stages = [
            self.draft.dim_after_seconds,
            self.draft.lock_after_seconds,
            self.draft.display_off_after_seconds,
            self.draft.suspend_after_seconds,
        ];
        if enabled {
            let fallback = [
                defaults.dim_after_seconds,
                defaults.lock_after_seconds,
                defaults.display_off_after_seconds,
                defaults.suspend_after_seconds,
            ][stage];
            let previous = stages[..stage]
                .iter()
                .rev()
                .copied()
                .find(|seconds| *seconds != 0)
                .unwrap_or(0);
            stages[stage] = fallback.max(previous.saturating_add(60));
        } else {
            stages[stage] = 0;
            // Power transitions without a lock boundary are never exposed as
            // a transient draft.
            if stage == 1 {
                stages[2] = 0;
                stages[3] = 0;
            }
        }
        [
            self.draft.dim_after_seconds,
            self.draft.lock_after_seconds,
            self.draft.display_off_after_seconds,
            self.draft.suspend_after_seconds,
        ] = stages;
        self.normalize_after(stage);
    }

    fn stage_editable(&self, stage: usize) -> bool {
        if !self.draft.enabled || (stage >= 2 && self.draft.lock_after_seconds == 0) {
            return false;
        }
        [
            self.draft.dim_after_seconds,
            self.draft.lock_after_seconds,
            self.draft.display_off_after_seconds,
            self.draft.suspend_after_seconds,
        ][..stage]
            .iter()
            .copied()
            .filter(|seconds| *seconds != 0)
            .max()
            .unwrap_or(0)
            < IdleSettings::MAX_TIMEOUT_SECONDS
    }
}

impl Default for PowerModule {
    fn default() -> Self {
        Self::new()
    }
}

impl SettingsModule for PowerModule {
    fn metadata(&self) -> ModuleMetadata {
        ModuleMetadata {
            id: POWER_MODULE_ID,
            title: Message::PowerManagement,
            icon: Icon::Zap,
            category: ModuleCategory::System,
            keywords: &[
                "power", "idle", "lock", "screen", "display", "sleep", "suspend",
            ],
            apply_policy: ApplyPolicy::Explicit,
            availability: ModuleAvailability::Available,
        }
    }

    fn render(
        &mut self,
        frame: &mut Frame,
        i18n: &Localizer,
        design: &Design,
        out: &mut ModuleEvents,
    ) {
        frame.heading(i18n.text(Message::PowerManagement), 2);
        frame.label_wrapped_sized(
            i18n.text(Message::PowerManagementDescription),
            design.typography.label,
            560.0,
        );

        frame.column_ex(&settings_card_layout(design), |frame| {
            frame.heading(i18n.text(Message::AutomaticIdle), 3);
            if frame
                .setting_switch(
                    "power-automatic-idle",
                    i18n.text(Message::AutomaticIdle),
                    i18n.text(Message::AutomaticIdleDescription),
                    &mut self.draft.enabled,
                    false,
                )
                .changed
            {
                self.dirty = true;
            }
        });

        frame.column_ex(&settings_card_layout(design), |frame| {
            frame.heading(i18n.text(Message::DimDisplay), 3);
            let before = self.draft.dim_after_seconds != 0;
            let editable = self.stage_editable(0);
            if stage_editor(
                frame,
                i18n,
                "power-dim",
                Message::DimDisplay,
                Message::DimDisplayDescription,
                &mut self.draft.dim_after_seconds,
                editable,
                design,
            ) {
                if before != (self.draft.dim_after_seconds != 0) {
                    self.toggle_stage(0, !before);
                } else {
                    self.normalize_after(0);
                }
                self.dirty = true;
            }
            if self.draft.dim_after_seconds != 0 {
                frame.row_ex(&section_heading_layout(), |frame| {
                    frame.label_sized(i18n.text(Message::DimLevel), design.typography.label);
                    frame.flex(1.0);
                    frame.spacer(0.0);
                    frame.label_sized(
                        &format!("{}%", self.draft.dim_percent),
                        design.typography.footnote,
                    );
                });
                let mut percent = f32::from(self.draft.dim_percent);
                if self.draft.enabled && frame.slider("##power-dim-level", &mut percent, 1.0, 100.0)
                {
                    self.draft.dim_percent = percent.round() as u8;
                    self.dirty = true;
                }
            }
        });

        frame.column_ex(&settings_card_layout(design), |frame| {
            frame.heading(i18n.text(Message::LockAfterIdle), 3);
            let before = self.draft.lock_after_seconds != 0;
            let editable = self.stage_editable(1);
            if stage_editor(
                frame,
                i18n,
                "power-lock",
                Message::LockAfterIdle,
                Message::LockAfterIdleDescription,
                &mut self.draft.lock_after_seconds,
                editable,
                design,
            ) {
                if before != (self.draft.lock_after_seconds != 0) {
                    self.toggle_stage(1, !before);
                } else {
                    self.normalize_after(1);
                }
                self.dirty = true;
            }
        });

        frame.column_ex(&settings_card_layout(design), |frame| {
            frame.heading(i18n.text(Message::TurnDisplayOff), 3);
            let before = self.draft.display_off_after_seconds != 0;
            let editable = self.stage_editable(2);
            if stage_editor(
                frame,
                i18n,
                "power-display-off",
                Message::TurnDisplayOff,
                Message::TurnDisplayOffDescription,
                &mut self.draft.display_off_after_seconds,
                editable,
                design,
            ) {
                if before != (self.draft.display_off_after_seconds != 0) {
                    self.toggle_stage(2, !before);
                } else {
                    self.normalize_after(2);
                }
                self.dirty = true;
            }
        });

        frame.column_ex(&settings_card_layout(design), |frame| {
            frame.heading(i18n.text(Message::SuspendAutomatically), 3);
            let before = self.draft.suspend_after_seconds != 0;
            let editable = self.stage_editable(3);
            if stage_editor(
                frame,
                i18n,
                "power-suspend",
                Message::SuspendAutomatically,
                Message::SuspendAutomaticallyDescription,
                &mut self.draft.suspend_after_seconds,
                editable,
                design,
            ) {
                if before != (self.draft.suspend_after_seconds != 0) {
                    self.toggle_stage(3, !before);
                } else {
                    self.normalize_after(3);
                }
                self.dirty = true;
            }
        });

        self.invalid = self.draft.validate().is_err();
        if self.invalid {
            frame.label_wrapped_sized(
                i18n.text(Message::InvalidPowerSettings),
                design.typography.footnote,
                560.0,
            );
        }
        frame.label_wrapped_sized(
            i18n.text(Message::PowerApplyHint),
            design.typography.footnote,
            560.0,
        );
        frame.row_ex(
            &LayoutOpts {
                height: 32.0,
                gap: 8.0,
                cross: Align::Center,
                ..Default::default()
            },
            |frame| {
                frame.size_next(210.0, 30.0);
                if frame.button(i18n.text(Message::ApplyPowerSettings))
                    && self.dirty
                    && !self.invalid
                {
                    out.actions.push(SettingsAction::SetIdle {
                        settings: self.draft,
                    });
                    self.dirty = false;
                }
                frame.size_next(92.0, 30.0);
                if frame.button(i18n.text(Message::ResetPowerSettings)) {
                    self.reset_editor();
                }
            },
        );
    }

    fn update_settings(&mut self, snapshot: &SettingsSnapshot) {
        self.authoritative = snapshot.idle;
        if !self.dirty {
            self.reset_editor();
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn stage_editor(
    frame: &mut Frame,
    i18n: &Localizer,
    id: &str,
    label: Message,
    description: Message,
    seconds: &mut u32,
    editable: bool,
    design: &Design,
) -> bool {
    let mut changed = false;
    let mut enabled = *seconds != 0;
    let switched = frame
        .setting_switch(
            id,
            i18n.text(label),
            i18n.text(description),
            &mut enabled,
            !editable,
        )
        .changed;
    if switched {
        // The caller substitutes the stage's secure default. A non-zero
        // placeholder lets it distinguish enable from disable without
        // duplicating the switch interaction here.
        *seconds = u32::from(enabled).saturating_mul(60);
        changed = true;
    }
    if enabled {
        frame.row_ex(&section_heading_layout(), |frame| {
            frame.flex(1.0);
            frame.spacer(0.0);
            if editable {
                let presets = timeout_presets(*seconds);
                let labels = presets
                    .iter()
                    .map(|seconds| format_duration(*seconds, i18n))
                    .collect::<Vec<_>>();
                let items = labels.iter().map(String::as_str).collect::<Vec<_>>();
                let mut selected = presets
                    .iter()
                    .position(|preset| preset == seconds)
                    .unwrap_or_default() as i32;
                frame.size_next(210.0, 30.0);
                if frame.dropdown(&format!("##{id}-timeout"), &mut selected, &items)
                    && let Some(selected) = usize::try_from(selected)
                        .ok()
                        .and_then(|index| presets.get(index))
                {
                    *seconds = *selected;
                    changed = true;
                }
            } else {
                frame.label_sized(&format_duration(*seconds, i18n), design.typography.footnote);
            }
        });
    }
    changed
}

fn timeout_presets(current: u32) -> Vec<u32> {
    let mut presets = vec![
        60,
        120,
        300,
        600,
        660,
        900,
        1_800,
        2_700,
        3_600,
        7_200,
        14_400,
        28_800,
        43_200,
        86_400,
        259_200,
        IdleSettings::MAX_TIMEOUT_SECONDS,
    ];
    if current != 0 && !presets.contains(&current) {
        presets.push(current);
        presets.sort_unstable();
    }
    presets
}

fn format_duration(seconds: u32, i18n: &Localizer) -> String {
    if i18n.language() == aegis_shell::Language::SimplifiedChinese {
        if seconds.is_multiple_of(86_400) {
            format!("闲置 {} 天后", seconds / 86_400)
        } else if seconds.is_multiple_of(3_600) {
            format!("闲置 {} 小时后", seconds / 3_600)
        } else if seconds.is_multiple_of(60) {
            format!("闲置 {} 分钟后", seconds / 60)
        } else {
            format!("闲置 {seconds} 秒后")
        }
    } else {
        let (amount, unit) = if seconds.is_multiple_of(86_400) {
            (seconds / 86_400, "day")
        } else if seconds.is_multiple_of(3_600) {
            (seconds / 3_600, "hour")
        } else if seconds.is_multiple_of(60) {
            (seconds / 60, "minute")
        } else {
            (seconds, "second")
        };
        format!(
            "After {amount} {unit}{}",
            if amount == 1 { "" } else { "s" }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moving_an_earlier_stage_preserves_security_order() {
        let mut module = PowerModule::new();
        module.draft.dim_after_seconds = 700;
        module.normalize_after(0);
        assert!(module.draft.lock_after_seconds > module.draft.dim_after_seconds);
        assert!(module.draft.validate().is_ok());
    }

    #[test]
    fn disabling_lock_also_disables_power_transitions() {
        let mut module = PowerModule::new();
        module.toggle_stage(1, false);
        assert_eq!(module.draft.lock_after_seconds, 0);
        assert_eq!(module.draft.display_off_after_seconds, 0);
        assert_eq!(module.draft.suspend_after_seconds, 0);
        assert!(module.draft.validate().is_ok());
    }

    #[test]
    fn custom_timeout_is_preserved_as_a_selectable_preset() {
        let presets = timeout_presets(737);
        assert!(presets.contains(&737));
        assert!(presets.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn duration_format_does_not_round_custom_seconds() {
        let i18n = Localizer::new("en-US");
        assert_eq!(format_duration(737, &i18n), "After 737 seconds");
    }

    #[test]
    fn selecting_the_maximum_timeout_disables_impossible_later_stages() {
        let mut module = PowerModule::new();
        module.draft.dim_after_seconds = IdleSettings::MAX_TIMEOUT_SECONDS;
        module.normalize_after(0);
        assert_eq!(module.draft.lock_after_seconds, 0);
        assert_eq!(module.draft.display_off_after_seconds, 0);
        assert_eq!(module.draft.suspend_after_seconds, 0);
        assert!(module.draft.validate().is_ok());
    }
}
