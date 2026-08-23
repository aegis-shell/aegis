# ADR-0140: Session power modes over the staged idle pipeline

- Status: Accepted
- Date: 2026-08-23

## Context

The command panel's single "Always On" toggle (`SystemAction::SetIdleInhibit`,
ADR-0078) folds two product questions into one boolean: whether the display
may blank, and whether the session may lock. Users reasonably want the
combinations separately — "stay awake and unlocked" for reading or monitoring,
"lock on schedule but never blank" for cooking recipes or kiosk duty, "the
full staged policy" as the default. One boolean cannot express these; turning
it on also suppresses dimming entirely, which is a burn-in liability rather
than a feature.

Three mechanisms already exist and must keep working unchanged:

- the compositor's inhibitor evaluation — per-surface
  `zwp_idle_inhibit_v1` objects and connection-scoped IPC inhibitors fold
  into one effective flag that keeps every `ext_idle_notification_v1`
  resumed, and a locked session ignores all of them;
- the security boundary — display power-off and suspend only ever run
  behind a compositor-confirmed session lock (`desired_output_power`,
  `validate_session_boundary`, and the runtime's forced output wake for an
  unlocked session);
- the coordinator's `--*-after` argv, which reconfigures by full process
  respawn (STOP + spawn), resetting every stage timer and forcing the
  outputs back On through the coordinator-recovery path.

The protocol carries `SystemAction` as an internally tagged enum;
`SystemStatus.idle_inhibited` is a compositor-owned boolean that older chrome
reads directly.

## Decision

1. **Power modes are policy over the existing mechanism, not a parallel
   inhibitor system.** `aegis_model::power::PowerMode` names three modes —
   `Balanced` (dim, lock, display-off, suspend), `Awake` (dim only), and
   `Secure` (dim, lock) — each mapping to a fixed set of armed stages. The
   coordinator expresses a mode by **selectively not arming** the disarmed
   stages' `ext_idle_notification_v1` objects: `IdlePolicy::stages` filters
   the product timeouts by the mode's armed set. The compositor's inhibitor
   evaluation, the security boundary, and the secure readiness handshake are
   untouched; a mode can only remove timers the policy would have armed, and
   `IdlePolicy::validate` still sees the full unfiltered stage set, so no
   mode can weaken the lock-before-power ordering.

2. **The fourth quadrant is projected, not offered.** "Never blank, never
   lock" cannot exist: blanking or suspending an unlocked session is
   forbidden by the boundary above. The Quick Controls therefore expose two
   switches — *Keep Screen Awake* (display axis) and *Automatic Lock*
   (security axis) — and the (awake, no-lock) combination projects onto
   `Awake`. The switches read the mode back honestly, so after that
   projection the awake switch shows on, telling the user the security axis
   won. `Awake` keeps the dim stage armed so an abandoned session still
   gets an idle response.

3. **Mode changes are live, not respawns.** A new `MODE <name>` control
   datagram re-arms the coordinator's stage notifications in place: the old
   notification objects are dropped (destroying their wl resources), pending
   dim/display-off/suspend state is reconciled for a mode that no longer
   wants it, and fresh notifications are created for the new armed set.
   `--mode` carries the mode across a respawn, and the runtime replaces the
   coordinator only when the datagram cannot be delivered. Manual locking
   (`LOCK`, `Super+L`) never passes through a stage notification, so no mode
   or mode change can touch it; lock-before-sleep is equally unaffected.

4. **Session-scoped runtime state, mirrored for older readers.** The mode is
   not persisted — the same contract as the toggle it generalizes; a fresh
   session starts `Balanced`. `SystemStatus.power_mode` (additive,
   `serde(default)`, so no protocol bump) is the new field; `idle_inhibited`
   remains as a derived read-only mirror — true exactly when the mode
   disarms the lock stage — so single-bit chrome keeps reading correctly.
   `SystemAction::SetIdleInhibit` stays on the wire and maps onto the mode
   (`true` → `Awake`, `false` → `Balanced`). The panel toggle no longer
   holds a separate inhibitor: the connection-scoped IPC inhibitors keep
   folding into the same effective compositor flag exactly as before.

5. **External surface.** `aegis system power-mode <balanced|awake|secure>`
   on the CLI; `Command::System { action: SetPowerMode { mode } }` on IPC
   under the existing `SystemControl` capability, refused while the session
   is locked like every other system action except `SetOutputPower`.

## Alternatives

- **Per-stage inhibition mask in the compositor.** Give
  `ext_idle_notification_v1` records a per-stage mask so a mode could
  selectively inhibit stage timers compositor-side. Rejected: it couples
  user policy into the mechanism layer, requires protocol-visible semantics
  for what a partially inhibited notification means, and duplicates what
  selective arming already expresses with zero compositor changes.
- **Four modes including `KeepLit`.** A `KeepLit` mode arming the full stage
  set is behaviorally `Balanced` under another name. Rejected as redundant;
  the two-switch projection needs exactly three modes to cover its reachable
  space.
- **Persist the mode in `[idle]`.** Rejected: a forgotten persisted
  `Awake`/`Secure` silently disables locking or blanking across reboots.
  Session scope keeps the blast radius of a forgotten toggle to one session,
  matching the previous toggle's contract and ADR-0078's trust posture.
- **Generalize `zwp_idle_inhibit_v1` semantics** (surfaceless inhibitors
  with per-stage scope). Rejected: the Wayland protocol has no such
  vocabulary; inventing it would fork the compositor from the ecosystem for
  a product-policy concern.

## Consequences

- The idle coordinator owns one new live code path (`set_mode`): dropping
  and re-creating notification objects, reconciling pending stage state.
  Its correctness burden is small but real — re-arm must not resurrect
  `display_off_pending` for a mode that disarmed the stage, and must keep
  an already-required lock required.
- `SystemStatus.idle_inhibited` becomes derived; any writer that treats it
  as authoritative must switch to `power_mode`. The mirror is removed when
  no in-tree consumer of the single-bit view remains.
- The panel's session group now shows two switches whose combination may
  not round-trip exactly (the projection snaps (off, off) to `Awake`); the
  switch readback, not a dialog, communicates the snap.
- Follow-ups this decision creates: a settings-module row that documents
  the mode alongside `[idle]` timeouts; considering a low-battery
  escalation path that forces `Balanced` (out of scope here — the
  low-battery latch machinery exists but is warning-only today); and
  retiring `SetIdleInhibit` once no external consumer needs the boolean.
