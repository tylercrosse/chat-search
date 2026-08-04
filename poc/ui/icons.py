#!/usr/bin/env python3
"""Rebuild the source-mark icons and inline them into styles.css.

    python3 poc/ui/icons.py path/to/claude.png claude
    python3 poc/ui/icons.py --rebuild          # re-inline whatever is in icons/

The marks are rendered as CSS masks tinted with the source palette rather than as
full-colour logos, for three reasons that came out of the actual files:

  * two of the five ship white-on-transparent, invisible on a light ground;
  * one ships on an opaque white square, a white block on a dark ground;
  * optical weight ranged from 17% ink to 70%, so sizing them to one box made some
    read four times heavier than others.

Masking fixes all three and keeps TUI-DESIGN §7's rule that source identity is a
discriminable categorical encoding rather than brand colour. The icon inherits the
row's colour, so shape, hue and text label end up as three redundant channels.

Inlined as data URIs because Chrome refuses file:// subresources and this prototype
must open from disk with no server.
"""

import base64
import math
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ICONS = os.path.join(HERE, "icons")
CSS = os.path.join(HERE, "styles.css")
NAMES = ["claude", "claude-code", "chatgpt", "codex", "gemini"]
TARGET_INK = 0.34
SIZE = 96


def build(src_path, name):
    from PIL import Image

    im = Image.open(src_path).convert("RGBA")
    r, g, b, a = im.split()

    # An opaque background means the mark was drawn for white paper; derive alpha from
    # distance-from-white so the glyph survives anywhere.
    if min(a.getdata()) > 250:
        a = Image.merge("RGB", (r, g, b)).convert("L").point(lambda v: 255 - v)

    box = a.getbbox()
    if box:
        a = a.crop(box)
    side = max(a.size)
    sq = Image.new("L", (side, side), 0)
    sq.paste(a, ((side - a.size[0]) // 2, (side - a.size[1]) // 2))

    ink = sum(sq.getdata()) / (255 * side * side)
    scale = max(0.70, min(1.30, math.sqrt(TARGET_INK / max(ink, 0.01))))
    inner = max(8, min(SIZE, int(SIZE * 0.92 * scale)))

    canvas = Image.new("L", (SIZE, SIZE), 0)
    glyph = sq.resize((inner, inner), Image.LANCZOS)
    canvas.paste(glyph, ((SIZE - inner) // 2,) * 2)

    out = Image.merge("RGBA", (Image.new("L", (SIZE, SIZE), 255),) * 3 + (canvas,))
    os.makedirs(ICONS, exist_ok=True)
    out.save(os.path.join(ICONS, f"{name}.png"), optimize=True)
    final = sum(canvas.getdata()) / (255 * SIZE * SIZE)
    print(f"{name:12} ink {ink:.0%} -> {final:.0%} (scale {scale:.2f})")


def inline():
    uris = {}
    for n in NAMES:
        p = os.path.join(ICONS, f"{n}.png")
        if not os.path.exists(p):
            print(f"missing {p}, skipping")
            continue
        uris[n] = "data:image/png;base64," + base64.b64encode(open(p, "rb").read()).decode()

    s = open(CSS).read()
    start = s.index("  --ico-claude:")
    end = s.index("\n\n", start)
    block = "\n".join(f'  --ico-{n}: url("{uris[n]}");' for n in NAMES if n in uris)
    open(CSS, "w").write(s[:start] + block + s[end:])
    print(f"inlined {len(uris)} icons into styles.css ({os.path.getsize(CSS)//1024} KB)")


if __name__ == "__main__":
    args = sys.argv[1:]
    if args and args[0] != "--rebuild":
        build(args[0], args[1])
    inline()
