//! Backend-agnostic input event types.
//!
//! Backends (nested-host, libinput, DRM/KMS) emit these; the main loop drains
//! and routes them — to the focused client via `wl_seat`, to the chrome via
//! `lens::Input`, or both. Keeping the types in `aegis-model` (rather than in
//! `aegis-backend`) means the server and shell never need to depend on a backend
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
    /// Pointer moved to `(x, y)` in logical pixels. `dx`/`dy` are the
    /// accelerated deltas of the motion and `dx_unaccel`/`dy_unaccel` the raw
    /// device deltas. Relative-pointer clients consume the deltas directly:
    /// unlike `x`/`y` they never clamp at output edges, so a locked pointer
    /// keeps reporting motion. Sources that only know absolute positions
    /// difference successive positions or report zero deltas.
    PointerMotion {
        x: f32,
        y: f32,
        dx: f64,
        dy: f64,
        dx_unaccel: f64,
        dy_unaccel: f64,
    },
    /// Pointer button state changed. `button` is a Linux `BTN_*` code.
    PointerButton { button: u32, state: ButtonState },
    /// One logically atomic pointer-axis frame. Source, high-resolution wheel
    /// steps, and real sequence termination stay attached to the continuous
    /// values so Wayland clients can distinguish wheels from touchpads.
    PointerAxis(PointerAxisFrame),
    /// Pointer left the surface area.
    PointerLeave,
    /// Touch contact `id` (0..max-1) went down at `(x, y)` in logical pixels.
    TouchDown { id: i32, x: f32, y: f32 },
    /// Touch contact `id` moved to `(x, y)`.
    TouchMotion { id: i32, x: f32, y: f32 },
    /// Touch contact `id` lifted.
    TouchUp { id: i32 },
    /// End of a batch of touch events for this frame (groups down/motion/up).
    TouchFrame,
    /// All active touch contacts cancelled (e.g. the seat lost the touch
    /// device). Clients should drop all ongoing touches.
    TouchCancel,
    /// Keyboard state changed. `code` is a Linux evdev scancode, suitable for
    /// forwarding directly to `wl_keyboard.key`.
    Key { code: u32, state: ButtonState },
    /// Graphics-tablet tool event. Kept distinct from the mouse pointer so
    /// tablet-aware clients receive pressure/tilt and independent proximity.
    Tablet { event: TabletEvent },
}

impl InputEvent {
    /// An absolute pointer move without device deltas, for sources that only
    /// know positions: synthesized automation input and motions re-injected
    /// after compositor chrome releases the pointer. Relative-pointer clients
    /// receive no delta from these events.
    pub fn pointer_move_to(x: f32, y: f32) -> InputEvent {
        InputEvent::PointerMotion {
            x,
            y,
            dx: 0.0,
            dy: 0.0,
            dx_unaccel: 0.0,
            dy_unaccel: 0.0,
        }
    }
}

/// Physical source of a pointer-axis frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerAxisSource {
    Wheel,
    Finger,
    Continuous,
    WheelTilt,
}

/// Physical direction relative to the reported axis direction.
///
/// `Inverted` is used by natural scrolling: the finger or wheel moved in the
/// opposite direction from the resulting `wl_pointer.axis` value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerAxisRelativeDirection {
    Identical,
    Inverted,
}

/// Data for one axis within a [`PointerAxisFrame`].
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PointerAxis {
    /// Continuous distance in surface-local logical units. `None` means this
    /// frame has no motion value for the axis.
    pub value: Option<f32>,
    /// Legacy whole wheel steps for Wayland pointer versions 5–7.
    pub discrete: Option<i32>,
    /// High-resolution wheel distance in 120ths of a logical detent.
    pub value120: Option<i32>,
    /// The continuous finger/device sequence ended on this axis.
    pub stop: bool,
    /// Physical motion direction relative to `value`, when known.
    pub relative_direction: Option<PointerAxisRelativeDirection>,
}

impl PointerAxis {
    pub fn has_data(self) -> bool {
        self.value.is_some() || self.discrete.is_some() || self.value120.is_some() || self.stop
    }

    /// Normalize wheel metadata to logical detents for UI toolkits whose
    /// wheel API is step-based.
    pub fn wheel_steps(self) -> f32 {
        self.value120
            .map(|value| value as f32 / 120.0)
            .or_else(|| self.discrete.map(|value| value as f32))
            .or_else(|| self.value.map(|value| value / 10.0))
            .unwrap_or(0.0)
    }
}

/// A group of horizontal and vertical scroll events that belong together.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PointerAxisFrame {
    /// Backend event time in milliseconds.
    pub time: u32,
    /// Physical source, or `None` when an older nested host cannot report it.
    pub source: Option<PointerAxisSource>,
    pub horizontal: PointerAxis,
    pub vertical: PointerAxis,
}

impl PointerAxisFrame {
    /// Build a continuous-value frame without inventing wheel-step metadata.
    pub fn from_values(time: u32, source: Option<PointerAxisSource>, dx: f32, dy: f32) -> Self {
        Self {
            time,
            source,
            horizontal: PointerAxis {
                value: (dx != 0.0).then_some(dx),
                ..PointerAxis::default()
            },
            vertical: PointerAxis {
                value: (dy != 0.0).then_some(dy),
                ..PointerAxis::default()
            },
        }
    }

    pub fn dx(self) -> f32 {
        self.horizontal.value.unwrap_or(0.0)
    }

    pub fn dy(self) -> f32 {
        self.vertical.value.unwrap_or(0.0)
    }

    pub fn has_data(self) -> bool {
        self.horizontal.has_data() || self.vertical.has_data()
    }
}

/// Scroll gesture selected for a touchpad.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "kebab-case"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TouchpadScrollMethod {
    /// Scroll by moving two fingers over the pad.
    #[default]
    TwoFinger,
    /// Scroll by moving one finger along a pad edge.
    Edge,
}

/// User-selected touchpad policy.
///
/// The compositor keeps this backend-agnostic so configuration, settings UI,
/// and libinput all exchange the same value. Unsupported fields are retained
/// as a device profile and applied when a capable touchpad becomes available.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default, deny_unknown_fields))]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TouchpadConfig {
    /// Make content follow the fingers instead of the scrollbar thumb.
    pub natural_scroll: bool,
    /// A light finger tap produces a primary-button click.
    pub tap_to_click: bool,
    /// Holding after a tap starts a drag.
    pub tap_and_drag: bool,
    /// Keep a tap-drag active briefly after the finger lifts.
    pub drag_lock: bool,
    /// Suppress accidental pointer input while typing.
    pub disable_while_typing: bool,
    /// libinput pointer acceleration in the inclusive range `-1.0..=1.0`.
    pub pointer_speed: f32,
    /// Multiplier applied to touchpad scroll motion, `1.0` leaving device
    /// motion untouched. Applied by the compositor; libinput has no
    /// equivalent device setting.
    pub scroll_speed: f32,
    /// Gesture used to produce scroll events.
    pub scroll_method: TouchpadScrollMethod,
}

impl Default for TouchpadConfig {
    fn default() -> Self {
        Self {
            natural_scroll: true,
            tap_to_click: true,
            tap_and_drag: true,
            drag_lock: false,
            disable_while_typing: true,
            pointer_speed: 0.0,
            scroll_speed: 1.0,
            scroll_method: TouchpadScrollMethod::TwoFinger,
        }
    }
}

/// Multiplier range accepted for scroll-speed settings, shared by the mouse
/// and touchpad profiles so validation and the settings UI stay in sync.
pub const SCROLL_SPEED_RANGE: std::ops::RangeInclusive<f32> = 0.1..=10.0;

/// User-selected mouse policy.
///
/// Like [`TouchpadConfig`], this stays backend-agnostic: configuration, the
/// settings UI, and libinput exchange the same value. `pointer_speed` and
/// `natural_scroll` map onto libinput device settings; `scroll_speed` has no
/// libinput counterpart and is applied by the compositor when it translates
/// wheel motion into `wl_pointer` axis frames.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default, deny_unknown_fields))]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MouseConfig {
    /// Make content follow the wheel instead of the scrollbar thumb.
    pub natural_scroll: bool,
    /// libinput pointer acceleration in the inclusive range `-1.0..=1.0`.
    pub pointer_speed: f32,
    /// Wheel scroll multiplier, `1.0` leaving device motion untouched.
    pub scroll_speed: f32,
}

impl Default for MouseConfig {
    fn default() -> Self {
        Self {
            natural_scroll: false,
            pointer_speed: 0.0,
            scroll_speed: 1.0,
        }
    }
}

/// Longest accepted keyboard repeat delay, in milliseconds.
pub const MAX_REPEAT_DELAY_MS: u32 = 2_000;
/// Fastest accepted keyboard repeat rate, in repeats per second.
pub const MAX_REPEAT_RATE: u32 = 150;

/// User-selected keyboard policy.
///
/// The compositor does not repeat keys itself; it advertises these values as
/// `wl_keyboard.repeat_info` so clients repeat locally (ADR-0010). A rate of
/// zero disables repetition.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default, deny_unknown_fields))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyboardConfig {
    /// Repeats per second, `0` to disable repetition.
    pub repeat_rate: u32,
    /// Milliseconds a key must be held before repeating starts.
    pub repeat_delay_ms: u32,
}

impl Default for KeyboardConfig {
    fn default() -> Self {
        // Weston and Mutter's out-of-the-box defaults.
        Self {
            repeat_rate: 25,
            repeat_delay_ms: 250,
        }
    }
}

/// Complete `[input]` policy for the seat: keyboard, mouse, and touchpad.
///
/// The settings UI edits and persists this as one domain ("Input"), so a
/// single transaction always carries a coherent profile for every device
/// class.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default, deny_unknown_fields))]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct InputConfig {
    /// Written as `[input.touchpad]`.
    pub touchpad: TouchpadConfig,
    /// Written as `[input.mouse]`.
    pub mouse: MouseConfig,
    /// Written as `[input.keyboard]`.
    pub keyboard: KeyboardConfig,
}

/// Features supported by the currently attached touchpad set.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TouchpadCapabilities {
    pub natural_scroll: bool,
    pub tap_to_click: bool,
    pub tap_and_drag: bool,
    pub drag_lock: bool,
    pub disable_while_typing: bool,
    pub pointer_speed: bool,
    pub two_finger_scroll: bool,
    pub edge_scroll: bool,
}

/// Live touchpad state exposed to shell settings.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TouchpadStatus {
    /// Whether this compositor directly owns the physical input devices.
    pub configurable: bool,
    pub device_names: Vec<String>,
    pub capabilities: TouchpadCapabilities,
    pub config: TouchpadConfig,
}

impl TouchpadStatus {
    pub fn device_count(&self) -> usize {
        self.device_names.len()
    }
}

/// Features supported by the currently attached mouse set.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MouseCapabilities {
    pub natural_scroll: bool,
    pub pointer_speed: bool,
}

/// Live mouse state exposed to shell settings, mirroring
/// [`TouchpadStatus`].
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MouseStatus {
    /// Whether this compositor directly owns the physical input devices.
    pub configurable: bool,
    pub device_names: Vec<String>,
    pub capabilities: MouseCapabilities,
    pub config: MouseConfig,
}

impl MouseStatus {
    pub fn device_count(&self) -> usize {
        self.device_names.len()
    }
}

/// Live input state for the whole seat, exposed to shell settings: the
/// touchpad and mouse device sets plus the keyboard profile.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Default)]
pub struct InputStatus {
    /// Whether this compositor directly owns the physical input devices.
    pub configurable: bool,
    pub touchpad: TouchpadStatus,
    pub mouse: MouseStatus,
    pub keyboard: KeyboardConfig,
}

#[derive(Debug, Clone, Copy)]
pub struct TabletToolInfo {
    pub serial: u64,
    pub hardware_id: u64,
    /// `zwp_tablet_tool_v2.type` wire value (0x140..0x147).
    pub kind: u32,
    /// Bit N means protocol capability N is present (tilt=1..wheel=6).
    pub capabilities: u32,
}

#[derive(Debug, Clone, Copy)]
pub enum TabletEvent {
    Proximity {
        tool: u64,
        info: TabletToolInfo,
        in_proximity: bool,
        x: f32,
        y: f32,
        time: u32,
    },
    Axes {
        tool: u64,
        x: f32,
        y: f32,
        pressure: Option<f32>,
        distance: Option<f32>,
        tilt: Option<(f32, f32)>,
        rotation: Option<f32>,
        slider: Option<f32>,
        wheel: Option<(f32, i32)>,
        time: u32,
    },
    Tip {
        tool: u64,
        state: ButtonState,
        time: u32,
    },
    Button {
        tool: u64,
        button: u32,
        state: ButtonState,
        time: u32,
    },
}

/// One self-contained input action requested by a trusted automation client.
///
/// Unlike [`InputEvent`], these actions cannot leave a button or key held
/// across IPC requests: clicks and key presses always synthesize their paired
/// release. Pointer coordinates are local to the target toplevel, which lets
/// the compositor validate the complete action before converting it to global
/// logical coordinates.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", serde(tag = "type"))]
pub enum SyntheticInputAction {
    /// Move the logical pointer within the target toplevel.
    PointerMove { position: crate::Point },
    /// Move to `position`, then press and release one Linux `BTN_*` code.
    Click { position: crate::Point, button: u32 },
    /// Move to `position`, then deliver a smooth scroll delta.
    Scroll {
        position: crate::Point,
        dx: f32,
        dy: f32,
    },
    /// Press and release one Linux evdev key code against the target toplevel.
    KeyPress { code: u32 },
}

impl SyntheticInputAction {
    /// Target-local pointer position used by this action, if it has one.
    pub fn pointer_position(self) -> Option<crate::Point> {
        match self {
            Self::PointerMove { position }
            | Self::Click { position, .. }
            | Self::Scroll { position, .. } => Some(position),
            Self::KeyPress { .. } => None,
        }
    }
}

/// Complete text-input state committed by an inner Wayland client. A nested
/// backend forwards this to the host compositor's `zwp_text_input_v3` object
/// so the host IME sees the same editor context as the focused inner client.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TextInputState {
    pub enabled: bool,
    pub surrounding_text: Option<String>,
    pub cursor: i32,
    pub anchor: i32,
    pub change_cause: u32,
    pub content_hint: u32,
    pub content_purpose: u32,
    pub cursor_rect: Option<(i32, i32, i32, i32)>,
}

/// Text produced by the host compositor's input method and routed back to the
/// enabled inner `zwp_text_input_v3` object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextInputEvent {
    Preedit {
        text: Option<String>,
        cursor_begin: i32,
        cursor_end: i32,
    },
    Commit(Option<String>),
    DeleteSurrounding {
        before_length: u32,
        after_length: u32,
    },
    Done,
}

/// High-level touchpad gestures forwarded from the nested host compositor.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PointerGestureEvent {
    SwipeBegin {
        time: u32,
        fingers: u32,
    },
    SwipeUpdate {
        time: u32,
        dx: f32,
        dy: f32,
    },
    SwipeEnd {
        time: u32,
        cancelled: bool,
    },
    PinchBegin {
        time: u32,
        fingers: u32,
    },
    PinchUpdate {
        time: u32,
        dx: f32,
        dy: f32,
        scale: f32,
        rotation: f32,
    },
    PinchEnd {
        time: u32,
        cancelled: bool,
    },
    HoldBegin {
        time: u32,
        fingers: u32,
    },
    HoldEnd {
        time: u32,
        cancelled: bool,
    },
}

// XKB keysym values for the few control keys the compositor chrome cares
// about. These are stable, public constants from X11/keysymdef.h; defining
// them here keeps `aegis-model` free of an `xkbcommon` dependency while letting
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
/// XKB `ISO_Left_Tab`, commonly resolved for Shift+Tab.
pub const XKB_KEY_ISO_Left_Tab: u32 = 0xfe20;
/// XKB up arrow.
pub const XKB_KEY_Up: u32 = 0xff52;
/// XKB left arrow.
pub const XKB_KEY_Left: u32 = 0xff51;
/// XKB right arrow.
pub const XKB_KEY_Right: u32 = 0xff53;
/// XKB down arrow.
pub const XKB_KEY_Down: u32 = 0xff54;
/// XKB `Print` / Print Screen.
pub const XKB_KEY_Print: u32 = 0xff61;
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

/// Destination that owns one physical key sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyRoute {
    /// Deliver the event through the focused Wayland seat.
    Client,
    /// Resolve the event for compositor chrome and withhold it from clients.
    Chrome,
}

/// Keeps a key press and its matching release on the same routing path.
///
/// Opening or closing compositor chrome may change where *new* presses go,
/// but it must not split an already-started sequence. The state only needs to
/// remember chrome-owned presses: every other key belongs to the client path.
#[derive(Debug, Clone, Default)]
pub struct KeyboardCaptureState {
    chrome_owned: std::collections::HashSet<u32>,
}

impl KeyboardCaptureState {
    /// Select the destination for one physical key event.
    ///
    /// `chrome_captures_new_presses` applies only when a sequence starts.
    /// Repeated presses and the final release retain the original owner.
    pub fn route(
        &mut self,
        code: u32,
        state: ButtonState,
        chrome_captures_new_presses: bool,
    ) -> KeyRoute {
        if state.is_pressed() {
            if self.chrome_owned.contains(&code) || chrome_captures_new_presses {
                self.chrome_owned.insert(code);
                KeyRoute::Chrome
            } else {
                KeyRoute::Client
            }
        } else if self.chrome_owned.remove(&code) {
            KeyRoute::Chrome
        } else {
            KeyRoute::Client
        }
    }
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
    /// `Left` arrow.
    Left,
    /// `Right` arrow.
    Right,
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
        XKB_KEY_Left => KeyAction::Left,
        XKB_KEY_Right => KeyAction::Right,
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
/// released while no other key was held or pressed.
///
/// The detector is a pure state machine over `(code, pressed)` pairs and has
/// no I/O. A caller can observe a modifier tap without intercepting the
/// modifier's ordinary key events. Aegis's built-in shortcuts do not assign
/// an action to a bare modifier.
///
/// Multiple target codes (e.g. left and right Super) are treated as one
/// logical key: a tap fires when the held-target set becomes empty without
/// any non-target key participating in the gesture.
#[derive(Debug, Clone)]
pub struct TapDetector {
    targets: Vec<u32>,
    held_targets: std::collections::HashSet<u32>,
    held_non_targets: std::collections::HashSet<u32>,
    clean: bool,
}

impl TapDetector {
    /// Construct a detector for the given modifier scancodes. Panics if empty
    /// (a detector with no target cannot fire).
    pub fn new(targets: &[u32]) -> Self {
        assert!(!targets.is_empty(), "TapDetector needs at least one target");
        TapDetector {
            targets: targets.to_vec(),
            held_targets: std::collections::HashSet::new(),
            held_non_targets: std::collections::HashSet::new(),
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
    /// A non-target already held when the target goes down, or pressed while
    /// the target is held, marks the gesture "dirty" and suppresses the tap.
    /// Tracking held sets also makes duplicate press events idempotent.
    pub fn on_key(&mut self, code: u32, pressed: bool) -> bool {
        let is_target = self.targets.contains(&code);
        if is_target {
            if pressed {
                if self.held_targets.insert(code) && self.held_targets.len() == 1 {
                    self.clean = self.held_non_targets.is_empty();
                }
            } else if self.held_targets.remove(&code) && self.held_targets.is_empty() {
                if self.clean {
                    self.clean = false;
                    return true;
                }
                self.clean = false;
            }
        } else if pressed {
            self.held_non_targets.insert(code);
            // Any non-target press while a target is held means the target is
            // being used as a modifier, not tapped.
            if !self.held_targets.is_empty() {
                self.clean = false;
            }
        } else {
            self.held_non_targets.remove(&code);
        }
        false
    }

    /// Mark the currently-held target as having been used for a non-keyboard
    /// gesture (for example, Super+pointer drag). The held-key depth remains
    /// intact so its eventual release is consumed normally, but it will not
    /// be reported as a clean tap.
    pub fn cancel_current(&mut self) {
        if !self.held_targets.is_empty() {
            self.clean = false;
        }
    }

    /// Reset internal state (e.g. on focus change).
    pub fn reset(&mut self) {
        self.held_targets.clear();
        self.held_non_targets.clear();
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
    fn touchpad_defaults_to_natural_scrolling() {
        assert!(TouchpadConfig::default().natural_scroll);
    }

    #[test]
    fn input_defaults_match_documented_values() {
        let input = InputConfig::default();
        assert_eq!(input.touchpad, TouchpadConfig::default());
        assert_eq!(input.mouse, MouseConfig::default());
        assert!(!input.mouse.natural_scroll);
        assert_eq!(input.mouse.scroll_speed, 1.0);
        assert_eq!(input.keyboard, KeyboardConfig::default());
        // Weston/Mutter defaults (ADR-0010).
        assert_eq!(input.keyboard.repeat_rate, 25);
        assert_eq!(input.keyboard.repeat_delay_ms, 250);
        assert_eq!(input.touchpad.scroll_speed, 1.0);
    }

    #[test]
    fn key_action_classifies_control_keys() {
        use super::*;
        assert_eq!(key_action(XKB_KEY_Escape, None), KeyAction::Escape);
        assert_eq!(key_action(XKB_KEY_Return, None), KeyAction::Enter);
        assert_eq!(key_action(XKB_KEY_BackSpace, None), KeyAction::Backspace);
        assert_eq!(key_action(XKB_KEY_Up, None), KeyAction::Up);
        assert_eq!(key_action(XKB_KEY_Left, None), KeyAction::Left);
        assert_eq!(key_action(XKB_KEY_Right, None), KeyAction::Right);
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
    fn chrome_owned_press_keeps_its_release_after_capture_closes() {
        let mut capture = KeyboardCaptureState::default();
        assert_eq!(
            capture.route(30, ButtonState::Pressed, true),
            KeyRoute::Chrome
        );
        assert_eq!(
            capture.route(30, ButtonState::Released, false),
            KeyRoute::Chrome
        );
    }

    #[test]
    fn client_owned_press_keeps_its_release_after_capture_opens() {
        let mut capture = KeyboardCaptureState::default();
        assert_eq!(
            capture.route(30, ButtonState::Pressed, false),
            KeyRoute::Client
        );
        assert_eq!(
            capture.route(30, ButtonState::Released, true),
            KeyRoute::Client
        );
    }

    #[test]
    fn repeated_press_keeps_the_sequence_owner() {
        let mut capture = KeyboardCaptureState::default();
        assert_eq!(
            capture.route(30, ButtonState::Pressed, true),
            KeyRoute::Chrome
        );
        assert_eq!(
            capture.route(30, ButtonState::Pressed, false),
            KeyRoute::Chrome
        );
        assert_eq!(
            capture.route(30, ButtonState::Released, false),
            KeyRoute::Chrome
        );
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
    fn tap_detector_ignores_modifier_held_before_target() {
        let mut d = super::TapDetector::new(&[super::KEY_LEFTMETA]);
        d.on_key(super::KEY_LEFTALT, true); // alt down first
        d.on_key(super::KEY_LEFTMETA, true); // super joins an existing chord
        assert!(!d.on_key(super::KEY_LEFTMETA, false));
        d.on_key(super::KEY_LEFTALT, false);
    }

    #[test]
    fn tap_detector_duplicate_target_press_is_idempotent() {
        let mut d = super::TapDetector::new(&[super::KEY_LEFTMETA]);
        assert!(!d.on_key(super::KEY_LEFTMETA, true));
        assert!(!d.on_key(super::KEY_LEFTMETA, true));
        assert!(d.on_key(super::KEY_LEFTMETA, false));
        assert!(!d.on_key(super::KEY_LEFTMETA, false));
    }

    #[test]
    fn tap_detector_reset_drops_held_key_snapshot() {
        let mut d = super::TapDetector::new(&[super::KEY_LEFTMETA]);
        d.on_key(super::KEY_LEFTALT, true);
        d.reset();
        assert!(!d.on_key(super::KEY_LEFTMETA, true));
        assert!(d.on_key(super::KEY_LEFTMETA, false));
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

    #[test]
    fn tap_detector_pointer_gesture_cancels_current_tap() {
        let mut d = super::TapDetector::super_tap();
        assert!(!d.on_key(super::KEY_LEFTMETA, true));
        d.cancel_current();
        assert!(!d.on_key(super::KEY_LEFTMETA, false));
    }
}
