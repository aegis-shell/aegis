# ADR-0099: Resource authority and out-of-process FileChooser

- Status: Accepted
- Date: 2026-08-02
- Supersedes: [ADR-0086](0086-full-stack-portal-via-user-consent-pick-chains.md), [ADR-0095](0095-independent-portal-repository-and-component-workspace.md)

## Context

ADR-0086 put FileChooser in compositor chrome alongside screen-target and
consent prompts. ADR-0095 then retained that picker and its IPC schema in the
Aegis repository while moving the D-Bus backend out. The resulting boundary
made the compositor synchronously enumerate arbitrary host directories,
receive selected paths, and maintain a partial file-browser model. The portal
adapter still lost FileChooser v3 semantics including parent windows,
modality, choices, current-filter selection, and typed filter rules.

Screen and window selection belong to the compositor because it owns those
resources. Files belong to the host filesystem and the FileChooser portal.
Window parenting is the only compositor-owned operation needed by an
out-of-process file dialog, and Wayland already defines that capability in
xdg-foreign-v2.

## Decision

Keep the portal in the independent `xdg-desktop-portal-aegis` repository with
its own workspace, release sequence, exact Aegis compatibility mapping,
encrypted Secret component, PAM helper, D-Bus metadata, CI, and lockfile.

Move FileChooser ownership completely to that repository:

- the resident backend owns D-Bus translation, Request lifecycle, and process
  supervision;
- a one-shot GTK4 `aegis-portal-prompter` client owns file enumeration and UI;
- one request and response cross anonymous pipes using lossless Unix path
  bytes; and
- closing the portal Request kills the prompter.

Remove `PickFile`, `FilePicked`, `FilePickOptions`, `FilePickResult`,
`FilePickMode`, `FileFilter`, and `OpClass::PickFile` from Aegis IPC. Remove
the core file-browser model, shell component, runtime channels, pending state,
and built-in portal grant. This is an incompatible schema deletion, so the
Aegis IPC protocol advances from 19 to 20.

Implement and advertise `xdg-foreign-unstable-v2` exporter and importer
globals. Handles contain 128 bits from the operating-system random source,
remain valid only while their export object and surface live, support multiple
imports, and revoke transient relationships on export, import, child, or
parent destruction. The globals are visible to Realm clients because the
unguessable handle is the explicit capability a sandboxed caller transfers to
the physical portal prompter. The compositor sees only surface identities and
parent relationships, never file data.

Preserve compositor chrome and scoped IPC for compositor-owned resources and
decisions: `PickTarget`, `PickApp`, `PromptSecret`, `PickConfirm`, capture,
streaming, notification, inhibit, and wallpaper operations remain unchanged.
The single-process encrypted vault and Secret Service compatibility decision
from ADR-0085 also remains unchanged.

## Alternatives

- **Keep the compositor file picker and fill its missing features.** Rejected
  because implementation completeness cannot repair the resource-ownership
  and failure-domain mismatch.
- **Use `xdg-desktop-portal-gtk` as the FileChooser backend.** Rejected because
  fallback routing would own the request. GTK is reused only as a toolkit
  inside the Aegis portal's supervised prompter.
- **Embed GTK in the resident backend.** Rejected because filesystem/provider
  stalls and toolkit faults would share the D-Bus service's lifetime.
- **Carry a private parent-window IPC operation.** Rejected because
  xdg-foreign-v2 is the standard capability protocol for out-of-process
  dialogs and avoids another Aegis-specific contract.
- **Keep deprecated PickFile wire types.** Rejected because protocol 20 is
  already an explicit compatibility break; dead public schema would preserve
  the false ownership boundary without helping protocol-19 clients, which
  cannot pass the version handshake.

## Consequences

- The compositor no longer links or runs a file browser, reads portal-selected
  directories, or observes filenames and paths.
- Portal packaging gains GTK4 and a private `/usr/lib/aegis-portal-prompter`
  executable; core Aegis retains no GTK dependency.
- Out-of-process dialogs gain standard cross-client transient parenting,
  including sandboxed Realm callers transferring an explicit handle.
- Aegis and Portal releases that adopt this boundary must be published as a
  coordinated compatibility pair using IPC protocol 20.
- App selection and security consent remain compositor-hosted because they
  authorize compositor/session policy rather than host filesystem resources.
