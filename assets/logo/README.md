# NovaProxy — logo & app icon

Concept: **nova + tia relay**. One horizontal axis at the centre — an incoming **wire**
(left dash), the **nova node** in the middle (the proxy itself), and an outgoing **arrow**
(right). Every relay element shares one stroke weight with round caps, matching the app's
Lucide 1.6px icon set. Colours are taken straight from `design.md`:
`--c-indigo → --c-violet → --c-cyan → --accent-hover`, plus a `--mint` bloom top-right.

## Layout

```
assets/logo/
  novaproxy-icon.svg              full-bleed squircle master (Windows / Linux / generic)
  novaproxy-icon-macos.svg        Apple grid — tile inset to 824/1024 + cast shadow (.icns)
  novaproxy-icon-small.svg        simplified glyph (node only, scaled up) — every size <= 32px
  novaproxy-icon-small-macos.svg  same, Apple grid
  novaproxy-mark.svg              transparent gradient lockup (favicon, README, splash)
  novaproxy-mark-node.svg         transparent gradient node only
  novaproxy-mark-simple.svg       node only, alternate proportions
  novaproxy-tray-template.svg     macOS menubar template (black + alpha)
  novaproxy-tray-light.svg        white — dark menubars / Windows dark tray
  novaproxy-tray-dark.svg         ink — light Windows tray
  novaproxy-tray-node-*.svg       tray variants without the arrow (safest at 16px)
  novaproxy-wordmark.svg          horizontal wordmark, Petrona + JetBrains Mono
  preview.html                    contact sheet: sizes, Dock/taskbar/menubar mock-ups
  build.py render.py icns.py preview.py   generators (see below)

src-tauri/icons/
  icon.png            1024 master
  icon.icns           macOS — 11 slots (icp4 icp5 icp6 ic07..ic14)
  icon.ico            Windows — 256 128 64 48 32 24 16
  32x32.png  128x128.png  128x128@2x.png
  Square{30,44,71,89,107,142,150,284,310}x*Logo.png  StoreLogo.png   MSIX / Store tiles
  tray/tray-{template,light,dark}[-22][@2x|@3x].png
  tray/tray-node-{template,light,dark}[@2x|@3x].png
```

Sizes <= 32px (and the `icp4` / `icp5` / `ic11` icns slots, and the 16/24/32 `.ico` entries)
are rendered from `novaproxy-icon-small.svg`: the wire and arrow disappear into mush below
about 40px, so those sizes drop to the nova node alone, scaled up.

## Regenerating

```sh
pip install cairosvg pillow
python3 build.py     # SVG sources  -> out/svg
python3 render.py    # PNG / .ico   -> out/icons
python3 icns.py      # .icns        -> out/icons/icon.icns
python3 preview.py   # contact sheet
```

Edit the geometry in `build.py` (`glyph()` holds the whole mark on a 1024 grid; `SW` is the
single relay stroke weight, `NUDGE` the optical recentre) rather than touching the SVGs.

## Tray icon in Tauri

macOS menubar icons must be **template images** — black with alpha, the system inverts them:

```rust
TrayIconBuilder::new()
    .icon(Image::from_bytes(include_bytes!("../icons/tray/tray-template@2x.png"))?)
    .icon_as_template(true)   // macOS only; on Windows use tray-light/tray-dark
    .build(app)?;
```
