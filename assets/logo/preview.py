"""Contact sheet -> preview.html. Run after render.py, from this directory.

Self-contained: every image is inlined as a data URI so the file can be opened
from anywhere, or attached to a PR, without the out/ tree next to it.
"""
import base64, os

HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(HERE, "out")


def uri(rel):
    path = os.path.join(OUT, rel)
    ext = "svg+xml" if rel.endswith(".svg") else "png"
    return f"data:image/{ext};base64," + base64.b64encode(open(path, "rb").read()).decode()


def img(rel, w, cls=""):
    c = f' class="{cls}"' if cls else ""
    return f'<img src="{uri(rel)}" width="{w}" height="{w}" alt=""{c}>'


ROWS_OS = [16, 24, 32, 48, 64, 128]


# The OS set has no per-size files on disk (they live inside .ico/.icns), so
# render the row straight from the SVG masters at CSS sizes instead.
def svg_row(svg_full, svg_small):
    cells = []
    for s in ROWS_OS:
        src = svg_small if s <= 32 else svg_full
        cells.append(f'<figure>{img("svg/" + src, s)}'
                     f'<figcaption>{s}px{"·s" if s <= 32 else ""}</figcaption></figure>')
    return "".join(cells)


CSS = """
:root{--bg:#F7FBF8;--card:#fff;--ink:#0D1A14;--mut:#5F7469;--line:#DBEAE1;--g:#2E9A59;--neutral:#DFE7E2}
@media(prefers-color-scheme:dark){:root:not([data-theme=light]){
 --bg:#080D0A;--card:#0E1712;--ink:#EAF4EE;--mut:#89A395;--line:#1E2C25;--g:#4CC77A;--neutral:#16211C}}
:root[data-theme=dark]{--bg:#080D0A;--card:#0E1712;--ink:#EAF4EE;--mut:#89A395;--line:#1E2C25;--g:#4CC77A;--neutral:#16211C}
*{box-sizing:border-box}
body{margin:0;background:var(--bg);color:var(--ink);
 font:16px/1.6 "Instrument Sans",system-ui,sans-serif}
.wrap{max-width:1080px;margin:0 auto;padding:40px 24px 72px}
h1{font-size:1.9rem;margin:0 0 6px;letter-spacing:-.02em}
h2{font-size:1.1rem;margin:36px 0 14px;letter-spacing:-.01em}
p.sub{color:var(--mut);margin:0 0 8px;max-width:62ch}
.card.neutral{background:var(--neutral)}
.card{background:var(--card);border:1px solid var(--line);border-radius:16px;padding:20px}
.row{display:flex;align-items:flex-end;gap:22px;flex-wrap:wrap}
figure{margin:0;display:flex;flex-direction:column;align-items:center;gap:7px}
figcaption{font:600 .62rem/1 ui-monospace,monospace;letter-spacing:.09em;
 text-transform:uppercase;color:var(--mut)}
.mock{border-radius:14px;padding:16px 20px;display:flex;align-items:center;gap:14px}
.dock{background:linear-gradient(160deg,#2b3a44,#101a20);gap:12px}
.taskbar{background:#1f2226}
.menubar{background:#e9ecef;gap:10px}
.menubar.d{background:#1b1e22}
.mock span{font:600 .68rem/1 ui-monospace,monospace;letter-spacing:.08em;color:#9fb3ab}
.grid2{display:grid;grid-template-columns:1fr 1fr;gap:16px}
.swatch{display:flex;gap:8px;flex-wrap:wrap;margin-top:10px}
.swatch b{font:600 .66rem/1 ui-monospace,monospace;padding:7px 11px;border-radius:999px;
 border:1px solid var(--line);color:var(--mut);display:flex;align-items:center;gap:7px}
.swatch i{width:15px;height:15px;border-radius:5px;display:block}
"""

HTML = f"""<!doctype html><html><head><meta charset="utf-8">
<title>NovaProxy — logo contact sheet</title><style>{CSS}</style></head><body><div class="wrap">
<h1>NovaProxy — Intercept Node</h1>
<p class="sub">Sinh ra từ <code>build.py</code>. Từ 32px xuống dùng bản đơn giản hoá
(<code>*-small.svg</code>): vòng rỗng bịt kín và ngôi sao biến mất ở size đó.</p>

<h2>Icon hệ điều hành — bản tối</h2>
<div class="card"><div class="row">{svg_row('novaproxy-icon.svg', 'novaproxy-icon-small.svg')}
<figure>{img('svg/novaproxy-icon.svg', 168)}<figcaption>master</figcaption></figure></div></div>

<h2>Trong app / web / docs — bản sáng (default)</h2>
<div class="card neutral"><div class="row">{svg_row('novaproxy-icon-light.svg', 'novaproxy-icon-light-small.svg')}
<figure>{img('svg/novaproxy-icon-light.svg', 168)}<figcaption>master</figcaption></figure></div></div>

<h2>Trong ngữ cảnh</h2>
<div class="card grid2">
 <div class="mock dock">{img('svg/novaproxy-icon-macos.svg', 56)}
  {img('svg/novaproxy-icon.svg', 56)}<span>DOCK</span></div>
 <div class="mock taskbar">{img('svg/novaproxy-icon-small.svg', 24)}<span>TASKBAR</span></div>
 <div class="mock menubar">{img('icons/tray/tray-dark@2x.png', 18)}<span
   style="color:#4a5560">MENU BAR · SÁNG</span></div>
 <div class="mock menubar d">{img('icons/tray/tray-light@2x.png', 18)}<span>MENU BAR · TỐI</span></div>
</div>

<h2>Lockup trong suốt</h2>
<div class="card grid2">
 <div style="background:#fff;border-radius:12px;padding:22px;display:flex;justify-content:center">
  <img src="{uri('svg/novaproxy-mark.svg')}" width="240" alt=""></div>
 <div style="background:#0A100C;border-radius:12px;padding:22px;display:flex;justify-content:center">
  <img src="{uri('svg/novaproxy-mark-on-dark.svg')}" width="240" alt=""></div>
</div>

<h2>Wordmark</h2>
<div class="card"><img src="{uri('svg/novaproxy-wordmark.svg')}" width="620" alt=""></div>

<h2>Màu</h2>
<div class="card"><div class="swatch">
 <b><i style="background:#3EB56D"></i>Green 500 · #3EB56D</b>
 <b><i style="background:#2E9A59"></i>Green 600 · #2E9A59</b>
 <b><i style="background:#9BE3B4"></i>Mint 200 · #9BE3B4</b>
</div></div>
</div></body></html>
"""

if __name__ == "__main__":
    open(os.path.join(HERE, "preview.html"), "w").write(HTML)
    print("preview.html", len(HTML))
