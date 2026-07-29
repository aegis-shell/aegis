#!/usr/bin/env python3
"""Prepare the bundled Bibata-Modern-Ice SVG cursor theme for aegis.

Bibata's ``svg/`` directory ships raw *template* art: the cursor shapes are
painted in placeholder colours (``#00FF00`` fill, ``#0000FF`` stroke,
``#FF0000`` accents) and carry **no hotspot data**. The real palette and the
per-cursor hotspots live in ``render.json`` and ``configs/normal/x.build.toml``
and are normally applied by ``clickgen`` when it produces Xcursor binaries.

Aegis draws cursors from SVG at runtime, so this script performs clickgen's
recolour + hotspot step *once*, as an offline asset step, producing
runtime-ready SVGs that the compositor embeds via ``include_dir``. The hotspot
(of each cursor, in the 256x256 viewBox coordinate space) is stamped onto the
``<svg>`` root as ``data-hotspot-x`` / ``data-hotspot-y`` attributes; the
compositor reads them from the raw SVG text (usvg strips unknown attributes)
and scales them by the requested render size.

Usage::

    scripts/prepare-bibata-cursors.py [--bibata PATH] [--out DIR]

``--bibata`` defaults to a fresh sparse checkout of
https://github.com/ful1e5/Bibata_Cursor. ``--out`` defaults to
``assets/cursors/Bibata-Modern-Ice`` relative to the repository root.

Bibata is GPL-3.0; the upstream ``LICENSE`` is copied next to the generated
theme so the binary distribution carries the required attribution.
"""

from __future__ import annotations

import argparse
import json
import re
import shutil
import subprocess
import sys
import tempfile
import tomllib
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
DEFAULT_OUT = REPO / "assets" / "cursors" / "Bibata-Modern-Ice"
BIBATA_URL = "https://github.com/ful1e5/Bibata_Cursor.git"

# Modern-Ice palette, copied from the "Bibata-Modern-Ice" entry of Bibata's
# render.json: the three template colours map to white fill, black stroke,
# white accent. Match is case-insensitive so it tolerates lowercase hex.
MODERN_ICE = {
    "#00ff00": "#FFFFFF",
    "#0000ff": "#000000",
    "#ff0000": "#FFFFFF",
}

HOTSPOT_RE = re.compile(r"<svg\b[^>]*>", re.IGNORECASE)
HEX_RE = re.compile(r"#(?:[0-9a-fA-F]{6})\b")


def obtain_bibata(path: Path | None) -> Path:
    if path is not None:
        if not (path / "render.json").exists():
            sys.exit(f"--bibata {path} does not look like a Bibata_Cursor checkout")
        return path
    tmp = Path(tempfile.mkdtemp(prefix="bibata-"))
    print(f"cloning Bibata_Cursor into {tmp} ...")
    subprocess.check_call(
        [
            "git", "clone", "--depth", "1", "--filter=blob:none",
            "--sparse", BIBATA_URL, str(tmp),
        ]
    )
    subprocess.check_call(["git", "-C", str(tmp), "sparse-checkout", "set", "svg", "configs"], )
    return tmp


def load_palette(bibata: Path) -> dict[str, str]:
    raw = json.loads((bibata / "render.json").read_text())
    entry = raw.get("Bibata-Modern-Ice")
    if entry is None:
        sys.exit("render.json has no Bibata-Modern-Ice entry")
    table: dict[str, str] = {}
    for rule in entry["colors"]:
        table[rule["match"].lower()] = rule["replace"]
    return table


def load_hotspots(bibata: Path) -> dict[str, tuple[int, int]]:
    cfg = tomllib.loads((bibata / "configs" / "normal" / "x.build.toml").read_text())
    fallback = cfg["cursors"].get("fallback_settings", {})
    default = (int(fallback.get("x_hotspot", 128)), int(fallback.get("y_hotspot", 128)))
    out: dict[str, tuple[int, int]] = {}
    for name, body in cfg["cursors"].items():
        if name == "fallback_settings" or not isinstance(body, dict):
            continue
        x = int(body.get("x_hotspot", default[0]))
        y = int(body.get("y_hotspot", default[1]))
        out[name] = (x, y)
    return out


def load_symlinks(bibata: Path) -> dict[str, list[str]]:
    cfg = tomllib.loads((bibata / "configs" / "normal" / "x.build.toml").read_text())
    out: dict[str, list[str]] = {}
    for name, body in cfg["cursors"].items():
        if name == "fallback_settings" or not isinstance(body, dict):
            continue
        out[name] = list(body.get("x11_symlinks", []))
    return out


def resolve_source_svg(bibata: Path, png_field: str) -> Path | None:
    """Map a clickgen ``png`` field to the SVG source under svg/modern."""
    stem = png_field[:-4] if png_field.endswith(".png") else png_field
    modern = bibata / "svg" / "modern"
    if "*" in stem:
        # Animated cursor (wait, left_ptr_watch): take the first frame as the
        # static representation, matching aegis's static-image cursor policy.
        base = stem.split("*", 1)[0].rstrip("-")
        first = sorted((modern / base).glob(f"{base}-*.svg"))
        return first[0] if first else None
    candidate = modern / f"{stem}.svg"
    return candidate if candidate.exists() else None


def recolor(svg: str, palette: dict[str, str]) -> str:
    def repl(match: re.Match[str]) -> str:
        return palette.get(match.group(0).lower(), match.group(0))

    return HEX_RE.sub(repl, svg)


def stamp_hotspot(svg: str, hotspot: tuple[int, int]) -> str:
    attrs = f' data-hotspot-x="{hotspot[0]}" data-hotspot-y="{hotspot[1]}"'
    return HOTSPOT_RE.sub(lambda m: m.group(0)[:-1] + attrs + ">", svg, count=1)


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--bibata", type=Path, default=None, help="path to a Bibata_Cursor checkout")
    ap.add_argument("--out", type=Path, default=DEFAULT_OUT, help="output theme directory")
    args = ap.parse_args()

    owns_checkout = args.bibata is None
    bibata = obtain_bibata(args.bibata)
    try:
        palette = load_palette(bibata)
        hotspots = load_hotspots(bibata)
        symlinks = load_symlinks(bibata)

        cfg = tomllib.loads((bibata / "configs" / "normal" / "x.build.toml").read_text())
        cursors = cfg["cursors"]

        out = args.out
        if out.exists():
            shutil.rmtree(out)
        cursors_dir = out / "cursors"
        cursors_dir.mkdir(parents=True)

        written: dict[str, str] = {}
        missing: list[str] = []
        for name, body in cursors.items():
            if name == "fallback_settings" or not isinstance(body, dict):
                continue
            src = resolve_source_svg(bibata, body["png"])
            if src is None:
                missing.append(name)
                continue
            svg = recolor(src.read_text(), palette)
            svg = stamp_hotspot(svg, hotspots.get(name, (128, 128)))
            target = cursors_dir / f"{name}.svg"
            target.write_text(svg)
            written[name] = svg
            for alias in symlinks.get(name, []):
                (cursors_dir / f"{alias}.svg").write_text(svg)

        if missing:
            print(f"warning: no SVG source for: {', '.join(sorted(missing))}", file=sys.stderr)

        (out / "index.theme").write_text(
            "[Icon Theme]\n"
            "Name=Bibata-Modern-Ice\n"
            "Comment=Bibata Modern Ice (white fill, black stroke) \u2014 bundled default cursor theme.\n"
            "Directories=cursors\n\n"
            "[cursors]\n"
            "Size=24\n"
            "MinSize=8\n"
            "MaxSize=512\n"
            "Type=Fixed\n"
        )
        shutil.copyfile(bibata / "LICENSE", out / "LICENSE")
        (out / "NOTICE").write_text(
            "The SVG cursor art in this directory is derived from Bibata Cursor\n"
            "(https://github.com/ful1e5/Bibata_Cursor), licensed under GPL-3.0.\n"
            "Recoloured to the Modern-Ice palette with hotspots stamped in by\n"
            "scripts/prepare-bibata-cursors.py. See LICENSE in this directory.\n"
        )

        print(f"wrote {len(written)} cursors ({sum(len(s) for s in symlinks.values())} aliases) to {out}")
    finally:
        if owns_checkout:
            shutil.rmtree(bibata, ignore_errors=True)


if __name__ == "__main__":
    main()
