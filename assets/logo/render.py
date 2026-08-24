"""SVG -> PNG / .ico. Run after build.py, from this directory."""
import io, os, struct
import cairosvg
from PIL import Image

HERE = os.path.dirname(os.path.abspath(__file__))
SVG = os.path.join(HERE, "out", "svg")
ICO = os.path.join(HERE, "out", "icons")
TRAY = os.path.join(ICO, "tray")
EX = os.path.join(HERE, "out", "png")
WEB = os.path.join(HERE, "out", "web")
for d in (ICO, TRAY, EX, WEB):
    os.makedirs(d, exist_ok=True)

FULL = "novaproxy-icon.svg"
MAC = "novaproxy-icon-macos.svg"
SMALL = "novaproxy-icon-small.svg"            # simplified glyph, <= 32px
LIGHT = "novaproxy-icon-light.svg"
LIGHT_SMALL = "novaproxy-icon-light-small.svg"


def raw(src, size):
    buf = io.BytesIO()
    cairosvg.svg2png(url=os.path.join(SVG, src), write_to=buf,
                     output_width=size, output_height=size)
    return buf.getvalue()


def png(src, dst, size):
    open(dst, "wb").write(raw(src, size))


def pick(size, dark=True):
    """Below 40px the rings bleed shut — drop to the simplified master."""
    if dark:
        return SMALL if size <= 32 else FULL
    return LIGHT_SMALL if size <= 32 else LIGHT


# ---- Tauri standard icon set (dark master: it sits on the wallpaper) --------
png(FULL, f"{ICO}/icon.png", 1024)
png(SMALL, f"{ICO}/32x32.png", 32)
png(FULL, f"{ICO}/128x128.png", 128)
png(FULL, f"{ICO}/128x128@2x.png", 256)
for n in (30, 44, 71, 89, 107, 142, 150, 284, 310):
    png(pick(n), f"{ICO}/Square{n}x{n}Logo.png", n)
png(FULL, f"{ICO}/StoreLogo.png", 50)
png(SMALL, f"{ICO}/icon-small.png", 512)

# ---- Windows .ico ----------------------------------------------------------
def dib(png_bytes, size):
    """A 32-bit bottom-up DIB plus the 1-bit AND mask the format still asks for.

    Only the 256 entry is PNG. Windows has read PNG at any size since Vista,
    but plenty of shell surfaces and Win32 callers (LoadIcon, older installer
    UI, remote sessions) still expect a DIB below that, and fall back to a
    blank or a scaled neighbour when they do not get one.
    """
    px = Image.open(io.BytesIO(png_bytes)).convert("RGBA").load()
    xor = bytearray()
    for y in range(size - 1, -1, -1):          # DIB rows run bottom-up
        for x in range(size):
            r, g, b, a = px[x, y]
            xor += bytes((b, g, r, a))
    # Alpha carries the shape, so the mask is all-opaque — but its rows are
    # 1bpp padded to 4 bytes, and the size has to be right or Windows reads
    # the next entry as mask data.
    mask = bytes(((size + 31) // 32) * 4 * size)
    head = struct.pack("<IiiHHIIiiII", 40, size, size * 2, 1, 32, 0,
                       len(xor) + len(mask), 0, 0, 0, 0)
    return head + bytes(xor) + mask


def ico_bytes(entries):
    hdr = struct.pack("<HHH", 0, 1, len(entries))
    off = 6 + 16 * len(entries)
    dir_, blob = b"", b""
    for sz, data in entries:
        d = 0 if sz >= 256 else sz
        dir_ += struct.pack("<BBBBHHII", d, d, 0, 0, 1, 32, len(data), off)
        off += len(data)
        blob += data
    return hdr + dir_ + blob


def ico_entry(src, size):
    data = raw(src, size)
    return (size, data if size >= 256 else dib(data, size))


sizes = (256, 128, 64, 48, 32, 24, 16)
open(f"{ICO}/icon.ico", "wb").write(ico_bytes([ico_entry(pick(s), s) for s in sizes]))

# ---- in-app / web / docs: the light tile is the default ---------------------
for s in (16, 32, 48, 180, 192, 256, 512):
    png(pick(s, dark=False), f"{WEB}/favicon-{s}.png", s)
open(f"{WEB}/favicon.ico", "wb").write(
    ico_bytes([ico_entry(pick(s, dark=False), s) for s in (48, 32, 16)]))
for name, src in (("apple-touch-icon.png", LIGHT), ("maskable-512.png", LIGHT)):
    png(src, f"{WEB}/{name}", 512 if "512" in name else 180)
png(LIGHT, f"{EX}/novaproxy-icon-light-512.png", 512)
png(LIGHT, f"{EX}/splash-light-256.png", 256)
png(FULL, f"{EX}/splash-dark-256.png", 256)

# ---- tray / menubar --------------------------------------------------------
for kind in ("template", "light", "dark"):
    src = f"novaproxy-tray-{kind}.svg"
    for s, suf in ((16, ""), (32, "@2x"), (48, "@3x"), (22, "-22"), (44, "-22@2x")):
        png(src, f"{TRAY}/tray-{kind}{suf}.png", s)
    src = f"novaproxy-tray-node-{kind}.svg"
    for s, suf in ((16, ""), (32, "@2x"), (48, "@3x")):
        png(src, f"{TRAY}/tray-node-{kind}{suf}.png", s)

# ---- docs PNGs -------------------------------------------------------------
png(FULL, f"{EX}/novaproxy-icon-512.png", 512)
png(MAC, f"{EX}/novaproxy-icon-macos-512.png", 512)
png("novaproxy-mark.svg", f"{EX}/novaproxy-mark-512.png", 512)
png("novaproxy-mark-on-dark.svg", f"{EX}/novaproxy-mark-on-dark-512.png", 512)
cairosvg.svg2png(url=os.path.join(SVG, "novaproxy-wordmark.svg"),
                 write_to=f"{EX}/novaproxy-wordmark-1240.png", output_width=1240)
cairosvg.svg2png(url=os.path.join(SVG, "novaproxy-wordmark-on-dark.svg"),
                 write_to=f"{EX}/novaproxy-wordmark-on-dark-1240.png", output_width=1240)

total = 0
for r, _, fs in os.walk(os.path.join(HERE, "out")):
    for f in fs:
        total += 1
print("rendered", total, "files")
