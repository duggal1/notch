#!/usr/bin/env python3
"""Brutal headless proof for the pixel-icon theme swap.

For every captured view:
  1. compare all non-icon element geometry before/after (layout shift = FAIL)
  2. compare icon boxes before/after (count + position = FAIL if moved)
  3. pixel-diff the screenshots; every changed pixel must fall inside the
     union of the icon boxes (expanded 1px). Anything else is a regression.
  4. assert every after-icon is a PixelIcon with a resolved data-icon name.
"""
from __future__ import annotations

import json
import sys
from pathlib import Path

from PIL import Image, ImageChops

PROOF = Path(__file__).resolve().parent.parent / ".proof" / "agents" / "icon-theme"
VIEWS = [
    "popover-rest", "popover-peek", "popover-open",
    "dashboard-tree", "dashboard-mux", "dashboard-comms", "dashboard-escalations",
]


def load(tag: str, view: str) -> dict:
    return json.loads((PROOF / f"{tag}-{view}.json").read_text())


def is_icon_path(path: str) -> bool:
    # An icon's own <svg>/<i> node and any descendant nodes are all icon pixels.
    return ">svg[" in path or ">i.pxicon" in path or path.startswith("svg[") or path.startswith("i.pxicon")


def check_geometry(before: dict, after: dict) -> tuple[list[str], list[str]]:
    b = {r["p"]: r for r in before["rects"] if not is_icon_path(r["p"])}
    a = {r["p"]: r for r in after["rects"] if not is_icon_path(r["p"])}
    problems = []
    for p in sorted(b.keys() & a.keys()):
        x, y = b[p], a[p]
        if any(abs(x[k] - y[k]) > 0.01 for k in ("x", "y", "w", "h")):
            problems.append(f"{p}: {x['x']},{x['y']},{x['w']},{x['h']} -> {y['x']},{y['y']},{y['w']},{y['h']}")
    changed = [p for p in sorted(set(b) ^ set(a)) if not is_icon_path(p)]
    return problems, changed


def check_icon_boxes(before: dict, after: dict) -> list[str]:
    bi, ai = before["icons"], after["icons"]
    if len(bi) != len(ai):
        return [f"icon count {len(bi)} -> {len(ai)}"]
    problems = []
    for i, (x, y) in enumerate(zip(bi, ai)):
        if any(abs(x[k] - y[k]) > 0 for k in ("x", "y", "w", "h")):
            problems.append(f"icon[{i}] {x['name']}: {(x['x'], x['y'], x['w'], x['h'])} -> {(y['x'], y['y'], y['w'], y['h'])}")
    return problems


def pixel_diff(before_png: Path, after_png: Path, before: dict, after: dict) -> dict:
    with Image.open(before_png) as im:
        b = im.convert("RGB")
    with Image.open(after_png) as im:
        a = im.convert("RGB")
    if b.size != a.size:
        return {"size_mismatch": [b.size, a.size]}
    diff = ImageChops.difference(b, a).convert("L")
    mask = Image.new("L", b.size, 0)
    for ic in before["icons"] + after["icons"]:
        x, y, w, h = ic["x"], ic["y"], ic["w"], ic["h"]
        for yy in range(max(0, y - 1), min(b.height, y + h + 1)):
            for xx in range(max(0, x - 1), min(b.width, x + w + 1)):
                mask.putpixel((xx, yy), 255)
    inside = outside = 0
    px_d, px_b, px_a, px_m = diff.load(), b.load(), a.load(), mask.load()
    for y in range(b.height):
        for x in range(b.width):
            if px_b[x, y] == px_a[x, y]:
                continue
            if px_m[x, y]:
                inside += 1
            else:
                outside += 1
    return {"changed_inside_icons": inside, "changed_outside_icons": outside}


def main() -> int:
    failed = False
    report = {"views": {}, "dom_icon_names": set()}
    for view in VIEWS:
        b, a = load("before", view), load("after", view)
        geom, added_removed = check_geometry(b, a)
        boxes = check_icon_boxes(b, a)
        px = pixel_diff(PROOF / f"before-{view}.png", PROOF / f"after-{view}.png", b, a)
        names = sorted({ic["name"] for ic in a["icons"]})
        report["dom_icon_names"].update(names)
        unresolved = [ic["name"] for ic in a["icons"]
                      if not isinstance(ic["name"], str) or not ic["name"] or ic["name"] == "svg"]
        ok = not geom and not added_removed and not boxes and not unresolved and px.get("changed_outside_icons", 1) == 0
        report["views"][view] = {
            "geometry_problems": geom,
            "added_or_removed_elements": added_removed,
            "icon_box_problems": boxes,
            "pixel_diff": px,
            "icon_names": names,
            "unresolved_icons": unresolved,
            "PASS": ok,
        }
        print(f"{view:22s} geom={len(geom)} added/removed={len(added_removed)} "
              f"boxes={len(boxes)} inside_px={px.get('changed_inside_icons')} "
              f"outside_px={px.get('changed_outside_icons')} {'PASS' if ok else 'FAIL'}")
        if not ok:
            failed = True
            for p in geom[:6]:
                print("   GEOM", p)
            for p in boxes[:6]:
                print("   BOX ", p)

    report["dom_icon_names"] = sorted(report["dom_icon_names"])
    (PROOF / "diff-report.json").write_text(json.dumps(report, indent=2) + "\n")
    print("resolved icon names:", ", ".join(report["dom_icon_names"]))
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())
