import base64, io, os, cairosvg
SVG="out/svg"
def b64png(src, size):
    buf=io.BytesIO(); cairosvg.svg2png(url=f"{SVG}/{src}", write_to=buf, output_width=size, output_height=size)
    return "data:image/png;base64,"+base64.b64encode(buf.getvalue()).decode()
def b64png_w(src, w):
    buf=io.BytesIO(); cairosvg.svg2png(url=f"{SVG}/{src}", write_to=buf, output_width=w)
    return "data:image/png;base64,"+base64.b64encode(buf.getvalue()).decode()

FULL="novaproxy-icon.svg"; MAC="novaproxy-icon-macos.svg"; SMALL="novaproxy-icon-small.svg"
ico   = {s: b64png(SMALL if s<=32 else FULL, s) for s in (512,256,128,96,64,48,32,24,16)}
icoF  = {s: b64png(FULL, s) for s in (32,24,16)}
mac   = {s: b64png(MAC, s)  for s in (256,)}
mark  = b64png("novaproxy-mark.svg", 512)
node  = b64png("novaproxy-mark-node.svg", 512)
trayD = b64png("novaproxy-tray-dark.svg", 88)
trayL = b64png("novaproxy-tray-light.svg", 88)
nodeD = b64png("novaproxy-tray-node-dark.svg", 88)
nodeL = b64png("novaproxy-tray-node-light.svg", 88)
word  = b64png_w("novaproxy-wordmark.svg", 1240)

def ladder(d, sizes):
    return "".join(
      f'<figure class="lad"><div class="cell" style="width:{s if s<=128 else 128}px;height:{s if s<=128 else 128}px">'
      f'<img src="{d[s]}" width="{s if s<=128 else 128}" height="{s if s<=128 else 128}" alt=""></div>'
      f'<figcaption>{s}px</figcaption></figure>' for s in sizes)

def zoomrow(d, sizes):
    return "".join(
      f'<figure class="lad"><div class="cell zoom" style="width:96px;height:96px">'
      f'<img src="{d[s]}" style="width:96px;height:96px" alt=""></div>'
      f'<figcaption>{s}px &rarr; 96</figcaption></figure>' for s in sizes)

HTML = f"""<!doctype html>
<html lang="vi"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>NovaProxy — Logo &amp; App Icon</title>
<link rel="preconnect" href="https://fonts.googleapis.com">
<link rel="preconnect" href="https://fonts.gstatic.com" crossorigin>
<link href="https://fonts.googleapis.com/css2?family=Petrona:wght@400;500&family=Inter:wght@400;500;600;700&family=JetBrains+Mono:wght@400;500&display=swap" rel="stylesheet">
<link rel="icon" href="{ico[64]}">
<style>
:root{{
  --accent:#3EB56D; --accent-hover:#2E9A59; --mint:#9BE3B4;
  --bg:#F2F6F3; --panel:rgba(255,255,255,.62); --card:#fff; --input:#F7FAF8;
  --border:rgba(27,26,61,.07); --border2:rgba(27,26,61,.12);
  --text:#1B1A3D; --text2:#3B3A5C; --muted:#6B6A86; --faint:#A9A8BE;
  --indigo:#8C78F3; --violet:#7C5CE0; --cyan:#3FA9A0;
  --r-card:16px; --r-panel:20px; --r-field:12px;
  --e-sm:0 6px 18px rgba(27,26,61,.10); --e-md:0 8px 24px rgba(27,26,61,.10);
  --e-lg:0 24px 60px rgba(27,26,61,.14);
  --font-display:'Petrona',Georgia,serif; --font-ui:'Inter',system-ui,sans-serif;
  --font-mono:'JetBrains Mono',ui-monospace,monospace;
}}
*{{box-sizing:border-box}}
body{{margin:0;font-family:var(--font-ui);color:var(--text);
  background:
    radial-gradient(60% 50% at 12% 0%, rgba(155,227,180,.42) 0%, transparent 70%),
    radial-gradient(55% 45% at 92% 8%, rgba(140,120,243,.20) 0%, transparent 72%),
    linear-gradient(160deg,#F8FBF8,#EFF5F1);
  background-attachment:fixed; min-height:100vh;}}
.wrap{{max-width:1120px;margin:0 auto;padding:56px 28px 96px}}
h1{{font-family:var(--font-display);font-weight:400;letter-spacing:-.03em;font-size:52px;margin:0 0 6px}}
h1 em{{font-style:normal;color:var(--accent-hover)}}
.sub{{font-family:var(--font-mono);font-size:12.5px;letter-spacing:.16em;color:var(--muted);text-transform:uppercase;margin:0 0 40px}}
section{{background:var(--card);border:1px solid var(--border);border-radius:var(--r-panel);
  padding:26px 28px 30px;margin:0 0 22px;box-shadow:var(--e-sm)}}
h2{{font-family:var(--font-display);font-weight:400;font-size:26px;letter-spacing:-.02em;margin:0 0 4px}}
.note{{color:var(--muted);font-size:13.5px;line-height:1.65;margin:0 0 22px;max-width:76ch}}
.note b{{color:var(--text2);font-weight:600}}
code{{font-family:var(--font-mono);font-size:12px;background:var(--input);
  border:1px solid var(--border);border-radius:7px;padding:1.5px 6px;color:var(--text2)}}
.row{{display:flex;flex-wrap:wrap;gap:26px;align-items:flex-end}}
.lad{{margin:0;text-align:center}}
.cell{{display:grid;place-items:center;margin:0 auto}}
.cell img{{display:block;image-rendering:auto}}
.zoom img{{image-rendering:pixelated}}
figcaption{{font-family:var(--font-mono);font-size:10.5px;color:var(--faint);margin-top:9px;letter-spacing:.04em}}
.hero{{display:grid;grid-template-columns:1fr 1fr;gap:18px}}
.stage{{border-radius:var(--r-card);padding:44px;display:grid;place-items:center;position:relative;overflow:hidden}}
.stage.light{{background:linear-gradient(150deg,#fff,#EEF4F0);border:1px solid var(--border)}}
.stage.dark{{background:linear-gradient(150deg,#14162B,#1B1A3D)}}
.stage.photo{{background:linear-gradient(135deg,#E9C46A,#F4A261 40%,#E76F51 75%,#8C78F3)}}
.stage.grey{{background:#8A8FA3}}
.stage img{{filter:drop-shadow(0 22px 44px rgba(27,26,61,.28))}}
.bigicon{{width:288px;height:288px;background:url({ico[512]}) center/contain no-repeat;
  filter:drop-shadow(0 22px 44px rgba(27,26,61,.28))}}
.markimg{{width:252px;height:252px;background:center/contain no-repeat}}
.mk1{{background-image:url({mark})}} .mk2{{background-image:url({node})}}
.tag{{position:absolute;top:12px;left:14px;font-family:var(--font-mono);font-size:10px;
  letter-spacing:.1em;text-transform:uppercase;color:var(--faint)}}
.stage.dark .tag,.stage.photo .tag,.stage.grey .tag{{color:rgba(255,255,255,.62)}}
/* dock + taskbar sims */
.dock{{margin-top:8px;background:linear-gradient(150deg,#5B7BA8,#8C78F3 55%,#3EB56D);
  border-radius:var(--r-card);padding:46px 0 18px;display:grid;place-items:center}}
.dockbar{{display:flex;gap:14px;align-items:flex-end;background:rgba(255,255,255,.30);
  backdrop-filter:blur(22px);border:1px solid rgba(255,255,255,.42);
  border-radius:22px;padding:9px 12px;box-shadow:0 14px 34px rgba(0,0,0,.22)}}
.dockbar img{{width:58px;height:58px;display:block}}
.dockbar .ph{{width:58px;height:58px;border-radius:14px;background:rgba(255,255,255,.34)}}
.dot{{width:5px;height:5px;border-radius:50%;background:rgba(255,255,255,.85);margin:6px auto 0}}
.taskbar{{margin-top:14px;background:#1E2029;border-radius:var(--r-card);padding:40px 0 0;display:grid;place-items:center}}
.tbar{{width:100%;background:rgba(32,34,44,.94);border-top:1px solid rgba(255,255,255,.08);
  display:flex;gap:20px;justify-content:center;padding:8px 0;border-radius:0 0 var(--r-card) var(--r-card)}}
.tbar img{{width:26px;height:26px;display:block}}
.tbar .ph{{width:26px;height:26px;border-radius:6px;background:rgba(255,255,255,.16)}}
/* menubar sims */
.mb{{display:flex;align-items:center;gap:16px;padding:0 14px;height:30px;border-radius:9px;
  font-family:var(--font-ui);font-size:12.5px;font-weight:500}}
.mb.macos{{background:rgba(255,255,255,.78);border:1px solid var(--border);color:var(--text2);
  backdrop-filter:blur(18px)}}
.mb.macdark{{background:rgba(28,28,34,.9);color:rgba(255,255,255,.82)}}
.mb img{{width:18px;height:18px;display:block}}
.mb .spacer{{flex:1}}
.grid2{{display:grid;grid-template-columns:1fr 1fr;gap:16px}}
.swatches{{display:flex;gap:10px;flex-wrap:wrap;margin-top:6px}}
.sw{{border-radius:12px;padding:10px 12px;border:1px solid var(--border);min-width:132px;background:var(--input)}}
.sw i{{display:block;height:26px;border-radius:7px;margin-bottom:8px}}
.sw span{{font-family:var(--font-mono);font-size:11px;color:var(--muted);display:block}}
.sw strong{{font-size:11.5px;font-weight:600;letter-spacing:.02em}}
table{{width:100%;border-collapse:collapse;font-size:13px;margin-top:6px}}
th,td{{text-align:left;padding:9px 12px;border-bottom:1px solid var(--border)}}
th{{font-size:10.5px;letter-spacing:.11em;text-transform:uppercase;color:var(--faint);font-weight:600}}
td:first-child{{font-family:var(--font-mono);font-size:11.5px;color:var(--text2);white-space:nowrap}}
td.d{{color:var(--muted)}}
.pill{{display:inline-block;font-family:var(--font-mono);font-size:10px;letter-spacing:.08em;
  text-transform:uppercase;background:rgba(62,181,109,.12);color:var(--accent-hover);
  border-radius:999px;padding:3px 9px;margin-left:8px;vertical-align:2px}}
.wordwrap{{background:var(--input);border:1px solid var(--border);border-radius:var(--r-card);padding:34px;text-align:center}}
.wordwrap img{{max-width:640px;width:100%}}
.wordwrap.dk{{background:#1B1A3D;border-color:transparent}}
.wordwrap.dk img{{filter:invert(1) hue-rotate(180deg) saturate(1.1) brightness(1.25)}}
@media(max-width:860px){{.hero,.grid2{{grid-template-columns:1fr}}}}
</style></head><body><div class="wrap">

<h1>NovaProxy — <em>logo</em> &amp; app icon</h1>
<p class="sub">Nova + tia relay · một trục: dây vào → nhân nova → gói tin đi ra</p>

<section>
  <h2>Ý tưởng</h2>
  <p class="note">Cả mark chỉ có <b>một trục ngang</b> tại tâm: một đoạn <b>dây</b> đi vào bên trái,
  <b>nhân nova</b> 4 cánh ở giữa (điểm trung gian — chính là proxy), và một <b>mũi tên</b> đi ra bên phải.
  Đọc được ngay là "traffic đi xuyên qua một node ở giữa", không phải sparkle AI chung chung.
  Mọi nét relay dùng <b>cùng một độ dày</b> và đầu tròn, khớp với Lucide 1.6px của app.
  Bảng màu lấy nguyên từ <code>design.md</code>: gradient <code>--c-indigo</code> →
  <code>--c-violet</code> → <code>--c-cyan</code> → <code>--accent-hover</code>, cộng bloom
  <code>--mint</code> góc trên phải.</p>
  <div class="swatches">
    <div class="sw"><i style="background:#8C78F3"></i><strong>--c-indigo</strong><span>#8C78F3</span></div>
    <div class="sw"><i style="background:#7C5CE0"></i><strong>--c-violet</strong><span>#7C5CE0</span></div>
    <div class="sw"><i style="background:#3FA9A0"></i><strong>--c-cyan</strong><span>#3FA9A0</span></div>
    <div class="sw"><i style="background:#3EB56D"></i><strong>--accent</strong><span>#3EB56D</span></div>
    <div class="sw"><i style="background:#2E9A59"></i><strong>--accent-hover</strong><span>#2E9A59</span></div>
    <div class="sw"><i style="background:#9BE3B4"></i><strong>--mint</strong><span>#9BE3B4</span></div>
    <div class="sw"><i style="background:#1B1A3D"></i><strong>--text</strong><span>#1B1A3D</span></div>
  </div>
</section>

<section>
  <h2>App icon<span class="pill">full-bleed · windows / linux</span></h2>
  <p class="note">Nền squircle superellipse (n=5) — cùng hình dạng Apple dùng cho Big Sur,
  cũng phù hợp Windows 11.</p>
  <div class="hero">
    <div class="stage light"><span class="tag">light</span><div class="bigicon"></div></div>
    <div class="stage dark"><span class="tag">dark</span><div class="bigicon"></div></div>
    <div class="stage photo"><span class="tag">wallpaper</span><div class="bigicon"></div></div>
    <div class="stage grey"><span class="tag">neutral 50%</span><div class="bigicon"></div></div>
  </div>
</section>

<section>
  <h2>Bản macOS<span class="pill">apple grid · 824/1024 + shadow</span></h2>
  <p class="note">Bản dùng cho <code>icon.icns</code>: tile thu vào 80.5% khung, chừa lề trong suốt
  đúng lưới Apple, kèm cast shadow — nên trong Dock nó có kích thước quang học ngang các app native.</p>
  <div class="dock">
    <div class="dockbar">
      <div><div class="ph"></div></div><div><div class="ph"></div></div>
      <div><img src="{mac[256]}" alt="NovaProxy"><div class="dot"></div></div>
      <div><div class="ph"></div></div><div><div class="ph"></div></div>
    </div>
  </div>
  <div class="taskbar">
    <div style="height:56px"></div>
    <div class="tbar"><div class="ph"></div><div class="ph"></div>
      <img src="{ico[64]}" alt="NovaProxy"><div class="ph"></div><div class="ph"></div></div>
  </div>
</section>

<section>
  <h2>Thang kích thước</h2>
  <p class="note">Render thật ở từng size (không phải scale CSS). Hàng dưới zoom pixelated để soi
  điểm hi sinh ở 16–24px.</p>
  <div class="row">{ladder(ico,[128,96,64,48,32,24,16])}</div>
  <div class="row" style="margin-top:26px">{zoomrow(ico,[32,24,16])}</div>
  <p class="note" style="margin-top:26px">Từ <b>32px trở xuống</b> bộ icon tự chuyển sang
  <b>glyph rút gọn</b> — chỉ còn nhân nova, phóng to lên. So sánh: hàng trên là bản rút gọn
  (đang dùng), hàng dưới là nếu giữ nguyên lockup đầy đủ.</p>
  <div class="row">{zoomrow(icoF,[32,24,16])}</div>
</section>

<section>
  <h2>Tray / menubar<span class="pill">monochrome</span></h2>
  <p class="note">macOS menubar cần <b>template image</b> (đen + alpha, hệ thống tự đảo màu).
  Bản <code>tray-*</code> giữ nova + mũi tên; bản <code>tray-node-*</code> chỉ còn nhân nova —
  chọn bản node nếu bạn thấy mũi tên bị bí ở 16px.</p>
  <div class="grid2">
    <div>
      <div class="mb macos"><span>NovaProxy</span><span style="color:var(--faint)">File</span>
        <span style="color:var(--faint)">Edit</span><span class="spacer"></span>
        <img src="{trayD}" alt=""><img src="{nodeD}" alt=""><span style="font-family:var(--font-mono);font-size:11px">14:32</span></div>
      <figcaption style="text-align:left">macOS menubar — light</figcaption>
    </div>
    <div>
      <div class="mb macdark"><span>NovaProxy</span><span style="opacity:.55">File</span>
        <span style="opacity:.55">Edit</span><span class="spacer"></span>
        <img src="{trayL}" alt=""><img src="{nodeL}" alt=""><span style="font-family:var(--font-mono);font-size:11px">14:32</span></div>
      <figcaption style="text-align:left">macOS menubar / Windows tray — dark</figcaption>
    </div>
  </div>
</section>

<section>
  <h2>Mark trong suốt</h2>
  <p class="note">Dùng cho favicon, README, splash, empty-state. Gradient dùng
  <code>gradientUnits="userSpaceOnUse"</code> nên cả ba phần tử nằm trên <b>một dải màu liên tục</b>,
  không bị mỗi phần tử một gradient riêng.</p>
  <div class="hero">
    <div class="stage light"><span class="tag">lockup</span><div class="markimg mk1"></div></div>
    <div class="stage light"><span class="tag">node only</span><div class="markimg mk2"></div></div>
  </div>
</section>

<section>
  <h2>Wordmark</h2>
  <p class="note">Petrona cho tên, JetBrains Mono cho tagline — đúng phân vai typography trong
  <code>design.md</code>.</p>
  <div class="wordwrap"><img src="{word}" alt="NovaProxy"></div>
</section>

<section>
  <h2>File đã tạo</h2>
  <table>
    <tr><th>Đường dẫn</th><th>Dùng ở đâu</th></tr>
    <tr><td>assets/logo/*.svg</td><td class="d">SVG nguồn — sửa ở đây rồi chạy lại script</td></tr>
    <tr><td>src-tauri/icons/icon.png</td><td class="d">1024px master · window icon, Linux</td></tr>
    <tr><td>src-tauri/icons/icon.icns</td><td class="d">macOS .app — 11 slot (icp4…ic14)</td></tr>
    <tr><td>src-tauri/icons/icon.ico</td><td class="d">Windows .exe / installer — 16…256px</td></tr>
    <tr><td>src-tauri/icons/32x32.png · 128x128.png · 128x128@2x.png</td><td class="d">Tauri / Linux desktop entry</td></tr>\n    <tr><td>assets/logo/novaproxy-icon-small.svg</td><td class="d">Master glyph rút gọn — dùng cho mọi size ≤ 32px</td></tr>
    <tr><td>src-tauri/icons/Square*Logo.png · StoreLogo.png</td><td class="d">MSIX / Microsoft Store tile</td></tr>
    <tr><td>src-tauri/icons/tray/tray-template*.png</td><td class="d">macOS menubar (template, auto invert)</td></tr>
    <tr><td>src-tauri/icons/tray/tray-light*.png · tray-dark*.png</td><td class="d">Windows tray, theo theme</td></tr>
    <tr><td>assets/logo/preview.html</td><td class="d">Trang này</td></tr>
  </table>
</section>

</div></body></html>
"""
os.makedirs("out", exist_ok=True)
open("out/preview.html","w").write(HTML)
print("preview.html", len(HTML))
