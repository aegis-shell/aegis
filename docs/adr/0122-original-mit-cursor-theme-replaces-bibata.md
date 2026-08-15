# ADR-0122: Original MIT cursor theme replaces the bundled Bibata fallback

- Status: Accepted
- Date: 2026-08-15

## Context

[ADR-0070](0070-svg-cursors-with-bundled-bibata-fallback.md) adopted SVG
cursors with a bundled Bibata-Modern-Ice theme as the universal fallback.
That art is GPL-3.0-only, which made the shipped `aegis` binary a combined
work under `MIT AND GPL-3.0-only` and forced distribution packaging to stage
Bibata's `LICENSE`/`NOTICE` under `/usr/share/licenses/aegis/`. The bundling
was explicitly provisional: the project recorded its intent to replace the
theme with original MIT-licensed art.

ADR-0070 rejected "draw a compositor-designed fallback cursor" on the
grounds that bespoke art drifts from the platform cursor contract and never
matches a user's chosen theme. That concern is weaker than it read at the
time: the bundled theme is the fallback of last resort behind every
installed filesystem theme, and the platform contract is a *naming and
hotspot* contract — which generated art satisfies exactly. Meanwhile the
licensing cost is real and recurring: every release review, packaging rule,
and downstream audit has to account for the one GPL asset in an otherwise
MIT tree.

## Decision

Replace the bundled Bibata-Modern-Ice theme with the **Aegis** cursor theme:
original MIT-licensed SVG art generated in-tree by
`scripts/prepare-aegis-cursors.py` from explicit geometry (polygons,
capsules, rounded rects), covering every `wp_cursor_shape` name plus the
common X11 aliases. The Agent-operation feedback sprite
(`crates/aegis-shell/assets/agent/cursor.svg`) uses the same original arrow.

Everything else from ADR-0070 stands: SVG as the cursor format, runtime
rasterization with `resvg`/`usvg`/`tiny-skia`, XDG theme resolution with the
bundled theme appended to every search chain, and the
`data-hotspot-x`/`data-hotspot-y` stamping convention on the 256-unit
viewBox.

This ADR supersedes ADR-0070.

## Alternatives

- **Keep bundling Bibata.** Rejected: the combined-work binary and its
  packaging disclosure are a permanent tax on every release and downstream
  redistribution, and the project already recorded the intent to drop it.
- **Bundle nothing.** Rejected: a bare TTY or minimal system with no
  installed cursor theme gets an invisible compositor-owned cursor — the
  original gap ADR-0070 closed.
- **Bundle a third-party MIT-permissive theme.** Rejected: imported art
  still carries attribution and license-audit surface, and its visual style
  would not match the shell. Generated geometry is trivially auditable in
  code review and regenerates offline with no network checkout.

## Consequences

- The `aegis` binary and the whole source tree are MIT through and through;
  packaging drops the Bibata `LICENSE`/`NOTICE` staging.
- The cursor asset step needs no network access or upstream checkout; the
  art is reviewable as code in `scripts/prepare-aegis-cursors.py`.
- Cursor shape names, alias coverage, and the hotspot convention are
  unchanged, so no runtime or configuration migration is needed.
- The bundled theme's look changes (a geometric white-on-black set in the
  same spirit as before). Users with any installed cursor theme see no
  change at all.
