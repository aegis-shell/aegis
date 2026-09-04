//! Wire-format checks for the settings transaction vocabulary. `SettingsAction`
//! variant names are the IPC's tagged-union discriminators (ADR-0027: JSON is
//! the wire format), so renames are protocol breaks and must be deliberate.
#![cfg(feature = "serde")]

use tessera_model::input::{InputConfig, InputStatus, KeyboardConfig, MouseConfig, TouchpadConfig};
use tessera_model::settings::{SettingsAction, SettingsSnapshot};

#[test]
fn set_input_action_round_trips_with_the_expected_tag() {
    let config = InputConfig {
        touchpad: TouchpadConfig {
            scroll_speed: 2.0,
            pointer_speed: 0.3,
            ..Default::default()
        },
        mouse: MouseConfig {
            scroll_speed: 0.5,
            pointer_speed: -0.1,
            natural_scroll: true,
        },
        keyboard: KeyboardConfig {
            repeat_rate: 45,
            repeat_delay_ms: 180,
        },
    };
    let action = SettingsAction::SetInput { config };
    let json = serde_json::to_string(&action).unwrap();
    assert!(json.contains("\"SetInput\""), "tagged union tag: {json}");
    let back: SettingsAction = serde_json::from_str(&json).unwrap();
    assert_eq!(back, action);
}

#[test]
fn settings_snapshot_exposes_the_input_domain() {
    let snapshot = SettingsSnapshot {
        input: InputStatus {
            configurable: true,
            keyboard: KeyboardConfig {
                repeat_rate: 45,
                repeat_delay_ms: 180,
            },
            ..Default::default()
        },
        ..Default::default()
    };
    let json = serde_json::to_string(&snapshot).unwrap();
    assert!(json.contains("\"input\""), "snapshot field: {json}");
    // The renamed field must not appear at the snapshot's top level; nested
    // touchpad status inside `input` is expected.
    let top: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert!(top.get("input").is_some());
    assert!(top.get("touchpad").is_none());
    let back: SettingsSnapshot = serde_json::from_str(&json).unwrap();
    assert_eq!(back.input.keyboard, snapshot.input.keyboard);
}

#[test]
fn keyboard_config_bounds_match_the_documented_ranges() {
    // Rate 0 disables repetition and is valid on the wire.
    let disabled = KeyboardConfig {
        repeat_rate: 0,
        repeat_delay_ms: 250,
    };
    assert!(
        SettingsAction::SetInput {
            config: InputConfig {
                keyboard: disabled,
                ..Default::default()
            },
        }
        .validate()
        .is_ok()
    );
    for bad in [
        KeyboardConfig {
            repeat_rate: 151,
            repeat_delay_ms: 250,
        },
        KeyboardConfig {
            repeat_rate: 25,
            repeat_delay_ms: 0,
        },
        KeyboardConfig {
            repeat_rate: 25,
            repeat_delay_ms: 2001,
        },
    ] {
        assert!(
            SettingsAction::SetInput {
                config: InputConfig {
                    keyboard: bad,
                    ..Default::default()
                },
            }
            .validate()
            .is_err(),
            "{bad:?}"
        );
    }
}
