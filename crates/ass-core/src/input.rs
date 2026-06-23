//! Backend-agnostic input event types.
//!
//! Backends (nested-host, libinput, DRM/KMS) emit these; the main loop drains
//! and routes them — to the focused client via `wl_seat`, to the chrome via
//! `lens::Input`, or both. Keeping the types in `ass-core` (rather than in
//! `ass-backend`) means the server and shell never need to depend on a backend
//! crate to consume input.

// The XKB keysym constants below mirror the C macros in X11/keysymdef.h
// verbatim (e.g. `XKB_KEY_Escape`); their non-conforming casing is intentional
// and silenced here so they stay greppable against the C source.
#![allow(non_upper_case_globals)]

/// A discrete press or release.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ButtonState {
    /// Released.
    #[default]
    Released,
    /// Pressed.
    Pressed,
}

impl ButtonState {
    pub fn is_pressed(self) -> bool {
        matches!(self, ButtonState::Pressed)
    }

    /// Build from a Wayland `wl_pointer.button_state` value: 0 = released,
    /// 1 = pressed. Anything else maps to released.
    pub fn from_wayland(value: u32) -> ButtonState {
        if value == 1 {
            ButtonState::Pressed
        } else {
            ButtonState::Released
        }
    }
}

/// One raw input event from a backend's input stream.
///
/// Coordinates are in compositor logical space (the same space the renderer
/// uses). Pointer-button and key codes follow Linux input-event codes so the
/// server can hand them to `wl_pointer.button` and `wl_keyboard.key` directly.
#[derive(Debug, Clone, Copy)]
pub enum InputEvent {
    /// Pointer moved to `(x, y)` in logical pixels.
    PointerMotion { x: f32, y: f32 },
    /// Pointer button state changed. `button` is a Linux `BTN_*` code.
    PointerButton { button: u32, state: ButtonState },
    /// Smooth scroll. Discrete wheel clicks arrive as multiples of 10.0
    /// (matching libinput's default), per the wl_pointer axis convention.
    PointerAxis { dx: f32, dy: f32 },
    /// Pointer left the surface area.
    PointerLeave,
    /// Keyboard state changed. `code` is a Linux evdev scancode, suitable for
    /// forwarding directly to `wl_keyboard.key`.
    Key { code: u32, state: ButtonState },
}

// XKB keysym values for the few control keys the compositor chrome cares
// about. These are stable, public constants from X11/keysymdef.h; defining
// them here keeps `ass-core` free of an `xkbcommon` dependency while letting
// the launcher interpret keysym output it receives from the server. The names
// intentionally match the C macros verbatim (greppable against keysymdef.h),
// so they do not follow Rust's UPPER_CASE globals convention; the file-level
// allow below silences the lint for them and their use in `match` patterns.
/// XKB `Escape`.
pub const XKB_KEY_Escape: u32 = 0xff1b;
/// XKB `Return` (Enter).
pub const XKB_KEY_Return: u32 = 0xff0d;
/// XKB `BackSpace`.
pub const XKB_KEY_BackSpace: u32 = 0xff08;
/// XKB `Tab`.
pub const XKB_KEY_Tab: u32 = 0xff09;
/// XKB up arrow.
pub const XKB_KEY_Up: u32 = 0xff52;
/// XKB down arrow.
pub const XKB_KEY_Down: u32 = 0xff54;
/// XKB `NoSymbol` — no keysym resolved for the key.
pub const XKB_KEY_NoSymbol: u32 = 0;

/// XKB modifier state, as a bitmask over the standard xkbcommon mod indices
/// for the default `evdev/pc104/us` keymap the server compiles. The server
/// fills this from `KeyOutcome.depressed`; the keybind matcher compares
/// against these bits. Indices: Shift=0, Control=2, Mod1(Alt)=3, Mod4(Super)=6.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Mods(pub u32);

impl Mods {
    pub const NONE: Mods = Mods(0);
    pub const SHIFT: Mods = Mods(1 << 0);
    pub const CTRL: Mods = Mods(1 << 2);
    pub const ALT: Mods = Mods(1 << 3);
    pub const SUPER: Mods = Mods(1 << 6);

    /// Whether all bits in `required` are set.
    pub fn has(self, required: Mods) -> bool {
        (self.0 & required.0) == required.0
    }
}

impl std::ops::BitOr for Mods {
    type Output = Mods;
    fn bitor(self, rhs: Mods) -> Mods {
        Mods(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for Mods {
    fn bitor_assign(&mut self, rhs: Mods) {
        self.0 |= rhs.0;
    }
}

/// The character and keysym a key event produced, as extracted by the
/// server's xkbcommon state. Forwarded to chrome for text-style input (the
/// launcher's search box). `ch` is `None` for control keys (Esc, arrows,
/// plain modifiers) that produce no printable character. `mods` is the
/// xkbcommon depressed-modifier mask active when the key was pressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyChar {
    /// XKB keysym (`XKB_KEY_*`). `XKB_KEY_NoSymbol` when xkbcommon resolved
    /// none for the key.
    pub keysym: u32,
    /// Printable character the key produced under the current layout and
    /// modifiers, if any.
    pub ch: Option<char>,
    /// Active modifier mask at press time, for global key-bindings.
    pub mods: Mods,
}

/// A chrome-facing classification of a key event. Built from a [`KeyChar`]
/// with [`key_action`]; the chrome consumes these without ever touching
/// xkbcommon or evdev scancodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyAction {
    /// A printable character was typed.
    Char(char),
    /// `BackSpace`.
    Backspace,
    /// `Return` / Enter.
    Enter,
    /// `Escape`.
    Escape,
    /// `Up` arrow.
    Up,
    /// `Down` arrow.
    Down,
    /// `Tab`.
    Tab,
    /// A key the chrome does not act on (modifier, function key, dead key,
    /// or a control character outside the chrome's interest).
    Ignore,
}

/// Classify a resolved key event into a [`KeyAction`].
///
/// Control keys are matched by keysym first; otherwise a printable `ch`
/// becomes [`KeyAction::Char`]. Control characters (code point below U+0020)
/// and `DEL` (U+007F) are dropped to [`KeyAction::Ignore`] so the launcher
/// never inserts them into a search string.
pub fn key_action(keysym: u32, ch: Option<char>) -> KeyAction {
    match keysym {
        XKB_KEY_Escape => KeyAction::Escape,
        XKB_KEY_Return => KeyAction::Enter,
        XKB_KEY_BackSpace => KeyAction::Backspace,
        XKB_KEY_Tab => KeyAction::Tab,
        XKB_KEY_Up => KeyAction::Up,
        XKB_KEY_Down => KeyAction::Down,
        _ => match ch {
            Some(c) if (c as u32) >= 0x20 && (c as u32) != 0x7f => KeyAction::Char(c),
            _ => KeyAction::Ignore,
        },
    }
}

// Linux input-event codes (`KEY_*` from input-event-codes.h) for the modifier
// keys and a few common triggers. Defined here so the compositor can track
// modifier state and detect taps without pulling in a Linux input constants
// crate.
pub const KEY_LEFTCTRL: u32 = 29;
pub const KEY_LEFTSHIFT: u32 = 42;
pub const KEY_RIGHTSHIFT: u32 = 54;
pub const KEY_LEFTALT: u32 = 56;
pub const KEY_LEFTMETA: u32 = 125;
pub const KEY_RIGHTCTRL: u32 = 97;
pub const KEY_RIGHTALT: u32 = 100;
pub const KEY_RIGHTMETA: u32 = 126;

/// Detects a "tap" of one or more modifier keys: the target was pressed and
/// released with no other key pressed in between.
///
/// Used by the main loop to recognize a bare Super tap as the global
/// launcher hotkey. Feeding the modifier keys themselves to the client is
/// unaffected — a tap is observed, not intercepted — so Super still works as a
/// modifier in every other combo (Super+letter, Super+drag, …) and never
/// reaches [`TapDetector::on_key`] as a fire. The detector is a pure state
/// machine over `(code, pressed)` pairs and has no I/O.
///
/// Multiple target codes (e.g. left and right Super) are treated as one
/// logical key: a tap fires when the depth of held targets returns to zero
/// without any non-target key having been pressed.
#[derive(Debug, Clone)]
pub struct TapDetector {
    targets: Vec<u32>,
    depth: u32,
    clean: bool,
}

impl TapDetector {
    /// Construct a detector for the given modifier scancodes. Panics if empty
    /// (a detector with no target cannot fire).
    pub fn new(targets: &[u32]) -> Self {
        assert!(!targets.is_empty(), "TapDetector needs at least one target");
        TapDetector {
            targets: targets.to_vec(),
            depth: 0,
            clean: false,
        }
    }

    /// Convenience: a tap detector for either Super key (left or right Meta).
    pub fn super_tap() -> Self {
        Self::new(&[KEY_LEFTMETA, KEY_RIGHTMETA])
    }

    /// Feed one key event. Returns `true` when the target modifier was tapped
    /// (pressed and released with no other key pressed while held).
    ///
    /// Order matters: a non-target key pressed while the target is held marks
    /// the hold "dirty" and suppresses the tap on release. A non-target
    /// *release* does not (only a press consumes the modifier).
    pub fn on_key(&mut self, code: u32, pressed: bool) -> bool {
        let is_target = self.targets.contains(&code);
        if is_target {
            if pressed {
                if self.depth == 0 {
                    self.clean = true;
                }
                self.depth = self.depth.saturating_add(1);
            } else if self.depth > 0 {
                self.depth -= 1;
                if self.depth == 0 && self.clean {
                    self.clean = false;
                    return true;
                }
            }
        } else if pressed {
            // Any non-target press while the target is held means the target
            // is being used as a modifier, not tapped.
            if self.depth > 0 {
                self.clean = false;
            }
        }
        false
    }

    /// Reset internal state (e.g. on focus change).
    pub fn reset(&mut self) {
        self.depth = 0;
        self.clean = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wayland_button_state_maps_one_to_pressed_else_released() {
        assert_eq!(ButtonState::from_wayland(1), ButtonState::Pressed);
        assert_eq!(ButtonState::from_wayland(0), ButtonState::Released);
        // Defensive: garbage values collapse to released rather than panic.
        assert_eq!(ButtonState::from_wayland(42), ButtonState::Released);
    }

    #[test]
    fn default_is_released() {
        assert_eq!(ButtonState::default(), ButtonState::Released);
        assert!(!ButtonState::default().is_pressed());
        assert!(ButtonState::Pressed.is_pressed());
    }

    #[test]
    fn key_action_classifies_control_keys() {
        use super::*;
        assert_eq!(key_action(XKB_KEY_Escape, None), KeyAction::Escape);
        assert_eq!(key_action(XKB_KEY_Return, None), KeyAction::Enter);
        assert_eq!(key_action(XKB_KEY_BackSpace, None), KeyAction::Backspace);
        assert_eq!(key_action(XKB_KEY_Up, None), KeyAction::Up);
        assert_eq!(key_action(XKB_KEY_Down, None), KeyAction::Down);
        assert_eq!(key_action(XKB_KEY_Tab, None), KeyAction::Tab);
    }

    #[test]
    fn key_action_passes_through_printable_chars() {
        use super::*;
        assert_eq!(
            key_action(XKB_KEY_NoSymbol, Some('a')),
            KeyAction::Char('a')
        );
        assert_eq!(
            key_action(XKB_KEY_NoSymbol, Some(' ')),
            KeyAction::Char(' ')
        );
        assert_eq!(
            key_action(XKB_KEY_NoSymbol, Some('Z')),
            KeyAction::Char('Z')
        );
    }

    #[test]
    fn key_action_drops_control_characters() {
        use super::*;
        // A keysym of 0 with a control char below U+0020 must not become a
        // search character.
        assert_eq!(
            key_action(XKB_KEY_NoSymbol, Some('\u{1}')),
            KeyAction::Ignore
        );
        assert_eq!(
            key_action(XKB_KEY_NoSymbol, Some('\u{7f}')),
            KeyAction::Ignore
        );
        // Unknown keysym with no char is ignored.
        assert_eq!(key_action(0x1234, None), KeyAction::Ignore);
    }

    #[test]
    fn tap_detector_fires_on_clean_modifier_tap() {
        let mut d = super::TapDetector::new(&[super::KEY_LEFTMETA]);
        assert!(!d.on_key(super::KEY_LEFTMETA, true)); // press
        assert!(d.on_key(super::KEY_LEFTMETA, false)); // release → tap
    }

    #[test]
    fn tap_detector_ignores_modifier_held_as_modifier() {
        let mut d = super::TapDetector::new(&[super::KEY_LEFTMETA]);
        d.on_key(super::KEY_LEFTMETA, true); // super down
        d.on_key(30, true); // 'a' down → super used as mod
        d.on_key(30, false); // 'a' up
        assert!(!d.on_key(super::KEY_LEFTMETA, false)); // super up → no tap
    }

    #[test]
    fn tap_detector_release_without_press_does_not_fire() {
        let mut d = super::TapDetector::new(&[super::KEY_LEFTMETA]);
        // Spurious release (e.g. missed press event).
        assert!(!d.on_key(super::KEY_LEFTMETA, false));
    }

    #[test]
    fn tap_detector_resets_between_taps() {
        let mut d = super::TapDetector::super_tap();
        // First tap fires on release.
        assert!(!d.on_key(super::KEY_LEFTMETA, true));
        assert!(d.on_key(super::KEY_LEFTMETA, false));
        // Second tap also fires (state reset after the first).
        assert!(!d.on_key(super::KEY_LEFTMETA, true));
        assert!(d.on_key(super::KEY_LEFTMETA, false));
    }

    #[test]
    fn tap_detector_treats_left_and_right_super_as_one() {
        let mut d = super::TapDetector::super_tap();
        // Right-super tap fires.
        assert!(!d.on_key(super::KEY_RIGHTMETA, true));
        assert!(d.on_key(super::KEY_RIGHTMETA, false));
        // Left-super tap also fires.
        assert!(!d.on_key(super::KEY_LEFTMETA, true));
        assert!(d.on_key(super::KEY_LEFTMETA, false));
    }

    #[test]
    fn tap_detector_non_target_release_keeps_clean() {
        // A key released while the target is held must NOT clear "clean"
        // (only a press consumes the modifier).
        let mut d = super::TapDetector::new(&[super::KEY_LEFTMETA]);
        d.on_key(super::KEY_LEFTMETA, true);
        d.on_key(30, false); // release of an unpressed key — no-op, clean stays
        assert!(d.on_key(super::KEY_LEFTMETA, false));
    }
}
