# ADR-0086: Full-stack portal via user-consent pick chains

- Status: Superseded by [ADR-0099](0099-resource-authority-and-out-of-process-file-chooser.md)
- Date: 2026-07-31

## Context

ADR-0075 kept Aegis the portal backend only for interfaces that need
compositor pixels (Settings, Screenshot, ScreenCast, Inhibit) and routed
everything else to `xdg-desktop-portal-gtk` by default. The product
direction is a full-stack, all-in-one portal: dialogs must come from
Aegis's own chrome (lens), not GTK, both for visual coherence and to drop
the GTK dependency surface over time.

Interactive portal dialogs (file chooser, app chooser, credential
prompts) share one shape: the backend method parks, the user decides in
session chrome, and the decision answers the call. ADR-0054 established
that shape for screen-target picking. Additionally, production evidence
showed the backend must serve the spec's lowercase `version` property
exactly — a toolkit that auto-PascalCases property names makes
`xdg-desktop-portal` skip the interface entirely (no frontend interface
is exported).

## Decision

Take over the GTK-held portal interfaces behind one reusable
**user-consent pick chain**:

- New IPC request kinds `PickFile`, `PickApp`, and `PromptSecret`
  (protocols 13–15), each fail-closed like `PickTarget`: `control`
  capability, live lease, an explicit never-inherited `OpClass` in the
  named scope, lock/VT gate, and a scope+lease re-check before delivery.
  One interactive pick at a time compositor-wide, shared across all pick
  kinds. Unlike `PickTarget` these never freeze the screen — the picker
  is ordinary modal chrome over the live scene.
- One modal chrome component per pick kind in the shell: a file picker
  (directory navigation, multi-select, filters, Save-mode filename
  field), an app picker (catalog-resolved names and icons, last-choice
  preselect), and a masked secret prompt (zeroized edit buffer).
- New backend interfaces in `aegis-portal`:
  `org.freedesktop.impl.portal.FileChooser` v3,
  `org.freedesktop.impl.portal.AppChooser` v2,
  `org.freedesktop.impl.portal.Notification` v2 (posts into the
  compositor's own notification queue; `external_id` carries the
  application's id so withdrawals match),
  `org.freedesktop.impl.portal.Email` v2 (hand-off to the session mailer
  via `xdg-email`), and a stateless all-permissive
  `org.freedesktop.impl.portal.Lockdown`.
- Routing keeps `default=gtk` while interfaces move one by one; each
  moved interface gets an explicit `aegis;gtk` fallback route until it
  proves itself, then `aegis` alone.
- Every backend interface must serve the lowercase `version` property
  verbatim; the end-to-end portal tests assert the wire name so a
  toolkit naming convention cannot regress it.

## Alternatives

- **Keep GTK as the dialog provider permanently.** Rejected: mixed visual
  language, a second toolkit in the session, and no path to a coherent
  all-in-one portal.
- **Dialogs as standalone Wayland client windows** (the aegis-lock
  pattern) instead of compositor chrome. Rejected for pickers: the modal
  overlay is exactly the chrome system's job, and a separate client
  duplicates input-grab, backdrop, and lifecycle handling the `Chrome`
  trait already owns.
- **One generic "choice list" IPC instead of per-kind requests.**
  Rejected: the three picks differ in semantics (paths with filters, app
  ids, a masked secret) and in what they must never reveal; typed
  requests keep the fail-closed scope ops honest.

## Consequences

- Everyday portal surfaces (open/save dialogs, open-with dialogs,
  notifications, compose-email hand-off, secret retrieval) are served by
  Aegis chrome end to end; GTK remains only for rare interfaces
  (Access/Account/Wallpaper/DynamicLauncher/Print/Location) that still
  need consent UI or heavy infrastructure.
- The pick chain is the template for the remaining consent dialogs
  (Access, Account, DynamicLauncher): a new IPC pick kind plus one chrome
  component each.
- Backend-interface version negotiation is covered by end-to-end tests
  that boot the real daemon on a private bus; the production outage
  class behind the `Version`/`version` bug is now test-locked.
- Follow-up: flip `default=` to `aegis` once the remaining consent
  dialogs land, PAM auto-unlock for the vault, and symbolic file-type
  icons for the file picker.
