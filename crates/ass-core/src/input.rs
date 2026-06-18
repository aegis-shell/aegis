//! Backend-agnostic input event types.
//!
//! Backends (nested-host, libinput, DRM/KMS) emit these; the main loop drains
//! and routes them — to the focused client via `wl_seat`, to the chrome via
//! `flux_ui::Input`, or both. Keeping the types in `ass-core` (rather than in
//! `ass-backend`) means the server and shell never need to depend on a backend
//! crate to consume input.

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
}
