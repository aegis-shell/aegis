# ADR-0141: The locker broadcasts the logind session-lock boundary

- Status: Accepted
- Date: 2026-08-23
- Related: [ADR-0078](0078-out-of-process-idle-and-session-lock.md),
  [ADR-0140](0140-session-power-modes.md)

## Context

The lock stack holds the desktop's authoritative lock state: the idle
policy spawns `aegis-lock`, the compositor confirms the secure lock
frame, and a successful PAM authentication returns the session. None of
that reached logind. `loginctl`-visible `LockedHint` never moved, the
`org.freedesktop.login1.Session.Lock`/`Unlock` signals — the
freedesktop-standard channel through which keyrings, secret vaults, and
agents couple their own locking to the screen lock — had no publisher,
and `aegis-lock`'s PAM conversation stopped after
`pam_authenticate`/`pam_acct_mgmt`, so the stack's committing hooks
(`pam_sm_setcred`) never ran: the PAM module that plants the
portal-vault unlock token (portal ADR-0010/0012) had no firing point at
a screen unlock, and the vault stayed locked behind a prompt after
every screen-lock cycle.

## Decision

1. **`aegis-idle` broadcasts the boundary.** When the compositor
   confirms the secure lock frame, the idle daemon calls
   `LockSession()` on its own logind session (resolved once per call
   via `GetSessionByPID`); when the locker exits successfully — the only
   exit that means an authenticated unlock — it calls `UnlockSession()`.
   The broadcast is fire-and-forget on its own thread (the
   `suspend_async` precedent): the lock never waits on a system-bus
   round trip, and a missing bus or session (nested compositors,
   `--no-logind`) is logged and skipped. `LockedHint` follows the same
   calls, so `loginctl` reports the truth.
2. **Ordering: after the secure frame, not before.** The signal is the
   *notification* of a state the compositor already enforces. Vault
   owners treat it as "zeroize now"; emitting it only once nothing but
   the lock surface is visible closes the window where an unlocked
   secret could be read through a still-visible application.
3. **Once per lock cycle.** A guard bit suppresses duplicate
   `LockSession()` broadcasts (a replacement locker re-confirming, the
   confirmation pipe re-firing) and resets only on the successful-unlock
   exit path.
4. **`aegis-lock` commits credentials.** After `pam_authenticate` and
   `pam_acct_mgmt` both succeed, the locker calls
   `pam_setcred(PAM_ESTABLISH_CRED)`. This is the credential-commit
   point portal ADR-0010 designates: `pam_aegis`'s `pam_sm_setcred`
   plants the vault-unlock token there (with `pam_sm_open_session` as
   the alternate first-committed hook — the packaged
   `/etc/pam.d/aegis-lock` now carries both the `auth` and `session`
   optional lines). A `setcred` failure never blocks the unlock:
   authentication already succeeded, and the failure only costs the
   token (the vault falls back to its own prompt).
5. **Semantics, not timing.** The broadcasts are advisory notifications
   of the desktop's own state; they do not gate the lock UI, the sleep
   inhibitor, or the output-power policy, which keep their existing
   ordering.

## Alternatives

- **Publish from the compositor.** Rejected: the compositor process
  would grow a system-bus dependency and a session identity it does not
  otherwise need; the idle daemon already owns every other logind
  coupling in this stack (sleep inhibitor, suspend, `PrepareForSleep`).
- **Emit on lock *request* rather than confirmation.** Rejected: a
  requested lock can fail to present (locker crash, compositor denial);
  subscribers would zeroize secrets for a lock that never became
  visible, and the fail-closed replacement cycle would broadcast lock
  and unlock churn with no state change behind it.
- **Skip `pam_setcred`, keep the token planted at `authenticate`.**
  Rejected by portal ADR-0010: authenticate-time planting publishes the
  password before the stack finishes committing and left the token in
  place on later failure; the commit hook is the designed point.
- **Drive vault locking over the compositor's own IPC.** Rejected: the
  portal owns its vault policy and must not grow compositor coupling
  (portal AGENTS boundary); logind's signal is the standard channel and
  serves any subscriber, not only the portal.

## Consequences

- `loginctl show-session -p LockedHint` is finally truthful, and any
  Secret Service implementation (`wssp`, the portal backend, future
  agents) can couple to the boundary through the standard signal with
  zero Aegis-specific protocol.
- The portal secret vault re-unlocks silently after a screen unlock on
  password-mode vaults (the token path) and never shows its dead-end
  prompt on keyfile vaults (portal ADR-0019).
- `aegis-lock` now invokes every `setcred`/`open_session` hook in the
  host's stack, not only `pam_aegis`: anything else planted there (home
  directory mounts, `pam_env` credential side effects) starts seeing
  screen unlocks too. That is the freedesktop-defined meaning of a
  locker that proves the user, and distributions relying on the old
  behavior can drop the `session` line from `/etc/pam.d/aegis-lock`.
- The broadcast is best-effort: on hosts where logind is absent or the
  process has no session, the lock stack works exactly as before and
  only the hint/signals are missing.
