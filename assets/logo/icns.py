import struct, cairosvg, io
BIG="out/svg/novaproxy-icon-macos.svg"
SMALL="out/svg/novaproxy-icon-small-macos.svg"  # simplified glyph <=32px
# type -> pixel size (PNG payload). Covers every slot modern macOS asks for.
TYPES=[("icp4",16),("icp5",32),("icp6",64),("ic07",128),("ic08",256),
       ("ic09",512),("ic10",1024),("ic11",32),("ic12",64),("ic13",256),("ic14",512)]
cache={}
def render(sz, src):
    if (sz,src) not in cache:
        buf=io.BytesIO(); cairosvg.svg2png(url=src, write_to=buf, output_width=sz, output_height=sz)
        cache[(sz,src)]=buf.getvalue()
    return cache[(sz,src)]

# icp4 (16@1x) and ic11 (16@2x) get the simplified glyph; everything else the full lockup
SIMPLE={"icp4","icp5","ic11"}
entries=[(t, render(s, SMALL if t in SIMPLE else BIG)) for t,s in TYPES]
body=b"".join(t.encode()+struct.pack(">I", len(d)+8)+d for t,d in entries)
toc = b"TOC "+struct.pack(">I", 8+8*len(entries))+b"".join(
      t.encode()+struct.pack(">I", len(d)+8) for t,d in entries)
out = b"icns"+struct.pack(">I", 8+len(toc)+len(body))+toc+body
open("out/icons/icon.icns","wb").write(out)
print("icns", len(out), "slots", len(entries))
