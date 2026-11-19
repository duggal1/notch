#!/usr/bin/env python3
"""Generates the two raster assets for the Notch shell, deterministically.

  app/assets/tray-glyph.png   18x18 black template ring on transparency (menu bar)
  app/src-tauri/icons/icon.png 1024x1024 app icon: black squircle, floor glow, white ring

No Pillow — stdlib zlib/struct only. Re-run: python3 app/assets/generate_icons.py
"""

import math
import struct
import sys
import zlib


def png_bytes(width, height, rgba):
    raw = b"".join(
        b"\x00" + bytes(rgba[y * width * 4:(y + 1) * width * 4]) for y in range(height)
    )

    def chunk(tag, data):
        c = struct.pack(">I", len(data)) + tag + data
        return c + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)

    ihdr = struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0)
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", ihdr)
        + chunk(b"IDAT", zlib.compress(raw, 9))
        + chunk(b"IEND", b"")
    )


def ss_coverage(px, py, inside, samples=4):
    """Supersampled alpha: fraction of sample points inside `inside`."""
    hits = 0
    step = 1.0 / samples
    for sy in range(samples):
        for sx in range(samples):
            x = px + (sx + 0.5) * step
            y = py + (sy + 0.5) * step
            if inside(x, y):
                hits += 1
    return hits / (samples * samples)


def tray_glyph():
    size = 18
    center = (size - 1) / 2.0
    ring_r = 6.6      # outer radius, px
    ring_w = 1.6     # stroke width, px
    px = bytearray(size * size * 4)
    inner_r = ring_r - ring_w
    for y in range(size):
        for x in range(size):
            def inside(x, y, cr=center, r_out=ring_r, r_in=inner_r):
                d = math.hypot(x - cr, y - cr)
                return r_in <= d <= r_out

            a = ss_coverage(x, y, inside)
            i = (y * size + x) * 4
            px[i + 0] = 0
            px[i + 1] = 0
            px[i + 2] = 0
            px[i + 3] = round(a * 255)
    return size, size, bytes(px)


def lerp(a, b, t):
    return tuple(round(a[i] + (b[i] - a[i]) * t) for i in range(3))


def app_icon():
    size = 1024
    radius = 229.0            # Apple squircle-ish corner radius
    inset = 6.0               # tiny bleed so the mask never shows edge artifacts
    ow = size - inset * 2

    # palette (DESIGN.md tokens, as rgb)
    black = (0, 0, 0)
    floor_lo = (58, 51, 200)    # indigo
    floor_hi = (68, 85, 238)    # blue
    lilac = (220, 213, 250)

    def squircle_d(x, y):
        """Signed distance to rounded rect centered at size/2 (negative inside)."""
        cx, cy = size / 2.0, size / 2.0
        qx = abs(x - cx) - (ow / 2 - radius)
        qy = abs(y - cy) - (ow / 2 - radius)
        ax, ay = max(qx, 0.0), max(qy, 0.0)
        return math.hypot(ax, ay) + min(max(qx, qy), 0.0) - radius

    def ring_mask(x, y, cr, r_out, r_in):
        d = math.hypot(x - size / 2.0, y - cr)
        return r_in <= d <= r_out

    px = bytearray(size * size * 4)
    ring_cy = size * 0.44
    r_out, r_in = 196.0, 196.0 - 44.0

    for y in range(size):
        for x in range(size):
            i = (y * size + x) * 4
            # squircle coverage (2x2 supersample is enough at this size)
            a = ss_coverage(x, y, lambda x, y: squircle_d(x, y) <= 0, samples=2)

            # floor glow: elliptical radial falloff anchored to the bottom edge
            gx = (x - size / 2.0) / (size * 0.52)
            gy = (y - size * 1.02) / (size * 0.62)
            g = max(0.0, 1.0 - math.hypot(gx, gy))
            g = g ** 2.6
            glow_rgb = lerp(floor_lo, floor_hi, min(1.0, g * 1.35)) if g > 0 else black

            # white → lilac vertical gradient on the ring stroke
            ry = (ring_cy - y) / r_out  # +1 at top of ring, -1 at bottom
            shade = min(1.0, max(0.0, (ry + 1.0) / 2.0))
            ring_rgb = lerp(lilac, (255, 255, 255), shade)

            c = black
            if g > 0:
                c = tuple(min(255, black[k] + glow_rgb[k] * g) for k in range(3))

            if ring_mask(x, y, ring_cy, r_out, r_in):
                # simple edge AA on the ring annulus
                d_out = abs(math.hypot(x - size / 2.0, y - ring_cy) - (r_out + r_in) / 2)
                half = (r_out - r_in) / 2.0
                edge = min(1.0, max(0.0, (half - d_out) / 1.5 + 0.5))
                c = lerp(c, ring_rgb, edge)

            # 1px faint top-edge light inside the squircle (glass hint, ref 1)
            edge_light = max(0.0, 1.0 - abs(squircle_d(x, y + 2) + 2.0) / 2.0) * 0.10
            if edge_light > 0:
                c = tuple(min(255, int(v + 255 * edge_light)) for v in c)

            px[i + 0] = int(c[0])
            px[i + 1] = int(c[1])
            px[i + 2] = int(c[2])
            px[i + 3] = round(a * 255)
    return size, size, bytes(px)


def main():
    root = sys.argv[1] if len(sys.argv) > 1 else "."
    for name, gen in [("tray-glyph.png", tray_glyph), ("icon", None)]:
        pass

    import os

    tray_path = os.path.join(root, "app", "assets", "tray-glyph.png")
    icon_path = os.path.join(root, "app", "src-tauri", "icons", "icon.png")

    w, h, data = tray_glyph()
    os.makedirs(os.path.dirname(tray_path), exist_ok=True)
    with open(tray_path, "wb") as f:
        f.write(png_bytes(w, h, data))
    print(f"wrote {tray_path} ({w}x{h}, {os.path.getsize(tray_path)} bytes)")

    w, h, data = app_icon()
    os.makedirs(os.path.dirname(icon_path), exist_ok=True)
    with open(icon_path, "wb") as f:
        f.write(png_bytes(w, h, data))
    print(f"wrote {icon_path} ({w}x{h}, {os.path.getsize(icon_path)} bytes)")


if __name__ == "__main__":
    main()
