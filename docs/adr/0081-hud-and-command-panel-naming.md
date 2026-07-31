# ADR-0081: HUD and command panel naming

- Status: Accepted
- Date: 2026-07-30

## Context

ADR-0080 replaced the interactive status bar with display-only HUD status
chips and moved the displaced interactions into a full-screen modal panel.
The names did not follow the change:

- The crate and configuration table still say "status bar" (`aegis-statusbar`,
  `[statusbar]`), but the component is no longer a bar: it reserves no
  space, captures no pointer input, and fades near the cursor. The name
  misdescribes the form users see.
- The panel carries "SAO", an internal Sword Art Online codename for its
  visual idiom, into user-facing surface: the `aegis-sao-panel` crate and
  the `sao` / `sao_panel` keybinding action names in `config.toml`. A
  codename tells the user nothing about what the component does.

User-facing names should tell users what a thing is. Aegis is pre-1.0, so a
clean break costs less than carrying aliases.

## Decision

Rename the components and their user-facing identifiers to describe their
function:

1. **Status bar becomes HUD.** The crate `aegis-statusbar` becomes
   `aegis-hud`, and the configuration table `[statusbar]` becomes `[hud]`
   with the same `enabled` key. There is no legacy alias for the old table
   name.
2. **SAO panel becomes command panel.** The crate `aegis-sao-panel` becomes
   `aegis-command-panel`. The keybinding action `Action::ToggleSaoPanel`
   becomes `Action::ToggleCommandPanel`, configured with the action name
   `command_panel` (aliases `commandpanel`, `panel`); the old `sao` and
   `sao_panel` names are removed. The default binding stays `Super+S`.
3. **The SAO visual idiom keeps its codename internally.** The `Sao`
   design tokens in `aegis-design` remain the developer-facing name of the
   frosted-white, amber-accent menu style. Style codenames are developer
   vocabulary, not user interface.

This amends the naming introduced by ADR-0045 and ADR-0080; those records
are immutable and keep their original text.

## Alternatives

- **Keep the old names.** Rejected: "status bar" misdescribes a
  display-only HUD that reserves no space, and "SAO" exposes an internal
  codename in user configuration. Both names mislead the people reading
  `config.toml`.
- **Keep a `sao` alias for the keybinding action.** Rejected: the project
  is pre-1.0, so a clean break is cheap, and every alias kept is another
  name users must learn and docs must carry.
- **Rename the `Sao` design tokens too.** Rejected: the token name is the
  internal name of a visual idiom, read by developers rather than written
  into user configuration. Renaming it would churn `aegis-design` and its
  consumers for no user-visible gain.

## Consequences

- Users must update their `config.toml`: `[statusbar]` becomes `[hud]`,
  and any `sao` / `sao_panel` keybinding action becomes `command_panel`.
  The compositor rejects the old names; there is no migration alias.
- User and reference documentation is updated to the new names, and the
  how-to guide moves from `hud-and-sao-panel.md` to
  `hud-and-command-panel.md`. A CHANGELOG entry records the breaking
  configuration change.
- ADR-0045 and ADR-0080 keep their original text as historical records;
  this record supersedes their naming only.
- Internal style references may continue to use "SAO" for the design-token
  idiom without confusing users, because the name no longer appears in any
  user-facing surface.
