# NovaProxy — Design Reference

> The visual language the app is built from, imported from the Claude Design project
> *NovaProxy Design System* (`NovaProxy Design System.dc.html` for the tokens and parts,
> `NovaProxy-standalone-src.dc.html` for the screens). This documents the system so any
> session can extend the UI consistently. `src/styles.css` is the implementation.

NovaProxy is a **desktop HTTPS-debugging proxy** (Charles/Proxyman-style) — it captures,
inspects, and manipulates live network traffic. The window is a left icon rail, a header bar,
one of five rail-switched sections, and a status bar, plus a command palette and overlays.

---

## 1. Look & Feel

- **Tone:** Professional developer tool, but light and calm rather than dense and dark.
  Depth comes from white panels floating on a mint wash with hairline borders — never from
  heavy strokes or dark surfaces.
- **One theme.** Light only. There is no `data-nova-theme` switch and no user-configurable
  accent; the green below *is* the accent. (Both existed before the redesign and were removed
  deliberately — do not reintroduce a second palette without redoing the whole system.)
- **Typography — three families, three jobs:**
  - **Petrona** (`--font-display`) — page titles only. Serif, 400 weight, negative tracking.
  - **Inter** (`--font-ui`) — everything a human wrote: labels, buttons, prose.
  - **JetBrains Mono** (`--font-mono`) — everything a machine wrote: URLs, headers, hashes,
    durations, status codes, code. If the proxy captured it, it is mono.
  All three are bundled locally via `@fontsource-variable/*` and imported in `src/main.tsx`,
  so the app keeps its typography with no network.
- **Green means live.** The accent is for the thing that acts — active rail item, primary
  button, toggle-on, active tab underline, focus ring, row selection. It never fills a
  whole surface.
- **Icons:** Lucide at **1.6px stroke**, via `src/icons.tsx`. That module owns the
  name→glyph map and the stroke weight; import `Icon` from there rather than reaching into
  `lucide-react` directly.

---

## 2. Tokens

All of them live in one `.nova` block at the top of `src/styles.css`. Reference the variable,
never the literal.

### Colour
| Token | Value | Role |
|---|---|---|
| `--accent` / `--accent-hover` | `#3EB56D` / `#2E9A59` | Primary action; hover & press |
| `--mint` | `#9BE3B4` | Gradient blooms, brandmark |
| `--bg` | `#F2F6F3` | Flat canvas base |
| `--wash` | two mint radials over `#F8FBF8 → #EFF5F1` | The page background itself |
| `--panel` | `rgba(255,255,255,.62)` | Rail and bars (with `backdrop-filter: blur(18px)`) |
| `--list` | `rgba(255,255,255,.82)` | Flow-list panel |
| `--card` | `#FFFFFF` | Cards, modals, detail panel |
| `--input` | `#F7FAF8` | Inset fields, code surfaces, table key cells |
| `--hover` | `rgba(27,26,61,.05)` | Hover wash |
| `--row-alt` | `rgba(242,246,243,.55)` | Zebra row in the flows table |
| `--thead` | `rgba(242,246,243,.96)` | Sticky table header, with `blur(10px)` |
| `--border` / `--border2` / `--bsoft` | `rgba(27,26,61,.07)` / `.12` / `.045` | Hairline, interactive, inner row |
| `--text` / `--text2` / `--muted` / `--faint` | `#1B1A3D` / `#3B3A5C` / `#6B6A86` / `#A9A8BE` | Four-level ink hierarchy |
| `--shadow` / `--overlay` | `rgba(27,26,61,.10)` / `.28` | Modal shadow, scrim |

**Semantics.** Green, salmon and violet come from the design. Blue, cyan, amber and pink are
**derived** — the design ships no equivalents, and the app needs seven for methods, chips,
timing phases and cert states. They are pitched in the same low-chroma, ink-leaning register;
if the design system ever specifies them, replace these values.

| Token | Value | Designed? | Used for |
|---|---|---|---|
| `--c-green` / `--c-green-deep` | `#3EB56D` / `#2E9A59` | yes | 2xx, GET/POST, download phase |
| `--c-violet` | `#7C5CE0` | yes | 3xx, PATCH, MCP, keywords |
| `--c-indigo` | `#8C78F3` | yes | WebSocket, MCP badge, gradients |
| `--c-red` / `--c-red-deep` | `#D88689` / `#A85B5E` | yes | 4xx / 5xx, PUT/DELETE, destructive |
| `--c-blue` | `#5E86C9` | derived | 1xx, TCP connect, plaintext chip |
| `--c-cyan` | `#3FA9A0` | derived | TLS handshake, resent |
| `--c-amber` | `#C08A4A` | derived | Server wait, armed breakpoint, untrusted CA |
| `--c-pink` | `#C77BB4` | derived | Spare semantic slot |

### Scale
```
radius     --r-tag 8  --r-field 12  --r-card 16  --r-panel 20  --r-pill 999
elevation  --e-xs 0 2px 6px  ·  --e-sm 0 6px 18px  ·  --e-md 0 8px 24px  ·  --e-lg 0 24px 60px
           --e-pop 0 14px 34px (dropdown panels), all rgba(27,26,61,·),
           plus --e-accent for green buttons
table      --row-h 34  --thead-h 32  --tree-w 252  --table-min 1140
spacing    4 icon→label · 8 chip gaps · 12 card gaps · 24 panel padding · 48 section breaks
motion     --t-micro 120ms · --t-base 200ms · --t-page 360ms, all on --ease
           cubic-bezier(.2,.8,.2,1)
```

### Type ramp
| Role | Spec |
|---|---|
| Page title | Petrona 400 · 38px · −0.018em |
| Section head | Petrona 400 · 27px · −0.015em |
| Body | Inter 400 · 14/1.6 |
| UI label | Inter 500 · 13px |
| Eyebrow | Inter 600 · 11px · +0.09–0.11em · uppercase |
| Data | JetBrains Mono 400 · 12px |

### Scrollbars
```css
::-webkit-scrollbar{width:10px;height:10px}
::-webkit-scrollbar-thumb{background:rgba(27,26,61,.16);border-radius:999px;border:3px solid transparent;background-clip:content-box}
::-webkit-scrollbar-track{background:transparent}
```

---

## 3. Layout

The root is a full-viewport flex column. There is **no in-app titlebar** — the OS draws the
window chrome, and the design does not double it.

```
┌──────┬───────────────────────────────────────────────────────────────┐
│ rail │  header (62px): title/sub │ Recording │ Clear │ … │ ⌘P │ proxy │
│ 78px ├────────────┬──────────────────────────────────────────────────┤
│      │ tree 252px │ Proto / Type / Status chip groups   Reset filters │
│  ≈   │  Favorites ├──────────────────────────────────────────────────┤
│  ⑂   │  All       │ table — sticky head 32px, 34px rows, zebra       │
│  ⏸   │  Apps      ├──────────────────────────────────────────────────┤
│  {}   │  Domains   │ POST · 204 · url                 N rows · 1 sel  │
│  ⛨   │            ├───────────────────────┬──────────────────────────┤
│  ⚙   │  ⌘⇧F       │ Request  Header Query │ Response  Header Body …  │
├──────┴────────────┴───────────────────────┴──────────────────────────┤
│ ● recording  N flows · M hosts  Auto Select   ↑17 ↓4 KB/s  CA  host  │  30px
└──────────────────────────────────────────────────────────────────────┘
```

### Rail (78px)
`--panel` + blur, right hairline. The 32px brandmark on top, then five items —
**Flows, Rules, Break, Scripts, Certs** — each a 58px-wide, 16px-radius stack of a 19px
Lucide icon over a 10px/600 label. Active item is a **white pill** with `--e-sm` and a
hairline border, icon and label in `--accent`; inactive is `--muted` on transparent, hover
`--hover`. A spacer pushes the settings gear to the bottom.

### Header (62px)
Bottom hairline, no fill. Left: the section title (Inter 600/14.5px) over
`default workspace · host:port`. Then a hairline divider and pill buttons —
**Recording/Paused** (salmon tint + pulsing dot when live) and **Clear**. Right:
**Commands** with a `⌘P` keycap, then the **System proxy** switch. The app filter is not
here — it is a row in the Flows scope tree (§4.1).

### Status bar (30px)
`rgba(255,255,255,.6)`, top hairline, 11px mono `--muted`. Left: a state pip (pulsing salmon
when recording) and `recording | paused | stopped`, then `N flows · M hosts`, then the
**Auto Select** toggle (follow-tail: keep the newest row selected). Right: throughput
(`↑ 17 KB/s ↓ 4 KB/s`, from `src/stats.ts`), `upstream: direct`, CA trust (green/amber),
bind address.

> Throughput lives here rather than in a card because the flows section spends its height on
> rows. It is the one number worth watching continuously, and 30px of status bar is where
> every proxy in this class puts it.

---

## 4. Sections

### 4.1 Flows — tree, filter bar, table, dual-pane inspector

> **Target design, not the current build.** What ships today is the older
> stat-strip-over-two-panes layout; this section describes what replaces it, landing in
> phases 3–4 of `issues/0002-flows-table-view.md`. The old anatomy is **not** kept as an
> alternative view — the table replaces it, stat strip and host grouping included.

Four zones in the section body, and the status bar carries the fifth reading (§3).

**Scope tree** (`--tree-w` default, dragged width persisted, `rgba(255,255,255,.66)`). Uppercase eyebrows over
1px-gap rows: 12.5px/500 `--text2`, 14px icon, 9px radius, `--hover` on hover.
- **Favorites** — Pinned, shown only once something is pinned; an empty row for a
  feature nobody has used is a control that does nothing. (Saved filters are
  chips in the filter bar, not rows here: a saved filter *is* a filter.)
- **All traffic** — selected by default: white, accent border tint, accent text.
- **Apps** — one row per `Flow.process`, count right-aligned in 11px mono `--faint`, expanding
  to that app's hosts. A generic Lucide glyph until real bundle icons land.
- **Domains** — one row per host in 11.5px mono, a 12px chevron when it has children,
  expanding into path segments.

Footer: a 32px `--input` field with a filter glyph, placeholder `Filter tree`, `⌘⇧F` keycap.

Selecting a row sets a **scope**, not a query: it ANDs with the chips and the search box.

**Filter bar.** Three chip groups, each behind a 10.5px uppercase eyebrow: `Proto`
(HTTP / HTTPS / WebSocket), `Type` (JSON / GraphQL / MCP / Form / XML / Document / Media /
Other), `Status` (1xx…5xx, ERR). **Multi-select inside a group (OR), AND between groups**; an
empty group means all of it, which is why there is no `All` chip. `.fchip` styling per §5,
with a text-only accent **Reset filters** at the right.

> Three rows cost ~56px that Proxyman spends on one scrollable row. Deliberate: one row
> cannot show which axis a chip belongs to, and `JSON + 4xx` — "which API is failing" — is
> the query this tool exists for.

**Table.** Sticky header (`--thead-h`, `--thead` + `blur(10px)`, bottom hairline, 10.5px/600
uppercase `+.06em` `--muted`) over `--row-h` rows, zebra `--row-alt` on odd rows, `--bsoft`
row hairlines.

| Column | Width | Content |
|---|---|---|
| `#` | 52px | status-class pip, then `seq` in 11px mono |
| URL | `minmax(320px,1fr)` | full URL, mono, ellipsised, 12px right padding |
| Client | 132px | 13px app glyph in `--c-indigo`, then `process` |
| Method | 62px | method badge (§5) |
| Status | 58px | status pill; `ERR` when `error` is set |
| Time | 96px | `started_at` as `HH:mm:ss.mmm` |
| Duration | 78px | right-aligned mono |
| Request / Response | 78px each | right-aligned size — **`–`, never `0 B`, when there is none** |
| SSL | 44px | `lock` decrypted HTTPS · `lock-open` `--faint` plaintext · `lock` `--c-amber` tunneled |

Opt-in columns behind the picker: **Protocol** (`http_version`), **Edited** —
`R`/`S`/`B` for a rule, the script or a breakpoint, each only when it actually
changed something — and **Comment**, the note written on that row in
`--c-indigo`. A pinned row carries an amber pin before its URL, so what is pinned
is visible without switching to the Pinned scope.

> `--table-min` (1140px) is what the full set needs; the **default set is seven** (`#`, URL,
> Client, Method, Status, Duration, SSL) and needs 746. The window's `minWidth` is 940, the
> rail takes 78 and the tree 252 — so the default set fits with the tree hidden (862px) and
> the table scrolls sideways with it open (610px). That is the intended answer at that size:
> **columns drop or the table scrolls; they never squeeze below their content.**

Row states: **selected** takes a 2px accent left border over `rgba(62,181,109,.11)`; hover is
`--hover`; **in flight** shows `···` in `--faint` for status, an empty duration and a pulsing
pip; **error** puts pip and status in `--c-red-deep`; **tunneled** shows host only, and the
body panes say why; **resent** carries a `--c-cyan` marker; **MCP** a violet `MCP` tag after
the path. Three empty states, per §8: nothing captured, capture paused, nothing matching.

> Fixed `--row-h` rows are what let the windowed list trade `sliceGroups` for a flat
> `sliceFlat` (`src/virtual.ts`): one height, one measurement, no per-group containing blocks.
> Sticky *group* headers were the only reason that geometry was per-group.

**Structured conditions.** Under the search box, hidden until asked for: rows of
*field · operator · value*, each with its own switch, ANDing with each other and
with everything else. The switch is the feature — "show me everything for a
moment" without losing the row you spent a minute building — and the same idea at
filter scale is `⌘B`, which turns every filter off without clearing one. A row
that is off keeps its place at 50% opacity. The panel opens by itself whenever
the active filter carries conditions, because a condition you cannot see is a
filter you cannot read. **Save** turns the whole filter into a chip in the
`Saved` group, named from its own parts.

**Sorting, columns, marks.** A sortable header cycles ascending → descending →
back to capture order, marked by a small `▲`/`▼` in `--accent`; capture order is a
state you can return to, not the absence of one. A hover-only 1px grip on each
header edge resizes that column, and a dragged width replaces the declared track
(the URL column's `1fr` included). The column picker sits at the right of the
filter bar, next to Reset — the decision is made while looking at the table that
is too wide. **Marked** rows (⌘-click, shift-click, `⌘⇧A`) take a `--c-indigo`
wash rather than the accent: they are a batch for the next action, not the one row
the inspector is showing.

**Summary bar** between table and panes: method badge, status pill, the URL in 12px mono with
the host emphasised, then right-aligned `N rows · n selected`.

**Inspector — two panes.** A strip under a top hairline (42% by default, dragged
height persisted), split by a vertical divider. Each pane is a column: a `rgba(243,243,248,.7)` head (12.5px/600 title, a 12px tab
row, a `minus-circle` collapse at the right) over a scrolling body.
- **Request** — Header · Query · Body · Cookies · Raw · Summary
- **Response** — Header · Body · Raw · Treeview · Timing · Summary
- **WebSocket flow** — one pane, `Messages`, replacing both.

Header / Query / Cookies bodies are a 176px key column beside the value in 11.5px mono, with
an uppercase `Key / Value` head and `--bsoft` row hairlines. Body and Raw sit on code surfaces
(§5). Timing keeps the waterfall the old inspector used. **cURL is not a tab** — it is `⌘⇧C`
and a palette command, because it is an action, not a view.

**Three dividers** — sidebar ↔ table, table ↔ inspector, request ↔ response — all
from one `Splitter` (§5), all persisted. The table keeps a floor of five rows and
a header: it is the surface this section exists for, so the inspector yields to it
rather than the other way round.

Every chord in this section is defined in `issues/0003-keyboard-shortcuts.md` §4.2–4.4. The
tree footer keycap and the Auto Select label are the only two places the UI spells one out.

### 4.2 Rules — centred column
Petrona title, a prose subtitle with mono spans in accent, and a green **New rule** pill.
Each rule is an 18px-radius `rgba(255,255,255,.86)` card: a head row of enable switch, mono
uppercase kind tag, name, and a trash icon; then a two-column body of uppercase field label
over an `--input` field.

### 4.3 Breakpoints — centred column
Title, subtitle, then a card with a 46px state tile (amber tint when armed), a title/subtitle
pair, and the Arm/Disarm button, followed by the match-URL field.

### 4.4 Scripts — centred column
Title with the enable switch and **Save & apply** in the head actions. The editor is an
18px-radius card: an `--input` tab strip with a file icon and `tamper.js`, then the textarea
on white at 12.5px/1.85 mono.

### 4.5 Certificate — centred column
Title, subtitle, then the CA card: a 52px gradient shield tile (when trusted), the CA name
with a trust pill, the SHA-256 fingerprint, the path, and a row of pill actions
(**Install & trust** / **Remove certificate** green or `#C4676B`, the rest neutral), closed by
a hint explaining which trust domain is in play. Below it the **SSL proxying scope** card.

---

## 5. Component patterns

### Method badge
`min-width:42px`, radius 5px, 9.5px/500 mono, `+.05em`. Green reads, salmon mutates, violet
is protocol traffic:

| Method | Text | Tint |
|---|---|---|
| GET | `--c-green` | `rgba(62,181,109,.10)` |
| POST | `--c-green-deep` | `rgba(62,181,109,.18)` |
| PUT | `--c-red-deep` | `rgba(216,134,137,.16)` |
| DELETE | `--c-red-deep` | `rgba(216,134,137,.20)` |
| PATCH | `--c-violet` | `rgba(124,92,224,.12)` |
| WS / other | `--c-indigo` | `rgba(140,120,243,.14)` |

### Status scale
`1xx` blue · `2xx` green · `3xx` violet · `4xx` salmon · `5xx`/aborted deep salmon ·
pending `--faint`.

### Data table
One `display:grid` with a fixed `grid-template-columns` shared by the header row and every
body row — never a `<table>`, and never per-row widths, because the two would drift the
moment a column becomes optional. Numerics (duration, sizes) are right-aligned; identifiers
(URL, host, header keys) are mono and ellipsised from the right; **an absent value is `–`, a
zero value is `0 B`** — collapsing the two hides whether a body existed. The header is
`position:sticky` with its own translucent fill, so rows scroll under it rather than past it.

### Splitter
A 7px hit target carrying a 1px accent line that appears only on hover or focus —
three permanent rules across the window would be three more things competing with
the data — over the hairline it replaces, so the panes always have a seam. One
component (`src/Splitter.tsx`) for all three: pointer **capture** rather than
window listeners, so a drag survives the cursor outrunning the handle and releases
itself if the pointer is lost; the value painted live and persisted **once** on
release, because a drag emits hundreds of moves; arrow keys to nudge (12px, 48 with
Shift) so it is not mouse-only; double-click to reset, so a pane dragged to nothing
is recoverable without opening Settings.

### Menu button (filter axis)
A pill with an uppercase eyebrow for the axis, the selection beside it, and a
chevron; a selection **tints** the pill (`rgba(62,181,109,.08)` on a
`rgba(62,181,109,.42)` hairline) rather than filling it, since several sit in one
row. The panel is a `--card` popover with `--e-pop`, capped at 300px and
scrolling, closed by click-outside or Escape (`src/usePopover.ts`) — not by a
scrim, because the point is that the rows behind it stay readable while you pick.

### Toggle switch
40×23px pill track, `rgba(27,26,61,.14)` off / `--accent` on with an accent glow. Knob is a
17px white circle at `top/left: 3px` that `translateX(17px)` on `--t-base`.

### Chips, tags, pills
Filter chips (`.fchip`) are pill-shaped, 11.5px/500, white-ish over a hairline; active is
solid accent with white text. Semantic chips (`.chip`) are pill-shaped, coloured text over a
~10% tint with a ~30% border. Method and kind tags are the squarer 5–8px radius mono form.

Filter chips come in **groups behind an eyebrow** (§4.1): many can be active inside one
group, and a group with none active means "all of it". So an active chip says *narrowed to
this*, never *this is the one setting* — there is no `All` chip to switch back to, only
**Reset filters**.

### Buttons
- **Primary** — accent pill, white text, `--e-accent`, hover `--accent-hover`, press
  `scale(.98)`. `.red` is `#C4676B`. `.cyan` is an alias of primary: the design collapses
  secondary-accent actions into the one green.
- **Neutral** — white pill, `--border2` hairline, `--text2`; hover darkens the border.

### Fields and focus
Inputs sit on `--input` with an 11–12px radius. Focus is
`border-color: rgba(62,181,109,.45)` plus `box-shadow: 0 0 0 3px rgba(62,181,109,.12)` —
a ring, not a colour swap.

### Dropdown
Not a native `<select>` — the design gives every row an icon, washes the selected row and ends
the panel with a destructive row, none of which a native control can carry. `src/Dropdown.tsx`
owns it; the price of leaving the native control is that its keyboard behaviour is written out
there (↑/↓/Home/End over the rows *including* the clear row, Enter/Space to pick, Esc to close
and hand focus back, click-outside to dismiss).

- **Trigger** — the search field's twin: 36px, 11px radius, `--input` on a `--border` hairline,
  12.5px/500 label that ellipsises, `chevron-down` 14px in `--muted`. Hover darkens the border
  to `rgba(27,26,61,.22)`; **open** swaps to `--card` with a `rgba(62,181,109,.45)` border and
  rotates the chevron 180° over `--t-base`. Disabled drops to 55% opacity.
- **Panel** — 42px below the trigger, 14px radius, `--card`, hairline, `--e-pop`; 6px padding,
  `max-height:300px` and its own scroll so a long list (every captured app) cannot outgrow the
  window.
- **Rows** — `8px 10px`, 9px radius, 14px icon in `--muted`; hover is `--bg`. **Selected** is
  the mint wash `rgba(62,181,109,.10)` with the icon and a trailing `check` in `--accent`.
- **Clear row** — a hairline separator, then `x` + label in `--c-red-deep` over a
  `rgba(216,134,137,.12)` hover. Rendered only when there is a filter to clear.

### Brandmark
The mark is **Intercept Node** — a transport axis running across the tile, broken in the
middle, with the letter **N** standing in the break. The two ends are hollow rings (client
and server); the N is the only solid mass on the axis, because the proxy is the only point
that actually holds the data. A four-point nova star sits on the N's right shoulder, the
same relationship NovaPad uses, so the two icons read as one family. Everything is one
stroke family with round caps so it sits with the Lucide set. Its gradient runs
`--accent → --accent-hover`, with a `--mint` bloom top-right on the app tiles. The mark
uses **no second hue**: `--c-indigo` / `--c-violet` / `--c-cyan` are semantic tokens
(status, method, protocol phase) and never brand.

- **At or below 32px the drawing changes** — the hollow rings bleed shut and the star
  disappears, so the wire becomes a solid tick and the N grows. `src/Brandmark.tsx` is that
  simplified form, inline SVG so the gradient runs on tokens; the rail is its only caller,
  at 32px. Below 24px it drops the wire too, leaving the N and the star.
- **The app defaults to light; the app icon does not.** Dock and taskbar icons sit on the
  user's wallpaper, not on our UI, and a white tile disappears on a light wallpaper. So
  `.icns` / `.ico` ship the dark tile, while the light tile (`novaproxy-icon-light.svg`) is
  the default *inside* the app — splash, About, empty state — and on the web and in docs.
- Sources and generators live in `assets/logo/` (`build.py` holds the geometry on a 1024
  grid; `render.py`, `icns.py`, `preview.py` cut the rasters). App-icon output is checked in
  under `src-tauri/icons/`, and `tauri.conf.json` bundles the 32/128/128@2x/`.icns`/`.ico`
  set. `assets/logo/README.md` documents the full ladder, the tray template rule and which
  variant goes where. The previous mark — nova node with wire and arrow, indigo→cyan tile —
  is kept in `assets/logo/_previous/`.

### Code surfaces
`--code-bg` (`#F7FAF8`), hairline border, 14px radius, 12px mono, `line-height:1.7`,
`pre-wrap`, `overflow-x:auto`.

---

## 6. Overlays

`position:absolute` within the root, over a `--overlay` scrim.

- **Intercept modal (z41):** 520px `--card` panel, `--r-panel`, `--e-lg`. Amber pulsing dot,
  the paused method badge + URL, an editable-headers textarea, then **Abort** (danger neutral)
  and **Continue** (primary).
- **Command palette (z50/51):** a 540px `rgba(255,255,255,.92)` blurred sheet, 88px from top.
  Search icon + input + `esc` keycap; rows are a 15px icon, a 12.5px/500 label, and a mono
  keycap, with the active row on `rgba(62,181,109,.10)` and its icon in accent.
- **Settings modal (z40):** 560px, four tabs — General / Network / MCP / Getting started.
- **Shortcuts dialog (z40):** the settings shell at 640px, opened from **Help → Keyboard
  Shortcuts**, `⌘/`, or the palette. A search field (matching label *and* chord) over rows
  grouped by scope: label left, key badges right in the `.kbd` form, formatted per platform
  (`⌘ ⇧ F` on macOS, `Ctrl Shift F` elsewhere). Every row renders from the registry in
  `src/shortcuts.ts` — nothing in this panel is hand-written, which is the whole reason the
  registry exists. The `⌘K` row carries its own warning line: Clear cannot be undone.
- **Onboarding wizard (z40):** the settings shell at 680px — wider than Settings because the
  pip rail carries four full step names — one step at a time. A pip rail
  replaces the tab strip — numbered 18px marks that turn solid accent with a check once the
  step is satisfied — over a fixed-height 210px body (fixed, not a minimum, so Next/Back
  never moves the buttons), a 44px step icon, a live status line on a `.dot`, and a footer
  carrying `Step n of m`, **Skip** and the step's own action. It reads status, never invents
  it: every tick comes from `HelperStatus` / `CaStatus` / `ProxyStatus`.
- **Coachmark (z55):** a 264px card pinned to a control that is already on screen — accent
  border tint, a rotated 10px arrow, one sentence and a text-only **Got it**. The one overlay
  with **no scrim**, because the whole point is that the control underneath stays clickable.
  Positioned inside `.nova`, clamped 12px off either edge.
- **Toast (z60):** bottom-centre white card, 14px radius, accent border tint, green check.

---

## 7. Animations

```css
@keyframes novapulse{0%,100%{opacity:1;transform:scale(1)}50%{opacity:.35;transform:scale(.82)}} /* live dot, breakpoint */
@keyframes novaflow {from{opacity:0;transform:translateY(-4px)}to{opacity:1;transform:translateY(0)}} /* modal, toast */
@keyframes novafade {from{opacity:0}to{opacity:1}}                                                    /* scrims, empty states */
@keyframes novapop  {from{opacity:0;transform:translate(-50%,8px) scale(.97)}to{opacity:1;transform:translate(-50%,0) scale(1)}} /* palette */
```

The flow list is windowed, so rows deliberately have **no** entry animation — one would flash
on every scroll.

---

## 8. Rules of thumb

- **Green means live.** Primary green is for the thing that acts. It never fills a surface.
- **Mono is machine text.** Anything the proxy captured is JetBrains Mono; anything we wrote
  is Inter. Petrona is page titles and nothing else.
- **Contrast by surface.** White on the mint wash plus hairline borders. If something needs
  to stand out, raise it with `--e-sm`, do not darken it.
- **Never hardcode a colour, radius, shadow or font stack.** Every one has a token; a literal
  in a rule is a bug the next palette change will not catch.
- Use the four-level ink hierarchy rather than ad-hoc opacities.
- Uppercase eyebrows (10.5–11px, `+.09em`) label sections and fields.
- Three empty states per list, not one: nothing captured, nothing matching, nothing
  configured. Telling someone to loosen a filter they never set is worse than saying nothing.
- **Columns drop, they never squeeze.** The window can be 940px wide; a table that shrinks
  its columns to fit becomes ten unreadable slivers instead of seven readable ones.
- **A keycap in the UI is a copy of the registry, not a second source.** `src/shortcuts.ts`
  owns every chord; a hardcoded `⌘K` in JSX is the bug that survives the next rebinding.
