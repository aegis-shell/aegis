# Persona Portraits

Persona presentation combines user-owned profile content with host-owned
product chrome. Content selection and rendering stay separate from visual
style so the lock screen, command panel, and future persona surfaces can
share one profile without becoming the same composition. Account values are
defaults; they are not the same as authenticated Actor principals.

## Responsibility boundary

| Responsibility | Owner |
|----------------|-------|
| Presentation profile and account defaults | `aegis-shell::persona` |
| Still-image and VRM source precedence | `aegis-shell::persona` with the `persona` feature |
| Still-image normalization and live reload | `aegis-shell::persona::portrait` |
| VRM parsing, animation, and offscreen texture | `aegis-shell::persona::portrait` |
| Ring, fallback colors, and initials scale | `aegis-design` |
| Portrait size, placement, camera, and motion trigger | Presentation host |

The complete source and reload contract is documented in the
[Persona Reference](../../reference/persona.md). The
module boundary is recorded in
[ADR-0111](../../adr/0111-persona-as-shell-domain-with-feature-gated-portrait-runtime.md).

## Visual roles

| Role | Ring | Fallback | Use |
|------|------|----------|-----|
| `PersonaHeader` | 1 px quiet amber | Warm graphite disc with neutral initials | Command-panel persona band |
| `LockHero` | 1 px neutral hairline | Appearance-aware disc and initials | Centered lock composition |

The host supplies portrait size. Every role remains circular and uses the
role's initials scale when no portrait is available. Dynamic host context may
override fallback colors: the lock screen adapts its fallback to the selected
appearance and background while retaining the `LockHero` ring and typography.

## Custom content

User customization changes profile content, not the design role. The ordered
`PortraitConfig` contract resolves canonical Aegis still images,
freedesktop-compatible faces, and the canonical VRM. The same immutable
configuration drives initial loading and filesystem observation, so custom
content appears consistently across every consumer. Future display-name and
behavior settings belong to the same presentation profile rather than the
security-principal model.

Presentation code never discovers files or chooses source precedence.
Likewise, `aegis-design` never receives account data, filesystem paths,
textures, or animation state.
