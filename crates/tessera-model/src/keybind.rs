//! Configurable global key bindings.
//!
//! Pure: a [`Keymap`] is an ordered list of [`Keybind`]s — (modifier mask,
//! keysym) → [`Action`]. The binary layers the configuration file over the
//! built-in defaults and matches each non-captured key press against it.
//! Keeping this module in `tessera-model` (no flux, lens, or Wayland dependency)
//! lets the binding table and name resolvers be unit-tested in isolation.
//!
//! Matching is exact on the depressed modifier mask: a binding fires only when
//! the active modifiers equal its mask exactly (so `Super+Q` does not also
//! fire on `Ctrl+Super+Q`). This is predictable and unambiguous; lock state
//! (CapsLock, NumLock) lives in xkbcommon's `locked` mask and does not pollute
//! the `depressed` mask the matcher reads.

use crate::input::{
    Mods, XKB_KEY_BackSpace, XKB_KEY_Down, XKB_KEY_Escape, XKB_KEY_ISO_Left_Tab, XKB_KEY_Print,
    XKB_KEY_Return, XKB_KEY_Tab, XKB_KEY_Up,
};

/// A compositor action a key binding can trigger.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Open or close the application launcher.
    ToggleLauncher,
    /// Open or close Prism application search.
    TogglePrism,
    /// Open or close the window/workspace overview (M9).
    ToggleOverview,
    /// Open or close the modal command panel (ADR-0080).
    ToggleCommandPanel,
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
    /// Toggle the focused toplevel between fullscreen and its prior state.
    ///
    /// The compositor-side counterpart of the client's
    /// `xdg_toplevel.set_fullscreen`/`unset_fullscreen` requests: it puts a
    /// window that never asked for fullscreen — a windowed game, for
    /// example — into the same output-covering, chrome-suppressing state a
    /// native fullscreen request produces (one `XDG_TOPLEVEL_STATE_FULLSCREEN`
    /// configure, saved floating rect restored on exit).
    ToggleFullscreen,
    /// Open the interactive screenshot region selector.
    Screenshot,
    /// Secure the current session with the trusted lock client.
    Lock,
    /// Quit the compositor.
    Quit,
}

impl Action {
    /// Whether this compositor action remains available while trusted shell
    /// chrome owns the keyboard.
    ///
    /// Modal chrome still receives ordinary navigation and text input, and
    /// actions that mutate obscured desktop state stay suppressed. The
    /// launcher, Prism, and command-panel toggles, the screenshot selector, and
    /// the emergency quit path are compositor-level controls that must
    /// remain reachable.
    const fn allowed_during_keyboard_capture(self) -> bool {
        matches!(
            self,
            Action::ToggleLauncher
                | Action::TogglePrism
                | Action::ToggleCommandPanel
                | Action::Screenshot
                | Action::Lock
                | Action::Quit
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
    /// Built-in defaults.
    pub fn defaults() -> Keymap {
        Keymap {
            binds: vec![
                kb(Mods::SUPER, XKB_KEY_Tab, Action::CycleFocus),
                kb(
                    Mods::SUPER | Mods::SHIFT,
                    XKB_KEY_Tab,
                    Action::CycleFocusBack,
                ),
                kb(Mods::SUPER, b'a' as u32, Action::ToggleLauncher),
                kb(Mods::SUPER, b' ' as u32, Action::TogglePrism),
                kb(Mods::SUPER, 0x6f, Action::ToggleOverview), /* 'o' */
                kb(Mods::SUPER, 0x73, Action::ToggleCommandPanel), /* 's' */
                kb(Mods::SUPER, 0x71, Action::CloseFocused),   /* 'q' */
                kb(Mods::SUPER, 0xff53, Action::WorkspaceNext), /* Right */
                kb(Mods::SUPER, 0xff51, Action::WorkspacePrev), /* Left */
                kb(Mods::SUPER, 0xffc8, Action::ToggleFullscreen), /* F11 */
                kb(Mods::SUPER, b'l' as u32, Action::Lock),
                kb(Mods::NONE, XKB_KEY_Print, Action::Screenshot), /* Print Screen */
                kb(Mods::SUPER | Mods::CTRL, b'q' as u32, Action::Quit),
                kb(Mods::SUPER | Mods::SHIFT, XKB_KEY_Return, Action::Quit),
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

    /// Keep only bindings whose actions are available in the consuming
    /// product build. This leaves the model's portable action vocabulary and
    /// defaults feature-independent while allowing a composition root to
    /// avoid claiming input for components it did not compile.
    pub fn retain_actions(mut self, mut keep: impl FnMut(Action) -> bool) -> Keymap {
        self.binds.retain(|binding| keep(binding.action));
        self
    }

    /// Find the action for a pressed key. Modifier masks match exactly; ASCII
    /// letter keysyms are normalized to lowercase. First match wins.
    pub fn match_key(&self, mods: Mods, keysym: u32) -> Option<Action> {
        self.binds
            .iter()
            .find(|b| b.mods == mods && normalize_keysym(b.keysym) == normalize_keysym(keysym))
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

const fn normalize_keysym(keysym: u32) -> u32 {
    if keysym >= b'A' as u32 && keysym <= b'Z' as u32 {
        keysym + (b'a' - b'A') as u32
    } else if keysym == XKB_KEY_ISO_Left_Tab {
        XKB_KEY_Tab
    } else {
        keysym
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
        "prism" | "toggleprism" | "spotlight" => Action::TogglePrism,
        "overview" | "toggleoverview" => Action::ToggleOverview,
        "command_panel" | "commandpanel" | "panel" => Action::ToggleCommandPanel,
        "close" | "closefocused" => Action::CloseFocused,
        "cycle" | "next" => Action::CycleFocus,
        "prev" | "previous" | "cycleback" => Action::CycleFocusBack,
        "workspace_next" | "next_workspace" | "ws_next" => Action::WorkspaceNext,
        "workspace_prev" | "prev_workspace" | "ws_prev" => Action::WorkspacePrev,
        "fullscreen" | "toggle_fullscreen" | "togglefullscreen" => Action::ToggleFullscreen,
        "screenshot" | "snapshot" | "prtsc" => Action::Screenshot,
        "lock" | "lockscreen" | "lock_screen" => Action::Lock,
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
        assert_eq!(
            km.match_key(Mods::SUPER | Mods::SHIFT, XKB_KEY_ISO_Left_Tab),
            Some(Action::CycleFocusBack)
        );
        // Super+A → launcher.
        assert_eq!(
            km.match_key(Mods::SUPER, b'a' as u32),
            Some(Action::ToggleLauncher)
        );
        // Super+Space → Prism.
        assert_eq!(
            km.match_key(Mods::SUPER, b' ' as u32),
            Some(Action::TogglePrism)
        );
        // Super+S → command panel.
        assert_eq!(
            km.match_key(Mods::SUPER, b's' as u32),
            Some(Action::ToggleCommandPanel)
        );
        // The old Super+Return default is gone.
        assert_eq!(km.match_key(Mods::SUPER, XKB_KEY_Return), None);
        // Super+Ctrl+Q → quit.
        assert_eq!(
            km.match_key(Mods::SUPER | Mods::CTRL, b'q' as u32),
            Some(Action::Quit)
        );
        // Super+Shift+Return remains an alternate quit binding.
        assert_eq!(
            km.match_key(Mods::SUPER | Mods::SHIFT, XKB_KEY_Return),
            Some(Action::Quit)
        );
        // Super+q → close focused ('q' keysym 0x71).
        assert_eq!(km.match_key(Mods::SUPER, 0x71), Some(Action::CloseFocused));
        // Bare Print → screenshot.
        assert_eq!(
            km.match_key(Mods::NONE, XKB_KEY_Print),
            Some(Action::Screenshot)
        );
        // Super+L → secure session lock.
        assert_eq!(km.match_key(Mods::SUPER, b'l' as u32), Some(Action::Lock));
        // Super+F11 → toggle fullscreen (0xffc8).
        assert_eq!(
            km.match_key(Mods::SUPER, 0xffc8),
            Some(Action::ToggleFullscreen)
        );
        // Bare F11 is left to the focused client (in-app fullscreen).
        assert_eq!(km.match_key(Mods::NONE, 0xffc8), None);
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
            km.match_key_during_keyboard_capture(Mods::SUPER, b'a' as u32),
            Some(Action::ToggleLauncher)
        );
        assert_eq!(
            km.match_key_during_keyboard_capture(Mods::SUPER, b' ' as u32),
            Some(Action::TogglePrism)
        );
        assert_eq!(
            km.match_key_during_keyboard_capture(Mods::SUPER, b's' as u32),
            Some(Action::ToggleCommandPanel)
        );
        assert_eq!(
            km.match_key_during_keyboard_capture(Mods::SUPER | Mods::CTRL, b'Q' as u32),
            Some(Action::Quit)
        );
        assert_eq!(
            km.match_key_during_keyboard_capture(Mods::SUPER, b'l' as u32),
            Some(Action::Lock)
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
    fn action_filter_removes_unavailable_bindings() {
        let km = Keymap::defaults().retain_actions(|action| action != Action::TogglePrism);
        assert_eq!(km.match_key(Mods::SUPER, b' ' as u32), None);
        assert_eq!(
            km.match_key(Mods::SUPER, b'a' as u32),
            Some(Action::ToggleLauncher)
        );
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
    fn prism_action_accepts_documented_names() {
        assert_eq!(action_from_name("prism"), Some(Action::TogglePrism));
        assert_eq!(action_from_name("toggleprism"), Some(Action::TogglePrism));
        assert_eq!(action_from_name("spotlight"), Some(Action::TogglePrism));
    }

    #[test]
    fn fullscreen_action_accepts_documented_names() {
        assert_eq!(
            action_from_name("fullscreen"),
            Some(Action::ToggleFullscreen)
        );
        assert_eq!(
            action_from_name("toggle_fullscreen"),
            Some(Action::ToggleFullscreen)
        );
        assert_eq!(
            action_from_name("togglefullscreen"),
            Some(Action::ToggleFullscreen)
        );
        assert_eq!(action_from_name("maximize"), None);
    }

    #[test]
    fn command_panel_action_accepts_documented_names() {
        assert_eq!(
            action_from_name("command_panel"),
            Some(Action::ToggleCommandPanel)
        );
        assert_eq!(
            action_from_name("commandpanel"),
            Some(Action::ToggleCommandPanel)
        );
        assert_eq!(action_from_name("panel"), Some(Action::ToggleCommandPanel));
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
