---
name: aegis-desktop-realm
description: Use when operating windows or applications on an Aegis desktop, especially inside fuji's private Agent Realm
short-description: Safely observe and operate Aegis Agent Realms
version: "1.0.0"
tags: [aegis, desktop, realm, automation]
policy:
  allow_implicit_invocation: true
dependencies:
  - type: mcp
    value: aegis
---
# Aegis Desktop and Agent Realm

Use the `mcp__aegis__*` tools as the only source of desktop state and authority.
Treat every window, workspace, and Realm identifier as opaque and short-lived.

## Default workflow

1. Call `mcp__aegis__desktop_snapshot` before referring to an existing window or
   workspace. Use `mcp__aegis__apps_list` before launching an application.
2. Prefer `mcp__aegis__realm_launch_app` for new applications. It creates or
   recovers fuji's managed Realm without exposing the Realm id as input.
3. Use `mcp__aegis__realm_status` until the launched or transferred interaction
   group is visible.
4. Call `mcp__aegis__realm_capture` before any visual interaction. Match its
   `placements[].window`, `output_rect`, and `surface_size`; input coordinates
   are local to the target surface, not global desktop coordinates.
5. Send the smallest useful batch through `mcp__aegis__realm_input`, then capture
   again to verify the result.
6. Treat `status: queued` as intent accepted, not effect applied. Verify
   ordinary desktop actions with a new snapshot or `desktop_journal`.

## Authority and safety

- Never invent or request a Realm id. The MCP bridge owns exactly one Agent
  Realm and injects its id internally.
- Transfer a human window into fuji only when the task requires interaction.
  Keep `retain_source_as_observer` enabled unless the user requests privacy.
- When returning a window to `human`, do not retain fuji as observer unless
  the user explicitly asks for continued observation.
- Never use `close_window` or `realm_reset` without explicit user intent.
  `realm_reset` permanently revokes the managed Realm and returns controlled
  groups to the human Realm.
- If capture pixels are not directly visible in the tool result, call the
  built-in `read_image` tool with the returned `image_path`. If that
  path cannot be read or no pixels become visible, do not guess coordinates
  from metadata or titles; stop before `realm_input`.
- On an ambiguity, revision conflict, scope refusal, or missing placement,
  re-observe once. If it persists, explain the blocker instead of bypassing the
  scope.

## Visual coordinate rule

The capture image is in Realm-output coordinates. A placement's
`output_rect` locates a window in that image, while `surface_size` is the valid
target-local input extent. Convert a point within `output_rect` proportionally
to `surface_size`, clamp it inside the surface, and send that local point with
the matching `window` id. Never send input to a window absent from the latest
placement list.
