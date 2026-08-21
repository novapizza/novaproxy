import math, os
ROOT="/home/claude/nova"; SVG=f"{ROOT}/out/svg"
os.makedirs(SVG, exist_ok=True)

ACCENT="#3EB56D"; ACCENT_DEEP="#2E9A59"; MINT="#9BE3B4"; INDIGO="#8C78F3"
VIOLET="#7C5CE0"; CYAN="#3FA9A0"; INK="#1B1A3D"
SW = 64.0                      # one stroke weight for every relay element
NUDGE = -10.0                   # optical recentre (arrowhead adds ink on the right)

def squircle(x,y,size,n=5.0,steps=160):
    r=size/2; cx,cy=x+r,y+r; pts=[]
    for i in range(steps*4+1):
        t=2*math.pi*i/(steps*4); ct,st=math.cos(t),math.sin(t)
        u=math.copysign(abs(ct)**(2/n),ct); v=math.copysign(abs(st)**(2/n),st)
        pts.append((cx+r*u, cy+r*v))
    return "M %.1f %.1f "%pts[0]+" ".join("L %.1f %.1f"%p for p in pts[1:])+" Z"

def nova(cx,cy,up,down,left,right,bulge=0.20):
    q=lambda hx,vy:(cx+bulge*hx, cy+bulge*vy)
    a=q(right,-up); b=q(right,down); c=q(-left,down); d=q(-left,-up)
    return (f"M {cx:.1f} {cy-up:.1f} Q {a[0]:.1f} {a[1]:.1f} {cx+right:.1f} {cy:.1f} "
            f"Q {b[0]:.1f} {b[1]:.1f} {cx:.1f} {cy+down:.1f} "
            f"Q {c[0]:.1f} {c[1]:.1f} {cx-left:.1f} {cy:.1f} "
            f"Q {d[0]:.1f} {d[1]:.1f} {cx:.1f} {cy-up:.1f} Z")

# ---- the mark: wire in ──  ✦ nova node  ── ▸ forwarded out -------------------
# visual extents chosen so the whole lockup is optically centred on 512
def glyph(scale=1.0, mode="full"):
    """mode: full | node | tray.  Design grid: lockup spans x 96..948, axis y=512."""
    s=scale; C=512.0
    m=lambda v: C+(v-C)*s
    sw=SW*s
    if mode=="full": up, hz, bulge = 274*s, 168*s, 0.200
    elif mode=="tray": up, hz, bulge = 262*s, 190*s, 0.235
    else:            up, hz, bulge = 268*s, 205*s, 0.230
    out=[("fill", nova(C, C, up, up, hz, hz, bulge))]
    if mode in ("full","tray"):
        out.append(("stroke", f"M {m(752):.1f} {C:.1f} L {m(916):.1f} {C:.1f}", sw))
        out.append(("stroke", f"M {m(816):.1f} {m(424):.1f} L {m(916):.1f} {C:.1f} L {m(816):.1f} {m(600):.1f}", sw))
    if mode=="full":
        out.append(("stroke", f"M {m(128):.1f} {C:.1f} L {m(264):.1f} {C:.1f}", sw))
    return out

def draw(parts, fill):
    o=[]
    for p in parts:
        if p[0]=="fill": o.append(f'<path d="{p[1]}" fill="{fill}"/>')
        else: o.append(f'<path d="{p[1]}" fill="none" stroke="{fill}" stroke-width="{p[2]:.1f}" '
                       f'stroke-linecap="round" stroke-linejoin="round"/>')
    return "".join(o)

def DEFS(gx1=150,gy1=170,gx2=874,gy2=850):
    return f'''<defs>
 <linearGradient id="bg" x1="0" y1="0" x2="1" y2="1">
  <stop offset="0" stop-color="{INDIGO}"/><stop offset=".28" stop-color="{VIOLET}"/>
  <stop offset=".62" stop-color="{CYAN}"/><stop offset="1" stop-color="{ACCENT_DEEP}"/></linearGradient>
 <radialGradient id="bloom" cx=".80" cy=".10" r=".78">
  <stop offset="0" stop-color="{MINT}" stop-opacity=".85"/><stop offset=".55" stop-color="{ACCENT}" stop-opacity=".18"/>
  <stop offset="1" stop-color="{ACCENT}" stop-opacity="0"/></radialGradient>
 <radialGradient id="bloom2" cx=".06" cy=".92" r=".70">
  <stop offset="0" stop-color="{INDIGO}" stop-opacity=".70"/><stop offset="1" stop-color="{INDIGO}" stop-opacity="0"/></radialGradient>
 <linearGradient id="gloss" x1="0" y1="0" x2="0" y2="1">
  <stop offset="0" stop-color="#FFFFFF" stop-opacity=".20"/><stop offset=".45" stop-color="#FFFFFF" stop-opacity=".02"/>
  <stop offset="1" stop-color="#FFFFFF" stop-opacity="0"/></linearGradient>
 <linearGradient id="gl" gradientUnits="userSpaceOnUse" x1="{gx1}" y1="{gy1}" x2="{gx2}" y2="{gy2}">
  <stop offset="0" stop-color="#FFFFFF"/><stop offset=".58" stop-color="#FFFFFF"/><stop offset="1" stop-color="#DFF9E9"/></linearGradient>
 <linearGradient id="mk" gradientUnits="userSpaceOnUse" x1="{gx1}" y1="{gy1}" x2="{gx2}" y2="{gy2}">
  <stop offset="0" stop-color="{INDIGO}"/><stop offset=".42" stop-color="{CYAN}"/><stop offset="1" stop-color="{ACCENT_DEEP}"/></linearGradient>
 <filter id="cast" x="-35%" y="-35%" width="170%" height="190%"><feGaussianBlur stdDeviation="20"/></filter>
 <filter id="tsh" x="-25%" y="-25%" width="150%" height="160%"><feGaussianBlur stdDeviation="16"/></filter>
</defs>'''

HDR='<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024" width="1024" height="1024">'

def tile(inset=0.0, tile_shadow=False, small=False):
    box=1024*(1-2*inset); off=1024*inset
    sq=squircle(off,off,box); ts=box/1024.0
    # small=True → simplified node-only glyph, sized up: survives 16–32px
    gs = ts*1.15 if small else ts*0.78
    parts = glyph(gs, "node" if small else "full")
    b=[DEFS(512-426*gs,512-300*gs,512+426*gs,512+300*gs)]
    if tile_shadow:
        b.append(f'<g filter="url(#tsh)" opacity=".20" transform="translate(0 14)"><path d="{sq}" fill="{INK}"/></g>')
    b+=[f'<path d="{sq}" fill="url(#bg)"/>',f'<path d="{sq}" fill="url(#bloom)"/>',
        f'<path d="{sq}" fill="url(#bloom2)"/>',f'<path d="{sq}" fill="url(#gloss)"/>']
    nudge = 0.0 if small else NUDGE*gs
    g=f'<g transform="translate({nudge:.1f} 0)">'
    b.append(f'<g filter="url(#cast)" opacity=".25" transform="translate(0 {20*gs:.1f})">{g}{draw(parts,INK)}</g></g>')
    b.append(f'{g}{draw(parts,"url(#gl)")}</g>')
    b.append(f'<path d="{sq}" fill="none" stroke="#FFFFFF" stroke-opacity=".16" stroke-width="3.5"/>')
    return HDR+"".join(b)+"</svg>\n"

def mark(mode="full", mono=None, scale=1.30):
    parts=glyph(scale,mode)
    nudge = NUDGE*scale if mode=="full" else 0
    body = "" if mono else DEFS(512-426*scale,512-300*scale,512+426*scale,512+300*scale)
    return (HDR+body+f'<g transform="translate({nudge:.1f} 0)">'
            +draw(parts, mono or "url(#mk)")+"</g></svg>\n")

def wordmark():
    gs=0.36; parts=glyph(gs,"full")
    return f'''<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1240 300" width="1240" height="300">
{DEFS(512-426*gs,512-300*gs,512+426*gs,512+300*gs)}
<g transform="translate({150-512+NUDGE*gs:.1f} {150-512:.1f})">{draw(parts,"url(#mk)")}</g>
<text x="322" y="138" font-family="Petrona, Georgia, serif" font-size="112" letter-spacing="-2" fill="{INK}" dominant-baseline="middle">Nova<tspan fill="{ACCENT_DEEP}">Proxy</tspan></text>
<text x="326" y="210" font-family="JetBrains Mono, ui-monospace, monospace" font-size="26" letter-spacing="5.5" fill="#6B6A86">HTTPS DEBUGGING PROXY</text>
</svg>
'''

files={
 "novaproxy-icon.svg":            tile(0.0),
 "novaproxy-icon-macos.svg":      tile(0.0977, tile_shadow=True),
 "novaproxy-icon-small.svg":      tile(0.0, small=True),
 "novaproxy-icon-small-macos.svg":tile(0.0977, tile_shadow=True, small=True),
 "novaproxy-mark.svg":            mark("full", scale=1.14),
 "novaproxy-mark-node.svg":       mark("node", scale=1.60),
 "novaproxy-tray-template.svg":   mark("tray", mono="#000000", scale=1.28),
 "novaproxy-tray-light.svg":      mark("tray", mono="#FFFFFF", scale=1.28),
 "novaproxy-tray-dark.svg":       mark("tray", mono=INK,       scale=1.28),
 "novaproxy-wordmark.svg":        wordmark(),
 "novaproxy-tray-node-template.svg": mark("node", mono="#000000", scale=1.62),
 "novaproxy-tray-node-light.svg":    mark("node", mono="#FFFFFF", scale=1.62),
 "novaproxy-tray-node-dark.svg":     mark("node", mono=INK,       scale=1.62),
}
for n,c in files.items(): open(f"{SVG}/{n}","w").write(c)
print("ok", len(files))
