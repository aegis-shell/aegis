use crate::*;

impl Server {
    fn switcher_candidates(&self) -> Vec<aegis_core::window::WindowId> {
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
        Vec<aegis_core::window::WindowId>,
        Option<aegis_core::window::WindowId>,
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
    pub fn switch_workspace(&mut self, dir: aegis_core::workspace::Switch) {
        self.state.workspaces.switch(self.state.output, dir);
        self.drop_focus_if_hidden();
        unsafe { ffi::wl_display_flush_clients(self.state.display) };
    }

    /// Switch directly to a workspace by id on the output that owns it. Same
    /// focus-drop contract as [`switch_workspace`](Self::switch_workspace).
    pub fn switch_workspace_to(&mut self, id: aegis_core::workspace::WorkspaceId) {
        self.state.workspaces.switch_to(id);
        self.drop_focus_if_hidden();
        unsafe { ffi::wl_display_flush_clients(self.state.display) };
    }

    /// Move a toplevel to a workspace by id (ADR-0025). If the target is not
    /// the current workspace, the window leaves the visible set. No-op if the
    /// window or workspace is unknown, or the physical session only observes
    /// the window.
    pub fn move_to_workspace(
        &mut self,
        window_id: aegis_core::window::WindowId,
        workspace: aegis_core::workspace::WorkspaceId,
    ) {
        if !self.human_controls_window(window_id) {
            return;
        }
        self.state.workspaces.move_toplevel(window_id, workspace);
        self.drop_focus_if_hidden();
    }

    /// The workspace/output snapshot for the IPC and chrome (ADR-0025/0027).
    pub fn workspace_snapshot(&self) -> aegis_core::workspace::WorkspaceSnapshot {
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
            aegis_core::layout::LayoutRole::Tiled
        } else {
            aegis_core::layout::LayoutRole::Floating
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
    pub fn set_window_rules(&mut self, rules: Vec<aegis_core::window_rule::WindowRule>) {
        self.state.window_rules = rules;
    }

    /// Set whether window positions and geometries are remembered across restarts.
    pub fn set_remember_window_positions(&mut self, remember: bool) {
        self.state.remember_window_positions = remember;
    }

    /// Replace the tiling layout parameters (gaps, master ratio) from the
    /// config (ADR-0024/0026). Applied on the next `apply_tiling`.
    pub fn set_layout_params(&mut self, params: aegis_core::layout::LayoutParams) {
        self.state.layout_params = params;
    }

    /// Set the reduced-motion policy (ADR-0029, from `[ui] reduced_motion`).
    /// When enabled, in-flight transitions resolve immediately and no new
    /// ones are recorded.
    pub fn set_reduced_motion(&mut self, reduced: bool) {
        self.state.reduced_motion = reduced;
        if reduced {
            for p in self.state.live_surfaces() {
                unsafe { (*p).window.transition = None };
            }
        }
    }

    /// Apply the desktop-wide xdg-decoration ownership policy.
    ///
    /// Existing decoration-aware toplevels receive a fresh decoration
    /// configure followed by the required xdg-surface configure, so config
    /// reload changes take effect without restarting applications.
    pub fn set_decoration_policy(&mut self, policy: aegis_core::window::DecorationPolicy) {
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

    /// Record a geometry transition for a non-interactive rect change
    /// (tiling, IPC geometry). The model moves to the target immediately;
    /// rendering interpolates from the current on-screen rect — the previous
    /// transition's mid-flight rect when changes come faster than the
    /// duration, else the previous model rect (ADR-0029).
    pub(crate) fn note_transition(&self, rec: *mut SurfaceRec, old: aegis_core::Rect) {
        if self.state.reduced_motion || rec.is_null() {
            return;
        }
        let now = self.now_ms();
        unsafe {
            let target = aegis_core::Rect {
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
                Some(aegis_core::transition::WindowTransition::new(from, now));
        }
    }

    /// The rect a surface renders at this frame: `Some(interpolated)` while
    /// its transition is in flight, `None` at the model target (ADR-0029).
    pub(crate) fn transition_render_rect(&self, s: &SurfaceRec) -> Option<aegis_core::Rect> {
        let target = aegis_core::Rect {
            origin: s.position,
            size: s.window.size,
        };
        s.window
            .transition
            .and_then(|t| t.rect_at(target, self.now_ms()))
    }

    /// Whether any toplevel has a transition still in flight — the main loop
    /// keeps ticking frames at cadence instead of blocking on the host queue.
    pub fn transitions_pending(&self) -> bool {
        self.state.live_surfaces().any(|p| unsafe {
            !(*p).xdg_toplevel.is_null() && self.transition_render_rect(&*p).is_some()
        })
    }

    /// Reconcile connector identities and geometries reported by the backend.
    /// Existing connector workspaces survive reordering; removed outputs are
    /// relocated by `WorkspaceModel`, and a replug restores their origin.
    pub fn set_outputs(&mut self, mut outputs: Vec<aegis_core::output::OutputInfo>) {
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
                output.geometry.scale = aegis_core::output::Scale(scale as f32);
            }
            if let Some(position) = policy.position {
                output.geometry.logical_origin = position;
            }
            if let Some(transform) = policy.transform
                && transform != aegis_core::Transform::Normal
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

    /// Set per-connector output policies from the config's `[[output]]`
    /// entries (ADR-0028), and re-apply them to the current output set.
    /// Unmatched connectors are ignored with a log line, so a monitor that is
    /// not plugged in yet still applies once it appears.
    pub fn set_output_policies(
        &mut self,
        policies: std::collections::HashMap<String, aegis_core::output::OutputPolicy>,
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
    pub fn set_output_geometry(&mut self, geo: aegis_core::output::OutputGeometry) {
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
    pub fn output_logical_rect(&self) -> aegis_core::Rect {
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
    pub fn output_infos(&self) -> Vec<aegis_core::output::OutputInfo> {
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
    pub fn apply_tiling(&mut self, work_area: aegis_core::Rect) {
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
                        (*rec).saved_floating_rect = Some(aegis_core::Rect {
                            origin: (*rec).position,
                            size: (*rec).window.size,
                        });
                    }
                    if (*rec).layout_target != Some(screen_rect) {
                        let old = aegis_core::Rect {
                            origin: (*rec).position,
                            size: (*rec).window.size,
                        };
                        (*rec).position = screen_rect.origin;
                        (*rec).window.position = screen_rect.origin;
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
                        (*rec).saved_floating_rect = Some(aegis_core::Rect {
                            origin: (*rec).position,
                            size: (*rec).window.size,
                        });
                    }
                    if (*rec).layout_target != Some(work_area) {
                        let old = aegis_core::Rect {
                            origin: (*rec).position,
                            size: (*rec).window.size,
                        };
                        (*rec).position = work_area.origin;
                        (*rec).window.position = work_area.origin;
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
        let tiled_ids: Vec<aegis_core::window::WindowId> = self
            .state
            .workspaces
            .visible_toplevels()
            .into_iter()
            .filter(|id| {
                let rec = self.find_surface_by_window_id(*id);
                !rec.is_null()
                    && unsafe {
                        let r = &(*rec).window;
                        r.layout_role == aegis_core::layout::LayoutRole::Tiled
                            && !r.state.maximized
                            && !r.state.fullscreen
                    }
            })
            .collect();
        let rects = aegis_core::layout::MasterStack.layout(
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
                let old = aegis_core::Rect {
                    origin: (*rec).position,
                    size: (*rec).window.size,
                };
                (*rec).position = rect.origin;
                (*rec).window.position = rect.origin;
                (*rec).window.size = rect.size;
                (*rec).window.layout_role = aegis_core::layout::LayoutRole::Tiled;
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
    eligible: &std::collections::HashSet<aegis_core::window::WindowId>,
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
    fn frozen_order_cycles_both_directions() {
        assert_eq!(stepped_index(0, 4, true), 1);
        assert_eq!(stepped_index(3, 4, true), 0);
        assert_eq!(stepped_index(0, 4, false), 3);
        assert_eq!(stepped_index(2, 4, false), 1);
    }

    #[test]
    fn rebuilding_mru_after_one_step_toggles_back() {
        use aegis_core::window::WindowId;

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
        use aegis_core::window::WindowId;
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
        use aegis_core::window::WindowId;
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
