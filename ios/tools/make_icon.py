#!/usr/bin/env python3
"""Draws coverwave's app icon: an ASCII disc with a beat shockwave rolling
out from it, on the app's page colour. Glyphs only, like the app itself.

    python3 ios/tools/make_icon.py ios/Coverwave/Assets.xcassets/AppIcon.appiconset/icon-1024.png

Needs Pillow and macOS's Menlo font.
"""

import math
import sys

from PIL import Image, ImageDraw, ImageFont

SIZE = 1024
COLS = 26  # glyph columns across the icon
PAGE = (14, 15, 18)  # Theme.page in CoverCanvas.swift
INK = (196, 204, 214)  # Theme.defaultAccent
ACCENT = (232, 160, 92)  # the one accent: the beat ring
RAMP = " .:-=+*#%@"


def field(u: float, v: float) -> tuple[float, float]:
    """Brightness of the disc and strength of the ring at (u, v) in -1..1."""
    r = math.hypot(u, v)
    disc = 0.0
    if r < 0.54:
        # Grooves: bright bands with darker gaps, brightest at the rim.
        disc = 0.62 + 0.38 * math.cos(r * 40.0)
        disc *= 0.55 + 0.45 * (r / 0.54)
        if r < 0.15:
            disc = 0.55  # the label
        if r < 0.05:
            disc = 0.0  # the spindle hole
    ring = math.exp(-(((r - 0.78) / 0.05) ** 2))
    return disc, ring


def main(out: str) -> None:
    img = Image.new("RGB", (SIZE, SIZE), PAGE)
    draw = ImageDraw.Draw(img)
    cell_w = SIZE / COLS
    cell_h = cell_w * 1.25
    rows = int(SIZE / cell_h) + 1
    font = ImageFont.truetype("/System/Library/Fonts/Menlo.ttc", int(cell_h * 0.92), index=1)
    top = (SIZE - rows * cell_h) / 2
    for row in range(rows):
        for col in range(COLS):
            cx = (col + 0.5) * cell_w
            cy = top + (row + 0.5) * cell_h
            u = cx / SIZE * 2 - 1
            v = cy / SIZE * 2 - 1
            disc, ring = field(u, v)
            if ring > disc and ring > 0.18:
                level, colour = ring, ACCENT
            else:
                level, colour = disc, INK
            glyph = RAMP[min(len(RAMP) - 1, int(level * len(RAMP)))]
            if glyph == " ":
                continue
            # Fade toward the corners so the edge reads as calm.
            fade = max(0.35, 1.0 - 0.5 * max(0.0, math.hypot(u, v) - 0.75))
            c = tuple(int(PAGE[i] + (colour[i] - PAGE[i]) * fade) for i in range(3))
            draw.text((cx, cy), glyph, font=font, fill=c, anchor="mm")
    img.save(out)


if __name__ == "__main__":
    main(sys.argv[1])
