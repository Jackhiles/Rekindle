"""Draw Rekindle's icon: writes rekindle.svg (the design), rekindle-256.png and
rekindle.ico (16-256 px) next to this script. Needs Pillow: `python make_icon.py`."""

from pathlib import Path
from PIL import Image, ImageDraw, ImageFilter

HERE = Path(__file__).parent

# Shapes in a 1 x 1 box (y down), as cubic Bezier segments: start point, then
# (control 1, control 2, end) triples.
OUTER = [(0.50, 0.88),
         ((0.31, 0.88), (0.20, 0.75), (0.21, 0.60)),
         ((0.22, 0.47), (0.31, 0.40), (0.36, 0.30)),
         ((0.38, 0.38), (0.42, 0.42), (0.47, 0.44)),
         ((0.46, 0.32), (0.50, 0.20), (0.58, 0.11)),
         ((0.60, 0.24), (0.70, 0.32), (0.76, 0.44)),
         ((0.82, 0.56), (0.81, 0.70), (0.75, 0.78)),
         ((0.69, 0.85), (0.60, 0.88), (0.50, 0.88))]
INNER = [(0.50, 0.86),
         ((0.39, 0.86), (0.34, 0.78), (0.36, 0.70)),
         ((0.38, 0.62), (0.46, 0.57), (0.52, 0.44)),
         ((0.56, 0.54), (0.64, 0.60), (0.65, 0.70)),
         ((0.66, 0.80), (0.60, 0.86), (0.50, 0.86))]

TILE_TOP, TILE_BOTTOM = (40, 38, 52), (16, 15, 22)
OUTER_TOP, OUTER_BOTTOM = (255, 178, 46), (255, 84, 22)
INNER_TOP, INNER_BOTTOM = (255, 236, 160), (255, 252, 236)
GLOW = (255, 110, 30)


def bezier_points(path, size, steps=48):
    pts = [path[0]]
    for c1, c2, end in path[1:]:
        p0 = pts[-1]
        for i in range(1, steps + 1):
            t = i / steps
            u = 1 - t
            x = u**3 * p0[0] + 3 * u * u * t * c1[0] + 3 * u * t * t * c2[0] + t**3 * end[0]
            y = u**3 * p0[1] + 3 * u * u * t * c1[1] + 3 * u * t * t * c2[1] + t**3 * end[1]
            pts.append((x, y))
    return [(x * size, y * size) for x, y in pts]


def vertical_gradient(size, top, bottom):
    img = Image.new("RGBA", (size, size))
    d = ImageDraw.Draw(img)
    for y in range(size):
        t = y / max(size - 1, 1)
        d.line([(0, y), (size, y)], fill=tuple(int(a + (b - a) * t) for a, b in zip(top, bottom)) + (255,))
    return img


def render(size):
    s = size * 4  # supersample for smooth edges
    img = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    # Rounded tile.
    tile_mask = Image.new("L", (s, s), 0)
    ImageDraw.Draw(tile_mask).rounded_rectangle([0, 0, s - 1, s - 1], radius=int(s * 0.22), fill=255)
    img.paste(vertical_gradient(s, TILE_TOP, TILE_BOTTOM), (0, 0), tile_mask)
    # Warm glow behind the flame.
    glow = Image.new("L", (s, s), 0)
    ImageDraw.Draw(glow).ellipse([s * 0.22, s * 0.38, s * 0.78, s * 0.94], fill=150)
    glow = glow.filter(ImageFilter.GaussianBlur(s * 0.08))
    glow_mask = Image.composite(glow, Image.new("L", (s, s), 0), tile_mask)
    img.paste(Image.new("RGBA", (s, s), GLOW + (255,)), (0, 0), glow_mask)
    # Flame and core.
    for path, top, bottom in [(OUTER, OUTER_TOP, OUTER_BOTTOM), (INNER, INNER_TOP, INNER_BOTTOM)]:
        mask = Image.new("L", (s, s), 0)
        ImageDraw.Draw(mask).polygon(bezier_points(path, s), fill=255)
        img.paste(vertical_gradient(s, top, bottom), (0, 0), mask)
    return img.resize((size, size), Image.LANCZOS)


def svg_path(path):
    f = lambda p: f"{p[0] * 256:.1f},{p[1] * 256:.1f}"
    return "M" + f(path[0]) + "".join(f" C{f(a)} {f(b)} {f(c)}" for a, b, c in path[1:]) + " Z"


def write_svg():
    hexc = lambda c: "#%02x%02x%02x" % c
    svg = f'''<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 256 256" width="256" height="256">
  <defs>
    <linearGradient id="tile" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="{hexc(TILE_TOP)}"/><stop offset="1" stop-color="{hexc(TILE_BOTTOM)}"/></linearGradient>
    <linearGradient id="outer" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="{hexc(OUTER_TOP)}"/><stop offset="1" stop-color="{hexc(OUTER_BOTTOM)}"/></linearGradient>
    <linearGradient id="inner" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="{hexc(INNER_TOP)}"/><stop offset="1" stop-color="{hexc(INNER_BOTTOM)}"/></linearGradient>
    <radialGradient id="glow" cx="0.5" cy="0.66" r="0.32"><stop offset="0" stop-color="{hexc(GLOW)}" stop-opacity="0.6"/><stop offset="1" stop-color="{hexc(GLOW)}" stop-opacity="0"/></radialGradient>
  </defs>
  <rect width="256" height="256" rx="56" fill="url(#tile)"/>
  <rect width="256" height="256" rx="56" fill="url(#glow)"/>
  <path d="{svg_path(OUTER)}" fill="url(#outer)"/>
  <path d="{svg_path(INNER)}" fill="url(#inner)"/>
</svg>
'''
    (HERE / "rekindle.svg").write_text(svg, encoding="utf-8")


if __name__ == "__main__":
    write_svg()
    big = render(256)
    big.save(HERE / "rekindle-256.png")
    sizes = [16, 24, 32, 48, 64, 128, 256]
    render(256).save(HERE / "rekindle.ico", sizes=[(n, n) for n in sizes])
    print("wrote rekindle.svg, rekindle-256.png, rekindle.ico")
