//! Window-management model: per-toplevel metadata and state.
//!
//! A `Window` is the renderer-agnostic, protocol-agnostic view of a mapped
//! xdg toplevel. The server populates it from `xdg_toplevel` requests; the
//! shell and the future AI-adaptation layer read it for chrome and
//! introspection. Keeping this in `ass-core` (rather than in `ass-server`)
//! means the shell never needs a server dependency to display window state.

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
    /// `XDG_TOPLEVEL_STATE_ACTIVATED` (value 4). The window has keyboard focus.
    pub activated: bool,
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
        // Value 3 is RESIZING; we do not currently set it.
        if self.activated {
            v.push(4);
        }
        v
    }

    /// `true` if no state bits are set. The corresponding `configure` states
    /// array is empty.
    pub fn is_empty(self) -> bool {
        !(self.maximized || self.fullscreen || self.activated)
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
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
    pub size_hints: SizeHints,
    pub state: WindowState,
    /// Compositor-internal minimized flag. Unlike the `state` bits this is not
    /// an `xdg_toplevel` configure state (the protocol has no minimized
    /// state); it records that the client requested `set_minimized` and the
    /// compositor hides the surface from rendering and input until the user
    /// restores it by focusing it from the window list or dock.
    pub minimized: bool,
    /// Whether the tiling policy (ADR-0024) or the floating policy owns this
    /// window's position and size. `Floating` by default; a tiled window
    /// still carries a position and size — the tiling policy sets them.
    pub layout_role: crate::layout::LayoutRole,
    /// Logical extent of the toplevel in compositor space. Set on first map
    /// (from the committed buffer size) and updated by interactive move and
    /// resize. The renderer reads `position`; the shell reads `position` and
    /// `size` for hit-testing and decorations.
    pub position: crate::Point,
    pub size: crate::Size,
}

impl Window {
    pub fn new(id: WindowId) -> Window {
        Window {
            id,
            ..Default::default()
        }
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
#[derive(Debug, Clone, Copy)]
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
            activated: true,
        };
        // Protocol order: maximized=1, fullscreen=2, (resizing=3 skipped),
        // activated=4.
        assert_eq!(s.to_state_array(), vec![1, 2, 4]);
        assert!(!s.is_empty());
    }

    #[test]
    fn partial_state_skips_unset_bits() {
        let s = WindowState {
            maximized: false,
            fullscreen: true,
            activated: false,
        };
        assert_eq!(s.to_state_array(), vec![2]);
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
}
