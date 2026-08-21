import os, cairosvg
from PIL import Image
SVG="out/svg"; ICO="out/icons"; TRAY="out/icons/tray"; EX="out/png"
for d in (ICO,TRAY,EX): os.makedirs(d, exist_ok=True)

def png(src, dst, size):
    cairosvg.svg2png(url=f"{SVG}/{src}", write_to=dst, output_width=size, output_height=size)

FULL="novaproxy-icon.svg"; MAC="novaproxy-icon-macos.svg"
SMALL="novaproxy-icon-small.svg"   # simplified glyph for <=32px

# ---- Tauri standard icon set (full-bleed master) ----
png(FULL, f"{ICO}/icon.png", 1024)
png(SMALL, f"{ICO}/32x32.png", 32)
png(FULL, f"{ICO}/128x128.png", 128)
png(FULL, f"{ICO}/128x128@2x.png", 256)
for n in (30,44,71,89,107,142,150,284,310):
    png(SMALL if n <= 44 else FULL, f"{ICO}/Square{n}x{n}Logo.png", n)
png(FULL, f"{ICO}/StoreLogo.png", 50)
png(SMALL, f"{ICO}/icon-small.png", 512)

# ---- Windows .ico (multi-size) ----
# multi-image .ico: large sizes from the full lockup, 16/24/32 from the simplified master
import struct
def ico_bytes(entries):
    hdr=struct.pack("<HHH",0,1,len(entries)); off=6+16*len(entries); dir_=b""; blob=b""
    for sz,data in entries:
        d = 0 if sz>=256 else sz
        dir_+=struct.pack("<BBBBHHII", d,d,0,0,1,32,len(data),off)
        off+=len(data); blob+=data
    return hdr+dir_+blob
import io as _io
def rp(src,sz):
    b=_io.BytesIO(); cairosvg.svg2png(url=f"{SVG}/{src}", write_to=b, output_width=sz, output_height=sz); return b.getvalue()
ico_entries=[(s, rp(SMALL if s<=32 else FULL, s)) for s in (256,128,64,48,32,24,16)]
open(f"{ICO}/icon.ico","wb").write(ico_bytes(ico_entries))

# ---- macOS .icns from the Apple-grid (inset) master ----
mac_sizes=[16,32,64,128,256,512,1024]
tmp={}
for s in mac_sizes:
    p=f"/tmp/mac_{s}.png"; png(MAC,p,s); tmp[s]=p
Image.open(tmp[1024]).convert("RGBA").save(f"{ICO}/icon.icns", format="ICNS")
png(MAC, f"{ICO}/icon-macos-1024.png", 1024)

# ---- tray / menubar ----
for kind,src in [("template","novaproxy-tray-template.svg"),
                 ("light","novaproxy-tray-light.svg"),
                 ("dark","novaproxy-tray-dark.svg")]:
    for s,suf in [(16,""),(32,"@2x"),(48,"@3x"),(22,"-22"),(44,"-22@2x")]:
        png(src, f"{TRAY}/tray-{kind}{suf}.png", s)
for kind,src in [("template","novaproxy-tray-node-template.svg"),
                 ("light","novaproxy-tray-node-light.svg"),
                 ("dark","novaproxy-tray-node-dark.svg")]:
    for s,suf in [(16,""),(32,"@2x"),(48,"@3x")]:
        png(src, f"{TRAY}/tray-node-{kind}{suf}.png", s)

# ---- preview / docs PNGs ----
png(FULL, f"{EX}/novaproxy-icon-512.png", 512)
png(MAC,  f"{EX}/novaproxy-icon-macos-512.png", 512)
png("novaproxy-mark.svg", f"{EX}/novaproxy-mark-512.png", 512)
png("novaproxy-mark-node.svg", f"{EX}/novaproxy-mark-node-512.png", 512)
cairosvg.svg2png(url=f"{SVG}/novaproxy-wordmark.svg", write_to=f"{EX}/novaproxy-wordmark-1240.png", output_width=1240)

for r,_,fs in os.walk("out"):
    for f in sorted(fs): print(os.path.join(r,f), os.path.getsize(os.path.join(r,f)))
