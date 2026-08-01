# ADR-0085: Portal secret absorption: vault, Secret backend, and a transitional Secret Service compat layer

- Status: Accepted
- Date: 2026-07-31

## Context

A full-stack desktop needs two secret-bearing D-Bus surfaces that Aegis
previously did not serve at all: `org.freedesktop.impl.portal.Secret`
(sandboxed applications retrieve a master secret through the portal) and
the classic `org.freedesktop.secrets` Secret Service API (un-sandboxed
libsecret clients). Users migrating from the standalone wssp daemon
already have wssp-format vaults on disk, and the long-term direction is
portal-native retrieval only.

The constraints:

- The portal backend process (ADR-0075) is the natural home: it is already
  D-Bus-activated on demand and owns no competing trust boundary.
- A vault must exist even before any password UI ships; the unlock UX must
  not block first-run setup.
- The compat API is transitional by definition, so it must be cheap to
  delete.
- Handing the raw vault master key to the portal frontend (wssp's
  shortcut) widens the blast radius of any frontend compromise.

## Decision

Absorb secret storage into `aegis-portal` as one all-in-one process:

- An at-rest vault under `$XDG_DATA_HOME/aegis/secrets`, byte-compatible
  with the wssp format (serde_json model, XChaCha20-Poly1305 with a
  24-byte nonce prefix, Argon2id password KDF). First run creates a
  mode-0600 keyfile and unlocks automatically; password-mode vaults start
  locked and unlock through the compositor's masked secret prompt
  (ADR-0086's `PromptSecret` chain).
- `org.freedesktop.impl.portal.Secret` v1 served natively. The secret
  handed to the frontend is HKDF-SHA256 derived from the vault master key
  with a fixed domain-separation info, never the raw key, and is written
  to the caller's file descriptor rather than returned over D-Bus.
- `org.freedesktop.secrets` served as a transitional compatibility layer
  (Service/Session/Collection/Item/Prompt, DH-IETF1024 + AES-128-CBC
  session transport matching libsecret byte-for-byte). Everything
  compat-only lives in one module with one registration call site, one
  well-known-name request (non-queued, so a running GNOME Keyring or
  wssp-daemon keeps ownership and we degrade to a warning), and one D-Bus
  activation file — removal is deleting those four. The native Secret
  interface never depends on the compat layer.

## Alternatives

- **Keep wssp as a separate daemon.** Rejected: two daemons own two
  unlock states and two prompters; the all-in-one portal is the stated
  product direction, and one vault format keeps migration trivial.
- **Depend on GNOME Keyring.** Rejected for the same reasons as in
  ADR-0051: an external keyring cannot share Aegis's unlock lifecycle or
  the compositor's chrome.
- **Implement only the portal interface and ignore libsecret clients.**
  Rejected: most un-sandboxed desktop applications still speak
  `org.freedesktop.secrets`; dropping them silently breaks passwords,
  calendars, and browser sync on day one.
- **Serve the raw master key via `RetrieveSecret` (wssp's approach).**
  Rejected: HKDF domain separation costs nothing and keeps the vault key
  inside the process.

## Consequences

- Secret storage works out of the box (keyfile mode) with no UI
  dependency; password unlock works through compositor chrome with the
  typed password zeroized after key derivation.
- The compat layer's removal is a small, pre-planned diff once
  portal-native retrieval is universal; its service file is documented as
  transitional.
- Follow-up: PAM-cached login-password auto-unlock (the wssp-pam token
  pattern), password set/change via `aegis-cli`, and re-lock on screen
  lock once a policy exists.
