//! Window-management model: per-toplevel metadata and state.
//!
//! A `Window` is the renderer-agnostic, protocol-agnostic view of a mapped
//! xdg toplevel. The server populates it from `xdg_toplevel` requests; the
//! shell and the future AI-adaptation layer read it for chrome and
//! introspection. Keeping this in `aegis-model` (rather than in `aegis-server`)
//! means the shell never needs a server dependency to display window state.

/// Desktop-wide ownership policy for Wayland toplevel decorations.
///
/// `Borderless` keeps window controls in compositor-owned gestures, borders,
/// the Dock, and other shell surfaces. On the wire this is server-side
/// decoration ownership: clients omit their own title bars, while the
/// compositor intentionally draws no per-window frame.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "kebab-case"))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DecorationPolicy {
    /// Compositor-owned controls without a per-window title bar.
    #[default]
    Borderless,
    /// Let each client draw and operate its own decoration frame.
    ClientSide,
}

/// Compositor drop-shadow style for floating windows (ADR-0139): the
/// *policy* is data here; the drawing mechanism lives in the Optics stack
/// (`flux::shadow_filter` renders a blurred mask; `aegis-render` composites
/// the result). Effect-free by design — pure data, no renderer types.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "kebab-case"))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum WindowShadowStyle {
    /// No shadow.
    None,
    /// The historic four-logical-pixel stroke shadow drawn inline by the
    /// renderer (no GPU effect work).
    #[default]
    Resize,
    /// A blurred drop shadow through the Optics `flux_shadow_filter`
    /// operator: rounded-rect mask, Gaussian blur, directional offset,
    /// focus-modulated opacity.
    Soft,
}

/// State bits advertised to the client via `xdg_toplevel.configure`'s states
/// array. Mapped one-to-one to the protocol's state enum values; the
/// compositor OR's the active bits into the array on each configure.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WindowState {
    /// `XDG_TOPLEVEL_STATE_MAXIMIZED` (value 1).
    pub maximized: bool,
    /// `XDG_TOPLEVEL_STATE_FULLSCREEN` (value 2).
    pub fullscreen: bool,
    /// `XDG_TOPLEVEL_STATE_RESIZING` (value 3). Set for the duration of an
    /// interactive edge/corner resize so clients can use cheaper live-resize
    /// rendering and restore normal quality when the grab ends.
    pub resizing: bool,
    /// `XDG_TOPLEVEL_STATE_ACTIVATED` (value 4). The window has keyboard focus.
    pub activated: bool,
}

/// Strongest way a visible window is consuming the current output's usable
/// space.
///
/// Maximized and fullscreen are deliberately separate states: maximized
/// windows still coexist with persistent top chrome and a revealable Dock,
/// while fullscreen windows own the complete output. Fullscreen has
/// precedence when several visible windows report different states.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "kebab-case"))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SpaceUse {
    /// No visible, non-minimized window consumes an output edge.
    #[default]
    Available,
    /// At least one visible, non-minimized window is maximized.
    Maximized,
    /// At least one visible, non-minimized window is fullscreen.
    Fullscreen,
}

impl SpaceUse {
    /// Derive output space use from the visible window snapshot delivered to
    /// shell chrome and IPC consumers.
    pub fn from_windows(windows: &[Window]) -> SpaceUse {
        let mut use_state = SpaceUse::Available;
        for window in windows.iter().filter(|window| !window.minimized) {
            if window.state.fullscreen {
                return SpaceUse::Fullscreen;
            }
            if window.state.maximized {
                use_state = SpaceUse::Maximized;
            }
        }
        use_state
    }
}

impl WindowState {
    /// Serialize the active state bits into a `u32` array matching the
    /// `xdg_toplevel.configure.states` argument layout. The order matches
    /// the protocol's enum order so clients parse it without extra work.
    pub fn to_state_array(self) -> Vec<u32> {
        let mut v = Vec::new();
        if self.maximized {
            v.push(1);
        }
        if self.fullscreen {
            v.push(2);
        }
        if self.resizing {
            v.push(3);
        }
        if self.activated {
            v.push(4);
        }
        v
    }

    /// `true` if no state bits are set. The corresponding `configure` states
    /// array is empty.
    pub fn is_empty(self) -> bool {
        !(self.maximized || self.fullscreen || self.resizing || self.activated)
    }
}

/// Minimum and maximum size hints from `xdg_toplevel.set_min_size` /
/// `set_max_size`. `0` means "no constraint" per the protocol.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SizeHints {
    pub min_w: i32,
    pub min_h: i32,
    pub max_w: i32,
    pub max_h: i32,
}

impl SizeHints {
    pub fn is_unconstrained(self) -> bool {
        self.min_w == 0 && self.min_h == 0 && self.max_w == 0 && self.max_h == 0
    }
}

/// Stable identifier for a window, opaque to chrome/IPC/agent. Allocated
/// monotonically by the compositor and never reused within the process
/// lifetime (ADR-0032). Outlives the surface: a retired id remains valid as
/// a journal or scope reference but is never reassigned.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct WindowId(pub u64);

/// Per-toplevel metadata. The server owns one per mapped `xdg_toplevel`; the
/// shell and introspection APIs read it.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Default)]
pub struct Window {
    /// Durable identifier (ADR-0032); never reused within the process.
    pub id: WindowId,
    pub title: Option<String>,
    pub app_id: Option<String>,
    pub parent: Option<usize>,
    /// Durable identifier of the parent window when this toplevel is a transient dialog.
    #[cfg_attr(feature = "serde", serde(default))]
    pub parent_id: Option<WindowId>,
    /// Whether this window is currently suspended because an active modal descendant is awaiting input.
    #[cfg_attr(feature = "serde", serde(default))]
    pub suspended_by_modal: bool,
    /// Whether this window is currently displaying an attention pulse animation.
    #[cfg_attr(feature = "serde", serde(default))]
    pub attention_pulse: bool,
    pub size_hints: SizeHints,
    pub state: WindowState,
    /// This snapshot is a presentation-only mirror for the physical human
    /// seat. Chrome may display and transfer it, but must not emit focus,
    /// resize, minimize, close, or client input operations. The one permitted
    /// mutation is moving the window (position is presentation state): the
    /// mirror guard's drag starts a compositor-owned move grab, and Agent
    /// input keeps working because its coordinates are target-local.
    #[cfg_attr(feature = "serde", serde(default))]
    pub read_only: bool,
    /// Compositor-internal minimized flag. Unlike the `state` bits this is not
    /// an `xdg_toplevel` configure state (the protocol has no minimized
    /// state); it records that the client requested `set_minimized` and the
    /// compositor hides the surface from rendering and input until the user
    /// restores it by focusing it from the window list or dock.
    pub minimized: bool,
    /// Compositor-internal always-on-top flag. Like `minimized` this is not
    /// an `xdg_toplevel` configure state (the protocol has no always-on-top
    /// state); it records that the user pinned the window above normal
    /// windows through chrome or IPC. The server keeps every always-on-top
    /// toplevel's surface tree in a band at the top of the stacking order.
    pub always_on_top: bool,
    /// Whether the tiling policy (ADR-0024) or the floating policy owns this
    /// window's position and size. `Floating` by default; a tiled window
    /// still carries a position and size — the tiling policy sets them.
    pub layout_role: crate::layout::LayoutRole,
    /// Logical extent of the toplevel in compositor space. Set on first map
    /// (from the committed buffer size) and updated by interactive move and
    /// resize. The renderer reads `position`; the shell reads `position` and
    /// `size` for hit-testing and window controls.
    pub position: crate::Point,
    pub size: crate::Size,
    /// In-flight geometry transition (ADR-0029), recorded when the window
    /// manager changes the rect non-interactively. The model above always
    /// reports the target; the server interpolates this for rendering only.
    pub transition: Option<crate::transition::WindowTransition>,
}

/// Compositor-owned direct-resize area outside a floating window, in logical
/// pixels. Pointer coordinates and window geometry are both expressed in
/// compositor-logical space, so the physical reach scales with the output.
pub const RESIZE_OUTER_MARGIN: f32 = 8.0;

/// Length of each direct-resize corner target along its adjacent edges, in
/// logical pixels. Keeping this independent from [`RESIZE_OUTER_MARGIN`]
/// produces an easy-to-acquire L-shaped corner without consuming client
/// content.
pub const RESIZE_CORNER_REACH: f32 = 24.0;

fn nearest_edge_within(
    point: f32,
    start: f32,
    end: f32,
    reach: f32,
    start_edge: u32,
    end_edge: u32,
) -> u32 {
    let start_distance = point - start;
    let end_distance = end - point;
    let start_is_near = start_distance <= reach;
    let end_is_near = end_distance <= reach;

    match (start_is_near, end_is_near) {
        (true, true) if start_distance <= end_distance => start_edge,
        (true, false) => start_edge,
        (_, true) => end_edge,
        _ => 0,
    }
}

impl Window {
    pub fn new(id: WindowId) -> Window {
        Window {
            id,
            ..Default::default()
        }
    }

    /// Whether a compositor-logical point is inside the window rectangle.
    pub fn contains_point(&self, x: f32, y: f32) -> bool {
        if self.size.w <= 0 || self.size.h <= 0 {
            return false;
        }
        let left = self.position.x as f32;
        let top = self.position.y as f32;
        let right = left + self.size.w as f32;
        let bottom = top + self.size.h as f32;
        x >= left && x < right && y >= top && y < bottom
    }

    /// Resolve a pointer position in the outer resize margin to xdg-shell
    /// edge bits. Corner targets extend along both adjacent edge strips by
    /// [`RESIZE_CORNER_REACH`], rather than requiring the pointer to enter the
    /// tiny diagonal overlap of the strips. The client-content rectangle is
    /// deliberately excluded, so compositor resize never consumes pixels
    /// belonging to an application. Both dimensions are expressed in logical
    /// pixels and therefore follow output scale.
    pub fn resize_edges_at(&self, x: f32, y: f32, margin: f32) -> ResizeEdges {
        if margin <= 0.0 || self.size.w <= 0 || self.size.h <= 0 {
            return ResizeEdges::NONE;
        }
        let left = self.position.x as f32;
        let top = self.position.y as f32;
        let right = left + self.size.w as f32;
        let bottom = top + self.size.h as f32;
        if x < left - margin
            || x >= right + margin
            || y < top - margin
            || y >= bottom + margin
            || self.contains_point(x, y)
        {
            return ResizeEdges::NONE;
        }
        let mut bits = 0;
        if x < left {
            bits |= ResizeEdges::LEFT.0;
        } else if x >= right {
            bits |= ResizeEdges::RIGHT.0;
        }
        if y < top {
            bits |= ResizeEdges::TOP.0;
        } else if y >= bottom {
            bits |= ResizeEdges::BOTTOM.0;
        }

        // Extend each diagonal target along both adjoining strips. If the
        // targets overlap on a small window, the physically nearest edge wins
        // so an axis never contains contradictory bits.
        if bits & (ResizeEdges::LEFT.0 | ResizeEdges::RIGHT.0) == 0 {
            bits |= nearest_edge_within(
                x,
                left,
                right,
                RESIZE_CORNER_REACH,
                ResizeEdges::LEFT.0,
                ResizeEdges::RIGHT.0,
            );
        }
        if bits & (ResizeEdges::TOP.0 | ResizeEdges::BOTTOM.0) == 0 {
            bits |= nearest_edge_within(
                y,
                top,
                bottom,
                RESIZE_CORNER_REACH,
                ResizeEdges::TOP.0,
                ResizeEdges::BOTTOM.0,
            );
        }
        ResizeEdges(bits)
    }

    /// Choose a resize edge or corner for a modifier-drag that may begin
    /// anywhere inside the window. The outer thirds select their adjacent
    /// edges; the center cell falls back to the physically nearest edge so a
    /// valid in-window press never produces [`ResizeEdges::NONE`].
    pub fn resize_edges_nearest(&self, x: f32, y: f32) -> ResizeEdges {
        if self.size.w <= 0 || self.size.h <= 0 {
            return ResizeEdges::NONE;
        }
        let left = self.position.x as f32;
        let top = self.position.y as f32;
        let right = left + self.size.w as f32;
        let bottom = top + self.size.h as f32;
        if x < left || x >= right || y < top || y >= bottom {
            return ResizeEdges::NONE;
        }

        let nx = (x - left) / self.size.w as f32;
        let ny = (y - top) / self.size.h as f32;
        let mut bits = if nx < 1.0 / 3.0 {
            ResizeEdges::LEFT.0
        } else if nx > 2.0 / 3.0 {
            ResizeEdges::RIGHT.0
        } else {
            0
        };
        bits |= if ny < 1.0 / 3.0 {
            ResizeEdges::TOP.0
        } else if ny > 2.0 / 3.0 {
            ResizeEdges::BOTTOM.0
        } else {
            0
        };

        if bits == 0 {
            let distances = [
                (x - left, ResizeEdges::LEFT.0),
                (right - x, ResizeEdges::RIGHT.0),
                (y - top, ResizeEdges::TOP.0),
                (bottom - y, ResizeEdges::BOTTOM.0),
            ];
            bits = distances
                .into_iter()
                .min_by(|a, b| a.0.total_cmp(&b.0))
                .map(|(_, edge)| edge)
                .unwrap_or(ResizeEdges::RIGHT.0);
        }
        ResizeEdges(bits)
    }
}

/// Edge bits from `xdg_toplevel.resize`. Matches the protocol's enum:
/// none=0, top=1, bottom=2, left=4, right=8, plus the four corners as the
/// OR of their axis bits.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ResizeEdges(pub u32);

impl ResizeEdges {
    pub const NONE: ResizeEdges = ResizeEdges(0);
    pub const TOP: ResizeEdges = ResizeEdges(1);
    pub const BOTTOM: ResizeEdges = ResizeEdges(2);
    pub const LEFT: ResizeEdges = ResizeEdges(4);
    pub const RIGHT: ResizeEdges = ResizeEdges(8);

    pub fn has_top(self) -> bool {
        self.0 & Self::TOP.0 != 0
    }
    pub fn has_bottom(self) -> bool {
        self.0 & Self::BOTTOM.0 != 0
    }
    pub fn has_left(self) -> bool {
        self.0 & Self::LEFT.0 != 0
    }
    pub fn has_right(self) -> bool {
        self.0 & Self::RIGHT.0 != 0
    }
    pub fn is_none(self) -> bool {
        self.0 == 0
    }
}

/// An ongoing interactive move or resize. Started by `xdg_toplevel.move` /
/// `xdg_toplevel.resize` when the supplied serial matches the last pointer
/// button press; cleared on button release.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Interactive {
    /// Move the window by the pointer delta from `origin`.
    Move {
        window_id: WindowId,
        /// Pointer position at the moment the move started. Subsequent
        /// motion events compute `position += current - origin`.
        origin: (f32, f32),
        /// The window's position at move start, so the new position is
        /// `start_position + (current - origin)`.
        start_position: crate::Point,
    },
    /// Resize the window. Edges determine which sides move.
    Resize {
        window_id: WindowId,
        edges: ResizeEdges,
        /// Pointer position at resize start.
        origin: (f32, f32),
        /// The window's geometry at resize start, used as the base for
        /// deltas along the moved edges.
        start_position: crate::Point,
        start_size: crate::Size,
    },
}

impl Interactive {
    pub fn window_id(self) -> WindowId {
        match self {
            Interactive::Move { window_id, .. } => window_id,
            Interactive::Resize { window_id, .. } => window_id,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoration_policy_defaults_to_borderless() {
        assert_eq!(DecorationPolicy::default(), DecorationPolicy::Borderless);
    }

    #[test]
    fn empty_state_serializes_to_empty_array() {
        let s = WindowState::default();
        assert!(s.is_empty());
        assert!(s.to_state_array().is_empty());
    }

    #[test]
    fn state_bits_serialize_in_protocol_enum_order() {
        let s = WindowState {
            maximized: true,
            fullscreen: true,
            resizing: true,
            activated: true,
        };
        // Protocol order: maximized=1, fullscreen=2, resizing=3, activated=4.
        assert_eq!(s.to_state_array(), vec![1, 2, 3, 4]);
        assert!(!s.is_empty());
    }

    #[test]
    fn partial_state_skips_unset_bits() {
        let s = WindowState {
            maximized: false,
            fullscreen: true,
            resizing: false,
            activated: false,
        };
        assert_eq!(s.to_state_array(), vec![2]);
    }

    #[test]
    fn visible_space_use_distinguishes_maximized_and_fullscreen() {
        let mut maximized = Window::new(WindowId(1));
        maximized.state.maximized = true;
        assert_eq!(SpaceUse::from_windows(&[maximized]), SpaceUse::Maximized);

        let mut fullscreen = Window::new(WindowId(2));
        fullscreen.state.fullscreen = true;
        assert_eq!(
            SpaceUse::from_windows(&[fullscreen.clone()]),
            SpaceUse::Fullscreen
        );

        fullscreen.minimized = true;
        assert_eq!(SpaceUse::from_windows(&[fullscreen]), SpaceUse::Available);
    }

    #[test]
    fn fullscreen_space_use_has_precedence_over_maximized() {
        let mut maximized = Window::new(WindowId(1));
        maximized.state.maximized = true;
        let mut fullscreen = Window::new(WindowId(2));
        fullscreen.state.fullscreen = true;
        assert_eq!(
            SpaceUse::from_windows(&[maximized, fullscreen]),
            SpaceUse::Fullscreen
        );
    }

    #[test]
    fn unconstrained_hints_round_trip() {
        assert!(SizeHints::default().is_unconstrained());
        let h = SizeHints {
            min_w: 100,
            min_h: 100,
            max_w: 0,
            max_h: 0,
        };
        // Max constraints are 0 (unconstrained) but min are set.
        assert!(!h.is_unconstrained());
    }

    #[test]
    fn window_new_initializes_id_only() {
        let w = Window::new(WindowId(42));
        assert_eq!(w.id, WindowId(42));
        assert!(w.title.is_none());
        assert!(w.app_id.is_none());
        assert!(w.parent.is_none());
        assert!(w.size_hints.is_unconstrained());
        assert!(w.state.is_empty());
        assert!(!w.minimized);
    }

    #[test]
    fn resize_edges_decode_axis_bits() {
        assert!(ResizeEdges::TOP.has_top());
        assert!(!ResizeEdges::TOP.has_bottom());
        assert!(ResizeEdges::LEFT.has_left());
        assert!(!ResizeEdges::LEFT.has_right());
        // Corner: TOP | RIGHT (= 1 | 8 = 9)
        let corner = ResizeEdges(ResizeEdges::TOP.0 | ResizeEdges::RIGHT.0);
        assert!(corner.has_top());
        assert!(corner.has_right());
        assert!(!corner.has_bottom());
        assert!(!corner.has_left());
        assert!(!corner.is_none());
    }

    #[test]
    fn interactive_reports_window_id() {
        let mv = Interactive::Move {
            window_id: WindowId(7),
            origin: (0.0, 0.0),
            start_position: crate::Point::default(),
        };
        assert_eq!(mv.window_id(), WindowId(7));
        let rs = Interactive::Resize {
            window_id: WindowId(9),
            edges: ResizeEdges::BOTTOM,
            origin: (0.0, 0.0),
            start_position: crate::Point::default(),
            start_size: crate::Size::default(),
        };
        assert_eq!(rs.window_id(), WindowId(9));
    }

    #[test]
    fn resize_hit_test_uses_an_outer_margin_with_extended_corners() {
        let mut w = Window::new(WindowId(1));
        w.position = crate::Point { x: 100, y: 50 };
        w.size = crate::Size { w: 400, h: 300 };
        let margin = RESIZE_OUTER_MARGIN;

        assert_eq!(w.resize_edges_at(99.0, 200.0, margin), ResizeEdges::LEFT);
        assert_eq!(w.resize_edges_at(500.0, 200.0, margin), ResizeEdges::RIGHT);
        assert_eq!(w.resize_edges_at(99.0, 49.0, margin).0, 5); // top-left
        assert_eq!(w.resize_edges_at(120.0, 49.0, margin).0, 5); // extended top-left
        assert_eq!(w.resize_edges_at(99.0, 70.0, margin).0, 5); // extended top-left
        assert_eq!(w.resize_edges_at(300.0, 49.0, margin), ResizeEdges::TOP);
        assert_eq!(w.resize_edges_at(300.0, 350.0, margin), ResizeEdges::BOTTOM);

        // Content pixels, including the inner edge, belong to the client.
        assert_eq!(w.resize_edges_at(100.0, 200.0, margin), ResizeEdges::NONE);
        assert_eq!(w.resize_edges_at(499.0, 200.0, margin), ResizeEdges::NONE);
        // The half-open margin is exactly eight logical pixels wide.
        assert_eq!(w.resize_edges_at(91.99, 200.0, margin), ResizeEdges::NONE);
        assert_eq!(w.resize_edges_at(508.0, 200.0, margin), ResizeEdges::NONE);
    }

    #[test]
    fn resize_hit_test_chooses_the_nearest_corner_on_small_windows() {
        let mut w = Window::new(WindowId(1));
        w.position = crate::Point { x: 100, y: 50 };
        w.size = crate::Size { w: 32, h: 32 };
        let margin = RESIZE_OUTER_MARGIN;

        assert_eq!(
            w.resize_edges_at(112.0, 49.0, margin),
            ResizeEdges(ResizeEdges::TOP.0 | ResizeEdges::LEFT.0)
        );
        assert_eq!(
            w.resize_edges_at(120.0, 49.0, margin),
            ResizeEdges(ResizeEdges::TOP.0 | ResizeEdges::RIGHT.0)
        );
        assert_eq!(
            w.resize_edges_at(99.0, 62.0, margin),
            ResizeEdges(ResizeEdges::LEFT.0 | ResizeEdges::TOP.0)
        );
        assert_eq!(
            w.resize_edges_at(99.0, 70.0, margin),
            ResizeEdges(ResizeEdges::LEFT.0 | ResizeEdges::BOTTOM.0)
        );
    }

    #[test]
    fn modifier_resize_selects_nearest_edges_from_any_window_point() {
        let mut w = Window::new(WindowId(1));
        w.position = crate::Point { x: 100, y: 100 };
        w.size = crate::Size { w: 300, h: 180 };

        assert_eq!(
            w.resize_edges_nearest(110.0, 110.0),
            ResizeEdges(ResizeEdges::LEFT.0 | ResizeEdges::TOP.0)
        );
        assert_eq!(w.resize_edges_nearest(250.0, 120.0), ResizeEdges::TOP);
        assert_eq!(
            w.resize_edges_nearest(390.0, 270.0),
            ResizeEdges(ResizeEdges::RIGHT.0 | ResizeEdges::BOTTOM.0)
        );
        assert!(!w.resize_edges_nearest(250.0, 190.0).is_none());
        assert!(w.resize_edges_nearest(99.0, 190.0).is_none());
    }
}
