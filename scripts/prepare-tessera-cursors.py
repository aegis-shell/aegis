#!/usr/bin/env python3
"""Generate the bundled ``Tessera`` SVG cursor theme — original MIT-licensed art.

Every cursor in this theme is authored programmatically in this file from
explicit geometry (polygons, capsules, rounded rects). Nothing is copied,
traced, or derived from any existing cursor theme, so the generated art is
covered by the project's MIT license like any other first-party source.

Conventions match the runtime contract in ``crates/tessera/src/cursor.rs``:

- one 256x256 ``viewBox`` SVG per cursor under ``cursors/<name>.svg``;
- white fill, black outline, round joins — legible on light and dark
  content at any scale;
- each cursor's hotspot is stamped on the ``<svg>`` root as
  ``data-hotspot-x`` / ``data-hotspot-y`` in viewBox coordinates;
- alias names are emitted as byte-identical copies of their canonical
  cursor, mirroring X11 symlink semantics for filesystem installs.

Compound shapes (hands, magnifiers) are unions of primitives. To keep the
outline clean, unions are painted in two passes: first every primitive
solidly in the ink colour with a double-width stroke (a silhouette that
extends ``OUTLINE`` units past the geometry), then every primitive again in
the fill colour with no stroke, which covers the inner half of the
silhouette. The visible result is a uniform ``OUTLINE``-wide band with no
interior seams where primitives overlap.

Usage::

    scripts/prepare-tessera-cursors.py [--out DIR]

``--out`` defaults to ``assets/cursors/Tessera`` relative to the repository
root. Regenerating rewrites the output directory from scratch.
"""

from __future__ import annotations

import argparse
import math
import shutil
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
DEFAULT_OUT = REPO / "assets" / "cursors" / "Tessera"

FILL = "#FFFFFF"
INK = "#000000"
OUTLINE = 16  # visible ink band width, in viewBox units
CENTER = 128.0


# ---------------------------------------------------------------------------
# SVG element plumbing
# ---------------------------------------------------------------------------


def n(v: float) -> str:
    """Compact number formatting for path data."""
    s = f"{v:.1f}"
    return s[:-2] if s.endswith(".0") else s


def _d(points: list[tuple[float, float]], close: bool = True) -> str:
    d = "M" + "L".join(f"{n(x)} {n(y)}" for x, y in points)
    return d + ("Z" if close else "")


def poly(
    points: list[tuple[float, float]],
    fill: str = FILL,
    stroke: str = INK,
    sw: float = OUTLINE,
    extra: str = "",
) -> str:
    return (
        f'<path d="{_d(points)}" fill="{fill}" stroke="{stroke}" '
        f'stroke-width="{n(sw)}" stroke-linejoin="round" stroke-linecap="round"{extra}/>'
    )


def circle(cx: float, cy: float, r: float, fill=FILL, stroke=INK, sw=OUTLINE) -> str:
    return (
        f'<circle cx="{n(cx)}" cy="{n(cy)}" r="{n(r)}" fill="{fill}" '
        f'stroke="{stroke}" stroke-width="{n(sw)}"/>'
    )


def dot(cx: float, cy: float, r: float) -> str:
    return f'<circle cx="{n(cx)}" cy="{n(cy)}" r="{n(r)}" fill="{INK}"/>'


def line(x1: float, y1: float, x2: float, y2: float, sw: float) -> str:
    return (
        f'<path d="M{n(x1)} {n(y1)}L{n(x2)} {n(y2)}" fill="none" stroke="{INK}" '
        f'stroke-width="{n(sw)}" stroke-linecap="round"/>'
    )


def stroke_path(d: str, sw: float) -> str:
    return (
        f'<path d="{d}" fill="none" stroke="{INK}" stroke-width="{n(sw)}" '
        f'stroke-linecap="round"/>'
    )


def rotate(
    points: list[tuple[float, float]], deg: float, c: float = CENTER
) -> list[tuple[float, float]]:
    """Rotate points about (c, c); positive angles turn clockwise on screen."""
    rad = math.radians(deg)
    co, si = math.cos(rad), math.sin(rad)
    return [
        (c + (x - c) * co - (y - c) * si, c + (x - c) * si + (y - c) * co)
        for x, y in points
    ]


# ---------------------------------------------------------------------------
# Union-of-primitives builder (two-pass paint; see module docstring)
# ---------------------------------------------------------------------------

# Primitives: ("rect", x, y, w, h, rx) | ("circle", cx, cy, r)
#             ("capsule", x1, y1, x2, y2, width)
Prim = tuple


def _prim_svg(p: Prim, ink: bool, out: float) -> str:
    color = INK if ink else FILL
    grow = 2.0 * out if ink else 0.0
    kind = p[0]
    if kind == "rect":
        _, x, y, w, h, rx = p
        stroke = f'stroke="{color}" stroke-width="{n(grow)}"' if ink else ""
        return (
            f'<rect x="{n(x)}" y="{n(y)}" width="{n(w)}" height="{n(h)}" '
            f'rx="{n(rx)}" fill="{color}" {stroke}/>'
        )
    if kind == "circle":
        _, cx, cy, r = p
        stroke = f'stroke="{color}" stroke-width="{n(grow)}"' if ink else ""
        return f'<circle cx="{n(cx)}" cy="{n(cy)}" r="{n(r)}" fill="{color}" {stroke}/>'
    if kind == "capsule":
        _, x1, y1, x2, y2, w = p
        return (
            f'<path d="M{n(x1)} {n(y1)}L{n(x2)} {n(y2)}" fill="none" '
            f'stroke="{color}" stroke-width="{n(w + grow)}" stroke-linecap="round"/>'
        )
    raise ValueError(f"unknown primitive {kind!r}")


def union(prims: list[Prim], out: float = OUTLINE) -> str:
    return "".join(_prim_svg(p, True, out) for p in prims) + "".join(
        _prim_svg(p, False, out) for p in prims
    )


# ---------------------------------------------------------------------------
# Shape geometry
# ---------------------------------------------------------------------------

# The primary pointer: a single concave polygon (tip, head-right corner,
# notch, tail bottom-right, tail bottom-left, left edge base). ~44° head
# opening, a deep notch, and a thick tail keep it reading as a pointer —
# not a paper plane — at 24px.
ARROW = [(64, 28), (198, 106), (136, 124), (170, 206), (118, 218), (90, 118)]
ARROW_HOTSPOT = (64, 28)

# Badge position for arrow+badge compound cursors (bottom-right of the arrow).
BADGE = (190.0, 184.0, 38.0)  # cx, cy, r


def arrow() -> str:
    return poly(ARROW)


def right_ptr() -> str:
    return poly([(256 - x, y) for x, y in ARROW])


# I-beam: vertical bar with straight serifs.
_XTERM_PTS = [
    (88, 40),
    (168, 40),
    (168, 54),
    (135, 54),
    (135, 202),
    (168, 202),
    (168, 216),
    (88, 216),
    (88, 202),
    (121, 202),
    (121, 54),
    (88, 54),
]


def xterm() -> str:
    return poly(_XTERM_PTS)


def vertical_text() -> str:
    return poly(rotate(_XTERM_PTS, 90))


def _plus(half_arm: float, half_span: float) -> list[tuple[float, float]]:
    a, s, c = half_arm, half_span, CENTER
    return [
        (c - a, c - s),
        (c + a, c - s),
        (c + a, c - a),
        (c + s, c - a),
        (c + s, c + a),
        (c + a, c + a),
        (c + a, c + s),
        (c - a, c + s),
        (c - a, c + a),
        (c - s, c + a),
        (c - s, c - a),
        (c - a, c - a),
    ]


def crosshair() -> str:
    return poly(_plus(7, 80))


def cell() -> str:
    # Chunky plus with a punched-out centre square (the "cell" read).
    hole = "M116 116L140 116L140 140L116 140Z"
    return (
        f'<path d="{_d(_plus(17, 72))}{hole}" fill="{FILL}" stroke="{INK}" '
        f'stroke-width="{n(OUTLINE)}" stroke-linejoin="round" fill-rule="evenodd"/>'
    )


def hourglass_pts(cx: float, cy: float, scale: float = 1.0) -> list[tuple[float, float]]:
    base = [
        (-52, -84),
        (52, -84),
        (52, -66),
        (12, -10),
        (12, 10),
        (52, 66),
        (52, 84),
        (-52, 84),
        (-52, 66),
        (-12, 10),
        (-12, -10),
        (-52, -66),
    ]
    return [(cx + x * scale, cy + y * scale) for x, y in base]


def watch() -> str:
    return poly(hourglass_pts(CENTER, CENTER))


def _badge(symbol: str) -> str:
    cx, cy, r = BADGE
    return circle(cx, cy, r, sw=12) + symbol


def _badge_q() -> str:
    cx, cy, _ = BADGE
    arc = f"M{n(cx - 13)} {n(cy - 6)}A15 15 0 1 1 {n(cx + 1)} {n(cy + 8)}L{n(cx + 1)} {n(cy + 14)}"
    return _badge(stroke_path(arc, 11) + dot(cx + 1, cy + 24, 6.5))


def _badge_plus() -> str:
    cx, cy, _ = BADGE
    return _badge(line(cx - 15, cy, cx + 15, cy, 11) + line(cx, cy - 15, cx, cy + 15, 11))


def _badge_menu() -> str:
    cx, cy, _ = BADGE
    return _badge(
        line(cx - 14, cy - 12, cx + 14, cy - 12, 9)
        + line(cx - 14, cy, cx + 14, cy, 9)
        + line(cx - 14, cy + 12, cx + 14, cy + 12, 9)
    )


def _badge_shortcut() -> str:
    cx, cy, _ = BADGE
    return _badge(
        stroke_path(
            f"M{n(cx - 13)} {n(cy + 13)}L{n(cx + 13)} {n(cy - 13)}"
            f"L{n(cx + 1)} {n(cy - 13)}M{n(cx + 13)} {n(cy - 13)}L{n(cx + 13)} {n(cy - 1)}",
            10,
        )
    )


def _badge_slash() -> str:
    cx, cy, _ = BADGE
    return _badge(line(cx - 17, cy + 17, cx + 17, cy - 17, 11))


def question_arrow() -> str:
    return arrow() + _badge_q()


def context_menu() -> str:
    return arrow() + _badge_menu()


def alias_cursor() -> str:
    return arrow() + _badge_shortcut()


def copy_cursor() -> str:
    return arrow() + _badge_plus()


def no_drop() -> str:
    return arrow() + _badge_slash()


def left_ptr_watch() -> str:
    cx, cy, _ = BADGE
    return arrow() + poly(hourglass_pts(cx, cy - 2, 0.5), sw=13)


def not_allowed() -> str:
    return circle(CENTER, CENTER, 74) + line(82, 174, 174, 82, 14)


def zoom(plus: bool) -> str:
    lens = [("circle", 108, 108, 54), ("capsule", 150, 150, 206, 206, 22)]
    mark = line(88, 108, 128, 108, 12)
    if plus:
        mark = line(108, 88, 108, 128, 12) + mark
    return union(lens) + mark


def zoom_in() -> str:
    return zoom(True)


def zoom_out() -> str:
    return zoom(False)


def fleur() -> str:
    c, r_tip, r_base, half_head, half_arm = CENTER, 92, 52, 26, 13
    pts = [
        (c, c - r_tip),
        (c + half_head, c - r_base),
        (c + half_arm, c - r_base),
        (c + half_arm, c - half_arm),
        (c + r_base, c - half_arm),
        (c + r_base, c - half_head),
        (c + r_tip, c),
        (c + r_base, c + half_head),
        (c + r_base, c + half_arm),
        (c + half_arm, c + half_arm),
        (c + half_arm, c + r_base),
        (c + half_head, c + r_base),
        (c, c + r_tip),
        (c - half_head, c + r_base),
        (c - half_arm, c + r_base),
        (c - half_arm, c + half_arm),
        (c - r_base, c + half_arm),
        (c - r_base, c + half_head),
        (c - r_tip, c),
        (c - r_base, c - half_head),
        (c - r_base, c - half_arm),
        (c - half_arm, c - half_arm),
        (c - half_arm, c - r_base),
        (c - half_head, c - r_base),
    ]
    return poly(pts)


# East-pointing single-headed arrow with a flat tail end; rotated for the
# other seven directions.
_SINGLE_E = [(216, 128), (172, 98), (172, 115), (48, 115), (48, 141), (172, 141), (172, 158)]

# East-west double-headed arrow; rotated for the other axes.
_DOUBLE_EW = [
    (40, 128),
    (84, 98),
    (84, 115),
    (172, 115),
    (172, 98),
    (216, 128),
    (172, 158),
    (172, 141),
    (84, 141),
    (84, 158),
]


def single(deg: float) -> str:
    return poly(rotate(_SINGLE_E, deg))


def double(deg: float) -> str:
    return poly(rotate(_DOUBLE_EW, deg))


def col_resize() -> str:
    # Horizontal double arrow with a raised vertical divider bar on top.
    bar = '<rect x="114" y="82" width="28" height="92" rx="8" fill="#FFFFFF" stroke="#000000" stroke-width="12"/>'
    return poly(_DOUBLE_EW) + bar


def row_resize() -> str:
    bar = '<rect x="82" y="114" width="92" height="28" rx="8" fill="#FFFFFF" stroke="#000000" stroke-width="12"/>'
    return poly(rotate(_DOUBLE_EW, 90)) + bar


# --- Hands (unions of capsules / rounded rects / circles) --------------------


def hand2() -> str:
    # Pointing hand: extended index finger, curled knuckles, thumb out.
    return union(
        [
            ("capsule", 118, 60, 118, 124, 30),  # index finger
            ("rect", 88, 104, 92, 96, 20),  # palm
            ("circle", 148, 100, 17),  # middle knuckle
            ("circle", 172, 106, 15),  # ring knuckle
            ("circle", 192, 120, 13),  # pinky knuckle
            ("capsule", 100, 150, 72, 114, 26),  # thumb
            ("rect", 100, 184, 68, 28, 10),  # wrist
        ]
    )


def hand1() -> str:
    # Open grab hand: four fingers together, thumb out.
    return union(
        [
            ("capsule", 100, 74, 100, 124, 21),
            ("capsule", 122, 66, 122, 124, 21),
            ("capsule", 144, 66, 144, 124, 21),
            ("capsule", 166, 76, 166, 124, 21),
            ("rect", 88, 116, 90, 88, 20),  # palm
            ("capsule", 96, 152, 66, 120, 24),  # thumb
        ]
    )


def closedhand() -> str:
    # Fist: rounded block with knuckle bumps and a thumb ridge across the front.
    return union(
        [
            ("rect", 78, 92, 108, 104, 24),
            ("circle", 102, 92, 14),
            ("circle", 126, 86, 15),
            ("circle", 150, 86, 15),
            ("circle", 174, 94, 13),
            ("capsule", 92, 150, 148, 166, 24),
        ]
    )


# ---------------------------------------------------------------------------
# Theme table: canonical name -> (builder, hotspot, aliases)
# ---------------------------------------------------------------------------

THEME: list[tuple[str, object, tuple[float, float], list[str]]] = [
    ("left_ptr", arrow, ARROW_HOTSPOT, ["default", "arrow", "top_left_arrow"]),
    ("right_ptr", right_ptr, (256 - ARROW_HOTSPOT[0], ARROW_HOTSPOT[1]), []),
    ("xterm", xterm, (128, 128), ["text", "ibeam"]),
    ("vertical-text", vertical_text, (128, 128), []),
    ("crosshair", crosshair, (128, 128), ["cross", "plus", "tcross"]),
    ("cell", cell, (128, 128), []),
    ("watch", watch, (128, 128), ["wait"]),
    ("left_ptr_watch", left_ptr_watch, ARROW_HOTSPOT, ["progress"]),
    ("question_arrow", question_arrow, ARROW_HOTSPOT, ["help", "left_ptr_help", "whats_this", "dnd-ask"]),
    ("context-menu", context_menu, ARROW_HOTSPOT, []),
    ("alias", alias_cursor, ARROW_HOTSPOT, ["link", "dnd-link"]),
    ("copy", copy_cursor, ARROW_HOTSPOT, ["dnd-copy"]),
    ("no-drop", no_drop, ARROW_HOTSPOT, ["dnd-no-drop", "dnd_no_drop"]),
    ("not-allowed", not_allowed, (128, 128), ["forbidden", "crossed_circle", "circle", "dnd-none"]),
    ("zoom-in", zoom_in, (108, 108), ["zoom_in"]),
    ("zoom-out", zoom_out, (108, 108), ["zoom_out"]),
    ("fleur", fleur, (128, 128), ["move", "all-scroll", "all-resize", "size_all", "dnd-move"]),
    ("hand2", hand2, (118, 46), ["pointer", "pointing_hand", "hand"]),
    ("hand1", hand1, (128, 128), ["grab", "openhand"]),
    ("closedhand", closedhand, (128, 128), ["grabbing"]),
    ("right_side", lambda: single(0), (128, 128), ["e-resize", "sb_right_arrow", "right-arrow"]),
    ("bottom_right_corner", lambda: single(45), (128, 128), ["se-resize", "lr_angle"]),
    ("bottom_side", lambda: single(90), (128, 128), ["s-resize", "sb_down_arrow", "down-arrow"]),
    ("bottom_left_corner", lambda: single(135), (128, 128), ["sw-resize", "ll_angle"]),
    ("left_side", lambda: single(180), (128, 128), ["w-resize", "sb_left_arrow", "left-arrow"]),
    ("top_left_corner", lambda: single(225), (128, 128), ["nw-resize", "ul_angle"]),
    ("top_side", lambda: single(270), (128, 128), ["n-resize", "sb_up_arrow", "up-arrow"]),
    ("top_right_corner", lambda: single(315), (128, 128), ["ne-resize", "ur_angle"]),
    ("sb_h_double_arrow", lambda: double(0), (128, 128), ["ew-resize", "h_double_arrow", "size_hor", "size-hor", "double_arrow"]),
    ("sb_v_double_arrow", lambda: double(90), (128, 128), ["ns-resize", "v_double_arrow", "size_ver", "size-ver"]),
    ("fd_double_arrow", lambda: double(45), (128, 128), ["nwse-resize", "size_fdiag"]),
    ("bd_double_arrow", lambda: double(135), (128, 128), ["nesw-resize", "size_bdiag"]),
    ("col-resize", col_resize, (128, 128), ["split_h"]),
    ("row-resize", row_resize, (128, 128), ["split_v"]),
]

SVG_HEAD = (
    '<svg xmlns="http://www.w3.org/2000/svg" width="256" height="256" '
    'viewBox="0 0 256 256" data-hotspot-x="{hx}" data-hotspot-y="{hy}">'
)


def render_svg(builder, hotspot: tuple[float, float]) -> str:
    return SVG_HEAD.format(hx=n(hotspot[0]), hy=n(hotspot[1])) + "\n" + builder() + "\n</svg>\n"


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--out", type=Path, default=DEFAULT_OUT, help="output theme directory")
    args = ap.parse_args()

    out = args.out
    if out.exists():
        shutil.rmtree(out)
    cursors_dir = out / "cursors"
    cursors_dir.mkdir(parents=True)

    written = 0
    aliases = 0
    for name, builder, hotspot, names in THEME:
        svg = render_svg(builder, hotspot)
        (cursors_dir / f"{name}.svg").write_text(svg)
        written += 1
        for alias_name in names:
            (cursors_dir / f"{alias_name}.svg").write_text(svg)
            aliases += 1

    (out / "index.theme").write_text(
        "[Icon Theme]\n"
        "Name=Tessera\n"
        "Comment=Tessera default cursor theme (white fill, black outline) — original MIT-licensed art.\n"
        "Directories=cursors\n\n"
        "[cursors]\n"
        "Size=24\n"
        "MinSize=8\n"
        "MaxSize=512\n"
        "Type=Fixed\n"
    )

    print(f"wrote {written} cursors ({aliases} aliases) to {out}")


if __name__ == "__main__":
    main()
