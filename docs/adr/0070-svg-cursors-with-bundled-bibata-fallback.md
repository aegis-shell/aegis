# ADR-0070: SVG cursors with a bundled Bibata fallback

- Status: Accepted
- Date: 2026-07-29

## Context

The DRM backend has no hardware cursor plane, so the compositor draws the
cursor itself. Until now that path parsed the legacy **Xcursor binary**
format: a fixed set of prerendered bitmaps per file, read from a
freedesktop cursor theme chosen through `$XCURSOR_THEME` / `$XCURSOR_SIZE`
and the standard icon roots. When no theme resolved a shape, the software
cursor simply did not appear — a bare TTY with no installed icon theme got
no compositor-owned cursor at all.

Xcursor bitmaps are fixed-size, so they blur when scaled, and the format
carries no real advantage for a compositor that already rasterizes on a GPU.
The freedesktop cursor *naming* and *theme inheritance* model (icon roots,
`index.theme`, `Inherits=`) is independent of the on-disk cursor format and
remains worth following.

## Decision

Adopt **SVG** as the cursor format, following the freedesktop cursor naming
and theme spec but rasterizing SVG (`cursors/<name>.svg`) instead of parsing
Xcursor binaries:

- Resolve themes through the same XDG roots and `index.theme` inheritance as
  before; look up `<name>.svg` instead of an Xcursor file.
- Rasterize at runtime with the pure-Rust `resvg` + `usvg` + `tiny-skia`
  stack, cached per `(shape, scale)`. `resvg`/`usvg` default features are
  off (no text shaping, no raster-image decode) since cursor art is
  path/fill/stroke only — this keeps the heavy `rustybuzz`/`fontdb`/image
  trees out of the compositor build.
- **Bundle a full Bibata-Modern-Ice SVG theme** in the binary
  (`assets/cursors/Bibata-Modern-Ice`, embedded via `include_dir`) as the
  universal fallback appended to every theme chain, so a standard cursor
  exists even with no `XCURSOR_THEME` and no installed icon theme.
- Encode each cursor's hotspot on the `<svg>` root as `data-hotspot-x` /
  `data-hotspot-y` in viewBox space; read it from the raw SVG text before
  rasterization (`usvg` drops unknown attributes) and scale it to the
  requested size. The offline asset step
  (`scripts/prepare-bibata-cursors.py`) performs clickgen's recolour and
  hotspot-stamping once, producing runtime-ready SVGs.

The old Xcursor parser is removed entirely; the public `CursorCache` and
`LoadedCursor` surface is unchanged.

## Alternatives

- **Keep the Xcursor binary format.** Rejected: fixed-size bitmaps blur when
  scaled, and the format adds a bespoke binary parser for no benefit over SVG
  the compositor rasterizes anyway.
- **Bundle Xcursor binaries instead of SVG.** Rejected: re-introduces the
  fixed-size scaling problem and needs a binary builder; SVG keeps one crisp
  source per cursor.
- **`librsvg` / `rsvg-convert` for rasterization.** Rejected: it is either a
  system C dependency or a forked subprocess per cursor. The status bar's own
  tray code flags the subprocess path as too slow for hot paths; `resvg` is
  pure-Rust and matches the project's no-system-dependency lean.
- **Ship Bibata's raw `svg/` templates and recolour at runtime.** Rejected:
  those SVGs are template-coloured placeholders with no hotspot data; carrying
  clickgen's `render.json`/build config into the compositor couples it to an
  upstream build format and adds runtime cost for a one-time transformation.
- **Draw a compositor-designed fallback cursor.** Rejected: bespoke art drifts
  from the platform cursor contract and never matches a user's chosen theme.

## Consequences

- Cursors stay crisp at any scale and HiDPI factor; one SVG source per shape.
- A standard cursor is always available — the bare-TTY gap is closed without
  hard-coded drawing.
- Bibata is GPL-3.0; its `LICENSE` and a `NOTICE` ship next to the generated
  theme, so the binary distribution carries the required attribution.
- New pure-Rust dependencies (`resvg`, `usvg`, `tiny-skia`, `include_dir`).
- A maintainer-run asset step regenerates the bundled theme from a Bibata
  checkout; it is not part of every build.
- Only static cursors ship: the animated `wait` and `left_ptr_watch` are
  represented by their first frame, matching the pre-existing static-image
  cursor policy.
- The Xcursor parser and its tests are gone; the cursor module tests now
  exercise SVG metadata parsing, hotspot scaling, and the bundled fallback.
