# ADR-0133: First-Map Focus Policy and ext-data-control-v1

- Status: Accepted
- Date: 2026-08-19

## Context

Two regressions shared one root: how aegis treats a newly mapped toplevel
that the compositor did not itself launch.

**Focus demotion was too aggressive.** The focus-stealing-prevention (FSP)
change shipped in v0.0.36 granted initial focus only to four kinds of maps:
a matching pending launch placement, the focused client's own windows, an
empty target workspace, or an xdg-activation token. Every other first map
was both denied focus *and* pushed to the bottom of the stacking order
(`place_toplevel_bottom` + `lower_toplevel_surfaces`). In practice the four
clauses miss most maps a user considers solicited:

- The launcher and dock spawn apps through `aegis-launcher` directly and
  register no launch placement, so a first launch from the app grid or dock
  landed *behind* every existing window without focus.
- A portal permission prompt (xdg-desktop-portal's prompter) is a separate
  process. It parents itself to the requesting app through
  `zxdg_importer_v2`, but it is not the focused client, holds no activation
  token, and consumed no launch placement — so the dialog the user must
  answer was buried underneath everything.

**wl-clipboard's invisible window interacted badly with it.** wl-clipboard
sets the Wayland clipboard through `wl_data_device.set_selection`, which
requires the calling client to hold keyboard focus (the core protocol ties
selection ownership to the focused surface). To obtain focus it creates a
real 1×1 transparent `xdg_toplevel` (title `wl-clipboard`), commits it, and
waits for keyboard enter before setting the selection and destroying the
window. That toplevel is a genuine window: it entered the Super+Tab switcher
MRU as "wl-clipboard", and under the FSP change it was demoted instead of
focused — so `set_selection` was never accepted and `wl-copy` hung until
killed.

wl-clipboard's own source documents the escape hatch: its device-manager
selection prefers `ext-data-control-v1` or `wlr-data-control-v1`, "as they
don't require us to use the popup surface hack". aegis advertised neither.

## Decision

1. **ext-data-control-v1 is implemented and advertised** at version 1.
   Clipboard managers bind a device per seat and set/read the selection
   without any focused surface. It writes the same per-seat selection slot
   as `wl_data_device` — both protocol families are views of one clipboard —
   and selection changes notify both symmetrically. Primary selection
   requests are served from the same slot (aegis models exactly one
   selection per seat; the dedicated primary-selection protocols stay
   unadvertised). `finished` is posted when a seat's runtime is quiesced so
   managers release their devices. With this global present, wl-clipboard
   stops creating its helper toplevel entirely, so it never reaches the
   window switcher, the dock, or the focus policy again.

2. **First-map focus policy recognizes two more solicitation clauses.** A
   newly mapped toplevel also takes initial focus when it is a dialog of a
   live mapped parent — including a cross-client parent wired through
   `zxdg_importer_v2`, which is how portal prompters parent themselves — or
   when it is the first live root toplevel of its `app_id` in the session.
   The first-map clause is sound because an application that is not running
   cannot spawn a window on its own: a first map is by construction the
   consequence of the user launching it, through whatever launcher. FSP
   demotion remains for the case it was built for: *additional* windows of
   an app that is already running and is not the focused client.

3. **FSP rejection no longer reorders the stack.** A rejected map keeps the
   top-of-workspace placement the map path already assigned; it simply does
   not enter `pending_activation` and so does not steal keyboard focus.
   The demote-to-bottom helper (`lower_toplevel_surfaces`,
   `place_toplevel_bottom`) is withdrawn from the map path; the model keeps
   `place_toplevel_bottom` for callers that want an explicit bottom
   placement.

## Alternatives

- *Keep the demotion and add per-app allowlists / portal token grants.*
  Chases symptoms: every not-yet-known solicited launcher or portal flow
  would resurface as another buried window. The first-map and dialog clauses
  generalize instead of enumerating.
- *Special-case the wl-clipboard toplevel in the switcher and FSP paths*
  (match `app_id == "io.github.bugaevc.wl-clipboard"`). Fragile: other
  tools ship the same helper-window pattern (older wl-clipboard builds,
  other clipboard managers), and the hack window would still appear in
  taskbars and MRU. Implementing the protocol the tools ask for removes the
  window instead of filtering it.
- *Implement wlr-data-control-v1 instead.* Same shape, but it is
  wlroots-specific and now superseded by the ext- namespace; implementing
  the portable one serves both.
- *Relax `wl_data_device.set_selection` to drop the focused-client check.*
  Rejected: that check is the core protocol's selection-ownership security
  model, and dropping it would let any background client replace the
  clipboard whenever it wants. The data-control protocol is the designed,
  opt-in manager channel.

## Consequences

- `wl-copy`/`wl-paste` (and anything preferring ext-data-control) work
  without focus and without creating windows; verified end-to-end against
  the real binaries in
  `tests/clipboard_e2e.rs::wl_clipboard_roundtrips_without_creating_a_window`.
- The FSP demotion semantics the v0.0.36 change introduced for background
  second windows are preserved (no focus steal), but no window is ever
  buried below the stack on first map.
- Two new solicitation predicates are compositor state queries
  (`toplevel_has_live_parent`, `is_first_toplevel_of_app`) and are unit
  tested, including the cross-client parenting case.
- Clipboard authority note: any client that can bind the seat's
  ext-data-control manager can read and replace that seat's clipboard, the
  same trust position wl_data_device already grants to the focused client.
  Interaction Domain (sandbox) seats can therefore manage only their own
  seat's clipboard; the human seat's clipboard is not reachable from
  sandbox clients because they never bind the human seat's resources.
  A future capability gate over manager binds is possible without protocol
  changes if manager access needs to be restricted per client.
- DnD remains wl_data_device-only; ext-data-control offers here are always
  selection offers.
