//! Configurable global key bindings.
//!
//! Pure: a [`Keymap`] is an ordered list of [`Keybind`]s — (modifier mask,
//! keysym) → [`Action`]. The binary builds one from the built-in defaults
//! plus an optional `ASS_KEYBINDS` env var, and matches each non-captured
//! key press against it. Keeping this module in `ass-core` (no flux, lens, or
//! Wayland dependency) lets the binding table and its parser be unit-tested
//! in isolation.
//!
//! Matching is exact on the depressed modifier mask: a binding fires only when
//! the active modifiers equal its mask exactly (so `Super+Q` does not also
//! fire on `Ctrl+Super+Q`). This is predictable and unambiguous; lock state
//! (CapsLock, NumLock) lives in xkbcommon's `locked` mask and does not pollute
//! the `depressed` mask the matcher reads.

use crate::input::{
    Mods, XKB_KEY_BackSpace, XKB_KEY_Down, XKB_KEY_Escape, XKB_KEY_Return, XKB_KEY_Tab, XKB_KEY_Up,
};

/// A compositor action a key binding can trigger.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Open or close the application launcher.
    ToggleLauncher,
    /// Close the currently focused toplevel.
    CloseFocused,
    /// Move keyboard focus to the next mapped toplevel (forward).
    CycleFocus,
    /// Move keyboard focus to the previous mapped toplevel (backward).
    CycleFocusBack,
    /// Switch to the next workspace on the focused output (ADR-0025).
    WorkspaceNext,
    /// Switch to the previous workspace on the focused output.
    WorkspacePrev,
    /// Toggle the current workspace between tiled and floating (ADR-0024).
    ToggleTiling,
    /// Quit the compositor.
    Quit,
}

/// One binding: an exact modifier mask plus a keysym, mapped to an action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Keybind {
    pub mods: Mods,
    pub keysym: u32,
    pub action: Action,
}

/// An ordered set of bindings; the first exact match wins.
#[derive(Debug, Clone, Default)]
pub struct Keymap {
    binds: Vec<Keybind>,
}

impl Keymap {
    /// Built-in defaults, used when `ASS_KEYBINDS` is unset or empty. A bare
    /// Super tap (handled by `TapDetector` in the binary, not a binding) also
    /// toggles the launcher, so these are additive.
    pub fn defaults() -> Keymap {
        Keymap {
            binds: vec![
                kb(Mods::SUPER, XKB_KEY_Tab, Action::CycleFocus),
                kb(
                    Mods::SUPER | Mods::SHIFT,
                    XKB_KEY_Tab,
                    Action::CycleFocusBack,
                ),
                kb(Mods::SUPER, XKB_KEY_Return, Action::ToggleLauncher),
                kb(Mods::SUPER, 0x71, Action::CloseFocused), /* 'q' */
                kb(Mods::SUPER, 0xff53, Action::WorkspaceNext), /* Right */
                kb(Mods::SUPER, 0xff51, Action::WorkspacePrev), /* Left */
                kb(Mods::SUPER, 0x74, Action::ToggleTiling), /* 't' */
                kb(Mods::SUPER | Mods::SHIFT, 0xff0d, Action::Quit),
            ],
        }
    }

    /// Parse `ASS_KEYBINDS`-style overrides: `Mod+Mod+key=action;...`
    /// (e.g. `super+space=launcher;super+q=close`). Returns the parsed
    /// bindings plus one error string per malformed entry, so the caller can
    /// log the rejects without aborting the good ones. Empty / whitespace-only
    /// entries are skipped silently.
    pub fn parse_overrides(s: &str) -> (Vec<Keybind>, Vec<String>) {
        let mut binds = Vec::new();
        let mut errs = Vec::new();
        for raw in s.split(';') {
            let entry = raw.trim();
            if entry.is_empty() {
                continue;
            }
            let Some((lhs, rhs)) = entry.split_once('=') else {
                errs.push(format!("'{entry}': missing '='"));
                continue;
            };
            let mut mods = Mods::NONE;
            let mut keysym = None;
            // At most one diagnostic per entry: the first problem seen wins,
            // later checks short-circuit so a malformed entry is not noisy.
            let mut err: Option<String> = None;
            for tok in lhs.split('+') {
                let t = tok.trim().to_ascii_lowercase();
                if t.is_empty() {
                    continue;
                }
                if err.is_some() {
                    continue;
                }
                if let Some(m) = mod_from_name(&t) {
                    mods |= m;
                } else if let Some(k) = keysym_from_name(&t) {
                    keysym = Some(k);
                } else {
                    err = Some(format!("unknown token '{}'", tok.trim()));
                }
            }
            if err.is_none() && keysym.is_none() {
                err = Some("no key found".to_string());
            }
            let action = action_from_name(rhs.trim());
            if err.is_none() && action.is_none() {
                err = Some(format!("unknown action '{}'", rhs.trim()));
            }
            match (err, keysym, action) {
                (Some(e), _, _) => errs.push(format!("'{entry}': {e}")),
                (None, Some(k), Some(a)) => binds.push(kb(mods, k, a)),
                // Unreachable: err is set whenever keysym or action is None.
                (None, _, _) => {}
            }
        }
        (binds, errs)
    }

    /// Prepend `overrides` so user bindings take precedence over the
    /// defaults. The returned keymap keeps the defaults as a fallback.
    pub fn with_overrides(mut self, overrides: Vec<Keybind>) -> Keymap {
        let mut combined = overrides;
        combined.append(&mut self.binds);
        Keymap { binds: combined }
    }

    /// Find the action for a pressed key, if any binding matches its modifier
    /// mask and keysym exactly. First match wins.
    pub fn match_key(&self, mods: Mods, keysym: u32) -> Option<Action> {
        self.binds
            .iter()
            .find(|b| b.mods == mods && b.keysym == keysym)
            .map(|b| b.action)
    }

    /// Number of bindings (for diagnostics / tests).
    pub fn len(&self) -> usize {
        self.binds.len()
    }

    /// Whether the keymap is empty.
    pub fn is_empty(&self) -> bool {
        self.binds.is_empty()
    }
}

const fn kb(mods: Mods, keysym: u32, action: Action) -> Keybind {
    Keybind {
        mods,
        keysym,
        action,
    }
}

pub fn mod_from_name(s: &str) -> Option<Mods> {
    Some(match s {
        "shift" => Mods::SHIFT,
        "ctrl" | "control" => Mods::CTRL,
        "alt" | "mod1" => Mods::ALT,
        "super" | "meta" | "logo" | "mod4" | "win" => Mods::SUPER,
        _ => return None,
    })
}

pub fn action_from_name(s: &str) -> Option<Action> {
    Some(match s.to_ascii_lowercase().as_str() {
        "launcher" | "togglelauncher" | "apps" => Action::ToggleLauncher,
        "close" | "closefocused" => Action::CloseFocused,
        "cycle" | "next" => Action::CycleFocus,
        "prev" | "previous" | "cycleback" => Action::CycleFocusBack,
        "workspace_next" | "next_workspace" | "ws_next" => Action::WorkspaceNext,
        "workspace_prev" | "prev_workspace" | "ws_prev" => Action::WorkspacePrev,
        "tiling" | "toggle_tiling" => Action::ToggleTiling,
        "quit" | "exit" => Action::Quit,
        _ => return None,
    })
}

/// Resolve a key name to its XKB keysym. Letters are lowercase (modifiers like
/// Super do not shift them, so xkbcommon reports the lowercase keysym). Names
/// cover the common control keys; unknown names return `None`.
pub fn keysym_from_name(s: &str) -> Option<u32> {
    let lower = s.to_ascii_lowercase();
    let mut chars = lower.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) if c.is_ascii_alphanumeric() => {
            return Some(c.to_ascii_lowercase() as u32);
        }
        _ => {}
    }
    Some(match lower.as_str() {
        "space" => 0x20,
        "return" | "enter" => XKB_KEY_Return,
        "escape" | "esc" => XKB_KEY_Escape,
        "tab" => XKB_KEY_Tab,
        "backspace" | "bs" => XKB_KEY_BackSpace,
        "up" => XKB_KEY_Up,
        "down" => XKB_KEY_Down,
        "left" => 0xff51,
        "right" => 0xff53,
        "home" => 0xff50,
        "end" => 0xff57,
        "pageup" | "pgup" => 0xff55,
        "pagedown" | "pgdn" => 0xff56,
        "delete" | "del" => 0xffff,
        "f1" => 0xffbe,
        "f2" => 0xffbf,
        "f3" => 0xffc0,
        "f4" => 0xffc1,
        "f5" => 0xffc2,
        "f6" => 0xffc3,
        "f7" => 0xffc4,
        "f8" => 0xffc5,
        "f9" => 0xffc6,
        "f10" => 0xffc7,
        "f11" => 0xffc8,
        "f12" => 0xffc9,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::XKB_KEY_NoSymbol;

    #[test]
    fn defaults_match_documented_bindings() {
        let km = Keymap::defaults();
        // Super+Tab → CycleFocus forward.
        assert_eq!(
            km.match_key(Mods::SUPER, XKB_KEY_Tab),
            Some(Action::CycleFocus)
        );
        // Super+Shift+Tab → backward.
        assert_eq!(
            km.match_key(Mods::SUPER | Mods::SHIFT, XKB_KEY_Tab),
            Some(Action::CycleFocusBack)
        );
        // Super+Return → launcher.
        assert_eq!(
            km.match_key(Mods::SUPER, XKB_KEY_Return),
            Some(Action::ToggleLauncher)
        );
        // Super+q → close focused ('q' keysym 0x71).
        assert_eq!(km.match_key(Mods::SUPER, 0x71), Some(Action::CloseFocused));
        // Bare modifier keys never match (no keysym).
        assert_eq!(km.match_key(Mods::SUPER, XKB_KEY_NoSymbol), None);
    }

    #[test]
    fn exact_modifier_match_excludes_extra_mods() {
        let km = Keymap::defaults();
        // Super+Tab must NOT fire when Ctrl is also held.
        assert_eq!(km.match_key(Mods::SUPER | Mods::CTRL, XKB_KEY_Tab), None);
    }

    #[test]
    fn override_takes_precedence_and_keeps_defaults() {
        let (binds, errs) = Keymap::parse_overrides("super+space=launcher");
        assert!(errs.is_empty());
        let km = Keymap::defaults().with_overrides(binds);
        // Override present.
        assert_eq!(
            km.match_key(Mods::SUPER, 0x20),
            Some(Action::ToggleLauncher)
        );
        // Defaults still present.
        assert_eq!(
            km.match_key(Mods::SUPER, XKB_KEY_Tab),
            Some(Action::CycleFocus)
        );
        assert!(km.len() >= 6);
    }

    #[test]
    fn parser_handles_multiple_entries_and_whitespace() {
        let (binds, errs) = Keymap::parse_overrides(" super + q = close ; super+f4 = quit ");
        assert!(errs.is_empty(), "{errs:?}");
        assert_eq!(binds.len(), 2);
        assert_eq!(binds[0].action, Action::CloseFocused);
        assert_eq!(binds[1].action, Action::Quit);
    }

    #[test]
    fn parser_collects_errors_without_aborting() {
        // One diagnostic per malformed entry; good entries still parse.
        let (binds, errs) = Keymap::parse_overrides("super+q=close; nokey=launcher; super=fake");
        assert_eq!(binds.len(), 1);
        assert_eq!(binds[0].action, Action::CloseFocused);
        // "nokey=launcher" → unknown token; "super=fake" → no key found.
        assert_eq!(errs.len(), 2, "{errs:?}");
    }

    #[test]
    fn keysym_name_round_trips_letters_and_controls() {
        assert_eq!(keysym_from_name("q"), Some(0x71));
        assert_eq!(keysym_from_name("Q"), Some(0x71));
        assert_eq!(keysym_from_name("1"), Some(0x31));
        assert_eq!(keysym_from_name("space"), Some(0x20));
        assert_eq!(keysym_from_name("return"), Some(XKB_KEY_Return));
        assert_eq!(keysym_from_name("f4"), Some(0xffc1));
        assert_eq!(keysym_from_name("nonsense"), None);
    }

    #[test]
    fn mod_aliases_resolve() {
        assert_eq!(mod_from_name("super"), Some(Mods::SUPER));
        assert_eq!(mod_from_name("win"), Some(Mods::SUPER));
        assert_eq!(mod_from_name("control"), Some(Mods::CTRL));
        assert_eq!(mod_from_name("mod1"), Some(Mods::ALT));
        assert_eq!(mod_from_name("shift"), Some(Mods::SHIFT));
        assert_eq!(mod_from_name("caps"), None);
    }
}
