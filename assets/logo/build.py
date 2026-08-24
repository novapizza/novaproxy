"""NovaProxy — logo & app icon sources.

Concept: **Intercept Node**. One horizontal axis at the centre — the wire comes in
from the left, stops, and leaves on the right; the letter **N** sits in the break.
The two ends are hollow rings (client and server); the N is the only solid mass on
the axis, because the proxy is the only point that actually holds the data. The
four-point nova star sits on the N's right shoulder, same relationship as NovaPad.

Everything is one stroke weight family with round caps, matching the app's Lucide
icon set. Palette is the app's white/green theme only — no second hue.

Edit the geometry constants below rather than the generated SVGs.
"""
import math, os

HERE = os.path.dirname(os.path.abspath(__file__))
SVG = os.path.join(HERE, "out", "svg")
os.makedirs(SVG, exist_ok=True)

# ---------------------------------------------------------------- palette
GREEN = "#3EB56D"        # Green 500
GREEN_DEEP = "#2E9A59"   # Green 600
MINT = "#9BE3B4"         # Mint 200
MINT_PALE = "#C9F3D9"
INK = "#0D1A14"

# ---------------------------------------------------------------- geometry
# Full lockup — every size >= 40px. Design grid 1024, axis y = WIRE_Y.
NX_L, NX_R = 418.0, 606.0     # centre lines of the N's two verticals
NY_T, NY_B = 330.0, 700.0     # centre points of the N's apex / base
NSW = 80.0                    # N stroke weight
WIRE_Y = 515.0                # the transport axis
WIRE_W = 34.0
GAP = 26.0                    # clearance between the N and the wire — the number
                              # that decides whether 18px survives
RING_R, RING_W = 38.0, 26.0
END_L, END_R = 150.0, 874.0
STAR = (638.0, 274.0, 58.0)

# Simplified lockup — every size <= 32px. The rings bleed shut and the star
# vanishes at that scale, so the wire becomes a solid tick and the N grows.
C_NX_L, C_NX_R = 404.0, 620.0
C_NY_T, C_NY_B = 300.0, 724.0
C_NSW, C_WIRE_W = 92.0, 46.0
C_SEG = ((154.0, 330.0), (694.0, 870.0))
C_STAR = (654.0, 256.0, 66.0)

N_OUT_L = NX_L - NSW / 2      # 378
N_OUT_R = NX_R + NSW / 2      # 646

STAR_D = ("M0,-1 C.1,-.29 .29,-.1 1,0 .29,.1 .1,.29 0,1 "
          "-.1,.29 -.29,.1 -1,0 -.29,-.1 -.1,-.29 0,-1 Z")


def squircle(x, y, size, n=5.0, steps=160):
    """Superellipse — closer to the macOS/Windows tile than a plain rounded rect."""
    r = size / 2
    cx, cy = x + r, y + r
    pts = []
    for i in range(steps * 4 + 1):
        t = 2 * math.pi * i / (steps * 4)
        ct, st = math.cos(t), math.sin(t)
        u = math.copysign(abs(ct) ** (2 / n), ct)
        v = math.copysign(abs(st) ** (2 / n), st)
        pts.append((cx + r * u, cy + r * v))
    return ("M %.1f %.1f " % pts[0]
            + " ".join("L %.1f %.1f" % p for p in pts[1:]) + " Z")


# ---------------------------------------------------------------- glyph parts
def _stroked(d, w, paint, opacity=None, extra=""):
    op = f' opacity="{opacity}"' if opacity is not None else ""
    return (f'<path d="{d}" fill="none" stroke="{paint}" stroke-width="{w:.1f}" '
            f'stroke-linecap="round" stroke-linejoin="round"{op}{extra}/>')


def _star_path(cx, cy, r, paint):
    return (f'<path transform="translate({cx:.1f},{cy:.1f}) scale({r:.1f})" '
            f'fill="{paint}" d="{STAR_D}"/>')


# Trimmed boxes, measured from the geometry above. Used by the tile-less
# lockups so the art fills its frame instead of floating in padding.
BOX_FULL = (99, 216, 826, 524)     # rings + wire + star
BOX_SMALL_SQ = (131, 99, 762, 762) # simplified with wire ticks, square (tray)
BOX_SMALL = (131, 190, 762, 580)   # simplified, trimmed
BOX_NODE_SQ = (249, 190, 580, 580) # N + star only, square — safest at 16px


def glyph(mode="full", n_paint="url(#ng)", wire_paint=GREEN, star_paint="#FFFFFF",
          wire_op=".95", star=True, wire=True):
    """mode: full | small | tray.

    full  — rings + gapped wire + star (>= 40px)
    small — solid ticks + bigger N + bigger star (<= 32px)
    tray  — like small but no tile behind it, so it stays legible at 16px mono
    """
    out = []
    if mode == "full":
        out.append(_stroked(f"M{NX_L} {NY_B}L{NX_L} {NY_T}L{NX_R} {NY_B}L{NX_R} {NY_T}",
                            NSW, n_paint))
        if wire:
            out.append(_stroked(f"M{END_L + RING_R} {WIRE_Y}L{N_OUT_L - GAP} {WIRE_Y}",
                                WIRE_W, wire_paint, wire_op))
            out.append(_stroked(f"M{N_OUT_R + GAP} {WIRE_Y}L{END_R - RING_R} {WIRE_Y}",
                                WIRE_W, wire_paint, wire_op))
            for cx in (END_L, END_R):
                out.append(f'<circle cx="{cx:.1f}" cy="{WIRE_Y}" r="{RING_R:.1f}" fill="none" '
                           f'stroke="{wire_paint}" stroke-width="{RING_W:.1f}" '
                           f'opacity="{wire_op}"/>')
        if star:
            out.append(_star_path(*STAR[:2], STAR[2], star_paint))
    else:
        out.append(_stroked(f"M{C_NX_L} {C_NY_B}L{C_NX_L} {C_NY_T}L{C_NX_R} {C_NY_B}"
                            f"L{C_NX_R} {C_NY_T}", C_NSW, n_paint))
        if wire:
            for a, b in C_SEG:
                out.append(_stroked(f"M{a} {WIRE_Y}L{b} {WIRE_Y}", C_WIRE_W,
                                    wire_paint, wire_op))
        if star:
            out.append(_star_path(*C_STAR[:2], C_STAR[2], star_paint))
    return "".join(out)


# ---------------------------------------------------------------- defs
HDR = ('<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024" '
       'width="1024" height="1024">')


def defs(theme="dark", inset=0.0):
    """Gradients are userSpaceOnUse so a zero-width path (a bare vertical) still paints."""
    lo, hi = 1024 * inset, 1024 * (1 - inset)
    if theme == "dark":
        bg = f'<stop offset="0" stop-color="#18271F"/><stop offset="1" stop-color="#060A08"/>'
        n1, n2 = MINT_PALE, GREEN
        ring = f'<stop offset="0" stop-color="{MINT}"/><stop offset=".5" stop-color="{GREEN}"/><stop offset="1" stop-color="{MINT}"/>'
        bloom_c, bloom_o = GREEN, ".30"
        gloss = ".14"
    else:
        bg = f'<stop offset="0" stop-color="#FFFFFF"/><stop offset="1" stop-color="#F4FBF7"/>'
        n1, n2 = GREEN, GREEN_DEEP
        ring = f'<stop offset="0" stop-color="#CDEBDB"/><stop offset=".5" stop-color="#8FDCAC"/><stop offset="1" stop-color="#CDEBDB"/>'
        bloom_c, bloom_o = MINT, ".34"
        gloss = "0"
    return f'''<defs>
 <linearGradient id="bg" x1="0" y1="0" x2="1" y2="1">{bg}</linearGradient>
 <radialGradient id="bloom" cx=".24" cy=".14" r=".85">
  <stop offset="0" stop-color="{bloom_c}" stop-opacity="{bloom_o}"/>
  <stop offset="1" stop-color="{bloom_c}" stop-opacity="0"/></radialGradient>
 <linearGradient id="gloss" x1="0" y1="0" x2="0" y2="1">
  <stop offset="0" stop-color="#FFFFFF" stop-opacity="{gloss}"/>
  <stop offset=".5" stop-color="#FFFFFF" stop-opacity="0"/></linearGradient>
 <linearGradient id="ring" x1="0" y1="0" x2="1" y2="1">{ring}</linearGradient>
 <linearGradient id="ng" gradientUnits="userSpaceOnUse"
   x1="{lo + 372 * (hi - lo) / 1024:.1f}" y1="{lo + 248 * (hi - lo) / 1024:.1f}"
   x2="{lo + 652 * (hi - lo) / 1024:.1f}" y2="{lo + 788 * (hi - lo) / 1024:.1f}">
  <stop offset="0" stop-color="{n1}"/><stop offset="1" stop-color="{n2}"/></linearGradient>
 <filter id="tsh" x="-25%" y="-25%" width="150%" height="160%">
  <feGaussianBlur stdDeviation="16"/></filter>
</defs>'''


# ---------------------------------------------------------------- builders
def tile(theme="dark", inset=0.0, tile_shadow=False, small=False):
    box = 1024 * (1 - 2 * inset)
    off = 1024 * inset
    sq = squircle(off, off, box)
    k = box / 1024.0
    star_paint = "#EAFBF1" if theme == "dark" else GREEN_DEEP
    wire_op = ".95" if theme == "dark" else "1"
    wire_paint = GREEN if theme == "dark" else "#79D3A0"
    b = [defs(theme, inset)]
    if tile_shadow:
        b.append(f'<g filter="url(#tsh)" opacity=".22" transform="translate(0 14)">'
                 f'<path d="{sq}" fill="{INK}"/></g>')
    b += [f'<path d="{sq}" fill="url(#bg)"/>',
          f'<path d="{sq}" fill="url(#bloom)"/>',
          f'<path d="{sq}" fill="url(#gloss)"/>']
    art = glyph("small" if small else "full", "url(#ng)", wire_paint, star_paint, wire_op)
    if k != 1.0:
        art = (f'<g transform="translate(512,512) scale({k:.5f}) translate(-512,-512)">'
               f"{art}</g>")
    b.append(art)
    b.append(f'<path d="{sq}" fill="none" stroke="url(#ring)" stroke-width="11" '
             f'opacity=".85"/>')
    return HDR + "".join(b) + "</svg>\n"


def mark(mode="full", theme="light", mono=None, box=None, wire=True):
    """Tile-less lockup, trimmed to the art — favicon, README, splash, tray."""
    if box is None:
        box = BOX_FULL if mode == "full" else BOX_SMALL
    x, y, w, h = box
    hdr = (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="{x} {y} {w} {h}" '
           f'width="{w}" height="{h}">')
    if mono:
        # One colour, no gradient: macOS menubar needs a template image.
        return hdr + glyph(mode, mono, mono, mono, ".62", wire=wire) + "</svg>\n"
    wp = GREEN if theme == "dark" else "#79D3A0"
    star_paint = "#EAFBF1" if theme == "dark" else GREEN_DEEP
    return (hdr + defs(theme)
            + glyph(mode, "url(#ng)", wp, star_paint, wire=wire) + "</svg>\n")


def wordmark(theme="light"):
    """Horizontal lockup. Type is set live — see README before shipping it."""
    text = INK if theme == "light" else "#EAF4EE"
    sub = "#6B7F74" if theme == "light" else "#89A395"
    k = 0.30
    art = glyph("full", "url(#ng)", GREEN if theme == "dark" else "#79D3A0",
                "#EAFBF1" if theme == "dark" else GREEN_DEEP)
    return f'''<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1240 300" width="1240" height="300">
{defs(theme)}
<g transform="translate({150 - 512 * k:.1f} {150 - 512 * k:.1f}) scale({k})">{art}</g>
<text x="356" y="138" font-family="Petrona, Georgia, serif" font-size="112" letter-spacing="-2" fill="{text}" dominant-baseline="middle">Nova<tspan fill="{GREEN_DEEP}">Proxy</tspan></text>
<text x="360" y="212" font-family="JetBrains Mono, ui-monospace, monospace" font-size="26" letter-spacing="5.5" fill="{sub}">HTTPS DEBUGGING PROXY</text>
</svg>
'''


# ---------------------------------------------------------------- emit
MACOS_INSET = 0.0977          # Apple's 824/1024 art box

files = {
    # OS icons — dark tile. The icon sits on the user's wallpaper, not on the
    # app's UI, so it stays dark even though the app defaults to light.
    "novaproxy-icon.svg":             tile("dark", 0.0),
    "novaproxy-icon-macos.svg":       tile("dark", MACOS_INSET, tile_shadow=True),
    "novaproxy-icon-small.svg":       tile("dark", 0.0, small=True),
    "novaproxy-icon-small-macos.svg": tile("dark", MACOS_INSET, tile_shadow=True, small=True),

    # Light tile — in-app (splash, About), web, docs, store listings.
    "novaproxy-icon-light.svg":             tile("light", 0.0),
    "novaproxy-icon-light-macos.svg":       tile("light", MACOS_INSET, tile_shadow=True),
    "novaproxy-icon-light-small.svg":       tile("light", 0.0, small=True),
    "novaproxy-icon-light-small-macos.svg": tile("light", MACOS_INSET, tile_shadow=True, small=True),

    # Transparent lockups. `mark` is the light-background default.
    "novaproxy-mark.svg":            mark("full", "light"),
    "novaproxy-mark-on-dark.svg":    mark("full", "dark"),
    "novaproxy-mark-small.svg":      mark("small", "light"),
    # Kept for callers that predate the Intercept Node mark: node-only forms.
    "novaproxy-mark-node.svg":       mark("small", "light", box=BOX_NODE_SQ, wire=False),
    "novaproxy-mark-simple.svg":     mark("small", "dark", box=BOX_NODE_SQ, wire=False),

    # Tray / menubar — one colour, no tile, simplified glyph, square box.
    "novaproxy-tray-template.svg":   mark("small", mono="#000000", box=BOX_SMALL_SQ),
    "novaproxy-tray-light.svg":      mark("small", mono="#FFFFFF", box=BOX_SMALL_SQ),
    "novaproxy-tray-dark.svg":       mark("small", mono=INK, box=BOX_SMALL_SQ),
    # …-node-* drop the wire entirely: at 16px the ticks are the first thing to
    # turn to mush, and N + star alone still reads.
    "novaproxy-tray-node-template.svg": mark("small", mono="#000000", box=BOX_NODE_SQ, wire=False),
    "novaproxy-tray-node-light.svg":    mark("small", mono="#FFFFFF", box=BOX_NODE_SQ, wire=False),
    "novaproxy-tray-node-dark.svg":     mark("small", mono=INK, box=BOX_NODE_SQ, wire=False),

    "novaproxy-wordmark.svg":         wordmark("light"),
    "novaproxy-wordmark-on-dark.svg": wordmark("dark"),
}

if __name__ == "__main__":
    for n, c in files.items():
        open(os.path.join(SVG, n), "w").write(c)
    print("ok", len(files), "->", SVG)
