# NovaProxy — Design Reference

> The visual language the app is built from: the Nova Suite tokens as the canvas
> *NovaProxy Redesign* applies them (board 01 Flows on Mac, 02 NovaProxy Dark, 03 Windows,
> 04–15 inspector, MCP, filters, palette, rules, breakpoints, scripts, certificate, setup and
> empty states). This documents the system so any session can extend the UI consistently.
> `src/styles.css` is the implementation.

NovaProxy is a **desktop HTTPS-debugging proxy** (Charles/Proxyman-style) — it captures,
inspects, and manipulates live network traffic. The window is a left icon rail, a header bar,
one of five rail-switched sections, and a status bar, plus a command palette and overlays.

---

## 1. Look & Feel

- **Tone:** Professional developer tool, light and calm rather than dense and dark. Depth
  comes from Mist panels beside white work areas with hairline borders, never from heavy
  strokes, glass blur or shadows on resting controls.
- **Two themes, light by default.** NovaProxy Dark is opt-in from the sun/moon button at the
  foot of the rail and is stored as the `theme` pref. Both themes use the same token names;
  dark redefines them under `:root[data-theme="dark"]`. `index.html` applies the saved
  theme before first paint so a dark window does not flash white. There is still no
  user-configurable accent.
- **Typography — three families, three jobs:**
  - **Space Grotesk** (`--font-display`): page titles, the setup dialog, empty-state headlines.
  - **Inter** (`--font-ui`): everything a human wrote: labels, buttons, prose.
  - **JetBrains Mono** (`--font-mono`): everything a machine wrote: URLs, headers, hashes,
    durations, status codes, code, the header subtitle and the status bar. If the proxy
    captured it, it is mono.
  All three are bundled locally via `@fontsource-variable/*` and imported in `src/main.tsx`,
  so the app keeps its typography with no network.
- **Green means live.** NovaProxy green (700 `#237A46` in light, 300 `#9BE3B4` in dark) is the
  only accent: active rail item (as a 10% tint), primary button, toggle-on, chip-on, active
  tab underline, focus ring, row selection. It never fills a whole surface.
- **Icons:** Lucide at **1.6px stroke**, via `src/icons.tsx`. That module owns the
  name→glyph map and the stroke weight; import `Icon` from there rather than reaching into
  `lucide-react` directly.

---

## 2. Tokens

All of them live in the `.nova` block at the top of `src/styles.css` (light) and its
`:root[data-theme="dark"] .nova` twin. Reference the variable, never the literal.

### Colour
| Token | Light | Dark | Role |
|---|---|---|---|
| `--accent` / `--accent-hover` | `#237A46` / `#1C6339` | `#9BE3B4` / `#B9ECCA` | Primary action; hover |
| `--accent-ink` | `#FFFFFF` | `#0B0A10` | Text on an accent fill |
| `--bg` / `--list` | `#FFFFFF` | `#0B0A10` | Window, flows table |
| `--panel` / `--thead` | `#F6F5FA` (Mist) | `#100E17` | Rail, tree, bars, table head, pane heads |
| `--card` | `#FFFFFF` | `#16131F` | Cards, modals |
| `--input` / `--code-bg` | `#EFEDF6` | `#1C1830` | Fields, code surfaces |
| `--hover` | ink 4.5% | white 5% | Hover wash |
| `--row-alt` | ink 2.2% | white 1.8% | Zebra row |
| `--border` / `--border2` / `--bsoft` | ink 9% / 16% / 5% | white 8% / 14% / 5% | Hairline, interactive, inner row |
| `--text` / `--text2` / `--muted` / `--faint` | `#1C1830` / `#3B3A5C` / `#5B5672` / `#8A85A0` | `#EDEAF7` / `#CFC9E4` / `#A9A3BF` / `#6B6585` | Four-level ink |
| `--overlay` | ink 28% | `rgba(7,6,11,.62)` | Scrim |

Tints are written `rgba(var(--accent-rgb), .10)` and the like (`--ink-rgb`, `--shadow-rgb`,
`--surface-rgb`, `--red-rgb`, `--amber-rgb`, `--indigo-rgb`), so both themes reach them.
`color-mix()` would be shorter but needs a newer WebKit than macOS 11 ships.

**Semantics** are the Nova app hues: 700 in light, 300 in dark.

| Token | Light | Dark | Used for |
|---|---|---|---|
| `--c-green` | `#237A46` | `#9BE3B4` | 2xx, GET/POST, download phase |
| `--c-violet` | `#6D28D9` | `#C4B5FD` | 3xx, PATCH, MCP, keywords |
| `--c-indigo` | `#4338CA` | `#A5B4FC` | WebSocket, marked rows |
| `--c-orange` | `#C2410C` | `#FDBA74` | 4xx, PUT |
| `--c-red` / `--c-red-deep` | `#B91C1C` / `#991B1B` | `#FCA5A5` / `#F87171` | 5xx, DELETE, destructive, recording |
| `--c-blue` | `#1D4ED8` | `#93C5FD` | 1xx, TCP connect, plaintext chip |
| `--c-cyan` | `#0E7490` | `#67E8F9` | TLS handshake, resent, GraphQL |
| `--c-amber` | `#B45309` | `#FCD34D` | Server wait, armed breakpoint, untrusted CA |
| `--c-pink` | `#BE185D` | `#F9A8D4` | Spare semantic slot |

### Scale
```
radius     --r-tag 6  --r-field 11  --r-card 18  --r-panel 20  --r-pill 999  (rail items 14)
elevation  --e-xs · --e-sm · --e-md · --e-lg · --e-pop, all rgba(var(--shadow-rgb),·);
           only floating things (menus, modals, toasts) cast a shadow
table      --row-h 32  --thead-h 32  --tree-w 252  --table-min 1140
spacing    4 icon→label · 8 chip gaps · 12 card gaps · 16–20 bar padding · 48 section breaks
motion     --t-micro 120ms · --t-base 200ms · --t-page 360ms, all on --ease
           cubic-bezier(.2,.8,.2,1)
```

### Type ramp
| Role | Spec |
|---|---|
| Page title | Space Grotesk 600 · 34px · −0.03em |
| Empty-state headline | Space Grotesk 600 · 22px · −0.02em |
| Body | Inter 400 · 14/1.6 |
| UI label | Inter 500 · 12.5px |
| Eyebrow | Inter 600 · 10.5px · +0.06–0.09em · uppercase |
| Data | JetBrains Mono 400 · 12px |

### Scrollbars
```css
::-webkit-scrollbar{width:10px;height:10px}
::-webkit-scrollbar-thumb{background:rgba(127,122,150,.32);border-radius:999px;border:3px solid transparent;background-clip:content-box}
::-webkit-scrollbar-track{background:transparent}
```

---

## 3. Layout

The root is a full-viewport flex column. There is **no in-app titlebar** — the OS draws the
window chrome, and the design does not double it.

```
┌──────┬───────────────────────────────────────────────────────────────┐
│ rail │  header (58px): title/sub │ Recording │ Clear │ … │ ⌘P │ proxy │
│ 74px ├────────────┬──────────────────────────────────────────────────┤
│      │ tree 252px │ Proto / Type / Status chip groups   Reset filters │
│  ≈   │  Favorites ├──────────────────────────────────────────────────┤
│  ⑂   │  All       │ table — sticky head 32px, 32px rows, zebra       │
│  ⏸   │  Apps      ├──────────────────────────────────────────────────┤
│  {}   │  Domains   │ POST · 204 · url                 N rows · 1 sel  │
│  ⛨   │            ├───────────────────────┬──────────────────────────┤
│  ⚙   │  ⌘⇧F       │ Request  Header Query │ Response  Header Body …  │
├──────┴────────────┴───────────────────────┴──────────────────────────┤
│ ● recording  N flows · M hosts  Auto Select   ↑17 ↓4 KB/s  CA  host  │  28px
└──────────────────────────────────────────────────────────────────────┘
```

### Rail (74px)
`--panel`, right hairline. The 36px app icon on top, then five items, **Flows, Rules,
Break, Scripts, Certs**, each a 58×54px, 14px-radius stack of a 19px Lucide icon over a
10px/600 label. The active item is a 10% accent tint with icon and label in `--accent`;
inactive is `--faint` on transparent, hover `--hover`. A spacer pushes the theme switch
(sun/moon) and the settings gear to the bottom.

### Header (58px)
Bottom hairline, no fill. Left: the section title (Inter 600/14.5px) over
`default workspace · host:port` in 11px mono `--faint`. Then a hairline divider and 30px
outline pills: **Recording/Paused** (red tint + pulsing dot when live) and **Clear**.
Right: **Commands** with a `⌘P` keycap, then the **System proxy** switch. The app filter is
not here; it is a row in the Flows scope tree (§4.1).

### Status bar (28px)
`--panel`, top hairline, 11px mono `--muted`. Left: a state pip (pulsing red
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
> rail takes 74 and the tree 252 — so the default set fits with the tree hidden (866px) and
> the table scrolls sideways with it open (614px). That is the intended answer at that size:
> **columns drop or the table scrolls; they never squeeze below their content.**

Row states: **selected** takes a 2px accent left border over `accent .09`; hover is
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
Space Grotesk title, a prose subtitle with mono spans in accent, and a green **New rule** pill.
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
`min-width:42px`, radius 5px, 9.5px/500 mono, `+.05em`. Green reads, orange and red
mutate, violet patches, indigo is protocol traffic:

| Method | Text | Tint |
|---|---|---|
| GET | `--c-green` | accent 10% |
| POST | `--c-green` | accent 18% |
| PUT | `--c-orange` | orange 12% |
| DELETE | `--c-red` | red 11% |
| PATCH | `--c-violet` | violet 11% |
| WS / other | `--c-indigo` | indigo 11% |

### Status scale
`1xx` blue · `2xx` green · `3xx` violet · `4xx` orange · `5xx`/aborted red ·
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
chevron; a selection **tints** the pill (`accent .08` on a
`accent .42` hairline) rather than filling it, since several sit in one
row. The panel is a `--card` popover with `--e-pop`, capped at 300px and
scrolling, closed by click-outside or Escape (`src/usePopover.ts`) — not by a
scrim, because the point is that the rows behind it stay readable while you pick.

### Toggle switch
36×20px pill track, `ink .14` off / `--accent` on, flat. Knob is a
16px white circle at `top/left: 2px` that `translateX(16px)` on `--t-base`.

### Chips, tags, pills
Filter chips (`.fchip`) are pill-shaped, 11.5px/500, white-ish over a hairline; active is
solid accent with white text. Semantic chips (`.chip`) are pill-shaped, coloured text over a
~10% tint with a ~30% border. Method and kind tags are the squarer 5–8px radius mono form.

Filter chips come in **groups behind an eyebrow** (§4.1): many can be active inside one
group, and a group with none active means "all of it". So an active chip says *narrowed to
this*, never *this is the one setting* — there is no `All` chip to switch back to, only
**Reset filters**.

### Buttons
- **Primary** — 34px accent pill, `--accent-ink` text, flat (no glow), hover
  `--accent-hover`, press `scale(.98)`. `.red` is `--c-red`. `.cyan` is an alias of primary: the design collapses
  secondary-accent actions into the one green.
- **Neutral** — transparent pill, `--border2` hairline, `--text2`; hover darkens the border.

### Fields and focus
Inputs sit on `--input` with an 11–12px radius. Focus is
`border-color: rgba(var(--accent-rgb),.45)` plus `box-shadow: 0 0 0 3px rgba(var(--accent-rgb),.12)` —
a ring, not a colour swap.

### Dropdown
Not a native `<select>` — the design gives every row an icon, washes the selected row and ends
the panel with a destructive row, none of which a native control can carry. `src/Dropdown.tsx`
owns it; the price of leaving the native control is that its keyboard behaviour is written out
there (↑/↓/Home/End over the rows *including* the clear row, Enter/Space to pick, Esc to close
and hand focus back, click-outside to dismiss).

- **Trigger** — the search field's twin: 36px, 11px radius, `--input` on a `--border` hairline,
  12.5px/500 label that ellipsises, `chevron-down` 14px in `--muted`. Hover darkens the border
  to `ink .22`; **open** swaps to `--card` with a `accent .45` border and
  rotates the chevron 180° over `--t-base`. Disabled drops to 55% opacity.
- **Panel** — 42px below the trigger, 14px radius, `--card`, hairline, `--e-pop`; 6px padding,
  `max-height:300px` and its own scroll so a long list (every captured app) cannot outgrow the
  window.
- **Rows** — `8px 10px`, 9px radius, 14px icon in `--muted`; hover is `--bg`. **Selected** is
  an `accent .10` tint with the icon and a trailing `check` in `--accent`.
- **Clear row** — a hairline separator, then `x` + label in `--c-red-deep` over a
  `rgba(216,134,137,.12)` hover. Rendered only when there is a filter to clear.

### Brandmark
The app icon is the Nova Suite icon family (canvas board 04, the same art as
`NovaSuite.AppIcon` on novasuite.coding4pizza.com): a near-black tile `#13111A` shared by
every suite app, a NovaProxy green glow in the top-left corner, a green hairline, and the
**Intercept Node** glyph: a transport axis broken in the middle, hollow rings for client and
server, and the **N** standing in the break in the app gradient (`#DCFCE7 → #6EDB9A →
#2E9A59`), with the white nova star on its right shoulder.

- The tile is dark in both themes, inside the app as on the Dock: it is the app's face, not
  a surface of the UI. `src/Brandmark.tsx` draws it inline (rail 36px, empty state 64px,
  setup dialog 30px).
- **At or below 32px the OS icon changes**: the rings bleed shut and the star goes, so the
  wire becomes a solid tick and the N grows (`novaproxy-icon-small*.svg`, used by the
  16/24/32 slots of `.ico` and `.icns`).
- macOS uses Apple's 824/1024 grid; Windows and Linux fill about 94% of the box, because a
  margin there reads as a small icon in the taskbar.
- Sources and generators live in `assets/logo/` (`build.py` → `suite_tile()` holds the tile;
  `render.py`, `icns.py`, `preview.py` cut the rasters). App-icon output is checked in under
  `src-tauri/icons/`. `assets/logo/README.md` documents the full ladder and the tray
  template rule. Older marks are kept in `assets/logo/_previous/`.

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
  keycap, with the active row on `accent .10` and its icon in accent.
- **Settings modal (z40):** 560px, three tabs — General / Network / MCP. General opens with a
  **Setup** row that reruns the first-run setup.
- **First-run setup (z40, board 14):** 720×520, two columns. Left, on `--panel`: the app icon
  and "Set up NovaProxy" in Space Grotesk, then the steps as 40px rows with a 22px numbered
  mark (accent ring when current, accent fill and a check when done). Right: an accent eyebrow
  `Step N of M`, the step title in Space Grotesk 26px, a live status line, the blurb, a mono
  info card (CA subject and fingerprint; proxy address), and for the system proxy step the
  curl and `export` lines for routing one app by hand. Footer: **Skip for now** on the left,
  **Back** and the step's action on the right. The modal's height is fixed so the buttons
  never move; a long step scrolls. Steps and their done-ness live in `src/onboarding.ts`.
- **Save filter (z40):** the settings shell at 440px. One text field, pre-filled
  with a short suggested name and selected, so accepting the default is still one
  keystroke; under it, the filter it will keep spelled out — the name is free
  text, so without that line the dialog asks you to name something it never
  shows you. A name that already exists turns the button into **Replace** and
  says so, because typing an existing name is how a person says "update this
  one".
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
  is Inter. Space Grotesk is for titles and headlines and nothing else.
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
