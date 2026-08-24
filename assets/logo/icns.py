import struct, cairosvg, io
import os
HERE=os.path.dirname(os.path.abspath(__file__))
SVG=os.path.join(HERE,"out","svg")
BIG=os.path.join(SVG,"novaproxy-icon-macos.svg")
SMALL=os.path.join(SVG,"novaproxy-icon-small-macos.svg")  # simplified glyph <=32px
# type -> (pixel size, encoding). Mirrors what Apple's own `iconutil -c icns`
# emits, slot for slot.
#
# The 1x 16 and 32 slots are `ic04`/`ic05` and hold *raw ARGB*, not PNG. The
# PNG-carrying `icp4`/`icp5`/`icp6` types exist in the format but macOS decodes
# them as ARGB regardless, so a PNG written there renders as noise wherever the
# system asks for a small icon — the Finder title bar, the copy/replace dialog.
# Everything from 16@2x up is PNG.
TYPES=[("ic04",16,"argb"),("ic05",32,"argb"),
       ("ic11",32,"png"),("ic12",64,"png"),("ic07",128,"png"),
       ("ic13",256,"png"),("ic08",256,"png"),("ic14",512,"png"),
       ("ic09",512,"png"),("ic10",1024,"png")]
cache={}
def render(sz, src):
    if (sz,src) not in cache:
        buf=io.BytesIO(); cairosvg.svg2png(url=src, write_to=buf, output_width=sz, output_height=sz)
        cache[(sz,src)]=buf.getvalue()
    return cache[(sz,src)]

def pack_rle(data):
    """Apple's PackBits variant: 0x80|n is a run of n+3 of the next byte,
    otherwise a literal run of n+1 bytes."""
    out=bytearray(); i=0; n=len(data)
    while i<n:
        run=1
        while i+run<n and data[i+run]==data[i] and run<130: run+=1
        if run>=3:
            out.append(0x80 | (run-3)); out.append(data[i]); i+=run
        else:
            start=i; i+=1
            while i<n and (i-start)<128:
                if i+2<n and data[i]==data[i+1]==data[i+2]: break
                i+=1
            chunk=data[start:i]; out.append(len(chunk)-1); out+=chunk
    return bytes(out)

def argb(png_bytes):
    """A, R, G, B as four separately RLE'd planes behind an 'ARGB' magic."""
    from PIL import Image
    px=list(Image.open(io.BytesIO(png_bytes)).convert("RGBA").getdata())
    planes=[bytes(p[3] for p in px), bytes(p[0] for p in px),
            bytes(p[1] for p in px), bytes(p[2] for p in px)]
    return b"ARGB"+b"".join(pack_rle(p) for p in planes)

# 16 and 32 get the simplified glyph; everything else the full lockup.
SIMPLE={"ic04","ic05","ic11"}
entries=[]
for t,s,enc in TYPES:
    png=render(s, SMALL if t in SIMPLE else BIG)
    entries.append((t, argb(png) if enc=="argb" else png))
body=b"".join(t.encode()+struct.pack(">I", len(d)+8)+d for t,d in entries)
toc = b"TOC "+struct.pack(">I", 8+8*len(entries))+b"".join(
      t.encode()+struct.pack(">I", len(d)+8) for t,d in entries)
out = b"icns"+struct.pack(">I", 8+len(toc)+len(body))+toc+body
os.makedirs(os.path.join(HERE,"out","icons"),exist_ok=True)
open(os.path.join(HERE,"out","icons","icon.icns"),"wb").write(out)
print("icns", len(out), "slots", len(entries))
