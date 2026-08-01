//! Per-call tool permission gate driven by `[permissions]` configuration.

use std::io::{BufRead, Write};

use crate::agent::config::{PermissionMode, PermissionsConfig};

/// Outcome of one permission decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Deny(String),
}

/// Checks each tool call against the configured policy. `Ask` decisions are
/// resolved by a prompt; `auto_approve` answers every prompt with yes.
pub struct PermissionGate {
    config: PermissionsConfig,
    auto_approve: bool,
}

impl PermissionGate {
    pub fn new(config: PermissionsConfig, auto_approve: bool) -> Self {
        Self {
            config,
            auto_approve,
        }
    }

    /// Decide using the interactive stdin prompt for `Ask`.
    pub fn check(&self, tool: &str, detail: &str) -> Decision {
        self.check_with(tool, detail, &mut |tool, detail| {
            eprintln!("\naegis-agent wants to run `{tool}`: {detail}");
            eprint!("allow? [y/N] ");

            let _ = std::io::stderr().flush();
            let mut line = String::new();
            match std::io::stdin().lock().read_line(&mut line) {
                Ok(_) => matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes"),
                Err(_) => false,
            }
        })
    }

    /// Decide with a caller-supplied prompt, so tests and non-interactive
    /// frontends can answer `Ask` themselves.
    pub fn check_with(
        &self,
        tool: &str,
        detail: &str,
        ask: &mut dyn FnMut(&str, &str) -> bool,
    ) -> Decision {
        match self.config.mode_for(tool) {
            PermissionMode::Allow => Decision::Allow,
            PermissionMode::Deny => {
                Decision::Deny(format!("tool `{tool}` is denied by configuration"))
            }
            PermissionMode::Ask => {
                if self.auto_approve || ask(tool, detail) {
                    Decision::Allow
                } else {
                    Decision::Deny(format!("tool `{tool}` was declined by the user"))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::config::PermissionsConfig;

    fn make_gate(text: &str, auto_approve: bool) -> PermissionGate {
        let config: PermissionsConfig = toml::from_str(text).expect("permissions");
        PermissionGate::new(config, auto_approve)
    }

    #[test]
    fn explicit_allow_and_deny_short_circuit() {
        let gate = make_gate(
            "default = \"ask\"\nbash = \"allow\"\nwrite_file = \"deny\"\n",
            false,
        );
        let mut never_ask = |_: &str, _: &str| panic!("must not ask");
        assert_eq!(
            gate.check_with("bash", "ls", &mut never_ask),
            Decision::Allow
        );
        assert!(matches!(
            gate.check_with("write_file", "/tmp/x", &mut never_ask),
            Decision::Deny(_)
        ));
    }

    #[test]
    fn ask_uses_prompt_then_falls_back_to_default() {
        let gate = make_gate("default = \"ask\"\n", false);
        let mut yes = |_: &str, _: &str| true;
        let mut no = |_: &str, _: &str| false;
        assert_eq!(gate.check_with("bash", "ls", &mut yes), Decision::Allow);
        assert!(matches!(
            gate.check_with("bash", "ls", &mut no),
            Decision::Deny(_)
        ));
    }

    #[test]
    fn auto_approve_answers_every_ask_with_yes() {
        let gate = make_gate("default = \"ask\"\n", true);
        let mut never_ask = |_: &str, _: &str| panic!("must not ask");
        assert_eq!(
            gate.check_with("bash", "rm -rf /", &mut never_ask),
            Decision::Allow
        );
        // Deny still wins over auto-approve.
        let denied = make_gate("default = \"ask\"\nbash = \"deny\"\n", true);
        assert!(matches!(
            denied.check_with("bash", "ls", &mut never_ask),
            Decision::Deny(_)
        ));
    }
}
