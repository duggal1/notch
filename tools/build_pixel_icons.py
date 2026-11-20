#!/usr/bin/env python3
"""Deterministic HugeIcons -> crisp pixel-grid PNG pipeline for notch UI icons.

Every icon is rasterised from the exact SVG shipped by
`@hugeicons/core-free-icons@4.4.0`, downsampled to its NATIVE display size
(so each cell is exactly one CSS pixel — no blurry rescale), alpha-quantised
to a 1-bit mask, and written as a white-on-transparent PNG. The React
`PixelIcon` component paints that alpha with `currentColor` via CSS mask, so
white-hot hover keeps flipping the glyph for free.

Outputs
  ui/src/assets/pixel-icons/<semantic>-<size>.png
  ui/src/assets/pixel-icons/manifest.json
  app/assets/tray-glyph.png            (orchestrator, 18px, black template)
  app/assets/tray-glyph.bitmap         (source of truth for generate_icons.py)

Requires: node (for the SVG exporter), rsvg-convert, Pillow. No network.
Re-run: python3 tools/build_pixel_icons.py
"""
from __future__ import annotations

import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

from PIL import Image

ROOT = Path(__file__).resolve().parent.parent
UI = ROOT / "ui"
ASSETS = UI / "src" / "assets" / "pixel-icons"
MAP_FILE = ROOT / "tools" / "pixel-icon-map.json"
EXPORTER = ROOT / "tools" / "export-hugeicon-svgs.mjs"
TRAY_PNG = ROOT / "app" / "assets" / "tray-glyph.png"
TRAY_BITMAP = ROOT / "app" / "assets" / "tray-glyph.bitmap"

# Native CSS pixel sizes actually used by call sites (9..17). One icon asset
# per (semantic, size); no runtime scaling, no mush.
SIZES = [9, 10, 11, 12, 13, 14, 15, 16, 17]
SUPERSAMPLE = 4            # rasterise at size*4, then LANCZOS down
# Four alpha levels keep a 1.5px stroke legible at 9-17px while staying blocky.
# CSS image-rendering:pixelated keeps each level a hard 1px cell on retina.
ALPHA_LEVELS = ((48, 0), (110, 85), (180, 170), (256, 255))
TRAY_CUTOFF = 104           # menu-bar template glyph stays 1-bit
TRAY_SIZE = 18
TRAY_ICON = "orchestrator"  # the app's hierarchy mark in the menu bar


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def quantise_alpha(src: Path, dst: Path, size: int, rgb=(255, 255, 255), binary=False) -> str:
    """Raster -> native-size PNG with quantised alpha; returns an ASCII bitmap."""
    with Image.open(src) as im:
        im = im.convert("RGBA").resize((size, size), Image.Resampling.LANCZOS)
        if binary:
            alpha = im.getchannel("A").point(lambda a: 255 if a >= TRAY_CUTOFF else 0)
        else:
            alpha = im.getchannel("A").point(
                lambda a: next(v for hi, v in ALPHA_LEVELS if a < hi)
            )
        out = Image.new("RGBA", (size, size), rgb + (0,))
        out.putalpha(alpha)
        out.save(dst, format="PNG", optimize=True)
        return "".join("#" if alpha.getpixel((x, y)) else "." for y in range(size) for x in range(size))


def main() -> int:
    if not shutil.which("rsvg-convert"):
        print("rsvg-convert missing — install librsvg", file=sys.stderr)
        return 2
    mapping = json.loads(MAP_FILE.read_text())
    entries = {k: v for k, v in mapping.items() if not k.startswith("_")}
    ASSETS.mkdir(parents=True, exist_ok=True)
    # clean only our own generated pngs, never any hand-authored asset
    for old in ASSETS.glob("*.png"):
        old.unlink()

    with tempfile.TemporaryDirectory(prefix="notch-pixel-icons-") as td:
        tmp = Path(td)
        subprocess.run(
            ["node", str(EXPORTER), str(tmp)],
            check=True, cwd=ROOT, capture_output=True, text=True,
        )
        manifest = {
            "source": "@hugeicons/core-free-icons@4.4.0",
            "pipeline": "tools/build_pixel_icons.py",
            "grid": "native CSS pixel; 1-bit alpha; white RGB for CSS mask",
            "supersample": SUPERSAMPLE,
            "alpha_levels": [[hi, v] for hi, v in ALPHA_LEVELS],
            "sizes": SIZES,
            "icons": {},
        }
        for semantic, export_name in sorted(entries.items()):
            svg = tmp / f"{semantic}.svg"
            if not svg.exists():
                raise RuntimeError(f"missing exported SVG for {semantic}")
            files = {}
            for size in SIZES:
                hi = tmp / f"{semantic}-{size}-hi.png"
                subprocess.run(
                    ["rsvg-convert", "-w", str(size * SUPERSAMPLE), "-h", str(size * SUPERSAMPLE),
                     "-o", str(hi), str(svg)],
                    check=True, capture_output=True,
                )
                dst = ASSETS / f"{semantic}-{size}.png"
                quantise_alpha(hi, dst, size)
                files[str(size)] = {"path": dst.name, "sha256": sha256(dst)}
            manifest["icons"][semantic] = {
                "hugeicons_export": export_name,
                "files": files,
            }
            print(f"{semantic:14s} <- {export_name:24s} {len(files)} sizes")

        # --- menu bar tray glyph: same family, pixelated, black template ---
        tray_svg = tmp / f"{TRAY_ICON}.svg"
        tray_hi = tmp / "tray-hi.png"
        subprocess.run(
            ["rsvg-convert", "-w", str(TRAY_SIZE * SUPERSAMPLE), "-h", str(TRAY_SIZE * SUPERSAMPLE),
             "-o", str(tray_hi), str(tray_svg)],
            check=True, capture_output=True,
        )
        rows = quantise_alpha(tray_hi, tmp / "tray.png", TRAY_SIZE, binary=True)
        # black RGB for macOS template rendering; alpha identical to the UI mask
        with Image.open(tmp / "tray.png") as im:
            tray = Image.new("RGBA", (TRAY_SIZE, TRAY_SIZE), (0, 0, 0, 0))
            tray.putalpha(im.getchannel("A"))
            TRAY_PNG.parent.mkdir(parents=True, exist_ok=True)
            tray.save(TRAY_PNG, format="PNG", optimize=True)
        TRAY_BITMAP.write_text("\n".join(
            rows[y * TRAY_SIZE:(y + 1) * TRAY_SIZE] for y in range(TRAY_SIZE)
        ) + "\n")
        manifest["tray"] = {
            "icon": TRAY_ICON,
            "size": TRAY_SIZE,
            "png": str(TRAY_PNG.relative_to(ROOT)),
            "bitmap": str(TRAY_BITMAP.relative_to(ROOT)),
            "sha256": sha256(TRAY_PNG),
        }

    (ASSETS / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    total = sum(len(v["files"]) for v in manifest["icons"].values())
    print(f"wrote {total} pixel PNGs + manifest in {ASSETS.relative_to(ROOT)}")
    print(f"wrote {TRAY_PNG.relative_to(ROOT)} / {TRAY_BITMAP.relative_to(ROOT)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
