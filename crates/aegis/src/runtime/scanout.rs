use super::*;

const SCANOUT_TELEMETRY_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);

/// Content most recently committed successfully to the KMS primary plane.
///
/// This is presentation state, not an intent bit: a failed direct-scanout or
/// composited commit must leave it unchanged because the previous framebuffer
/// still owns the physical plane. Nested presentation uses the composited
/// variant as well, which keeps the runtime policy backend-independent.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum PrimaryPlaneState {
    #[default]
    Unassigned,
    Composited,
    DirectScanout {
        surface_id: usize,
    },
}

impl PrimaryPlaneState {
    pub(super) fn is_direct(self) -> bool {
        matches!(self, Self::DirectScanout { .. })
    }

    /// Forget framebuffer ownership when output power, target topology, or the
    /// backend device epoch disappears. No future transition may be inferred
    /// from a framebuffer that the kernel has already retired.
    pub(super) fn invalidate(&mut self) {
        *self = Self::Unassigned;
    }

    /// Record a successful client-buffer commit and report whether this starts
    /// a new direct-scanout ownership session.
    pub(super) fn commit_direct(&mut self, surface_id: usize) -> bool {
        let entered = !matches!(
            *self,
            Self::DirectScanout {
                surface_id: current
            } if current == surface_id
        );
        *self = Self::DirectScanout { surface_id };
        entered
    }

    /// Record a successful compositor-framebuffer commit and report whether
    /// it reclaimed the primary plane from a directly scanned-out client.
    pub(super) fn commit_composited(&mut self) -> bool {
        let left_direct = self.is_direct();
        *self = Self::Composited;
        left_direct
    }
}

/// Complete primary-plane plan for the next frame. Exactly one field is set by
/// the private constructors. A flat representation avoids adding a heap
/// allocation to the per-frame direct candidate path merely to balance enum
/// variant sizes.
pub(super) struct PrimaryPlanePlan {
    direct: Option<aegis_model::SurfaceDmabuf>,
    rejection: Option<ScanoutRejection>,
}

impl PrimaryPlanePlan {
    fn direct(candidate: aegis_model::SurfaceDmabuf) -> Self {
        Self {
            direct: Some(candidate),
            rejection: None,
        }
    }

    fn composite(rejection: ScanoutRejection) -> Self {
        Self {
            direct: None,
            rejection: Some(rejection),
        }
    }

    pub(super) fn direct_candidate(&self) -> Option<&aegis_model::SurfaceDmabuf> {
        self.direct.as_ref()
    }

    pub(super) fn rejection(&self) -> Option<&ScanoutRejection> {
        self.rejection.as_ref()
    }
}

/// Stable, machine-countable reasons why the primary-plane fast path was not
/// selected.  Keeping this policy vocabulary separate from rendering makes a
/// scanout miss observable and prevents future chrome from adding another
/// silent, ad-hoc `return None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) enum ScanoutRejectReason {
    SessionLocked,
    CapturePending,
    ScreenshotFreeze,
    SurfaceTransition,
    Overview,
    WindowSwitcher,
    LiveBackdropEffect,
    VisibleShellPixels,
    ClientOverlay,
    SoftwareCursor,
    ShmClientSurface,
    NoDmabufCandidate,
    MultipleDmabufSurfaces,
    NotSingleToplevel,
    NonTrivialTransform,
    NonZeroOrigin,
    Viewport,
    GeometryMismatch,
    NotFullyOpaque,
    PlaneUnsupported,
    KmsRejected,
}

pub(super) struct ScanoutRejection {
    pub(super) reason: ScanoutRejectReason,
    pub(super) plausible_candidate: bool,
}

impl ScanoutRejectReason {
    fn label(self) -> &'static str {
        match self {
            Self::SessionLocked => "session-locked",
            Self::CapturePending => "capture-pending",
            Self::ScreenshotFreeze => "screenshot-freeze",
            Self::SurfaceTransition => "surface-transition",
            Self::Overview => "overview",
            Self::WindowSwitcher => "window-switcher",
            Self::LiveBackdropEffect => "live-backdrop-effect",
            Self::VisibleShellPixels => "visible-shell-pixels",
            Self::ClientOverlay => "client-overlay",
            Self::SoftwareCursor => "software-cursor",
            Self::ShmClientSurface => "shm-client-surface",
            Self::NoDmabufCandidate => "no-dmabuf-candidate",
            Self::MultipleDmabufSurfaces => "multiple-dmabuf-surfaces",
            Self::NotSingleToplevel => "not-single-toplevel",
            Self::NonTrivialTransform => "non-trivial-transform",
            Self::NonZeroOrigin => "non-zero-origin",
            Self::Viewport => "viewport",
            Self::GeometryMismatch => "geometry-mismatch",
            Self::NotFullyOpaque => "not-fully-opaque",
            Self::PlaneUnsupported => "plane-unsupported",
            Self::KmsRejected => "kms-rejected",
        }
    }
}

impl std::fmt::Display for ScanoutRejectReason {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.label())
    }
}

/// Facts about scene ownership that are independent of any particular client
/// buffer.  They are deliberately semantic: only pixels/effects that would
/// actually reach the next output frame reject scanout.
#[derive(Debug, Clone, Copy, Default)]
struct ScanoutSceneFacts {
    session_locked: bool,
    capture_pending: bool,
    screenshot_freeze: bool,
    transition_pending: bool,
    overview_active: bool,
    window_switcher_active: bool,
    shell: aegis_shell::CompositionRequirements,
    client_overlay_count: usize,
    software_cursor_visible: bool,
    shm_surface_count: usize,
    dmabuf_surface_count: usize,
    toplevel_dmabuf_count: usize,
    candidate_matches_scene: bool,
}

impl ScanoutSceneFacts {
    fn plausible_candidate(self) -> bool {
        self.shm_surface_count == 0
            && self.dmabuf_surface_count == 1
            && self.toplevel_dmabuf_count == 1
            && self.candidate_matches_scene
    }
}

fn evaluate_scene(facts: ScanoutSceneFacts) -> Result<(), ScanoutRejectReason> {
    if facts.session_locked {
        return Err(ScanoutRejectReason::SessionLocked);
    }
    if facts.capture_pending {
        return Err(ScanoutRejectReason::CapturePending);
    }
    if facts.screenshot_freeze {
        return Err(ScanoutRejectReason::ScreenshotFreeze);
    }
    if facts.transition_pending {
        return Err(ScanoutRejectReason::SurfaceTransition);
    }
    if facts.overview_active {
        return Err(ScanoutRejectReason::Overview);
    }
    if facts.window_switcher_active {
        return Err(ScanoutRejectReason::WindowSwitcher);
    }
    // Report the effect before its foreground pixels: this tells operators
    // exactly why a visible Dock/HUD cannot ride the primary plane.
    if facts.shell.live_backdrop_effect {
        return Err(ScanoutRejectReason::LiveBackdropEffect);
    }
    if facts.shell.visible_pixels {
        return Err(ScanoutRejectReason::VisibleShellPixels);
    }
    if facts.client_overlay_count != 0 {
        return Err(ScanoutRejectReason::ClientOverlay);
    }
    if facts.software_cursor_visible {
        return Err(ScanoutRejectReason::SoftwareCursor);
    }
    if facts.shm_surface_count != 0 {
        return Err(ScanoutRejectReason::ShmClientSurface);
    }
    match facts.dmabuf_surface_count {
        0 => return Err(ScanoutRejectReason::NoDmabufCandidate),
        1 => {}
        _ => return Err(ScanoutRejectReason::MultipleDmabufSurfaces),
    }
    if facts.toplevel_dmabuf_count != 1 || !facts.candidate_matches_scene {
        return Err(ScanoutRejectReason::NotSingleToplevel);
    }
    Ok(())
}

fn surface_is_fully_opaque(surface: &aegis_model::SurfaceDmabuf) -> bool {
    if aegis_model::dmabuf::is_format_opaque(surface.drm_format) {
        return true;
    }
    if surface.width <= 0 || surface.height <= 0 {
        return false;
    }
    let scale = surface.geometry.buffer_scale.max(1) as f32;
    let logical = aegis_model::Rect::new(
        0,
        0,
        (surface.width as f32 / scale).round().max(1.0) as i32,
        (surface.height as f32 / scale).round().max(1.0) as i32,
    );
    surface
        .opaque_region
        .as_deref()
        .is_some_and(|regions| logical.fully_covered_by(regions))
}

fn evaluate_surface(
    surface: &aegis_model::SurfaceDmabuf,
    physical_size: (u32, u32),
    plane_supported: bool,
) -> Result<(), ScanoutRejectReason> {
    if surface.geometry.transform != aegis_model::Transform::Normal {
        return Err(ScanoutRejectReason::NonTrivialTransform);
    }
    if surface.geometry.position != (aegis_model::Point { x: 0, y: 0 }) {
        return Err(ScanoutRejectReason::NonZeroOrigin);
    }
    if surface.geometry.viewport_src.is_some() || surface.geometry.viewport_dst.is_some() {
        return Err(ScanoutRejectReason::Viewport);
    }
    if surface.width <= 0
        || surface.height <= 0
        || (surface.width as u32, surface.height as u32) != physical_size
    {
        return Err(ScanoutRejectReason::GeometryMismatch);
    }
    if !surface_is_fully_opaque(surface) {
        return Err(ScanoutRejectReason::NotFullyOpaque);
    }
    if !plane_supported {
        return Err(ScanoutRejectReason::PlaneUnsupported);
    }
    Ok(())
}

/// Rate-limited rejection counters.  A plausible full-output candidate is
/// reported at info level; ordinary multi-window/no-candidate desktop states
/// stay at debug level.  Repeated video commits therefore provide useful
/// diagnostics without producing one line per refresh.
pub(super) struct ScanoutTelemetry {
    interval_counts: std::collections::BTreeMap<ScanoutRejectReason, u64>,
    last_candidate_report: Option<std::time::Instant>,
    last_background_report: Option<std::time::Instant>,
}

impl ScanoutTelemetry {
    pub(super) fn new() -> Self {
        Self {
            interval_counts: std::collections::BTreeMap::new(),
            last_candidate_report: None,
            last_background_report: None,
        }
    }

    pub(super) fn record_rejection(
        &mut self,
        host: &str,
        reason: ScanoutRejectReason,
        plausible_candidate: bool,
    ) {
        *self.interval_counts.entry(reason).or_default() += 1;
        let now = std::time::Instant::now();
        let last_report = if plausible_candidate {
            self.last_candidate_report
        } else {
            self.last_background_report
        };
        if last_report
            .is_some_and(|last| now.saturating_duration_since(last) < SCANOUT_TELEMETRY_INTERVAL)
        {
            return;
        }
        let mut summary = String::new();
        for (index, (reason, count)) in self.interval_counts.iter().enumerate() {
            use std::fmt::Write as _;
            if index != 0 {
                summary.push_str(", ");
            }
            let _ = write!(summary, "{}={count}", reason.label());
        }
        if plausible_candidate {
            log::info!(
                "{host}: direct scanout blocked ({reason}); recent rejection counts: {summary}"
            );
        } else {
            log::debug!(
                "{host}: direct scanout unavailable ({reason}); recent rejection counts: {summary}"
            );
        }
        self.interval_counts.clear();
        if plausible_candidate {
            self.last_candidate_report = Some(now);
        } else {
            self.last_background_report = Some(now);
        }
    }

    pub(super) fn record_success(&mut self) {
        self.interval_counts.clear();
    }
}

impl CompositorRuntime {
    /// Evaluate the physical output as a primary-plane scene.  This is based
    /// exclusively on actual surface geometry/coverage and current visible
    /// compositor output; xdg fullscreen state is intentionally irrelevant.
    pub(super) fn plan_primary_plane(
        &self,
        physical_size: (u32, u32),
        cursor_hidden: bool,
        frame_capture_pending: bool,
    ) -> PrimaryPlanePlan {
        let shell = self
            .shell
            .composition_requirements(self.input_acc.display_size);
        // One shared visibility/occlusion snapshot for every count here: the
        // scanout planner previously rebuilt both full frame lists (each an
        // O(windows × surfaces) occlusion walk plus per-surface Vec clones of
        // damage/opaque regions) only to read `.len()` from them — and it ran
        // before the cheap SHM rejection, so a pure-SHM desktop paid the full
        // dma-buf collection every frame for a candidate that always fails.
        let sets = self.server.desktop_frame_sets();
        let visible = &sets.visible;
        let occluded = &sets.occluded;
        let shm_surface_count = sets.shm.len();
        let dmabuf_surface_count = sets.dmabuf.len();
        let mut toplevel_dmabufs = self.server.toplevel_dmabuf_frames_with(visible, occluded);
        let candidate_matches_scene = dmabuf_surface_count == 1
            && toplevel_dmabufs.len() == 1
            && sets.dmabuf[0].id == toplevel_dmabufs[0].id;
        let facts = ScanoutSceneFacts {
            session_locked: self.server.session_locked(),
            capture_pending: frame_capture_pending
                || self.pending_capture.is_some()
                || self.pending_interaction_domain_capture.is_some(),
            screenshot_freeze: self.screenshot_freeze.armed,
            transition_pending: self.server.transitions_pending(),
            overview_active: self.shell.overview_active(),
            window_switcher_active: self.shell.window_switcher_active(),
            shell,
            client_overlay_count: self.server.overlay_frames().len()
                + self.server.overlay_dmabuf_frames().len(),
            software_cursor_visible: self.host.uses_software_cursor() && !cursor_hidden,
            shm_surface_count,
            dmabuf_surface_count,
            toplevel_dmabuf_count: toplevel_dmabufs.len(),
            candidate_matches_scene,
        };
        let plausible_candidate = facts.plausible_candidate();
        if let Err(reason) = evaluate_scene(facts) {
            return PrimaryPlanePlan::composite(ScanoutRejection {
                reason,
                plausible_candidate,
            });
        }
        let Some(candidate) = toplevel_dmabufs.pop() else {
            return PrimaryPlanePlan::composite(ScanoutRejection {
                reason: ScanoutRejectReason::NoDmabufCandidate,
                plausible_candidate,
            });
        };
        if let Err(reason) = evaluate_surface(
            &candidate,
            physical_size,
            self.host
                .supports_scanout(candidate.drm_format, candidate.modifier),
        ) {
            return PrimaryPlanePlan::composite(ScanoutRejection {
                reason,
                plausible_candidate,
            });
        }
        PrimaryPlanePlan::direct(candidate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(format: u32) -> aegis_model::SurfaceDmabuf {
        aegis_model::SurfaceDmabuf {
            id: 7,
            window: Some(aegis_model::window::WindowId(9)),
            width: 3072,
            height: 1920,
            generation: 1,
            damage: Vec::new(),
            buffer_id: 1,
            fd: -1,
            drm_format: format,
            modifier: 0,
            offset: 0,
            stride: 3072 * 4,
            acquire_fence: -1,
            geometry: aegis_model::SurfaceGeometry {
                buffer_scale: 2,
                ..Default::default()
            },
            opaque_region: None,
            color: None,
        }
    }

    fn eligible_scene() -> ScanoutSceneFacts {
        ScanoutSceneFacts {
            dmabuf_surface_count: 1,
            toplevel_dmabuf_count: 1,
            candidate_matches_scene: true,
            ..Default::default()
        }
    }

    #[test]
    fn maximized_xrgb_geometry_is_scanout_eligible_without_fullscreen_state() {
        let surface = candidate(aegis_model::dmabuf::DRM_FORMAT_XRGB8888);
        // SurfaceDmabuf contains no xdg fullscreen flag: actual output
        // placement, extent, opacity, and plane support are the entire policy.
        assert_eq!(evaluate_scene(eligible_scene()), Ok(()));
        assert_eq!(evaluate_surface(&surface, (3072, 1920), true), Ok(()));
    }

    #[test]
    fn argb_full_opaque_region_is_scanout_eligible() {
        let mut surface = candidate(aegis_model::dmabuf::DRM_FORMAT_ARGB8888);
        surface.opaque_region = Some(vec![aegis_model::Rect::new(0, 0, 1536, 960)]);
        assert_eq!(evaluate_surface(&surface, (3072, 1920), true), Ok(()));
    }

    #[test]
    fn incomplete_argb_opaque_region_is_rejected() {
        let mut surface = candidate(aegis_model::dmabuf::DRM_FORMAT_ARGB8888);
        surface.opaque_region = Some(vec![aegis_model::Rect::new(0, 0, 1535, 960)]);
        assert_eq!(
            evaluate_surface(&surface, (3072, 1920), true),
            Err(ScanoutRejectReason::NotFullyOpaque)
        );
    }

    #[test]
    fn output_geometry_must_match_exactly() {
        let mut surface = candidate(aegis_model::dmabuf::DRM_FORMAT_XRGB8888);
        surface.width -= 1;
        assert_eq!(
            evaluate_surface(&surface, (3072, 1920), true),
            Err(ScanoutRejectReason::GeometryMismatch)
        );
    }

    #[test]
    fn popup_or_subsurface_blocks_scanout() {
        let facts = ScanoutSceneFacts {
            dmabuf_surface_count: 2,
            toplevel_dmabuf_count: 1,
            candidate_matches_scene: false,
            ..Default::default()
        };
        assert_eq!(
            evaluate_scene(facts),
            Err(ScanoutRejectReason::MultipleDmabufSurfaces)
        );
        let facts = ScanoutSceneFacts {
            shm_surface_count: 1,
            dmabuf_surface_count: 1,
            toplevel_dmabuf_count: 1,
            candidate_matches_scene: true,
            ..Default::default()
        };
        assert_eq!(
            evaluate_scene(facts),
            Err(ScanoutRejectReason::ShmClientSurface)
        );
    }

    #[test]
    fn visible_shell_pixels_block_but_hidden_dock_does_not() {
        let mut facts = eligible_scene();
        facts.shell.visible_pixels = true;
        assert_eq!(
            evaluate_scene(facts),
            Err(ScanoutRejectReason::VisibleShellPixels)
        );

        facts.shell = aegis_shell::CompositionRequirements::default();
        assert_eq!(evaluate_scene(facts), Ok(()));
    }

    #[test]
    fn visible_live_dock_blur_has_a_precise_rejection_reason() {
        let mut facts = eligible_scene();
        facts.shell = aegis_shell::CompositionRequirements {
            visible_pixels: true,
            live_backdrop_effect: true,
        };
        assert_eq!(
            evaluate_scene(facts),
            Err(ScanoutRejectReason::LiveBackdropEffect)
        );
    }

    #[test]
    fn client_overlay_never_disappears_under_scanout() {
        let mut facts = eligible_scene();
        facts.client_overlay_count = 1;
        assert_eq!(
            evaluate_scene(facts),
            Err(ScanoutRejectReason::ClientOverlay)
        );
    }

    #[test]
    fn window_switcher_reclaims_primary_plane_without_mutating_fullscreen_state() {
        let mut facts = eligible_scene();
        facts.window_switcher_active = true;
        assert_eq!(
            evaluate_scene(facts),
            Err(ScanoutRejectReason::WindowSwitcher)
        );

        let mut state = PrimaryPlaneState::default();
        assert!(state.commit_direct(7));
        assert!(state.is_direct());
        // Planning a composite does not lie about physical ownership. The
        // state changes only after that compositor framebuffer really lands.
        assert!(state.commit_composited());
        assert_eq!(state, PrimaryPlaneState::Composited);
    }

    #[test]
    fn primary_plane_state_tracks_successful_owner_changes() {
        let mut state = PrimaryPlaneState::default();
        assert!(state.commit_direct(7));
        assert!(!state.commit_direct(7));
        assert!(state.commit_direct(9));
        assert_eq!(state, PrimaryPlaneState::DirectScanout { surface_id: 9 });
        assert!(state.commit_composited());
        assert!(!state.commit_composited());
        state.invalidate();
        assert_eq!(state, PrimaryPlaneState::Unassigned);
    }
}
