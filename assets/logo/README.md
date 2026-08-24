# NovaProxy — logo & app icon

Concept: **Intercept Node**. Một trục ngang ở giữa — đường truyền đi vào từ bên
trái, **dừng lại**, rồi đi ra bên phải; chữ **N** ngồi đúng chỗ ngắt đó. Hai đầu là
vòng rỗng (client và server), chữ N là khối đặc duy nhất trên trục, vì proxy là
điểm duy nhất thực sự nắm dữ liệu. Ngôi sao 4 cánh đặt ở vai phải chữ N — cùng
quan hệ như NovaPad, chỉ khác hue.

Mọi nét dùng cùng một họ độ dày với đầu bo tròn, khớp bộ icon Lucide của app.
Màu chỉ lấy từ theme trắng–xanh của app, không có hue thứ hai:
Green 500 `#3EB56D`, Green 600 `#2E9A59`, Mint 200 `#9BE3B4`.

## Sáng hay tối dùng ở đâu

App mặc định light, nhưng **icon hệ điều hành vẫn là bản tối**. Icon trong Dock
hay taskbar nằm trên wallpaper của người dùng, không nằm trên UI của app — tile
trắng bị chìm trên wallpaper sáng. Bản sáng là default cho *bên trong* app và web.

| Dùng ở | Bản |
|---|---|
| Dock macOS, taskbar Windows, `.icns` / `.ico` | tối — `novaproxy-icon.svg` |
| Splash, About, empty state trong app | sáng — `novaproxy-icon-light.svg` |
| Favicon, web, docs, store listing | sáng — `out/web/*` |
| Lockup trên nền sáng (README, landing) | `novaproxy-mark.svg` |
| Lockup trên nền tối | `novaproxy-mark-on-dark.svg` |
| Menu bar macOS | `tray-template*.png` (template image) |
| Tray Windows | `tray-light*.png` / `tray-dark*.png` |

## Hai bản vẽ, không phải một

Từ **32px xuống**, vòng rỗng ở hai đầu bị bịt kín và ngôi sao biến mất. Nên có
bản thứ hai (`*-small.svg`): wire thành nét đặc, chữ N to hơn, ngôi sao to hơn.
Cùng một mark, khác mức chi tiết — như cách một typeface có optical size.

`icon.ico` và `icon.icns` đã tự nhúng đúng bản cho từng slot (16/24/32 và
`icp4` / `icp5` / `ic11` lấy bản nhỏ), không cần làm gì thêm.

`novaproxy-tray-node-*.svg` bỏ hẳn wire, chỉ còn N + ngôi sao — an toàn nhất ở
16px nếu menu bar trông rối.

## Layout

```
assets/logo/
  novaproxy-icon.svg                   squircle full-bleed, tối (Windows / Linux / generic)
  novaproxy-icon-macos.svg             lưới Apple — tile inset 824/1024 + cast shadow (.icns)
  novaproxy-icon-small.svg             glyph đơn giản hoá — mọi size <= 32px
  novaproxy-icon-small-macos.svg       cùng vậy, lưới Apple
  novaproxy-icon-light*.svg            bốn bản trên, nền sáng
  novaproxy-mark.svg                   lockup trong suốt, cho nền sáng
  novaproxy-mark-on-dark.svg           lockup trong suốt, cho nền tối
  novaproxy-mark-small.svg             lockup đơn giản hoá, trong suốt
  novaproxy-tray-template.svg          menubar macOS (đen + alpha)
  novaproxy-tray-light.svg             trắng — menubar tối / tray Windows tối
  novaproxy-tray-dark.svg              ink — tray Windows sáng
  novaproxy-tray-node-*.svg            bản tray không có wire (an toàn nhất ở 16px)
  novaproxy-wordmark.svg               wordmark ngang, nền sáng
  novaproxy-wordmark-on-dark.svg       wordmark ngang, nền tối
  preview.html                         contact sheet: sizes, mock Dock / taskbar / menubar
  build.py render.py icns.py preview.py    generators

src-tauri/icons/
  icon.png            1024 master
  icon.icns           macOS — 11 slot (icp4 icp5 icp6 ic07..ic14)
  icon.ico            Windows — 256 128 64 48 32 24 16
  32x32.png  128x128.png  128x128@2x.png
  Square{30,44,71,89,107,142,150,284,310}x*Logo.png  StoreLogo.png   MSIX / Store tiles
  tray/tray-{template,light,dark}[-22][@2x|@3x].png
  tray/tray-node-{template,light,dark}[@2x|@3x].png
  web/favicon-{16,32,48,180,192,256,512}.png  favicon.ico  apple-touch-icon.png
```

## Regenerating

```sh
pip install cairosvg pillow
python3 build.py     # SVG sources  -> out/svg
python3 render.py    # PNG / .ico   -> out/icons, out/web, out/png
python3 icns.py      # .icns        -> out/icons/icon.icns
python3 preview.py   # contact sheet -> preview.html
```

Sửa hình học trong `build.py` — phần constant ở đầu file giữ toàn bộ mark trên
lưới 1024 — thay vì sửa SVG. `GAP` là con số nhạy nhất: nó là khoảng hở giữa đầu
nét N và đường truyền, hẹp hơn 26 thì ở 18px hai thứ dính vào nhau.

Cả bốn script đều dùng đường dẫn tương đối tới file của chính nó, nên chạy từ
thư mục nào cũng được.

## Thông số

Lưới 1024. Chữ N: nét 80, đầu bo tròn, trục dọc x 418 / 606, đỉnh y 330, đáy y 700.
Đường truyền: nét 34 trên trục y 515, hở 26 so với đầu nét N. Node hai đầu: vòng
rỗng r 38 nét 26. Ngôi sao: r 58, tâm (638, 274).

Bản nhỏ: chữ N nét 92, trục x 404 / 620, đỉnh y 300, đáy y 724; wire nét 46;
ngôi sao r 66 tâm (654, 256).

## Tray icon trong Tauri

Menubar macOS bắt buộc là **template image** — đen với alpha, hệ thống tự đảo màu:

```rust
TrayIconBuilder::new()
    .icon(Image::from_bytes(include_bytes!("../icons/tray/tray-template@2x.png"))?)
    .icon_as_template(true)   // chỉ macOS; Windows dùng tray-light / tray-dark
    .build(app)?;
```

## Bản trước

Mark cũ (ngôi sao làm node, wire + mũi tên, tile gradient indigo→violet→cyan)
nằm trong `_previous/`. Nó không có chữ N và không theo theme trắng–xanh, nên
được thay bằng hướng Intercept Node.
