# ADR-0002: Hand-rolled Wayland server on raw libwayland

- Status: Accepted
- Date: 2026-06-04

## Context

Per [ADR-0001](0001-scope-and-responsibility-boundary.md), ass owns the
Wayland server: the protocol globals, input, output, and session
management. Several Rust paths exist for this layer, ranging from a full
compositor framework to raw bindings.

The project values direct control over the compositor's behavior, both for
the human-user phase and for the later phase in which an AI agent operates
the machine through ass. Implementation effort is explicitly not the
limiting constraint.

## Decision

ass implements the Wayland server by hand on the raw libwayland C API,
through thin FFI: `wayland-server-sys` (and the equivalent thin crates for
DRM, libinput, and libseat when the DRM/KMS backend lands). The compositor
manages `wl_global`, `wl_resource`, and dispatchers directly rather than
delegating object lifecycle and protocol handling to a higher-level
framework.

## Alternatives

- **Smithay.** A mature Rust compositor-building framework. Rejected: it
  introduces a large dependency with its own abstractions and state model,
  reducing the direct control the project wants over protocol behavior.
- **wlroots via FFI.** The most complete and battle-tested C library.
  Rejected: it would make ass a Rust shell over wlroots, and wlroots
  carries its own renderer that would have to be reconciled with flux.
- **wayland-rs safe `wayland-server`.** The idiomatic safe Rust binding
  (Smithay builds on it). A reasonable middle path, but rejected in favor
  of the raw layer for maximum control and to keep one consistent FFI style
  across the server and backend stack.

## Consequences

- ass writes protocol marshalling and object management directly; the
  signatures come from the Wayland protocol definitions rather than a
  generated safe layer.
- The same raw style applies to the nested backend's host-window client,
  which drives libwayland-client directly with hand-declared xdg-shell
  interface tables; see [ADR-0003](0003-nested-first-bring-up.md).
- Memory- and protocol-safety are the implementation's responsibility, not
  a framework's. This is an accepted cost given the control goal.
- No upstream framework dictates the surface and scene model, leaving ass
  free to shape it for the AI-adaptation phase
  ([Architecture](../explanation/architecture.md)).
