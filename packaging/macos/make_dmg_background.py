#!/usr/bin/env python3
"""Genera lo sfondo della finestra del DMG di Mixfonocraft AI Studio.

Output (nella cartella assets/dmg/ del repository):
  background.png     660x400   (1x)
  background@2x.png  1320x800  (retina)

Il disegno è supersampled (x4) e poi ridotto, per bordi morbidi.
Le posizioni di icone e frecce sono coerenti con dmg_settings.py:
  app a sinistra   centro (170, 200)
  /Applications    centro (490, 200)
"""

import os
import sys

from PIL import Image, ImageDraw, ImageFont, ImageFilter

WIDTH, HEIGHT = 660, 400
TOP = (0x1B, 0x1E, 0x22)
BOTTOM = (0x12, 0x14, 0x17)
ORANGE = (0xFF, 0x8A, 0x1E)
TITLE_GREY = (0xEC, 0xEE, 0xF1)
SUBTITLE_GREY = (0xA7, 0xAD, 0xB5)

SS = 4  # supersampling

FONT_CANDIDATES_BOLD = [
    "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
    "/usr/share/fonts/truetype/liberation/LiberationSans-Bold.ttf",
    "/System/Library/Fonts/Helvetica.ttc",
    "/Library/Fonts/Arial Bold.ttf",
]
FONT_CANDIDATES_REGULAR = [
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
    "/System/Library/Fonts/Helvetica.ttc",
    "/Library/Fonts/Arial.ttf",
]


def load_font(candidates, size):
    for path in candidates:
        if os.path.exists(path):
            try:
                return ImageFont.truetype(path, size)
            except OSError:
                continue
    # Fallback: font incorporato di Pillow (supporta la dimensione dal 10.1).
    return ImageFont.load_default(size=size)


def gradient(scale):
    w, h = WIDTH * scale, HEIGHT * scale
    img = Image.new("RGB", (w, h), TOP)
    d = ImageDraw.Draw(img)
    for y in range(h):
        t = y / (h - 1)
        color = tuple(round(TOP[i] + (BOTTOM[i] - TOP[i]) * t) for i in range(3))
        d.line([(0, y), (w, y)], fill=color)
    return img


def bezier(p0, p1, p2, steps=120):
    pts = []
    for i in range(steps + 1):
        t = i / steps
        x = (1 - t) ** 2 * p0[0] + 2 * (1 - t) * t * p1[0] + t ** 2 * p2[0]
        y = (1 - t) ** 2 * p0[1] + 2 * (1 - t) * t * p1[1] + t ** 2 * p2[1]
        pts.append((x, y))
    return pts


def draw_arrow(img, scale):
    s = scale
    # Freccia curva dall'icona dell'app verso /Applications (in coordinate 1x).
    p0 = (252 * s, 200 * s)
    p1 = (331 * s, 150 * s)
    p2 = (404 * s, 200 * s)
    pts = bezier(p0, p1, p2)

    # Alone morbido sotto la freccia.
    glow = Image.new("RGBA", img.size, (0, 0, 0, 0))
    gd = ImageDraw.Draw(glow)
    gd.line(pts, fill=ORANGE + (120,), width=int(16 * s), joint="curve")
    glow = glow.filter(ImageFilter.GaussianBlur(radius=6 * s))
    img.paste(glow, (0, 0), glow)

    layer = Image.new("RGBA", img.size, (0, 0, 0, 0))
    d = ImageDraw.Draw(layer)
    d.line(pts, fill=ORANGE + (255,), width=int(5 * s), joint="curve")

    # Punta della freccia orientata lungo la tangente finale.
    tx, ty = p2[0] - p1[0], p2[1] - p1[1]
    norm = (tx * tx + ty * ty) ** 0.5
    ux, uy = tx / norm, ty / norm
    nx, ny = -uy, ux
    tip = p2
    length, half = 24 * s, 13 * s
    base = (tip[0] - ux * length, tip[1] - uy * length)
    head = [
        tip,
        (base[0] + nx * half, base[1] + ny * half),
        (base[0] - nx * half, base[1] - ny * half),
    ]
    d.polygon(head, fill=ORANGE + (255,))
    img.paste(layer, (0, 0), layer)


def draw_text(img, scale):
    s = scale
    d = ImageDraw.Draw(img)
    title_font = load_font(FONT_CANDIDATES_BOLD, int(26 * s))
    sub_font = load_font(FONT_CANDIDATES_REGULAR, int(14 * s))

    def centered(text, y, font, fill):
        left, top, right, bottom = d.textbbox((0, 0), text, font=font)
        x = (WIDTH * s - (right - left)) / 2 - left
        d.text((x, y * s - top), text, font=font, fill=fill)

    centered("Mixfonocraft AI Studio", 44, title_font, TITLE_GREY)
    centered("Trascina l'app nella cartella Applicazioni", 300, sub_font, SUBTITLE_GREY)


def render(scale):
    img = gradient(scale)
    draw_arrow(img, scale)
    draw_text(img, scale)
    return img


def main():
    root = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
    out_dir = os.path.join(root, "assets", "dmg")
    os.makedirs(out_dir, exist_ok=True)

    # Disegno a 4x (SS), poi riduzione a 1x e 2x con LANCZOS.
    hi = render(SS)
    one_x = hi.resize((WIDTH, HEIGHT), Image.LANCZOS)
    two_x = hi.resize((WIDTH * 2, HEIGHT * 2), Image.LANCZOS)
    one_x.save(os.path.join(out_dir, "background.png"), optimize=True)
    two_x.save(os.path.join(out_dir, "background@2x.png"), optimize=True)
    print("scritti:", os.path.join(out_dir, "background.png"), os.path.join(out_dir, "background@2x.png"))
    return 0


if __name__ == "__main__":
    sys.exit(main())
