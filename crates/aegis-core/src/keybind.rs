//! Configurable global key bindings.
//!
//! Pure: a [`Keymap`] is an ordered list of [`Keybind`]s — (modifier mask,
//! keysym) → [`Action`]. The binary layers the configuration file over the
//! built-in defaults and matches each non-captured key press against it.
//! Keeping this module in `aegis-core` (no flux, lens, or Wayland dependency)
//! lets the binding table and name resolvers be unit-tested in isolation.
//!
//! Matching is exact on the depressed modifier mask: a binding fires only when
//! the active modifiers equal its mask exactly (so `Super+Q` does not also
//! fire on `Ctrl+Super+Q`). This is predictable and unambiguous; lock state
//! (CapsLock, NumLock) lives in xkbcommon's `locked` mask and does not pollute
//! the `depressed` mask the matcher reads.

use crate::input::{
    Mods, XKB_KEY_BackSpace, XKB_KEY_Down, XKB_KEY_Escape, XKB_KEY_Print, XKB_KEY_Return,
    XKB_KEY_Tab, XKB_KEY_Up,
};

/// A compositor action a key binding can trigger.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Open or close the application launcher.
    ToggleLauncher,
    /// Open or close the window/workspace overview (M9).
    ToggleOverview,
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
    /// Open the interactive screenshot region selector.
    Screenshot,
    /// Quit the compositor.
    Quit,
}

impl Action {
    /// Whether this compositor action remains available while trusted shell
    /// chrome owns the keyboard.
    ///
    /// Modal chrome still receives ordinary navigation and text input, and
    /// actions that mutate obscured desktop state stay suppressed. The
    /// launcher toggle, screenshot selector, and emergency quit path are
    /// compositor-level controls that must remain reachable.
    const fn allowed_during_keyboard_capture(self) -> bool {
        matches!(
            self,
            Action::ToggleLauncher | Action::Screenshot | Action::Quit
        )
    }
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
    /// Built-in defaults. A bare Super tap (handled by `TapDetector` in the
    /// binary, not a binding) also toggles the launcher, so these are additive.
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
                kb(Mods::SUPER, 0x6f, Action::ToggleOverview), /* 'o' */
                kb(Mods::SUPER, 0x71, Action::CloseFocused),   /* 'q' */
                kb(Mods::SUPER, 0xff53, Action::WorkspaceNext), /* Right */
                kb(Mods::SUPER, 0xff51, Action::WorkspacePrev), /* Left */
                kb(Mods::SUPER, 0x74, Action::ToggleTiling),   /* 't' */
                kb(Mods::NONE, XKB_KEY_Print, Action::Screenshot), /* Print Screen */
                kb(Mods::SUPER | Mods::SHIFT, 0xff0d, Action::Quit),
            ],
        }
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

    /// Match only compositor controls that remain available while trusted
    /// shell chrome owns the keyboard.
    pub fn match_key_during_keyboard_capture(&self, mods: Mods, keysym: u32) -> Option<Action> {
        self.match_key(mods, keysym)
            .filter(|action| action.allowed_during_keyboard_capture())
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
        "overview" | "toggleoverview" => Action::ToggleOverview,
        "close" | "closefocused" => Action::CloseFocused,
        "cycle" | "next" => Action::CycleFocus,
        "prev" | "previous" | "cycleback" => Action::CycleFocusBack,
        "workspace_next" | "next_workspace" | "ws_next" => Action::WorkspaceNext,
        "workspace_prev" | "prev_workspace" | "ws_prev" => Action::WorkspacePrev,
        "tiling" | "toggle_tiling" => Action::ToggleTiling,
        "screenshot" | "snapshot" | "prtsc" => Action::Screenshot,
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
        "print" | "prtsc" | "snapshot" => XKB_KEY_Print,
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
        // Bare Print → screenshot.
        assert_eq!(
            km.match_key(Mods::NONE, XKB_KEY_Print),
            Some(Action::Screenshot)
        );
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
    fn only_modal_safe_actions_bypass_keyboard_capture() {
        let km = Keymap::defaults();
        assert_eq!(
            km.match_key_during_keyboard_capture(Mods::NONE, XKB_KEY_Print),
            Some(Action::Screenshot)
        );
        assert_eq!(
            km.match_key_during_keyboard_capture(Mods::SUPER, XKB_KEY_Return),
            Some(Action::ToggleLauncher)
        );
        assert_eq!(
            km.match_key_during_keyboard_capture(Mods::SUPER | Mods::SHIFT, XKB_KEY_Return),
            Some(Action::Quit)
        );
        assert_eq!(
            km.match_key_during_keyboard_capture(Mods::SUPER, 0x71),
            None
        );
        assert_eq!(
            km.match_key_during_keyboard_capture(Mods::SUPER, XKB_KEY_Tab),
            None
        );
    }

    #[test]
    fn override_takes_precedence_and_keeps_defaults() {
        let km =
            Keymap::defaults().with_overrides(vec![kb(Mods::SUPER, 0x20, Action::ToggleLauncher)]);
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
    fn keysym_name_round_trips_letters_and_controls() {
        assert_eq!(keysym_from_name("q"), Some(0x71));
        assert_eq!(keysym_from_name("Q"), Some(0x71));
        assert_eq!(keysym_from_name("1"), Some(0x31));
        assert_eq!(keysym_from_name("space"), Some(0x20));
        assert_eq!(keysym_from_name("return"), Some(XKB_KEY_Return));
        assert_eq!(keysym_from_name("print"), Some(XKB_KEY_Print));
        assert_eq!(keysym_from_name("prtsc"), Some(XKB_KEY_Print));
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
