//! Workspace and output model.
//!
//! A pure, backend- and renderer-agnostic model of dynamic per-output
//! workspaces, implementing [ADR-0025](../../docs/adr/0025-workspace-model.md).
//! It owns the invariants the chrome, the renderer, the IPC, and the agent
//! layer all read; the server drives it on map/unmap/switch, the renderer
//! asks which toplevels are visible, the IPC addresses workspaces by stable
//! id.
//!
//! # Model
//!
//! - An [`Output`] owns an ordered list of [`WorkspaceId`]s and a current
//!   index. Outputs are ordered; index 0 is treated as primary.
//! - A [`Workspace`] owns the [`Window`](crate::window::Window)::`id`s
//!   placed on it, in z-order, plus the [`OutputId`] it belongs to.
//! - A toplevel belongs to exactly one workspace on exactly one output.
//!
//! # Invariants (GNOME/niri-style dynamic workspaces)
//!
//! - **B (trailing empty):** the last workspace on each output is always
//!   empty. Placing a toplevel on the last workspace appends a fresh empty
//!   one after it, so there is always a blank workspace ahead.
//! - **Reap:** an empty workspace that is neither the current one nor the
//!   last is removed, so the list stays tight. Reap runs after
//!   [`WorkspaceModel::remove_toplevel`] and after a switch.
//!
//! # Out of scope (follow-ups)
//!
//! Workspace *replug-restore* — moving workspaces back to a reconnected
//! output when its connector returns — is not yet implemented;
//! [`WorkspaceModel::remove_output`] relocates workspaces to a survivor and
//! forgets the origin. Restore lands when the multi-output milestone
//! ([ADR-0028](../../docs/adr/0028-output-and-monitor-model.md)) exercises
//! real hotplug.

use crate::window::WindowId;
use std::collections::HashMap;

/// Stable identifier for a workspace, opaque to chrome/IPC/agent. Lives as
/// long as the workspace does; the visible "number" is chrome presentation.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WorkspaceId(pub u64);

/// Stable identifier for an output.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OutputId(pub u64);

/// One workspace: a stable id, the output it belongs to, and the toplevels
/// placed on it in z-order (front-to-back). Toplevel ids are
/// [`Window::id`](crate::window::Window::id) values, matching the
/// renderer/IPC convention.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone)]
pub struct Workspace {
    pub id: WorkspaceId,
    pub output: OutputId,
    /// The connector this workspace was created on — its "home" output,
    /// stable across unplug/replug (ADR-0025). A workspace displaced by an
    /// output removal keeps its origin; when the same connector returns, the
    /// workspace moves back to it.
    pub origin: String,
    pub toplevels: Vec<WindowId>,
    /// Whether this workspace is in tiled mode (ADR-0024 per-workspace
    /// layout): when true, the master-stack policy lays out its `Tiled`-role
    /// windows. Per-workspace, so one workspace can tile while another floats.
    pub tiled: bool,
    /// Optional user-facing label. The visible number is the chrome's
    /// presentation; this is a name a user or the IPC may set.
    pub label: Option<String>,
}

/// One output and the workspaces it currently owns.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone)]
pub struct Output {
    pub id: OutputId,
    /// Stable connector identity (e.g. "HDMI-A-1", "nested"). Two outputs
    /// should not share a connector; replug reuses it so the model can
    /// restore a disconnected output's workspaces (ADR-0025).
    pub connector: String,
    /// Ordered workspace ids; the last is always empty (invariant B).
    pub workspaces: Vec<WorkspaceId>,
    /// Index into `workspaces` of the workspace currently shown.
    pub current: usize,
}

/// An owned, serializable snapshot of the whole workspace model at an
/// instant — what the IPC sends ([`WorkspaceModel::snapshot`]) and the
/// chrome renders. Decoupled from the live [`Output`]/[`Workspace`] structs
/// so consumers read a consistent point-in-time view.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceSnapshot {
    pub outputs: Vec<OutputSnapshot>,
}

/// One output's view within a [`WorkspaceSnapshot`].
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputSnapshot {
    pub id: OutputId,
    /// The stable connector name (ADR-0025/0028).
    pub connector: String,
    /// The workspace currently shown on this output. `None` only if the
    /// output has no workspaces (the invariant keeps it `Some` in practice).
    pub current: Option<WorkspaceId>,
    pub workspaces: Vec<WorkspaceEntry>,
}

/// One workspace within a [`WorkspaceSnapshot`].
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceEntry {
    pub id: WorkspaceId,
    pub label: Option<String>,
    /// Whether this workspace is in tiled mode (ADR-0024).
    pub tiled: bool,
    /// Toplevel ids on this workspace, in z-order.
    pub toplevels: Vec<WindowId>,
}

/// Relative switch direction.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(tag = "type"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Switch {
    Next,
    Prev,
}

/// The workspace model: the pure, testable brain that owns outputs,
/// workspaces, and the dynamic invariants (ADR-0025). No flux, lens, or
/// Wayland dependency.
#[derive(Debug, Default)]
pub struct WorkspaceModel {
    /// Ordered outputs; index 0 is primary (the relocation target).
    outputs: Vec<Output>,
    /// Every live workspace, keyed by id.
    workspaces: HashMap<WorkspaceId, Workspace>,
    next_workspace_id: u64,
    next_output_id: u64,
}

impl WorkspaceModel {
    /// An empty model with no outputs.
    pub fn new() -> WorkspaceModel {
        WorkspaceModel::default()
    }

    /// Add an output identified by `connector` (stable across unplug/replug,
    /// ADR-0025) with one empty workspace. If any workspaces displaced from a
    /// previously-removed output of the same connector are parked elsewhere,
    /// they move back here (replug-restore).
    pub fn add_output(&mut self, connector: &str) -> OutputId {
        let oid = self.alloc_output();
        let wid = self.fresh_workspace(connector, oid);
        self.outputs.push(Output {
            id: oid,
            connector: connector.to_string(),
            workspaces: vec![wid],
            current: 0,
        });
        let new_oi = self.outputs.len() - 1;
        // Reclaim workspaces whose home connector is this one (replug-restore).
        self.restore_originating(connector, new_oi);
        self.ensure_trailing_empty(new_oi);
        self.reap_output(new_oi);
        oid
    }

    /// Move every workspace whose `origin` is `connector` and that currently
    /// lives on another output onto the output at `dest_oi` (its returned
    /// home). Used by [`Self::add_output`] to restore a replugged output.
    fn restore_originating(&mut self, connector: &str, dest_oi: usize) {
        let dest_oid = self.outputs[dest_oi].id;
        // (workspace id, its current output index) for each displaced
        // workspace that calls this connector home.
        let moves: Vec<(WorkspaceId, usize)> = self
            .workspaces
            .iter()
            .filter(|(_, ws)| ws.origin == connector && ws.output != dest_oid)
            .map(|(wid, ws)| (*wid, self.output_index(ws.output).unwrap_or(dest_oi)))
            .collect();
        if moves.is_empty() {
            return;
        }
        let moved: Vec<WorkspaceId> = moves.iter().map(|(w, _)| *w).collect();
        let mut sources: Vec<usize> = moves.iter().map(|(_, o)| *o).collect();
        sources.sort_unstable();
        sources.dedup();

        // Reassign to the destination and append to its workspace list.
        for wid in &moved {
            if let Some(ws) = self.workspaces.get_mut(wid) {
                ws.output = dest_oid;
            }
        }
        self.outputs[dest_oi].workspaces.extend(moved.iter());

        // Drop them from each source, fixing `current` and the invariants.
        for src_oi in sources {
            if src_oi == dest_oi {
                continue;
            }
            let cur_id = {
                let o = &self.outputs[src_oi];
                o.workspaces.get(o.current).copied()
            };
            let o = &mut self.outputs[src_oi];
            o.workspaces.retain(|w| !moved.contains(w));
            o.current = match cur_id {
                Some(id) if o.workspaces.contains(&id) => {
                    o.workspaces.iter().position(|w| *w == id).unwrap_or(0)
                }
                _ => 0,
            };
            self.reap_output(src_oi);
            self.ensure_trailing_empty(src_oi);
        }
    }

    /// Remove an output. Its non-empty workspaces relocate to the first
    /// surviving output (primary first), keeping their `origin` so a later
    /// [`Self::add_output`] of the same connector restores them; empty ones
    /// (the scratch) are dropped. If it was the last output, the workspaces
    /// are dropped too (nowhere to put them). Returns the number of
    /// workspaces relocated.
    pub fn remove_output(&mut self, oid: OutputId) -> usize {
        let Some(oi) = self.output_index(oid) else {
            return 0;
        };
        let ws_ids: Vec<WorkspaceId> = self.outputs[oi].workspaces.clone();
        self.outputs.remove(oi);
        if self.outputs.is_empty() {
            for wid in &ws_ids {
                self.workspaces.remove(wid);
            }
            return 0;
        }
        // Relocate non-empty workspaces to the primary survivor; drop empty.
        let survivor_oi = 0;
        let survivor_oid = self.outputs[survivor_oi].id;
        let mut relocated = 0;
        for wid in &ws_ids {
            let empty = self
                .workspaces
                .get(wid)
                .map(|ws| ws.toplevels.is_empty())
                .unwrap_or(true);
            if empty {
                self.workspaces.remove(wid);
                continue;
            }
            if let Some(ws) = self.workspaces.get_mut(wid) {
                ws.output = survivor_oid;
            }
            self.outputs[survivor_oi].workspaces.push(*wid);
            relocated += 1;
        }
        self.ensure_trailing_empty(survivor_oi);
        self.reap_output(survivor_oi);
        relocated
    }

    /// All outputs, in order (index 0 is primary).
    pub fn outputs(&self) -> &[Output] {
        &self.outputs
    }

    /// Look up a workspace by id.
    pub fn workspace(&self, wid: WorkspaceId) -> Option<&Workspace> {
        self.workspaces.get(&wid)
    }

    /// Look up an output by id.
    pub fn output(&self, oid: OutputId) -> Option<&Output> {
        self.outputs.iter().find(|o| o.id == oid)
    }

    /// The workspace currently shown on `oid`, if any.
    pub fn current_workspace(&self, oid: OutputId) -> Option<WorkspaceId> {
        let o = self.output(oid)?;
        o.workspaces.get(o.current).copied()
    }

    /// Whether the workspace currently shown on `oid` is in tiled mode
    /// (ADR-0024 per-workspace layout). `false` if the output or workspace
    /// is unknown.
    pub fn current_workspace_tiled(&self, oid: OutputId) -> bool {
        self.current_workspace(oid)
            .and_then(|wid| self.workspaces.get(&wid))
            .map(|ws| ws.tiled)
            .unwrap_or(false)
    }

    /// Set a workspace's tiled flag (ADR-0024). No-op if the workspace is
    /// unknown.
    pub fn set_tiled(&mut self, wid: WorkspaceId, tiled: bool) {
        if let Some(ws) = self.workspaces.get_mut(&wid) {
            ws.tiled = tiled;
        }
    }

    /// Switch the current workspace on `oid` by a relative step, clamped to
    /// the available range. Reaps the workspace left behind if it is empty.
    pub fn switch(&mut self, oid: OutputId, dir: Switch) -> Option<WorkspaceId> {
        let oi = self.output_index(oid)?;
        let len = self.outputs[oi].workspaces.len();
        if len == 0 {
            return None;
        }
        let prev = self.outputs[oi].current;
        let new = match dir {
            Switch::Next => (prev + 1).min(len - 1),
            Switch::Prev => prev.saturating_sub(1),
        };
        self.outputs[oi].current = new;
        self.reap_output(oi);
        self.current_workspace(oid)
    }

    /// Switch directly to a specific workspace on its own output. No-op (and
    /// returns `None`) if the id is unknown.
    pub fn switch_to(&mut self, wid: WorkspaceId) -> Option<WorkspaceId> {
        let oid = self.workspaces.get(&wid)?.output;
        let oi = self.output_index(oid)?;
        let idx = self.outputs[oi].workspaces.iter().position(|w| *w == wid)?;
        self.outputs[oi].current = idx;
        self.reap_output(oi);
        Some(wid)
    }

    /// Place `toplevel` on workspace `wid`, moving it from wherever it was.
    /// Restores invariant B on the target (appending a fresh empty workspace
    /// if the target was the last) and reaps an emptied source workspace.
    pub fn place_toplevel(&mut self, wid: WorkspaceId, toplevel: WindowId) {
        let src = self.workspace_of(toplevel);
        let src_oi = src.and_then(|s| self.output_of_workspace(s));
        if let Some(s) = src {
            if let Some(ws) = self.workspaces.get_mut(&s) {
                ws.toplevels.retain(|&t| t != toplevel);
            }
        }
        if let Some(ws) = self.workspaces.get_mut(&wid) {
            ws.toplevels.push(toplevel);
        }
        if let Some(oi) = self.output_of_workspace(wid) {
            self.ensure_trailing_empty(oi);
        }
        if let Some(oi) = src_oi {
            self.reap_output(oi);
        }
    }

    /// Move `toplevel` onto `target`; convenience over
    /// [`Self::place_toplevel`].
    pub fn move_toplevel(&mut self, toplevel: WindowId, target: WorkspaceId) {
        self.place_toplevel(target, toplevel);
    }

    /// Remove `toplevel` entirely (on close/unmap). Reaps its workspace if it
    /// emptied and is neither current nor last.
    pub fn remove_toplevel(&mut self, toplevel: WindowId) {
        let Some(swid) = self.workspace_of(toplevel) else {
            return;
        };
        let oi = self.output_of_workspace(swid);
        if let Some(ws) = self.workspaces.get_mut(&swid) {
            ws.toplevels.retain(|&t| t != toplevel);
        }
        if let Some(oi) = oi {
            self.reap_output(oi);
        }
    }

    /// Which workspace `toplevel` is on, if any.
    pub fn workspace_of(&self, toplevel: WindowId) -> Option<WorkspaceId> {
        self.workspaces
            .iter()
            .find(|(_, ws)| ws.toplevels.contains(&toplevel))
            .map(|(wid, _)| *wid)
    }

    /// Every toplevel currently visible: the union, in output order, of the
    /// toplevels on each output's current workspace.
    pub fn visible_toplevels(&self) -> Vec<WindowId> {
        let mut v = Vec::new();
        for o in &self.outputs {
            if let Some(&wid) = o.workspaces.get(o.current) {
                if let Some(ws) = self.workspaces.get(&wid) {
                    v.extend_from_slice(&ws.toplevels);
                }
            }
        }
        v
    }

    /// An owned, serializable snapshot of the whole model: outputs, the
    /// current workspace per output, and every workspace's label and
    /// toplevels. The IPC sends this verbatim; the chrome renders it.
    pub fn snapshot(&self) -> WorkspaceSnapshot {
        let outputs = self
            .outputs
            .iter()
            .map(|o| OutputSnapshot {
                id: o.id,
                connector: o.connector.clone(),
                current: o.workspaces.get(o.current).copied(),
                workspaces: o
                    .workspaces
                    .iter()
                    .map(|&wid| {
                        let ws = self.workspaces.get(&wid);
                        WorkspaceEntry {
                            id: wid,
                            label: ws.and_then(|w| w.label.clone()),
                            tiled: ws.map(|w| w.tiled).unwrap_or(false),
                            toplevels: ws.map(|w| w.toplevels.clone()).unwrap_or_default(),
                        }
                    })
                    .collect(),
            })
            .collect();
        WorkspaceSnapshot { outputs }
    }

    // ----- internals -----------------------------------------------------

    fn alloc_output(&mut self) -> OutputId {
        let oid = OutputId(self.next_output_id);
        self.next_output_id += 1;
        oid
    }

    fn fresh_workspace(&mut self, connector: &str, oid: OutputId) -> WorkspaceId {
        let wid = WorkspaceId(self.next_workspace_id);
        self.next_workspace_id += 1;
        self.workspaces.insert(
            wid,
            Workspace {
                id: wid,
                output: oid,
                origin: connector.to_string(),
                toplevels: Vec::new(),
                tiled: false,
                label: None,
            },
        );
        wid
    }

    fn output_index(&self, oid: OutputId) -> Option<usize> {
        self.outputs.iter().position(|o| o.id == oid)
    }

    fn output_of_workspace(&self, wid: WorkspaceId) -> Option<usize> {
        self.workspaces
            .get(&wid)
            .and_then(|ws| self.output_index(ws.output))
    }

    /// Invariant B: if the output's last workspace is non-empty, append a
    /// fresh empty one.
    fn ensure_trailing_empty(&mut self, oi: usize) {
        let need = {
            let o = &self.outputs[oi];
            let last = o.workspaces.last().expect("output always has ≥1 workspace");
            !self.workspaces[last].toplevels.is_empty()
        };
        if need {
            let connector = self.outputs[oi].connector.clone();
            let oid = self.outputs[oi].id;
            let wid = self.fresh_workspace(&connector, oid);
            self.outputs[oi].workspaces.push(wid);
        }
    }

    /// Reap empty workspaces that are neither the current one nor the last
    /// on `oi`. Keeps the list tight. Never removes the current or last.
    fn reap_output(&mut self, oi: usize) {
        let (cur_id, to_drop) = {
            let o = &self.outputs[oi];
            let last = o.workspaces.len() - 1;
            let cur_id = o.workspaces[o.current];
            let to_drop: Vec<WorkspaceId> = o
                .workspaces
                .iter()
                .enumerate()
                .filter(|(i, wid)| {
                    *i != last && *i != o.current && self.workspaces[wid].toplevels.is_empty()
                })
                .map(|(_, wid)| *wid)
                .collect();
            (cur_id, to_drop)
        };
        if to_drop.is_empty() {
            return;
        }
        for wid in &to_drop {
            self.workspaces.remove(wid);
        }
        let o = &mut self.outputs[oi];
        o.workspaces.retain(|wid| !to_drop.contains(wid));
        // current survived (it was excluded); recompute its new index.
        o.current = o.workspaces.iter().position(|w| *w == cur_id).unwrap_or(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh model with one output; returns its id and its sole workspace id.
    fn one_output() -> (WorkspaceModel, OutputId, WorkspaceId) {
        let mut m = WorkspaceModel::new();
        let oid = m.add_output("a");
        let wid = m.current_workspace(oid).unwrap();
        (m, oid, wid)
    }

    #[test]
    fn new_output_has_one_empty_workspace() {
        let (m, oid, wid) = one_output();
        let o = m.output(oid).unwrap();
        assert_eq!(o.workspaces.len(), 1);
        assert_eq!(o.current, 0);
        assert!(m.workspace(wid).unwrap().toplevels.is_empty());
        assert!(m.visible_toplevels().is_empty());
    }

    #[test]
    fn placing_on_the_last_appends_a_fresh_empty() {
        let (mut m, oid, wid) = one_output();
        m.place_toplevel(wid, WindowId(100));
        // Invariant B: a new empty workspace now trails the filled one.
        let o = m.output(oid).unwrap();
        assert_eq!(o.workspaces.len(), 2);
        let last = *o.workspaces.last().unwrap();
        assert!(m.workspace(last).unwrap().toplevels.is_empty());
        assert_eq!(m.workspace(wid).unwrap().toplevels, vec![WindowId(100)]);
        // The visible set is the current workspace's toplevels.
        assert_eq!(m.visible_toplevels(), vec![WindowId(100)]);
    }

    #[test]
    fn switch_clamps_at_the_empty_trailing_workspace() {
        let (mut m, oid, wid) = one_output();
        m.place_toplevel(wid, WindowId(1)); // now 2 workspaces: [full, empty]
                                            // On the first (current). Next → index 1 (the empty one). Next again
                                            // clamps; it does not create a third.
        assert_eq!(
            m.switch(oid, Switch::Next),
            m.output(oid).unwrap().workspaces.get(1).copied()
        );
        let before = m.output(oid).unwrap().workspaces.len();
        m.switch(oid, Switch::Next);
        assert_eq!(m.output(oid).unwrap().workspaces.len(), before);
        // Prev returns to index 0; Prev again clamps at 0.
        m.switch(oid, Switch::Prev);
        assert_eq!(m.output(oid).unwrap().current, 0);
        m.switch(oid, Switch::Prev);
        assert_eq!(m.output(oid).unwrap().current, 0);
    }

    #[test]
    fn remove_toplevel_reaps_an_emptied_non_current_workspace() {
        let (mut m, oid, wid) = one_output();
        // Build [ws0=[1], ws1=[2], ws2=[]] with current on ws0.
        m.place_toplevel(wid, WindowId(1)); // ws0=[1], trailing ws1 appended
        let mid = m.switch(oid, Switch::Next).unwrap(); // current = ws1 (empty)
        m.place_toplevel(mid, WindowId(2)); // ws1=[2], trailing ws2 appended
        m.switch(oid, Switch::Prev).unwrap(); // current back to ws0
        assert_eq!(m.output(oid).unwrap().workspaces.len(), 3);
        // Removing the toplevel on the non-current, non-last ws1 empties it →
        // reaped. Back to [ws0=[1], ws2=[]].
        m.remove_toplevel(WindowId(2));
        assert_eq!(m.output(oid).unwrap().workspaces.len(), 2);
        assert!(m.workspace_of(WindowId(2)).is_none());
    }

    #[test]
    fn current_and_last_empty_workspaces_are_kept() {
        let (mut m, oid, wid) = one_output();
        m.place_toplevel(wid, WindowId(1)); // [1], []
        let empty = m.switch(oid, Switch::Next).unwrap(); // current = empty
                                                          // Removing the only toplevel leaves ws0 empty but it is NOT current
                                                          // and NOT last → reaped. Wait: current is `empty` (the last), ws0 is
                                                          // neither current nor last → reaped. Result: just [empty] remains?
                                                          // No: last is always kept; ws0 reaped → [empty] becomes the only,
                                                          // which is then the last. current recomputed to 0.
        let _ = empty;
        m.remove_toplevel(WindowId(1));
        let o = m.output(oid).unwrap();
        assert_eq!(
            o.workspaces.len(),
            1,
            "reaped ws0; one empty workspace remains"
        );
        assert!(m.workspace(o.workspaces[0]).unwrap().toplevels.is_empty());
        assert_eq!(o.current, 0);
    }

    #[test]
    fn move_toplevel_changes_workspace() {
        let (mut m, oid, wid) = one_output();
        m.place_toplevel(wid, WindowId(5));
        let second = m.switch(oid, Switch::Next).unwrap();
        m.move_toplevel(WindowId(5), second);
        assert_eq!(m.workspace_of(WindowId(5)), Some(second));
        // ws0 emptied and is not current (current still on second? no —
        // switch moved current to second). ws0 not current, not last → reaped.
        let o = m.output(oid).unwrap();
        // After moving 5 onto `second` (the last), a fresh trailing empty is
        // appended. ws0 reaped. So: [second(5)], [].
        assert_eq!(o.workspaces.len(), 2);
    }

    #[test]
    fn switch_to_jumps_directly() {
        let (mut m, oid, _wid) = one_output();
        // Make several workspaces by filling.
        let mut ws = vec![_wid];
        let mut cur = _wid;
        for i in 0..3u64 {
            m.place_toplevel(cur, WindowId(100 + i));
            cur = m.switch(oid, Switch::Next).unwrap();
            ws.push(cur);
        }
        // Jump back to the first.
        m.switch_to(ws[0]);
        assert_eq!(m.output(oid).unwrap().current, 0);
        assert_eq!(m.current_workspace(oid), Some(ws[0]));
    }

    #[test]
    fn multiple_outputs_are_independent() {
        let mut m = WorkspaceModel::new();
        let a = m.add_output("a");
        let b = m.add_output("b");
        let wa = m.current_workspace(a).unwrap();
        let wb = m.current_workspace(b).unwrap();
        m.place_toplevel(wa, WindowId(1));
        m.place_toplevel(wb, WindowId(2));
        // Visible is the union of both current workspaces.
        assert_eq!(m.visible_toplevels(), vec![WindowId(1), WindowId(2)]);
        // Switching one output does not affect the other.
        m.switch(a, Switch::Next);
        assert_eq!(m.visible_toplevels(), vec![WindowId(2)]); // a now on its empty
    }

    #[test]
    fn remove_output_relocates_non_empty_workspaces() {
        let mut m = WorkspaceModel::new();
        let primary = m.add_output("a");
        let second = m.add_output("b");
        let ws_p = m.current_workspace(primary).unwrap();
        let ws_s = m.current_workspace(second).unwrap();
        m.place_toplevel(ws_p, WindowId(7));
        m.place_toplevel(ws_s, WindowId(9)); // lives on the second output
                                             // Remove the second output: ws_s (non-empty) relocates to primary as
                                             // its own workspace (ADR-0025: workspaces move, they don't merge).
        let relocated = m.remove_output(second);
        assert_eq!(relocated, 1);
        assert!(m.output(second).is_none());
        assert_eq!(m.outputs().len(), 1);
        // Toplevel 9 is retained on the primary output (not lost). It sits on
        // a relocated workspace that is not the current one, so it is not
        // immediately visible — switching to it shows it.
        let ws9 = m.workspace_of(WindowId(9)).expect("toplevel 9 retained");
        assert_eq!(m.workspace(ws9).unwrap().output, primary);
        // Toplevel 7 (on the survivor's current workspace) stays visible.
        assert!(m.visible_toplevels().contains(&WindowId(7)));
        assert!(!m.visible_toplevels().contains(&WindowId(9)));
    }

    #[test]
    fn remove_last_output_drops_everything() {
        let mut m = WorkspaceModel::new();
        let oid = m.add_output("a");
        let wid = m.current_workspace(oid).unwrap();
        m.place_toplevel(wid, WindowId(42));
        assert_eq!(m.remove_output(oid), 0);
        assert!(m.outputs().is_empty());
        assert!(m.visible_toplevels().is_empty());
    }

    #[test]
    fn replug_restores_displaced_workspaces() {
        let mut m = WorkspaceModel::new();
        let primary = m.add_output("a");
        let second = m.add_output("b");
        let ws_b = m.current_workspace(second).unwrap();
        m.place_toplevel(ws_b, WindowId(9)); // lives on the "b" output

        // Unplug "b": its workspace relocates to "a" but remembers "b" as home.
        m.remove_output(second);
        let parked_on = m.workspace_of(WindowId(9)).expect("toplevel retained");
        assert_eq!(m.workspace(parked_on).unwrap().output, primary);
        assert_eq!(m.workspace(parked_on).unwrap().origin, "b");
        assert_eq!(m.outputs().len(), 1);

        // Replug "b": the displaced workspace moves back to it.
        let second2 = m.add_output("b");
        assert_ne!(second2, second, "a replugged output gets a fresh id");
        let home = m.workspace_of(WindowId(9)).expect("toplevel restored");
        assert_eq!(
            m.workspace(home).unwrap().output,
            second2,
            "back on its home output"
        );
        assert_eq!(m.outputs().len(), 2);
        // Toplevel 9 is now on "b"'s current workspace only if it is current;
        // it sits on a (possibly non-current) restored workspace. Either way
        // it is no longer on "a".
        assert_ne!(m.workspace(home).unwrap().output, primary);
    }

    #[test]
    fn snapshot_carries_connector() {
        let mut m = WorkspaceModel::new();
        let _ = m.add_output("HDMI-A-1");
        let snap = m.snapshot();
        assert_eq!(snap.outputs[0].connector, "HDMI-A-1");
    }

    #[test]
    fn tiling_is_per_workspace_and_persists_across_switch() {
        let (mut m, oid, ws0) = one_output();
        m.place_toplevel(ws0, WindowId(1)); // fill ws0 → trailing ws1 appended
        let ws1 = m.switch(oid, Switch::Next).unwrap(); // now on ws1

        // Tile ws1 only.
        m.set_tiled(ws1, true);
        assert!(m.current_workspace_tiled(oid));
        assert!(m.workspace(ws1).unwrap().tiled);
        assert!(!m.workspace(ws0).unwrap().tiled, "ws0 stays floating");

        // Switch back to ws0: it is floating; ws1 remembers it is tiled.
        m.switch(oid, Switch::Prev);
        assert!(!m.current_workspace_tiled(oid));
        assert!(m.workspace(ws1).unwrap().tiled, "ws1 tiled flag persists");

        // Snapshot surfaces the flag.
        let snap = m.snapshot();
        let entry1 = snap.outputs[0]
            .workspaces
            .iter()
            .find(|w| w.id == ws1)
            .unwrap();
        assert!(entry1.tiled);
    }

    #[test]
    fn snapshot_reflects_place_and_switch() {
        let (mut m, oid, wid) = one_output();
        m.place_toplevel(wid, WindowId(11));
        let second = m.switch(oid, Switch::Next).unwrap();
        m.place_toplevel(second, WindowId(22));

        let snap = m.snapshot();
        assert_eq!(snap.outputs.len(), 1);
        let o = &snap.outputs[0];
        assert_eq!(o.id, oid);
        // Three workspaces: [11], [22], [] (trailing empty).
        assert_eq!(o.workspaces.len(), 3);
        assert_eq!(o.workspaces[0].toplevels, vec![WindowId(11)]);
        assert_eq!(o.workspaces[1].toplevels, vec![WindowId(22)]);
        assert!(o.workspaces[2].toplevels.is_empty());
        // Current is the workspace we switched to (index 1).
        assert_eq!(o.current, Some(second));
    }

    #[test]
    fn invariant_b_holds_after_a_sequence() {
        // Stress the invariants: the last workspace is always empty after a
        // mixed sequence of operations.
        let (mut m, oid, mut cur) = one_output();
        for i in 0..5u64 {
            m.place_toplevel(cur, WindowId(i));
            cur = m.switch(oid, Switch::Next).unwrap();
        }
        // Remove a middle toplevel to trigger a reap.
        m.remove_toplevel(WindowId(2));
        let o = m.output(oid).unwrap();
        let last = *o.workspaces.last().unwrap();
        assert!(
            m.workspace(last).unwrap().toplevels.is_empty(),
            "trailing workspace must stay empty"
        );
        // current is a valid index.
        assert!(o.current < o.workspaces.len());
        // No duplicate toplevel ids across workspaces.
        let mut seen = std::collections::HashSet::new();
        for wid in &o.workspaces {
            for t in &m.workspace(*wid).unwrap().toplevels {
                assert!(seen.insert(*t), "toplevel {:?} on two workspaces", t);
            }
        }
    }
}
