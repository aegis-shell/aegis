//! Best-effort backlight dimming with exact restoration.

use std::process::{Command, Stdio};

pub struct Dimmer {
    target_percent: u8,
    saved_raw: Option<u64>,
    dimmed: bool,
    unavailable_reported: bool,
}

impl Dimmer {
    pub fn new(target_percent: u8) -> Self {
        Self {
            target_percent,
            saved_raw: None,
            dimmed: false,
            unavailable_reported: false,
        }
    }

    pub fn dim(&mut self) {
        if self.dimmed {
            return;
        }
        let Some(current) = brightness_value("get") else {
            self.report_unavailable();
            return;
        };
        let Some(maximum) = brightness_value("max") else {
            self.report_unavailable();
            return;
        };
        let target = maximum
            .saturating_mul(u64::from(self.target_percent))
            .saturating_div(100)
            .max(1);
        if current <= target {
            return;
        }
        if set_brightness(&target.to_string()) {
            self.saved_raw = Some(current);
            self.dimmed = true;
            log::debug!(
                "idle: backlight dimmed from {current} to {target} ({}%)",
                self.target_percent
            );
        } else {
            self.report_unavailable();
        }
    }

    pub fn restore(&mut self) {
        if !self.dimmed {
            return;
        }
        if let Some(value) = self.saved_raw.take() {
            if set_brightness(&value.to_string()) {
                log::debug!("idle: restored backlight to {value}");
            } else {
                log::warn!("idle: could not restore the previous backlight level");
            }
        }
        self.dimmed = false;
    }

    fn report_unavailable(&mut self) {
        if !self.unavailable_reported {
            log::info!("idle: backlight dimming unavailable; continuing with lock policy");
            self.unavailable_reported = true;
        }
    }
}

impl Drop for Dimmer {
    fn drop(&mut self) {
        self.restore();
    }
}

fn brightness_value(argument: &str) -> Option<u64> {
    let output = Command::new("brightnessctl")
        .args(["--class=backlight", argument])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().parse().ok())
        .flatten()
}

fn set_brightness(value: &str) -> bool {
    Command::new("brightnessctl")
        .args(["--class=backlight", "--quiet", "set", value])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}
