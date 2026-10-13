#!/usr/bin/env python3
"""Genera le icone dell'app Mixfonocraft AI Studio (richiede Pillow).

Produce, nelle posizioni usate da packaging/ e dal build:
  assets/icon/icon-1024.png         master 1024x1024 (canvas trasparente)
  assets/icon/icon.icns             macOS: 16 .. 1024 con varianti @2x
  assets/icon/icon.ico              Windows: 16 .. 256
  crates/mfc-gui/assets/icon-256.png icona della finestra (eframe)

Stile: squircle stile macOS Big Sur (824 px su 1024, margini trasparenti), ombra morbida,
sfondo grafite con gradiente verticale, barre da equalizzatore arancio/giallo con alone
e riflesso. Sotto i 32 px il disegno è semplificato (tre barre spesse, squircle a tutto
campo, niente ombra né alone) invece di essere ridotto dal master.

Uso:  python3 assets/icon/make_icon.py
"""

import io
import math
import struct
from pathlib import Path

from PIL import Image, ImageChops, ImageDraw, ImageFilter

ICON_DIR = Path(__file__).resolve().parent
ROOT = ICON_DIR.parents[1]
WINDOW_ICON = ROOT / "crates" / "mfc-gui" / "assets" / "icon-256.png"

LANCZOS = Image.Resampling.LANCZOS
BOX = Image.Resampling.BOX

# Palette (il giallo e l'arancio dell'icona precedente).
BG_TOP = (0x2D, 0x32, 0x3A)
BG_BOTTOM = (0x0E, 0x0F, 0x12)
BAR_TOP = (0xFF, 0xD2, 0x5E)
BAR_MID = (0xFF, 0x9A, 0x2A)
BAR_BOTTOM = (0xFF, 0x72, 0x14)
GLOW = (0xFF, 0x8C, 0x24)
BASELINE = (0xE8, 0xEC, 0xF2)

# Geometria del master, in unità di 1024 px.
CANVAS = 1024
BODY = 824
SQUIRCLE_N = 5.0
BAR_W = 64
BAR_GAP = 28
BAR_MAX = 340
BASE_Y = 652
BARS_X0 = 512 - (6 * BAR_W + 5 * BAR_GAP) // 2
BAR_HEIGHTS = (0.38, 0.64, 1.0, 0.58, 0.82, 0.46)
BASELINE_Y = 700
BASELINE_H = 14


def lerp(a, b, t):
    return tuple(round(a[i] + (b[i] - a[i]) * t) for i in range(3))


def bg_color(t):
    return lerp(BG_TOP, BG_BOTTOM, t)


def bar_color(t):
    """Gradiente delle barre: giallo in alto, arancio in basso."""
    if t < 0.5:
        return lerp(BAR_TOP, BAR_MID, t / 0.5)
    return lerp(BAR_MID, BAR_BOTTOM, (t - 0.5) / 0.5)


def column(size, y_top, y_bottom, color_at, alpha=255):
    """Immagine size x size con colore funzione della y (assoluta, in pixel)."""
    col = Image.new("RGBA", (1, size))
    px = col.load()
    span = max(1e-9, y_bottom - y_top)
    for y in range(size):
        t = min(1.0, max(0.0, (y + 0.5 - y_top) / span))
        r, g, b = color_at(t)
        px[0, y] = (r, g, b, alpha)
    return col.resize((size, size), Image.Resampling.NEAREST)


def squircle(cx, cy, r, n=SQUIRCLE_N, steps=512):
    """Superellisse |x/r|^n + |y/r|^n = 1: angoli continui stile Apple."""
    pts = []
    for i in range(steps):
        a = 2 * math.pi * i / steps
        c, s = math.cos(a), math.sin(a)
        x = cx + r * math.copysign(abs(c) ** (2 / n), c)
        y = cy + r * math.copysign(abs(s) ** (2 / n), s)
        pts.append((x, y))
    return pts


def rounded_mask(size, box, radius):
    m = Image.new("L", (size, size), 0)
    ImageDraw.Draw(m).rounded_rectangle(box, radius=radius, fill=255)
    return m


def scaled_alpha(mask, factor):
    return mask.point(lambda v: round(v * factor))


def draw_master(k=2):
    """Master 1024 px disegnato a k volte la risoluzione e poi ridotto (antialias)."""
    w = CANVAS * k

    def p(v):
        return v * k

    body_pts = [(p(x), p(y)) for x, y in squircle(512, 512, BODY / 2)]
    body_mask = Image.new("L", (w, w), 0)
    ImageDraw.Draw(body_mask).polygon(body_pts, fill=255)

    # Ombra: stesso profilo, spostato in basso e sfocato.
    shadow_alpha = Image.new("L", (w, w), 0)
    ImageDraw.Draw(shadow_alpha).polygon([(x, y + p(14)) for x, y in body_pts], fill=140)
    shadow_alpha = shadow_alpha.filter(ImageFilter.GaussianBlur(p(16)))
    canvas = Image.new("RGBA", (w, w), (0, 0, 0, 0))
    shadow = Image.new("RGBA", (w, w), (2, 3, 6, 255))
    shadow.putalpha(shadow_alpha)
    canvas.alpha_composite(shadow)

    # Corpo: gradiente verticale grafite -> quasi nero.
    body = column(w, 0, w, bg_color).convert("RGBA")
    body.putalpha(body_mask)
    canvas.alpha_composite(body)

    # Riflesso morbido nella parte alta del corpo.
    # Gradiente nero->trasparente: 255 in alto (riflesso pieno) e 0 a metà altezza.
    fade = column(w, 0, w * 0.5, lambda t: (round(255 * t), 0, 0)).getchannel("R")
    sheen_alpha = scaled_alpha(fade.point(lambda v: 255 - v), 0.13)
    sheen_alpha = ImageChops.multiply(sheen_alpha, body_mask)
    sheen = Image.new("RGBA", (w, w), (255, 255, 255, 0))
    sheen.putalpha(sheen_alpha)
    canvas.alpha_composite(sheen)

    # Bordo interno sottile, più chiaro in alto.
    rim_mask = Image.new("L", (w, w), 0)
    rim_pts = [(p(x), p(y)) for x, y in squircle(512, 512, BODY / 2 - 2)]
    ImageDraw.Draw(rim_mask).line(rim_pts + [rim_pts[0]], fill=255, width=p(3), joint="curve")
    rim_alpha = ImageChops.multiply(scaled_alpha(rim_mask, 0.16), body_mask)
    rim = Image.new("RGBA", (w, w), (255, 255, 255, 0))
    rim.putalpha(rim_alpha)
    canvas.alpha_composite(rim)

    # Barre: pill arrotondate con gradiente verticale.
    bar_mask = Image.new("L", (w, w), 0)
    bd = ImageDraw.Draw(bar_mask)
    tops = []
    for i, hf in enumerate(BAR_HEIGHTS):
        x0 = BARS_X0 + i * (BAR_W + BAR_GAP)
        h = hf * BAR_MAX
        top = BASE_Y - h
        tops.append((x0, top))
        bd.rounded_rectangle(
            [p(x0), p(top), p(x0 + BAR_W), p(BASE_Y)], radius=p(BAR_W / 2), fill=255
        )
    bar_grad = column(w, p(BASE_Y - BAR_MAX), p(BASE_Y), bar_color)
    bars = Image.new("RGBA", (w, w), (0, 0, 0, 0))
    bars.paste(bar_grad, (0, 0), bar_mask)

    # Alone arancio attorno alle barre.
    glow_alpha = scaled_alpha(bar_mask.filter(ImageFilter.GaussianBlur(p(22))), 0.75)
    glow_alpha = ImageChops.multiply(glow_alpha, body_mask)
    glow = Image.new("RGBA", (w, w), GLOW + (0,))
    glow.putalpha(glow_alpha)
    canvas.alpha_composite(glow)
    canvas.alpha_composite(bars)

    # Riflessi piccoli e netti sul bordo sinistro di ogni barra.
    gl_mask = Image.new("L", (w, w), 0)
    gd = ImageDraw.Draw(gl_mask)
    for (x0, top), hf in zip(tops, BAR_HEIGHTS):
        gx = x0 + BAR_W * 0.2
        gl_len = min(BAR_MAX * hf * 0.42, 96)
        gd.rounded_rectangle(
            [p(gx), p(top + 16), p(gx + 9), p(top + 16 + gl_len)], radius=p(4.5), fill=255
        )
    gl_mask = ImageChops.multiply(gl_mask, bar_mask)
    gloss = Image.new("RGBA", (w, w), (255, 255, 255, 0))
    gloss.putalpha(scaled_alpha(gl_mask, 0.62))
    canvas.alpha_composite(gloss)

    # Linea di base (stesso elemento della versione precedente).
    base_x0 = BARS_X0
    base_x1 = BARS_X0 + 6 * BAR_W + 5 * BAR_GAP
    base = rounded_mask(
        w,
        [p(base_x0), p(BASELINE_Y), p(base_x1), p(BASELINE_Y + BASELINE_H)],
        p(BASELINE_H / 2),
    )
    line = Image.new("RGBA", (w, w), BASELINE + (0,))
    line.putalpha(scaled_alpha(base, 0.9))
    canvas.alpha_composite(line)

    return canvas.resize((CANVAS, CANVAS), LANCZOS)


def render_small(size, ss=8):
    """Versione semplificata per 16-32 px: tre barre spesse, squircle a tutto campo."""
    w = size * ss
    canvas = Image.new("RGBA", (w, w), (0, 0, 0, 0))
    bg = column(w, 0, w, bg_color)
    bg.putalpha(rounded_mask(w, [0, 0, w - 1, w - 1], round(w * 0.22)))
    canvas.alpha_composite(bg)

    bar_w = 0.19 * w
    gap = 0.05 * w
    x0 = 0.5 * w - (3 * bar_w + 2 * gap) / 2
    base = 0.82 * w
    max_h = 0.68 * w
    heights = (0.55, 1.0, 0.72)
    bar_mask = Image.new("L", (w, w), 0)
    bd = ImageDraw.Draw(bar_mask)
    for i, hf in enumerate(heights):
        x = x0 + i * (bar_w + gap)
        top = base - hf * max_h
        bd.rounded_rectangle([x, top, x + bar_w, base], radius=bar_w / 2, fill=255)
    grad = column(w, base - max_h, base, bar_color)
    bars = Image.new("RGBA", (w, w), (0, 0, 0, 0))
    bars.paste(grad, (0, 0), bar_mask)
    canvas.alpha_composite(bars)
    return canvas.resize((size, size), BOX)


# Voci ICNS: codice, lato in pixel, versione semplificata.
ICNS_ENTRIES = [
    (b"icp4", 16, True),
    (b"icp5", 32, True),
    (b"icp6", 64, False),
    (b"ic07", 128, False),
    (b"ic08", 256, False),
    (b"ic09", 512, False),
    (b"ic10", 1024, False),
    (b"ic11", 32, True),  # 16 pt @2x
    (b"ic12", 64, False),  # 32 pt @2x
    (b"ic13", 256, False),  # 128 pt @2x
    (b"ic14", 512, False),  # 256 pt @2x
]

ICO_SIZES = (16, 20, 24, 32, 40, 48, 64, 96, 128, 256)
ICO_SMALL_MAX = 32


def png_bytes(im):
    buf = io.BytesIO()
    im.save(buf, "PNG", optimize=True)
    return buf.getvalue()


def render_for(size, master, small):
    if small:
        return render_small(size)
    if size == CANVAS:
        return master
    return master.resize((size, size), LANCZOS)


def write_icns(path, master):
    """Contenitore ICNS con TOC, come quello di Pillow ma con anche icp4/icp5 (16 e 32 pt).

    Pillow 12 non scrive le voci 1x a 16 e 32 px, quindi il file viene composto qui; resta
    leggibile da Pillow e da iconutil/Finder.
    """
    chunks = []
    for code, size, small in ICNS_ENTRIES:
        chunks.append((code, png_bytes(render_for(size, master, small))))
    toc_entries = b"".join(code + struct.pack(">I", 8 + len(data)) for code, data in chunks)
    toc = b"TOC " + struct.pack(">I", 8 + len(toc_entries)) + toc_entries
    body = b"".join(code + struct.pack(">I", 8 + len(data)) + data for code, data in chunks)
    total = 8 + len(toc) + len(body)
    path.write_bytes(b"icns" + struct.pack(">I", total) + toc + body)


def write_ico(path, master):
    """ICO con immagini PNG (Windows Vista e successivi): piccole dimensioni disegnate a parte."""
    images = []
    for size in ICO_SIZES:
        small = size <= ICO_SMALL_MAX
        images.append((size, png_bytes(render_for(size, master, small))))
    header = struct.pack("<HHH", 0, 1, len(images))
    offset = 6 + 16 * len(images)
    directory = b""
    for size, data in images:
        dim = 0 if size >= 256 else size
        directory += struct.pack("<BBBBHHII", dim, dim, 0, 0, 1, 32, len(data), offset)
        offset += len(data)
    path.write_bytes(header + directory + b"".join(data for _, data in images))


def main():
    master = draw_master()
    master.save(ICON_DIR / "icon-1024.png", optimize=True)
    write_icns(ICON_DIR / "icon.icns", master)
    write_ico(ICON_DIR / "icon.ico", master)
    WINDOW_ICON.parent.mkdir(parents=True, exist_ok=True)
    master.resize((256, 256), LANCZOS).save(WINDOW_ICON, optimize=True)
    print("icone generate in", ICON_DIR, "e", WINDOW_ICON)


if __name__ == "__main__":
    main()
