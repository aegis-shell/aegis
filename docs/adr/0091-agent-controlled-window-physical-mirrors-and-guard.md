# ADR-0091: Agent-controlled window physical mirrors and guard

- Status: Accepted
- Date: 2026-08-01

## Context

[ADR-0040](0040-realms-seats-and-transferable-interaction-authority.md)
separates control authority from observation. A window transferred from the
human Realm can retain the human Realm as an observer, but an application
launched directly inside an Agent Realm has no source Realm to retain. Its
first interaction group therefore appears only on the Agent Realm's directed
virtual output unless another observer is added.

An Agent-only presentation is useful for explicitly hidden work, but it is a
poor default for supervised desktop automation. The physical user cannot see
which new application is running, distinguish a controlled window from an
ordinary window, or recover control from the visible desktop. A normal-looking
read-only mirror is also misleading: it invites clicks even though the human
seat is not authorized to deliver them.

[ADR-0048](0048-compositor-owned-agent-operation-feedback.md) provides trusted,
short-lived feedback after Agent input succeeds. That feedback identifies an
operation, not the persistent authority state of the window. The authority
boundary needs a persistent compositor-owned presentation that remains clear
between operations and while a Realm is paused.

## Decision

When a client launched through an Agent Realm creates its first interaction
group, Aegis adds the human Realm as a read-only observer by default. Every
toplevel on the same Wayland client connection remains in that interaction
group and inherits the same observation policy. The Agent Realm remains the
only controlling Realm, and only its independent seat can deliver client
input.

The physical desktop publishes and renders a window only when the human Realm
controls or observes its interaction group. An Agent-only group with no human
observer remains absent from the physical scene and its window snapshot.
Explicit no-mirror transfers therefore keep their hidden-presentation
semantics.

Every physical read-only mirror receives a compositor-owned **controlled
window guard**. The guard:

- visually subdues the client content and labels the controlling Realm and
  read-only state;
- owns physical pointer routing across the complete visible window rectangle;
- requests the standard `not-allowed` cursor while the pointer is over the
  mirror; and
- remains present while the Agent Realm is paused, until control returns to
  the human Realm or physical observation is removed.

The guard is presentation and defense in depth, not the authority mechanism.
The compositor server continues to reject physical pointer, keyboard, touch,
tablet, focus, move, resize, minimize, close, and other window-control paths
when the human seat does not control the interaction group. A guarded mirror
also remains an input barrier, so a physical click cannot reach a lower window.

Trusted Agent operation feedback renders above the persistent guard. Ordinary
HUD, notification, and modal chrome retain their existing higher layers.
Human desktop screenshots include the guard because it is part of the visible
physical state. Directed Realm capture renders client surfaces without
physical Shell chrome, so neither the guard nor Agent feedback enters the
Agent's own observation loop.

## Alternatives

- **Keep Agent-launched windows only on the virtual output.** Rejected as the
  default because supervised automation needs a visible recovery surface.
  Explicit removal of the human observer remains available for hidden work.
- **Show a normal-looking read-only mirror.** Rejected because appearance and
  cursor feedback would contradict the actual input authority.
- **Let the application draw a disabled state.** Rejected because the client
  does not own Realm authority and cannot provide trusted evidence of which
  seat controls it.
- **Make Shell pointer capture the authority boundary.** Rejected because
  keyboard, touch, tablet, IPC, and protocol input bypass Shell chrome. The
  server-owned Realm model remains the single source of truth.
- **Reuse the Agent operation marker as the persistent state.** Rejected
  because the marker is intentionally ephemeral and describes successful
  operations rather than current window authority.

## Consequences

Agent-launched applications are visible and recoverable on the physical
desktop without sharing focus, modifiers, grabs, or client input with the
human seat. A user can recognize the protected state before clicking, and a
covered lower window cannot receive an accidental click.

Returning the interaction group to the human Realm removes the read-only state
and guard through the same authority snapshot; no second UI ownership flag is
required. Removing the human observer removes the window from physical chrome
and presentation as one policy change.

The persistent guard requires physical composition while a guarded mirror is
visible, so direct scanout is unavailable for that scene. This cost is accepted
for an explicit supervision and recovery surface.

The conceptual relationship between Realm authority, observation, and the
guard is described in
[Architecture](../explanation/architecture.md#realm-authority). The user
workflow is documented in
[How to Use Agent Workspaces](../how-to/ai-workspaces.md).
