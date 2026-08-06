# ADR-0112: Native portal Secret with a hardened vault and Portal-owned prompts

- Status: Accepted
- Date: 2026-08-06
- Supersedes: [ADR-0085](0085-portal-secret-absorption-and-secret-service-compat.md)

## Context

[ADR-0085](0085-portal-secret-absorption-and-secret-service-compat.md)
absorbed secret storage into the then in-repository portal: an at-rest vault,
a native `org.freedesktop.impl.portal.Secret` backend, a transitional
`org.freedesktop.secrets` compatibility layer, and password unlock routed
through compositor chrome via the `PromptSecret` IPC operation.

[ADR-0095](0095-independent-portal-repository-and-component-workspace.md)
moved the portal into the independent `xdg-desktop-portal-aegis` repository,
and [ADR-0099](0099-resource-authority-and-out-of-process-file-chooser.md)
kept it there while declaring the ADR-0085 secret decision unchanged. The
portal repository then reworked its production boundary on its own ADR
sequence. Its
[ADR-0003](https://github.com/aegis-shell/xdg-desktop-portal-aegis/blob/main/docs/adr/0003-production-interface-boundary.md)
removed the partial `org.freedesktop.secrets` shim — a partial
credential-storage API lets clients observe missing semantics and creates
silent corruption and interoperability risk — and its
[ADR-0004](https://github.com/aegis-shell/xdg-desktop-portal-aegis/blob/main/docs/adr/0004-portal-ownership-and-runtime-ipc-boundary.md)
moved Account, FileChooser, and Secret password input into a Portal-owned,
supervised GTK prompter process instead of compositor IPC.

Two consequences land in this repository. First, ADR-0085 — and ADR-0099's
statement that its decision "remains unchanged" — no longer describes the
running service: the compat layer no longer exists anywhere in Aegis, and
secret password input no longer crosses the compositor socket. Second, the
compositor retains the `PromptSecret` IPC operation and masked prompt chrome
(ADR-0099) with no production caller, while the built-in `aegis-portal` scope
grant still lists `PromptSecret`.

## Decision

Supersede ADR-0085. The compositor hosts neither the secret vault nor any
`org.freedesktop.secrets` surface. The portal's own repository records the
secret service decisions and this record defers to them:

- the service is native `org.freedesktop.impl.portal.Secret` v1 only;
- the served secret is a stable HKDF-SHA256 derivation of the vault master
  key and the frontend-supplied application ID (never the raw key, never a
  D-Bus return value — it is written to the caller's file descriptor);
- password-mode unlock uses a one-shot PAM token or the Portal-owned masked
  prompt, and locked-vault `RetrieveSecret` callers queue behind one shared
  unlock worker (bounded, with per-request cancellation);
- the vault lives under `$XDG_DATA_HOME/aegis/secrets` with keyfile or
  password modes, atomic same-directory replacement with file and directory
  synchronization, and symlink, ownership, mode, and size validation.

Keep the compositor-side `PromptSecret` IPC operation and masked prompt
chrome as a reserved, runtime-gated surface (ADR-0099) with no production
consumer, but remove `PromptSecret` from the built-in `aegis-portal` scope
grant so no current client can trigger a compositor-hosted secret prompt. A
future Aegis-owned consumer re-grants the operation explicitly.

## Alternatives

- **Keep the `org.freedesktop.secrets` compatibility layer.** Rejected by the
  portal's ADR-0003: a partial Secret Service implementation is not a
  compatibility layer when clients can observe missing collection, alias,
  lock, prompt, session, and item semantics.
- **Route secret password input through compositor IPC again.** Rejected by
  the portal's ADR-0004: the encrypted vault is Portal-owned, and sending the
  interaction through compositor IPC widens the runtime authority boundary
  without adding a required compositor capability.
- **Remove the `PromptSecret` IPC surface from this repository now.**
  Deferred. Removal is a protocol-24 wire break that must be coordinated with
  the Portal's pinned compatibility target; the surface is runtime-gated and,
  after this decision, ungranted, so retention is the lower-risk step until a
  coordinated protocol change is scheduled.

## Consequences

- Distributions must provide a complete separate keyring service (for example
  GNOME Keyring) for un-sandboxed `org.freedesktop.secrets` clients. Flatpak
  applications such as Chrome that store login credentials through
  `org.freedesktop.secrets` depend on that keyring being present, because
  Aegis no longer serves the API.
- The vault format, per-application derivation, and prompt behavior are
  defined and tested in the portal repository; the portal `v0.0.4` ↔ Aegis
  `v0.0.13` pair is the current compatibility target.
- `PromptSecret` remains a gated compositor capability with no production
  grantee. Re-granting it restores the compositor-hosted prompt; removing it
  entirely is a coordinated protocol change.
