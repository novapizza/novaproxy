---
name: icons
description: Regenerate NovaProxy's logo, app icons, tray images and favicons from the SVG sources in assets/logo. Use when the mark changes, when an icon looks wrong at some size, or when asked to rebuild the icon set.
---

# Regenerating the icon set

Everything is generated from four scripts in `assets/logo/`. Never hand-edit the
SVGs in `out/` or the rasters in `src-tauri/icons/` — they are outputs.

The geometry lives in the constants at the top of `build.py`, on a 1024 grid.
`GAP` is the sensitive one: it is the clearance between the N's stroke ends and
the transport wire, and below 26 the two touch at 18px.

`assets/logo/README.md` is the reference for which variant goes where.

## Environment

The scripts need `cairosvg` and `pillow`. macOS system Python has neither, and
`cairosvg` needs Homebrew's libcairo on the dynamic loader path:

```sh
python3 -m venv /tmp/logovenv && /tmp/logovenv/bin/pip install cairosvg pillow
export DYLD_FALLBACK_LIBRARY_PATH=/opt/homebrew/lib   # or cairocffi cannot find libcairo
```

Without that export the import fails with `no library called "cairo-2" was found`.

## Run

In order — each reads the previous one's output:

```sh
cd assets/logo
python3 build.py     # geometry  -> out/svg
python3 render.py    # PNG / ICO -> out/icons, out/web, out/png
python3 icns.py      # .icns     -> out/icons/icon.icns
python3 preview.py   # contact sheet -> preview.html
```

`out/` is gitignored. Open `preview.html` to eyeball sizes, mock Dock, taskbar
and menu bar in one sheet.

## Copy only what is checked in

`src-tauri/icons/` holds exactly six files — the five `tauri.conf.json` bundles,
plus the 1024 master:

```sh
cd ../..
for f in 32x32.png 128x128.png 128x128@2x.png icon.png icon.icns icon.ico; do
  cp "assets/logo/out/icons/$f" "src-tauri/icons/$f"
done
cp assets/logo/out/svg/novaproxy-mark-small.svg public/logo.svg
```

The Store tiles, the tray set and the web favicons are generated but **not**
checked in: no bundle target reads the MSIX tiles, no `TrayIconBuilder` exists in
`src-tauri/`, and `index.html` points at `/logo.svg`. Copy them out of `out/`
when something actually starts using them.

## Two drawings, not one

From 32px down the hollow end rings bleed shut and the star disappears, so there
is a second master (`*-small.svg`) with a thicker wire and a larger N. Anything
that picks a raster by size has to switch masters at 32:

- `render.py` — `pick()` returns `SMALL` at `size <= 32`
- `icns.py` — `SIMPLE = {"ic04", "ic05", "ic11"}`
- `Brandmark.tsx` — drops the wire entirely below 24px

## The `.icns` slot rule

**The 1x 16 and 32 slots are `ic04` and `ic05`, and they hold raw ARGB, not PNG.**

The format also has `icp4` / `icp5` / `icp6`, documented as PNG-carrying, and
they are a trap: macOS decodes them as ARGB regardless, so a PNG written there
renders as static wherever the system asks for a small icon — the Finder title
bar, the copy/replace dialog on install, list views. This shipped in every build
before 0.2.0.

Apple's `iconutil -c icns` never emits those three. `icns.py` mirrors what it
does emit, slot for slot, and its ARGB encoder is byte-identical to iconutil's.

## The `.ico` rule

Only the 256 entry is PNG. Everything below it is a 32bpp bottom-up DIB plus the
1-bit AND mask — all-opaque, since alpha carries the shape. Windows has read PNG
at any size since Vista, but plenty of shell surfaces and Win32 callers still
want a DIB and fall back to a blank or a scaled neighbour without one.

## Verify

Apple's own decoder is the test that matters for `.icns` — if a slot is wrong,
the extracted PNG is visible noise rather than an error:

```sh
iconutil -c iconset -o /tmp/check.iconset src-tauri/icons/icon.icns
open /tmp/check.iconset          # every size must be a clean glyph
```

For `.ico`, decode each entry and compare against its source PNG:

```python
from PIL import Image
ico = Image.open('src-tauri/icons/icon.ico')
ico.ico.sizes()                          # {(16,16), (24,24), ... (256,256)}
ico.ico.getimage((16, 16))               # must be the glyph, not noise
```

Then look at it in the real thing — small-icon bugs only show up in system UI:

```sh
npm run tauri build -- --bundles app,dmg
open target/release/bundle/dmg/NovaProxy_<version>_aarch64.dmg
```

Check the DMG window's **title bar icon** and the icon in the copy/replace
dialog. macOS caches icons aggressively; `killall Finder` if a stale one sticks.

## Dark or light

The app defaults to light but **the OS icon is the dark tile**. A Dock or taskbar
icon sits on the user's wallpaper, not on our UI, and a white tile disappears on
a light wallpaper. The light tile is the default *inside* the app, on the web and
in docs.
