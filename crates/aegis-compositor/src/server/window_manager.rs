use crate::*;

impl Server {
    fn switcher_candidates(&self) -> Vec<aegis_model::window::WindowId> {
        let visible = self.visible();
        let mut candidates: Vec<_> = self
            .state
            .live_surfaces()
            .map(|p| unsafe { &*p })
            .filter(|s| {
                !s.xdg_toplevel.is_null()
                    && s.mapped
                    && !s.window.minimized
                    && visible.contains(&s.window.id)
                    && self
                        .state
                        .authority
                        .seat_controls_window(self.state.active_seat, s.window.id)
            })
            .map(|s| s.window.id)
            .collect();
        candidates.reverse();
        candidates
    }

    /// Freeze the current most-recently-used order for a held-Super cycle.
    pub fn start_window_switcher(&mut self) {
        if self.state.window_switcher.is_some() {
            return;
        }
        let order = self.switcher_candidates();
        let selected = self
            .focused_toplevel_id()
            .and_then(|id| order.iter().position(|candidate| *candidate == id))
            .unwrap_or(0);
        self.state.window_switcher = Some(WindowSwitcherSession {
            order,
            selected,
            last_forward: true,
        });
    }

    /// Commit the current preview selection, then end the held-Super session.
    /// This is the only keyboard-switcher path that raises a window or moves
    /// the Wayland keyboard focus.
    pub fn finish_window_switcher(&mut self) {
        let selected = self
            .window_switcher_snapshot()
            .and_then(|(_, selected)| selected);
        self.state.window_switcher = None;
        if let Some(selected) = selected {
            self.focus_surface_by_id(selected);
            self.rehit_pointer_after_stack_change();
        }
    }

    /// Dismiss the held-Super session without applying its preview selection.
    pub fn cancel_window_switcher(&mut self) {
        self.state.window_switcher = None;
    }

    pub fn window_switcher_active(&self) -> bool {
        self.state.window_switcher.is_some()
    }

    /// Update the preview target in a frozen MRU order while a switcher
    /// session is active. Outside a held session this remains an immediate,
    /// one-shot focus command for IPC and non-session callers.
    pub fn cycle_focus(&mut self, forward: bool) {
        if self.state.window_switcher.is_none() {
            let order = self.switcher_candidates();
            if order.len() < 2 {
                return;
            }
            let selected = self
                .focused_toplevel_id()
                .and_then(|id| order.iter().position(|candidate| *candidate == id))
                .unwrap_or(0);
            let next = order[stepped_index(selected, order.len(), forward)];
            self.focus_surface_by_id(next);
            self.rehit_pointer_after_stack_change();
            return;
        }

        self.refresh_window_switcher();
        let Some(session) = self.state.window_switcher.as_mut() else {
            return;
        };
        if session.order.len() < 2 {
            return;
        }
        session.selected = stepped_index(session.selected, session.order.len(), forward);
        session.last_forward = forward;
    }

    /// Return the frozen order and latest preview target for shell rendering.
    /// Closed windows are pruned first; new windows are never inserted into an
    /// active session.
    pub fn window_switcher_snapshot(
        &mut self,
    ) -> Option<(
        Vec<aegis_model::window::WindowId>,
        Option<aegis_model::window::WindowId>,
    )> {
        self.refresh_window_switcher();
        self.state.window_switcher.as_ref().map(|session| {
            (
                session.order.clone(),
                session.order.get(session.selected).copied(),
            )
        })
    }

    fn refresh_window_switcher(&mut self) {
        let eligible: std::collections::HashSet<_> =
            self.switcher_candidates().into_iter().collect();
        let Some(session) = self.state.window_switcher.as_mut() else {
            return;
        };
        reconcile_switcher_session(session, &eligible);
    }

    /// Switch to an adjacent workspace on the focused output (ADR-0025). The
    /// visible set changes on the next frame; if the focused toplevel is no
    /// longer visible, keyboard focus is dropped (a `wl_keyboard.leave` is
    /// posted) so keystrokes do not route to a hidden window.
    pub fn switch_workspace(&mut self, dir: aegis_model::workspace::Switch) {
        let old = self.state.workspaces.current_workspace(self.state.output);
        let new = self.state.workspaces.switch(self.state.output, dir);
        if let (Some(old), Some(new)) = (old, new)
            && old != new
        {
            let direction = match dir {
                aegis_model::workspace::Switch::Next => 1,
                aegis_model::workspace::Switch::Prev => -1,
            };
            self.begin_workspace_slide(old, new, direction);
        }
        self.drop_focus_if_hidden();
        unsafe { ffi::wl_display_flush_clients(self.state.display) };
    }

    /// Switch directly to a workspace by id on the output that owns it. Same
    /// focus-drop contract as [`switch_workspace`](Self::switch_workspace).
    pub fn switch_workspace_to(&mut self, id: aegis_model::workspace::WorkspaceId) {
        let Some(target_output) = self
            .state
            .workspaces
            .workspace(id)
            .map(|workspace| workspace.output)
        else {
            return;
        };
        let old = self.state.workspaces.current_workspace(target_output);
        let old_index = old.and_then(|workspace| {
            self.state
                .workspaces
                .output(target_output)?
                .workspaces
                .iter()
                .position(|candidate| *candidate == workspace)
        });
        let new_index = self
            .state
            .workspaces
            .output(target_output)
            .and_then(|output| {
                output
                    .workspaces
                    .iter()
                    .position(|candidate| *candidate == id)
            });
        let new = self.state.workspaces.switch_to(id);
        if let (Some(old), Some(new)) = (old, new)
            && old != new
        {
            let direction = if new_index.unwrap_or(0) >= old_index.unwrap_or(0) {
                1
            } else {
                -1
            };
            self.begin_workspace_slide(old, new, direction);
        }
        self.drop_focus_if_hidden();
        unsafe { ffi::wl_display_flush_clients(self.state.display) };
    }

    /// Move a toplevel to a workspace by id (ADR-0025). If the target is not
    /// the current workspace, the window leaves the visible set. No-op if the
    /// window or workspace is unknown, or the physical session only observes
    /// the window.
    pub fn move_to_workspace(
        &mut self,
        window_id: aegis_model::window::WindowId,
        workspace: aegis_model::workspace::WorkspaceId,
    ) {
        if !self.human_controls_window(window_id) {
            return;
        }
        self.state.workspaces.move_toplevel(window_id, workspace);
        self.drop_focus_if_hidden();
    }

    /// The workspace/output snapshot for the IPC and chrome (ADR-0025/0027).
    pub fn workspace_snapshot(&self) -> aegis_model::workspace::WorkspaceSnapshot {
        self.state.workspaces.snapshot()
    }

    /// Cheap content hash of the workspace model. The frame loop compares
    /// this per frame and only rebuilds the owned
    /// [`Self::workspace_snapshot`] when it moves.
    pub fn workspace_signature(&self) -> u64 {
        self.state.workspaces.signature()
    }

    /// Revision of the backend-reported output list, bumped on every
    /// mutation. Lets the frame loop skip re-cloning
    /// [`Self::output_infos`] while unchanged.
    pub fn outputs_revision(&self) -> u64 {
        self.state.outputs_revision
    }

    /// Whether the current workspace is in tiled mode (ADR-0024).
    pub fn tiling(&self) -> bool {
        self.state
            .workspaces
            .current_workspace_tiled(self.state.output)
    }

    /// Toggle the current workspace between tiled and floating (ADR-0024).
    /// On, the workspace's windows are marked `Tiled` and laid out next
    /// `apply_tiling`; off, they revert to `Floating` and keep their current
    /// geometry. Layout targets are cleared so the next apply reconfigures.
    pub fn set_tiling(&mut self, on: bool) {
        if let Some(wid) = self.state.workspaces.current_workspace(self.state.output) {
            self.state.workspaces.set_tiled(wid, on);
        }
        let role = if on {
            aegis_model::layout::LayoutRole::Tiled
        } else {
            aegis_model::layout::LayoutRole::Floating
        };
        for id in self.state.workspaces.visible_toplevels() {
            let rec = self.find_surface_by_window_id(id);
            if rec.is_null() {
                continue;
            }
            unsafe {
                // Transient dialogs (xdg_toplevel.set_parent) stay floating
                // (ADR-0024 floating exception); the sweep skips them.
                if on && (*rec).window.parent.is_some() {
                    log::debug!(
                        "[server] tiling sweep skips transient {:?}",
                        (*rec).window.id
                    );
                    continue;
                }
                (*rec).window.layout_role = role;
                (*rec).layout_target = None;
            }
        }
        log::info!(
            "[server] workspace tiling {}",
            if on { "on" } else { "off" }
        );
    }

    /// Set whether newly created workspaces start in tiled mode (ADR-0024),
    /// from the config's `[layout] default_tiled`. Existing workspaces keep
    /// their own flag. Called at startup and on config reload.
    pub fn set_tiling_default(&mut self, on: bool) {
        self.state.workspaces.set_default_tiled(on);
    }

    /// Replace the window rules (ADR-0026). Called at startup and on config
    /// reload. Rules apply to windows mapped after they are set.
    pub fn set_window_rules(&mut self, rules: Vec<aegis_model::window_rule::WindowRule>) {
        self.state.window_rules = rules;
    }

    /// Set whether window positions and geometries are remembered across restarts.
    pub fn set_remember_window_positions(&mut self, remember: bool) {
        self.state.remember_window_positions = remember;
    }

    /// Replace the tiling layout parameters (gaps, master ratio) from the
    /// config (ADR-0024/0026). Applied on the next `apply_tiling`.
    pub fn set_layout_params(&mut self, params: aegis_model::layout::LayoutParams) {
        self.state.layout_params = params;
    }

    /// Set the reduced-motion policy (ADR-0029, from `[ui] reduced_motion`).
    /// When enabled, in-flight transitions resolve immediately and no new
    /// ones are recorded.
    pub fn set_reduced_motion(&mut self, reduced: bool) {
        self.state.reduced_motion = reduced;
        if reduced {
            self.state.workspace_slide = None;
            for p in self.state.live_surfaces() {
                unsafe { (*p).window.transition = None };
            }
        }
    }

    /// Set the minimize flight style (`[dock] minimize_animation`). Applies
    /// to the next minimize/restore; in-flight transitions keep their style.
    pub fn set_minimize_animation(&mut self, style: aegis_model::dock::MinimizeAnimationStyle) {
        self.state.minimize_animation = style;
    }

    /// Replace the minimize flight targets — the resting dock-icon rect per
    /// window — with the shell's latest report. Pushed every frame so even a
    /// client-initiated `xdg_toplevel.set_minimized` flies at the real icon.
    pub fn set_minimize_targets(
        &mut self,
        targets: Vec<(aegis_model::window::WindowId, aegis_model::Rect)>,
    ) {
        self.state.minimize_targets = targets.into_iter().collect();
    }

    /// Apply the desktop-wide xdg-decoration ownership policy.
    ///
    /// Existing decoration-aware toplevels receive a fresh decoration
    /// configure followed by the required xdg-surface configure, so config
    /// reload changes take effect without restarting applications.
    pub fn set_decoration_policy(&mut self, policy: aegis_model::window::DecorationPolicy) {
        if self.state.decoration_policy == policy {
            return;
        }
        self.state.decoration_policy = policy;
        for rec in self.state.live_surfaces() {
            unsafe {
                if !(*rec).xdg_decoration.is_null() {
                    extensions::configure_decoration((*rec).xdg_decoration, rec);
                }
            }
        }
        unsafe { ffi::wl_display_flush_clients(self.state.display) };
    }

    /// Compositor-relative millisecond timestamp for transition records.
    pub(crate) fn now_ms(&self) -> u64 {
        self.epoch.elapsed().as_millis() as u64
    }

    fn begin_workspace_slide(
        &mut self,
        outgoing: aegis_model::workspace::WorkspaceId,
        incoming: aegis_model::workspace::WorkspaceId,
        direction: i32,
    ) {
        if self.state.reduced_motion {
            self.state.workspace_slide = None;
            return;
        }
        let output = self
            .state
            .workspaces
            .workspace(outgoing)
            .and_then(|workspace| self.state.workspaces.output(workspace.output))
            .and_then(|workspace_output| {
                self.state
                    .output_infos
                    .iter()
                    .find(|output| output.connector == workspace_output.connector)
            })
            .map(|output| output.geometry.logical_rect())
            .unwrap_or_else(|| self.output_logical_rect());
        let width = output.size.w.max(1) as f32;
        let now = self.now_ms();
        let previous = self.state.workspace_slide.take();
        let positions = previous
            .as_ref()
            .filter(|slide| slide.output == output && slide.is_active_at(now))
            .map(|slide| {
                slide
                    .layers
                    .iter()
                    .filter_map(|layer| {
                        slide
                            .offset_at(layer.workspace, now)
                            .map(|offset| (layer.workspace, offset))
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let layers = retarget_workspace_strip(positions, outgoing, incoming, direction, width);
        self.state.workspace_slide = Some(WorkspaceSlide {
            output,
            layers,
            started_ms: now,
            duration_ms: WORKSPACE_SLIDE_DURATION_MS,
        });
    }

    pub(crate) fn workspace_slide_pending(&self) -> bool {
        self.state
            .workspace_slide
            .as_ref()
            .is_some_and(|slide| slide.is_active_at(self.now_ms()))
    }

    pub fn workspace_slide_presentation(&self) -> Option<WorkspaceSlidePresentation> {
        let now = self.now_ms();
        let slide = self
            .state
            .workspace_slide
            .as_ref()
            .filter(|slide| slide.is_active_at(now))?;
        let layers = slide
            .layers
            .iter()
            .filter_map(|layer| {
                let offset_x = slide.offset_at(layer.workspace, now)?;
                let windows = self
                    .state
                    .workspaces
                    .workspace(layer.workspace)
                    .map(|workspace| workspace.toplevels.clone())
                    .unwrap_or_default();
                Some(WorkspaceSlideLayerPresentation { windows, offset_x })
            })
            .collect();
        Some(WorkspaceSlidePresentation {
            output: slide.output,
            layers,
        })
    }

    /// Current workspace surfaces plus the source surfaces retained by a
    /// live workspace slide. This is presentation visibility only; input and
    /// focus continue to use [`Self::visible`].
    pub(crate) fn render_visible(
        &self,
    ) -> std::collections::HashSet<aegis_model::window::WindowId> {
        let mut visible = self.visible();
        if let Some(slide) = self.workspace_slide_presentation() {
            visible.extend(slide.layers.into_iter().flat_map(|layer| layer.windows));
        }
        visible
    }

    /// Normalize geometry and workspace transition state after each animation
    /// interval has elapsed. Transition records are state, not history:
    /// keeping settled values makes scene consumers mistake old animation for
    /// live work (notably disabling opaque coverage or retaining source
    /// workspace surfaces).
    ///
    /// The runtime calls this once at the start of every event iteration. An
    /// active transition schedules those iterations until its deadline, so the
    /// terminal tick always retires it even if no client submits another
    /// buffer.
    pub fn settle_finished_transitions(&mut self) -> usize {
        let now = self.now_ms();
        let mut settled = 0;
        if self
            .state
            .workspace_slide
            .as_ref()
            .is_some_and(|slide| !slide.is_active_at(now))
        {
            self.state.workspace_slide = None;
            settled += 1;
        }
        let before = self.state.closing_frames.len();
        self.state
            .closing_frames
            .retain(|frame| frame.transition.is_active_at(now));
        settled += before - self.state.closing_frames.len();
        for pointer in self.state.live_surfaces() {
            // SAFETY: `live_surfaces` yields non-null records owned by this
            // single-threaded server for the duration of the iteration.
            let surface = unsafe { &mut *pointer };
            if surface
                .window
                .transition
                .is_some_and(|transition| !transition.is_active_at(now))
            {
                surface.window.transition = None;
                settled += 1;
            }
        }
        settled
    }

    /// Record a geometry transition for a non-interactive rect change
    /// (tiling, IPC geometry). The model moves to the target immediately;
    /// rendering interpolates from the current on-screen rect — the previous
    /// transition's mid-flight rect when changes come faster than the
    /// duration, else the previous model rect (ADR-0029).
    pub(crate) fn note_transition(&self, rec: *mut SurfaceRec, old: aegis_model::Rect) {
        if self.state.reduced_motion || rec.is_null() {
            return;
        }
        let now = self.now_ms();
        unsafe {
            let target = aegis_model::Rect {
                origin: (*rec).position,
                size: (*rec).window.size,
            };
            if old == target {
                (*rec).window.transition = None;
                return;
            }
            let from = (*rec)
                .window
                .transition
                .and_then(|t| t.rect_at(old, now))
                .unwrap_or(old);
            (*rec).window.transition =
                Some(aegis_model::transition::WindowTransition::new(from, now));
        }
    }

    /// Ghost frames for windows that are closing (fading out) right now,
    /// snapshotted for the renderer. Each entry carries its interpolated rect
    /// and fading opacity at the current frame time.
    pub fn closing_frame_views(&self) -> Vec<aegis_model::ClosingGhostView<'_>> {
        let now = self.now_ms();
        self.state
            .closing_frames
            .iter()
            .filter_map(|frame| {
                let rect = frame.rect_at(now)?;
                let opacity = frame.opacity_at(now).unwrap_or(0.0);
                let dmabuf = frame.dmabuf.as_ref();
                Some(aegis_model::ClosingGhostView {
                    id: frame.id,
                    rect,
                    buffer_width: frame.buffer_width,
                    buffer_height: frame.buffer_height,
                    pixels: &frame.pixels,
                    dmabuf_fd: dmabuf.map_or(-1, |db| db.fd),
                    drm_format: dmabuf.map_or(0, |db| db.drm_format),
                    modifier: dmabuf.map_or(0, |db| db.modifier),
                    stride: dmabuf.map_or(0, |db| db.stride),
                    offset: dmabuf.map_or(0, |db| db.offset),
                    opacity,
                    color: frame.color.clone(),
                })
            })
            .collect()
    }

    /// The rect a surface renders at this frame: `Some(interpolated)` while
    /// its transition is in flight, `None` at the model target (ADR-0029).
    pub(crate) fn transition_render_rect(&self, s: &SurfaceRec) -> Option<aegis_model::Rect> {
        let target = aegis_model::Rect {
            origin: s.position,
            size: s.window.size,
        };
        s.window
            .transition
            .and_then(|t| t.rect_at(target, self.now_ms()))
    }

    /// Whether a workspace slide or toplevel geometry transition is in flight
    /// — the main loop keeps ticking frames instead of blocking on the host.
    pub fn transitions_pending(&self) -> bool {
        self.workspace_slide_pending()
            || self
                .state
                .closing_frames
                .iter()
                .any(|frame| frame.transition.is_active_at(self.now_ms()))
            || self.state.live_surfaces().any(|p| unsafe {
                !(*p).xdg_toplevel.is_null() && self.transition_render_rect(&*p).is_some()
            })
    }

    /// Reconcile connector identities and geometries reported by the backend.
    /// Existing connector workspaces survive reordering; removed outputs are
    /// relocated by `WorkspaceModel`, and a replug restores their origin.
    pub fn set_outputs(&mut self, mut outputs: Vec<aegis_model::output::OutputInfo>) {
        outputs.retain(|output| !output.connector.is_empty());
        let mut seen = std::collections::HashSet::new();
        outputs.retain(|output| seen.insert(output.connector.clone()));
        if outputs.is_empty() {
            return;
        }
        // Configured per-connector policy (ADR-0028) wins over the
        // backend-reported geometry: scale and position apply here. A
        // configured transform is accepted but not yet applied — the
        // renderer's output-transform support is still pending.
        for output in &mut outputs {
            let Some(policy) = self.state.output_policies.get(&output.connector) else {
                continue;
            };
            if let Some(scale) = policy.scale {
                output.geometry.scale = aegis_model::output::Scale(scale as f32);
            }
            if let Some(position) = policy.position {
                output.geometry.logical_origin = position;
            }
            if let Some(transform) = policy.transform
                && transform != aegis_model::Transform::Normal
            {
                log::warn!(
                    "[server] output '{}': transform configured but not yet applied \
                         (renderer output-transform support pending)",
                    output.connector
                );
            }
        }
        // A `primary` policy moves its output to the front: index 0 is the
        // primary/focused output below. When several entries claim primary,
        // the one that appears first in the backend's output order wins.
        if let Some(primary) = outputs.iter().position(|output| {
            self.state
                .output_policies
                .get(&output.connector)
                .is_some_and(|policy| policy.primary)
        }) {
            let output = outputs.remove(primary);
            outputs.insert(0, output);
        }

        let desired = outputs
            .iter()
            .map(|output| output.connector.as_str())
            .collect::<std::collections::HashSet<_>>();
        for output in &outputs {
            if !self
                .state
                .workspaces
                .outputs()
                .iter()
                .any(|current| current.connector == output.connector)
            {
                self.state.workspaces.add_output(&output.connector);
            }
        }
        let removed = self
            .state
            .workspaces
            .outputs()
            .iter()
            .filter(|output| !desired.contains(output.connector.as_str()))
            .map(|output| output.id)
            .collect::<Vec<_>>();
        for output in removed {
            self.state.workspaces.remove_output(output);
        }

        let primary = outputs[0].clone();
        if let Some(output) = self
            .state
            .workspaces
            .outputs()
            .iter()
            .find(|output| output.connector == primary.connector)
        {
            self.state.output = output.id;
        }
        unsafe { reconcile_output_globals(self.state.as_mut(), &outputs) };
        self.state.output_infos = outputs;
        self.state.outputs_revision = self.state.outputs_revision.wrapping_add(1);
        self.set_output_geometry(primary.geometry);
    }

    /// Install the session color pipeline reported by the backend (ADR-0001
    /// boundary: the pixel encoding is a flux capability; this server only
    /// advertises it). On change, broadcast
    /// `wp_color_management_output_v1.image_description_changed` and
    /// `wp_color_management_surface_feedback_v1.preferred_changed`.
    pub fn set_color_pipeline(&mut self, pipeline: aegis_model::output::ColorPipeline) {
        if self.state.color_pipeline == pipeline {
            return;
        }
        self.state.color_pipeline = pipeline;
        // The pipeline change retires the old output description record:
        // mint a fresh identity so `preferred_changed` names the new one.
        self.state.color_pipeline_identity = self.state.alloc_color_identity();
        unsafe { crate::extensions::resend_color_pipeline(self.state.as_mut()) };
    }

    /// Set per-connector output policies from the config's `[[output]]`
    /// entries (ADR-0028), and re-apply them to the current output set.
    /// Unmatched connectors are ignored with a log line, so a monitor that is
    /// not plugged in yet still applies once it appears.
    pub fn set_output_policies(
        &mut self,
        policies: std::collections::HashMap<String, aegis_model::output::OutputPolicy>,
    ) {
        for connector in policies.keys() {
            if !self
                .state
                .output_infos
                .iter()
                .any(|o| &o.connector == connector)
            {
                log::info!("[server] output policy for '{connector}' matches no current output");
            }
        }
        self.state.output_policies = policies;
        let outputs = self.state.output_infos.clone();
        if !outputs.is_empty() {
            self.set_outputs(outputs);
        }
    }

    /// Replace the focused output's geometry (ADR-0028). The backend calls
    /// this on resize; the tiling work-area is the geometry's logical rect.
    /// Re-sends the wl_output geometry/mode/scale/done sequence to every bound
    /// client so they update their scale and surface buffer scale.
    pub fn set_output_geometry(&mut self, geo: aegis_model::output::OutputGeometry) {
        self.state.output_geometry = geo;
        if let Some(primary) = self.state.output_infos.first_mut() {
            primary.geometry = geo;
        }
        self.state.outputs_revision = self.state.outputs_revision.wrapping_add(1);
        let infos = self.state.output_infos.clone();
        unsafe { reconcile_output_globals(self.state.as_mut(), &infos) };
        // Resend to every bound wl_output resource.
        let resources: Vec<*mut ffi::wl_resource> = self
            .state
            .output_resources
            .iter()
            .copied()
            .filter(|p| !p.is_null())
            .collect();
        for res in resources {
            unsafe { send_output_geometry(res) };
        }
        // Refresh xdg-output logical extents too.
        self.resend_xdg_outputs();
        // Re-send fractional-scale hints so HiDPI-aware clients resize buffers.
        unsafe {
            extensions::resend_fractional_scales(self.state.as_ref() as *const State as *mut State)
        };
        unsafe { extensions::session_lock_outputs_changed(self.state.as_mut()) };
        unsafe { ffi::wl_display_flush_clients(self.state.display) };
    }

    /// The focused output's logical rect (ADR-0028). The chrome-aware
    /// tiling work-area is this inset by the chrome's reserved edges.
    pub fn output_logical_rect(&self) -> aegis_model::Rect {
        self.state.output_geometry.logical_rect()
    }

    /// Resend the `zxdg_output_v1` logical geometry events to every bound
    /// xdg-output resource. Called whenever the output's logical extents
    /// change (resize / scale / transform) so clients reposition. Pairs with
    /// the wl_output geometry re-send in [`Server::set_output_geometry`].
    pub(crate) fn resend_xdg_outputs(&self) {
        let resources: Vec<*mut ffi::wl_resource> = self
            .state
            .xdg_output_resources
            .iter()
            .copied()
            .filter(|p| !p.is_null())
            .collect();
        for res in resources {
            unsafe {
                let output = self
                    .state
                    .xdg_output_links
                    .get(&(res as usize))
                    .copied()
                    .unwrap_or(std::ptr::null_mut());
                extensions::send_xdg_output_geometry(
                    res,
                    output,
                    self.state.as_ref() as *const State as *mut State,
                );
                extensions::finish_xdg_output_batch(res, output);
            }
        }
    }

    /// The live backend-reported outputs for IPC and chrome.
    pub fn output_infos(&self) -> Vec<aegis_model::output::OutputInfo> {
        self.state.output_infos.clone()
    }

    /// Apply the master-stack tiling policy to the current workspace's
    /// windows when tiled mode is on (ADR-0024). Runs the layout over
    /// `work_area` and reconfigures only the windows whose target rect moved,
    /// so steady state sends no configure events. No-op when tiling is off.
    /// Apply the master-stack tiling policy to the current workspace's
    /// windows when tiled mode is on (ADR-0024). Runs the layout over
    /// `work_area` (the chrome-aware logical rect) and reconfigures only the
    /// windows whose target rect moved, so steady state sends no configure
    /// events. No-op when tiling is off.
    pub fn apply_tiling(&mut self, work_area: aegis_model::Rect) {
        self.state.last_work_area = work_area;
        let screen_rect = self.state.output_geometry.logical_rect();

        let mut flushed = false;
        for id in self.state.workspaces.visible_toplevels() {
            let rec = self.find_surface_by_window_id(id);
            if rec.is_null() {
                continue;
            }
            unsafe {
                if (*rec).xdg_toplevel.is_null() || !(*rec).mapped {
                    continue;
                }
                if (*rec).window.state.fullscreen {
                    if (*rec).saved_floating_rect.is_none() {
                        (*rec).saved_floating_rect = Some(aegis_model::Rect {
                            origin: (*rec).position,
                            size: (*rec).window.size,
                        });
                    }
                    if (*rec).layout_target != Some(screen_rect) {
                        let old = aegis_model::Rect {
                            origin: (*rec).position,
                            size: (*rec).window.size,
                        };
                        reposition_toplevel_with_popups(rec, screen_rect.origin);
                        (*rec).window.size = screen_rect.size;
                        (*rec).layout_target = Some(screen_rect);
                        self.note_transition(rec, old);
                        reconfigure_with_size(rec, screen_rect.size.w, screen_rect.size.h);
                        if !flushed {
                            ffi::wl_display_flush_clients(self.state.display);
                            flushed = true;
                        }
                    }
                } else if (*rec).window.state.maximized {
                    if (*rec).saved_floating_rect.is_none() {
                        (*rec).saved_floating_rect = Some(aegis_model::Rect {
                            origin: (*rec).position,
                            size: (*rec).window.size,
                        });
                    }
                    if (*rec).layout_target != Some(work_area) {
                        let old = aegis_model::Rect {
                            origin: (*rec).position,
                            size: (*rec).window.size,
                        };
                        reposition_toplevel_with_popups(rec, work_area.origin);
                        (*rec).window.size = work_area.size;
                        (*rec).layout_target = Some(work_area);
                        self.note_transition(rec, old);
                        reconfigure_with_size(rec, work_area.size.w, work_area.size.h);
                        if !flushed {
                            ffi::wl_display_flush_clients(self.state.display);
                            flushed = true;
                        }
                    }
                }
            }
        }

        if !self
            .state
            .workspaces
            .current_workspace_tiled(self.state.output)
        {
            return;
        }
        let tiled_ids: Vec<aegis_model::window::WindowId> = self
            .state
            .workspaces
            .visible_toplevels()
            .into_iter()
            .filter(|id| {
                let rec = self.find_surface_by_window_id(*id);
                !rec.is_null()
                    && unsafe {
                        let r = &(*rec).window;
                        r.layout_role == aegis_model::layout::LayoutRole::Tiled
                            && !r.state.maximized
                            && !r.state.fullscreen
                    }
            })
            .collect();
        let rects = aegis_model::layout::MasterStack.layout(
            work_area,
            tiled_ids.len(),
            &self.state.layout_params,
        );
        for (id, rect) in tiled_ids.iter().zip(rects.iter()) {
            let rec = self.find_surface_by_window_id(*id);
            if rec.is_null() {
                continue;
            }
            unsafe {
                if (*rec).xdg_toplevel.is_null() || !(*rec).mapped {
                    continue;
                }
                if (*rec).layout_target == Some(*rect) {
                    continue; // already at the target; do not reconfigure
                }
                let old = aegis_model::Rect {
                    origin: (*rec).position,
                    size: (*rec).window.size,
                };
                reposition_toplevel_with_popups(rec, rect.origin);
                (*rec).window.size = rect.size;
                (*rec).window.layout_role = aegis_model::layout::LayoutRole::Tiled;
                (*rec).layout_target = Some(*rect);
                self.note_transition(rec, old);
                reconfigure_with_size(rec, rect.size.w, rect.size.h);
                if !flushed {
                    ffi::wl_display_flush_clients(self.state.display);
                    flushed = true;
                }
            }
        }
    }

    /// If the keyboard-focused surface is not on a visible workspace, clear
    /// focus (post leave, deactivate). Idempotent.
    pub(crate) fn drop_focus_if_hidden(&mut self) {
        let visible = self.visible();
        let Some(wid) = self.focused_toplevel_id() else {
            return;
        };
        if !visible.contains(&wid) {
            self.change_keyboard_focus(std::ptr::null_mut());
        }
    }
}

/// Retarget a partially moved workspace strip without snapping it back to a
/// new two-page animation. Every retained page receives the same translation,
/// preserving the output-width spacing that makes page clips meet exactly.
fn retarget_workspace_strip(
    mut positions: Vec<(aegis_model::workspace::WorkspaceId, f32)>,
    outgoing: aegis_model::workspace::WorkspaceId,
    incoming: aegis_model::workspace::WorkspaceId,
    direction: i32,
    width: f32,
) -> Vec<WorkspaceSlideLayer> {
    if !positions
        .iter()
        .any(|(workspace, _)| *workspace == outgoing)
    {
        positions.push((outgoing, 0.0));
    }
    let outgoing_offset = positions
        .iter()
        .find(|(workspace, _)| *workspace == outgoing)
        .map(|(_, offset)| *offset)
        .unwrap_or(0.0);
    if !positions
        .iter()
        .any(|(workspace, _)| *workspace == incoming)
    {
        positions.push((
            incoming,
            outgoing_offset + direction.signum() as f32 * width,
        ));
    }
    let incoming_offset = positions
        .iter()
        .find(|(workspace, _)| *workspace == incoming)
        .map(|(_, offset)| *offset)
        .unwrap_or(0.0);
    let shift = -incoming_offset;
    positions
        .into_iter()
        .map(|(workspace, from_x)| WorkspaceSlideLayer {
            workspace,
            from_x,
            to_x: from_x + shift,
        })
        .collect()
}

fn stepped_index(current: usize, len: usize, forward: bool) -> usize {
    debug_assert!(len > 0);
    if forward {
        (current + 1) % len
    } else {
        (current + len - 1) % len
    }
}

fn reconcile_switcher_session(
    session: &mut WindowSwitcherSession,
    eligible: &std::collections::HashSet<aegis_model::window::WindowId>,
) {
    let selected_id = session.order.get(session.selected).copied();
    let old_index = session.selected.min(session.order.len().saturating_sub(1));
    session.order.retain(|id| eligible.contains(id));
    if session.order.is_empty() {
        session.selected = 0;
        return;
    }
    if let Some(index) =
        selected_id.and_then(|id| session.order.iter().position(|candidate| *candidate == id))
    {
        session.selected = index;
        return;
    }

    session.selected = if session.last_forward {
        old_index % session.order.len()
    } else {
        (old_index + session.order.len() - 1) % session.order.len()
    };
}

#[cfg(test)]
mod window_switcher_tests {
    use super::*;

    #[test]
    fn workspace_slide_moves_old_and_new_desktops_in_the_same_direction() {
        use aegis_model::workspace::WorkspaceId;

        let slide = WorkspaceSlide {
            output: aegis_model::Rect::new(0, 0, 1000, 800),
            layers: vec![
                WorkspaceSlideLayer {
                    workspace: WorkspaceId(1),
                    from_x: 0.0,
                    to_x: -1000.0,
                },
                WorkspaceSlideLayer {
                    workspace: WorkspaceId(2),
                    from_x: 1000.0,
                    to_x: 0.0,
                },
            ],
            started_ms: 10,
            duration_ms: 100,
        };
        assert_eq!(slide.offset_at(WorkspaceId(1), 10), Some(0.0));
        assert_eq!(slide.offset_at(WorkspaceId(2), 10), Some(1000.0));
        let outgoing_mid = slide.offset_at(WorkspaceId(1), 60).unwrap();
        let incoming_mid = slide.offset_at(WorkspaceId(2), 60).unwrap();
        assert!(outgoing_mid < 0.0);
        assert!(incoming_mid > 0.0);
        assert!((incoming_mid - outgoing_mid - 1000.0).abs() < f32::EPSILON);
        assert_eq!(slide.offset_at(WorkspaceId(1), 110), None);
        assert_eq!(slide.offset_at(WorkspaceId(2), 110), None);
    }

    #[test]
    fn workspace_strip_retargets_without_breaking_page_spacing() {
        use aegis_model::workspace::WorkspaceId;

        let layers = retarget_workspace_strip(
            vec![(WorkspaceId(1), -500.0), (WorkspaceId(2), 500.0)],
            WorkspaceId(2),
            WorkspaceId(3),
            1,
            1000.0,
        );
        assert_eq!(layers.len(), 3);
        assert_eq!(layers[0].to_x, -2000.0);
        assert_eq!(layers[1].to_x, -1000.0);
        assert_eq!(layers[2].to_x, 0.0);
    }

    #[test]
    fn reversing_workspace_slide_keeps_continuous_positions() {
        use aegis_model::workspace::WorkspaceId;

        let layers = retarget_workspace_strip(
            vec![(WorkspaceId(1), -400.0), (WorkspaceId(2), 600.0)],
            WorkspaceId(2),
            WorkspaceId(1),
            -1,
            1000.0,
        );
        assert_eq!(layers[0].from_x, -400.0);
        assert_eq!(layers[1].from_x, 600.0);
        assert_eq!(layers[0].to_x, 0.0);
        assert_eq!(layers[1].to_x, 1000.0);
    }

    #[test]
    fn frozen_order_cycles_both_directions() {
        assert_eq!(stepped_index(0, 4, true), 1);
        assert_eq!(stepped_index(3, 4, true), 0);
        assert_eq!(stepped_index(0, 4, false), 3);
        assert_eq!(stepped_index(2, 4, false), 1);
    }

    #[test]
    fn rebuilding_mru_after_one_step_toggles_back() {
        use aegis_model::window::WindowId;

        // Bottom-to-top stacking order; C starts focused.
        let mut stack = vec![WindowId(1), WindowId(2), WindowId(3)];
        let first_mru: Vec<_> = stack.iter().rev().copied().collect();
        let target = first_mru[stepped_index(0, first_mru.len(), true)];
        assert_eq!(target, WindowId(2));

        stack.retain(|id| *id != target);
        stack.push(target);
        let second_mru: Vec<_> = stack.iter().rev().copied().collect();
        let target = second_mru[stepped_index(0, second_mru.len(), true)];
        assert_eq!(target, WindowId(3));
    }

    #[test]
    fn closing_the_selection_chooses_the_neighbour_in_the_last_direction() {
        use aegis_model::window::WindowId;
        use std::collections::HashSet;

        let eligible = HashSet::from([WindowId(1), WindowId(3), WindowId(4)]);
        let mut forward = WindowSwitcherSession {
            order: vec![WindowId(1), WindowId(2), WindowId(3), WindowId(4)],
            selected: 1,
            last_forward: true,
        };
        reconcile_switcher_session(&mut forward, &eligible);
        assert_eq!(forward.order[forward.selected], WindowId(3));

        let mut backward = WindowSwitcherSession {
            order: vec![WindowId(1), WindowId(2), WindowId(3), WindowId(4)],
            selected: 1,
            last_forward: false,
        };
        reconcile_switcher_session(&mut backward, &eligible);
        assert_eq!(backward.order[backward.selected], WindowId(1));
    }

    #[test]
    fn refreshing_a_session_never_inserts_new_windows() {
        use aegis_model::window::WindowId;
        use std::collections::HashSet;

        let mut session = WindowSwitcherSession {
            order: vec![WindowId(1), WindowId(2)],
            selected: 0,
            last_forward: true,
        };
        let eligible = HashSet::from([WindowId(1), WindowId(2), WindowId(3)]);
        reconcile_switcher_session(&mut session, &eligible);
        assert_eq!(session.order, vec![WindowId(1), WindowId(2)]);
    }
}
