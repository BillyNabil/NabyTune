"""Draws NabyTune's icon: a record's ring with one line of sound through it, lime on a dark
rounded square. Writes nabytune.ico (16 to 256 px, each size drawn on its own, with heavier
lines the smaller it gets) and nabytune.png (256 px) next to this file.

    python assets/make_icon.py
"""
import math
from pathlib import Path

from PIL import Image, ImageDraw

S = 1024
BG = (20, 20, 20, 255)
LIME = (198, 242, 78, 255)
SIZES = [256, 128, 96, 64, 48, 40, 32, 24, 20, 16]


def stroke(d, pts, w):
    """A thick polyline with round caps and joins, as dabs along the path."""
    r = w / 2
    for (x0, y0), (x1, y1) in zip(pts, pts[1:]):
        n = max(1, int(math.hypot(x1 - x0, y1 - y0) / 2))
        for i in range(n + 1):
            t = i / n
            x, y = x0 + (x1 - x0) * t, y0 + (y1 - y0) * t
            d.ellipse((x - r, y - r, x + r, y + r), fill=LIME)


def draw(weight=1.0, margin=40):
    """The icon at 1024 px; `weight` thickens the lines for small sizes."""
    im = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    d = ImageDraw.Draw(im)
    d.rounded_rectangle((margin, margin, S - margin, S - margin), radius=230 * (S - 2 * margin) / (S - 80), fill=BG)
    c = S / 2
    ring = 72 * weight
    r = 306 - ring / 2  # The ring's outer edge stays put.
    d.ellipse((c - r - ring / 2, c - r - ring / 2, c + r + ring / 2, c + r + ring / 2), fill=LIME)
    d.ellipse((c - r + ring / 2, c - r + ring / 2, c + r - ring / 2, c + r - ring / 2), fill=BG)
    # One period of a sine across the middle: up, then down.
    half, amp = 150, 80
    pts = [(c - half + t * 2 * half, c - math.sin(t * math.pi * 2) * amp) for t in (i / 300 for i in range(301))]
    stroke(d, pts, 64 * weight)
    return im


def at(size):
    weight = 1.45 if size <= 20 else 1.3 if size <= 32 else 1.12 if size <= 48 else 1.0
    margin = 8 if size <= 32 else 40  # Small icons use the whole square.
    return draw(weight, margin).resize((size, size), Image.LANCZOS)


def main():
    here = Path(__file__).resolve().parent
    images = [at(s) for s in SIZES]
    images[0].save(here / "nabytune.ico", sizes=[(s, s) for s in SIZES], append_images=images[1:])
    images[0].save(here / "nabytune.png")
    print("wrote", here / "nabytune.ico", here / "nabytune.png")


if __name__ == "__main__":
    main()
