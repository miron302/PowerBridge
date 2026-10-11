#!/usr/bin/env python3
"""Generate PowerBridge's retro (early-Macintosh style) app icon.

Outputs into assets/: PowerBridge-1024.png, PowerBridge.iconset/*.png, PowerBridge.icns
Pure Python + Pillow (the .icns is written directly, so iconutil is not required).
"""
import io, os, struct
from PIL import Image, ImageDraw, ImageFilter

OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "assets")

S = 1024                     # canvas
SHAPE = 824                  # macOS icon grid: 824px rounded square centred in the 1024 canvas
MARGIN = (S - SHAPE) // 2
GRID, CELL = 36, 20          # chunky pixel art: 36x36 cells of 20px
ART0 = MARGIN + (SHAPE - GRID * CELL) // 2

BG_TOP, BG_BOT = (48, 146, 80), (19, 108, 50)       # System-1-on-green wallpaper gradient
DARK, DARK_HI = (9, 66, 28), (34, 104, 56)          # dark green "ink" with embossed edge
CREAM, CREAM_HI, CREAM_SH = (241, 237, 215), (255, 253, 242), (198, 194, 168)
SCREEN = (196, 236, 176)                            # pale phosphor green

g = [[None] * GRID for _ in range(GRID)]

def put(x, y, c):
    if 0 <= x < GRID and 0 <= y < GRID:
        g[y][x] = c

def rect(x0, y0, x1, y1, c):
    for y in range(y0, y1 + 1):
        for x in range(x0, x1 + 1):
            put(x, y, c)

def line(x0, y0, x1, y1, c):
    dx, dy = abs(x1 - x0), -abs(y1 - y0)
    sx, sy = (1 if x0 < x1 else -1), (1 if y0 < y1 else -1)
    err = dx + dy
    while True:
        put(x0, y0, c)
        if (x0, y0) == (x1, y1):
            break
        e2 = 2 * err
        if e2 >= dy: err += dy; x0 += sx
        if e2 <= dx: err += dx; y0 += sy

# ---------------------------------------------------------------- compact Macintosh
bx0, bx1, by0, by1 = 6, 29, 2, 32
rect(bx0, by0, bx1, by1, DARK)
rect(bx0 + 1, by0 + 1, bx1 - 1, by1 - 1, CREAM)
rect(bx0 + 1, by0 + 1, bx1 - 1, by0 + 1, CREAM_HI)          # bevel: top / left light
rect(bx0 + 1, by0 + 1, bx0 + 1, by1 - 1, CREAM_HI)
rect(bx0 + 1, by1 - 1, bx1 - 1, by1 - 1, CREAM_SH)          # bottom / right shade
rect(bx1 - 1, by0 + 1, bx1 - 1, by1 - 1, CREAM_SH)
for cx, cy in ((bx0, by0), (bx1, by0), (bx0, by1), (bx1, by1)):   # rounded corners
    put(cx, cy, None)
    put(cx + (1 if cx == bx0 else -1), cy + (1 if cy == by0 else -1), DARK)

# screen bezel + phosphor (interior 16 x 14 cells, origin (10, 6))
sx0, sx1, sy0, sy1 = 9, 26, 5, 20
rect(sx0, sy0, sx1, sy1, DARK)
rect(sx0 + 1, sy0 + 1, sx1 - 1, sy1 - 1, SCREEN)
for cx, cy in ((sx0 + 1, sy0 + 1), (sx1 - 1, sy0 + 1), (sx0 + 1, sy1 - 1), (sx1 - 1, sy1 - 1)):
    put(cx, cy, DARK)                                        # rounded CRT corners

# the bridge on the screen
ox, oy = 10, 6
def b(x, y): put(ox + x, oy + y, DARK)
TL, TR = 3, 12                                               # tower columns (1 cell wide)
for x in range(16): b(x, 9)                                  # deck
for y in range(1, 14): b(TL, y); b(TR, y)                    # towers / piers
cable = {x: round(6 - 5 * ((x - 7.5) / 4.5) ** 2) for x in range(TL, TR + 1)}
for x in range(TL, TR):                                      # main cable (sagging between tower tops)
    line(ox + x, oy + cable[x], ox + x + 1, oy + cable[x + 1], DARK)
line(ox + TL, oy + 1, ox + 0, oy + 8, DARK)                  # back stays
line(ox + TR, oy + 1, ox + 15, oy + 8, DARK)
for x in (5, 10):                                            # hangers
    for y in range(cable[x] + 1, 9): b(x, y)
for x in range(16):                                          # dithered water
    if x not in (TL, TR):
        if x % 2 == 0: b(x, 11); b(x, 13)
        else: b(x, 12)

# floppy slot, vent marks and base
rect(16, 27, 24, 27, DARK)
rect(16, 28, 24, 28, CREAM_SH)
for x in (9, 11, 13): put(x, 27, CREAM_SH)
rect(8, 33, 27, 33, DARK)

# classic dithered drop shadow (50% checkerboard, offset right/down)
mask = {(x, y) for y in range(GRID) for x in range(GRID) if g[y][x] is not None}
for (x, y) in list(mask):
    for dx, dy in ((2, 1), (3, 1), (3, 2), (2, 2)):
        sx_, sy_ = x + dx, y + dy
        if (sx_, sy_) not in mask and 0 <= sx_ < GRID and 0 <= sy_ < GRID and (sx_ + sy_) % 2 == 0:
            g[sy_][sx_] = DARK

# ---------------------------------------------------------------- rendering
def lerp(a, b, t): return tuple(int(round(a[i] + (b[i] - a[i]) * t)) for i in range(3))

def squircle_mask(size, radius, n=5.0, ss=4):
    big = Image.new("L", (size * ss, size * ss), 0)
    d = ImageDraw.Draw(big)
    cx = cy = size * ss / 2
    r = radius * ss
    pts = []
    steps = 720
    import math
    for i in range(steps):
        t = 2 * math.pi * i / steps
        c, s = math.cos(t), math.sin(t)
        x = cx + r * (abs(c) ** (2 / n)) * (1 if c >= 0 else -1)
        y = cy + r * (abs(s) ** (2 / n)) * (1 if s >= 0 else -1)
        pts.append((x, y))
    d.polygon(pts, fill=255)
    return big.resize((size, size), Image.LANCZOS)

def render():
    base = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    shape = squircle_mask(S, SHAPE / 2)
    # soft drop shadow
    sh = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    sh.putalpha(shape.point(lambda v: int(v * 0.40)))
    sh = sh.filter(ImageFilter.GaussianBlur(14))
    sh = Image.composite(sh, Image.new("RGBA", (S, S), (0, 0, 0, 0)), sh.split()[3])
    shifted = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    shifted.paste(sh, (0, 12))
    base = Image.alpha_composite(base, shifted)

    # gradient fill
    grad = Image.new("RGBA", (S, S))
    gp = grad.load()
    for y in range(S):
        t = min(max((y - MARGIN) / SHAPE, 0), 1)
        col = lerp(BG_TOP, BG_BOT, t) + (255,)
        for x in range(S):
            gp[x, y] = col
    tile = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    tile.paste(grad, (0, 0), shape)
    base = Image.alpha_composite(base, tile)

    # faint pixel-grid wallpaper texture (like the reference's tile edges)
    tex = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    td = ImageDraw.Draw(tex)
    for i in range(GRID + 1):
        p = ART0 + i * CELL
        td.line([(p, MARGIN), (p, MARGIN + SHAPE)], fill=(0, 40, 10, 22), width=1)
        td.line([(MARGIN, p), (MARGIN + SHAPE, p)], fill=(0, 40, 10, 22), width=1)
    tex.putalpha(Image.composite(tex.split()[3], Image.new("L", (S, S), 0), shape))
    base = Image.alpha_composite(base, tex)

    # edge light / shade for depth
    rim = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    ImageDraw.Draw(rim).ellipse((0, 0, 0, 0))
    inner = squircle_mask(S, SHAPE / 2 - 3)
    edge = Image.composite(Image.new("L", (S, S), 255), Image.new("L", (S, S), 0), shape)
    edge = Image.eval(edge, lambda v: v)
    ring = Image.new("L", (S, S), 0)
    ring.paste(shape, (0, 0))
    ring = Image.composite(Image.new("L", (S, S), 0), ring, inner)  # shape minus inner = rim
    hi = Image.new("RGBA", (S, S), (255, 255, 255, 0))
    hi.putalpha(ring.point(lambda v: int(v * 0.22)))
    base = Image.alpha_composite(base, hi)

    # pixel art
    art = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    ad = ImageDraw.Draw(art)
    for y in range(GRID):
        for x in range(GRID):
            c = g[y][x]
            if c is None:
                continue
            x0, y0 = ART0 + x * CELL, ART0 + y * CELL
            ad.rectangle((x0 + 1, y0 + 1, x0 + CELL - 2, y0 + CELL - 2), fill=c + (255,))
            if c == DARK:   # embossed tile edge, as in the reference
                ad.rectangle((x0 + 1, y0 + 1, x0 + CELL - 2, y0 + 2), fill=DARK_HI + (255,))
                ad.rectangle((x0 + 1, y0 + 1, x0 + 2, y0 + CELL - 2), fill=DARK_HI + (255,))
    base = Image.alpha_composite(base, art)
    return base

def icns_bytes(images):
    # type code -> pixel size (PNG payloads, macOS 10.7+)
    table = [(b"icp4", 16), (b"icp5", 32), (b"icp6", 64), (b"ic07", 128), (b"ic08", 256),
             (b"ic09", 512), (b"ic10", 1024), (b"ic11", 32), (b"ic12", 64), (b"ic13", 256), (b"ic14", 512)]
    body = b""
    for code, px in table:
        buf = io.BytesIO()
        images[px].save(buf, "PNG", optimize=True)
        data = buf.getvalue()
        body += code + struct.pack(">I", 8 + len(data)) + data
    return b"icns" + struct.pack(">I", 8 + len(body)) + body

def main():
    os.makedirs(OUT, exist_ok=True)
    master = render()
    images = {px: master.resize((px, px), Image.LANCZOS) for px in (16, 32, 64, 128, 256, 512, 1024)}
    master.save(os.path.join(OUT, "PowerBridge-1024.png"))
    iconset = os.path.join(OUT, "PowerBridge.iconset")
    os.makedirs(iconset, exist_ok=True)
    for name, px in [("icon_16x16", 16), ("icon_16x16@2x", 32), ("icon_32x32", 32), ("icon_32x32@2x", 64),
                     ("icon_128x128", 128), ("icon_128x128@2x", 256), ("icon_256x256", 256),
                     ("icon_256x256@2x", 512), ("icon_512x512", 512), ("icon_512x512@2x", 1024)]:
        images[px].save(os.path.join(iconset, name + ".png"))
    with open(os.path.join(OUT, "PowerBridge.icns"), "wb") as f:
        f.write(icns_bytes(images))
    print("wrote", os.path.abspath(OUT))

if __name__ == "__main__":
    main()
