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
           (all rgba(27,26,61,·)), plus --e-accent for green buttons
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
│ rail │  header (62px): title/sub │ Recording │ Clear │ … │ ⌘K │ proxy │
│ 78px ├───────────────────────────────────────────────────────────────┤
│      │                     active section body                        │
│  ≈   │  ┌ stats ─────────────────────────────────── throughput ─┐    │
│  ⑂   │  ├ flow list (412px) ─┬─ inspector (flex) ───────────────┤    │
│  ⏸   │  │ search + chips     │ head + tabs + body               │    │
│  {}   │  │ grouped rows       │                                  │    │
│  ⛨   │  └────────────────────┴──────────────────────────────────┘    │
│  ⚙   │                                                                │
├──────┴───────────────────────────────────────────────────────────────┤
│ ● recording  N flows · M hosts      upstream: direct  CA…  host:port  │  30px
└───────────────────────────────────────────────────────────────────────┘
```

### Rail (78px)
`--panel` + blur, right hairline. A 32px gradient brandmark on top, then five items —
**Flows, Rules, Break, Scripts, Certs** — each a 58px-wide, 16px-radius stack of a 19px
Lucide icon over a 10px/600 label. Active item is a **white pill** with `--e-sm` and a
hairline border, icon and label in `--accent`; inactive is `--muted` on transparent, hover
`--hover`. A spacer pushes the settings gear to the bottom.

### Header (62px)
Bottom hairline, no fill. Left: the section title (Inter 600/14.5px) over
`default workspace · host:port`. Then a hairline divider and pill buttons —
**Recording/Paused** (salmon tint + pulsing dot when live) and **Clear**, plus the app filter
`<select>` on the Flows section. Right: **Commands** with a `⌘K` keycap, then the
**System proxy** switch.

### Status bar (30px)
`rgba(255,255,255,.6)`, top hairline, 11px mono `--muted`. Left: a state pip (pulsing salmon
when recording) and `recording | paused | stopped`, then `N flows · M hosts`. Right:
`upstream: direct`, CA trust (green/amber), bind address.

---

## 4. Sections

### 4.1 Flows — stat strip over two panes
**Stat strip** (`.stat-row`): four cards — Flows (visible of total), Median ms, Failed
(4xx/5xx), MCP calls — each an eyebrow with a 15px icon over a 25px mono figure and a faint
unit. Then a 240px **Throughput** card with a sparkline: an accent stroke over a fading
accent fill, with a leading dot. All five values come from `src/stats.ts`; the series buckets
`response_size` by `started_at` over the trailing 60s.

**Flow list** (`.flow-list`, 412px default, resizable) — a `--list` panel, `--r-panel`
corners, `--e-xs`. Header holds the mono search field, the chip row
(**All / Errors / Slow / MCP**, `src/filter.ts`), and a meta line with the count and the
internal-traffic and grouping toggles. Group headers are sticky, with a host dot, host name,
a **TLS** chip and a count. Rows are: method badge, mono path over a faint sub-line
(host · MCP · resent), then right-aligned status and duration. Selected rows take an accent
left border and an `rgba(62,181,109,.11)` tint.

> Row and header heights feed `src/virtual.ts` (`ROW_H_GUESS` / `HEADER_H_GUESS`). They are
> first-frame estimates that `FlowList` re-measures from the DOM, but changing row padding
> without updating them causes a visible jump on first paint.

**Inspector** (`.detail`) — a `rgba(255,255,255,.9)` panel. Empty state is a mint-tinted
rounded tile with the Flows icon. Head: method badge, mono URL, status pill, green **Resend**
pill. Tabs (Overview / Request / Response / Timing / cURL, plus WebSocket when relevant) use
a 2px accent underline on the active one.
- **Overview** — a two-column grid of `--input` cards, uppercase key over a mono value, then
  a row of semantic pill chips.
- **Request / Response** — an eyebrow, then a bordered `.hlist` table: a 230px `--input` key
  column beside the value. Then the body `<pre>` on `--code-bg`.
- **Timing** — a waterfall on a 130px / bar / 72px grid; 8px pill track, per-phase colour
  from `src/timing.ts` (which emits `var(--c-*)`, so retuning the tokens is enough).
- **cURL** — eyebrow + Copy, then the command on a code surface.

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

### Toggle switch
40×23px pill track, `rgba(27,26,61,.14)` off / `--accent` on with an accent glow. Knob is a
17px white circle at `top/left: 3px` that `translateX(17px)` on `--t-base`.

### Chips, tags, pills
Filter chips (`.fchip`) are pill-shaped, 11.5px/500, white-ish over a hairline; active is
solid accent with white text. Semantic chips (`.chip`) are pill-shaped, coloured text over a
~10% tint with a ~30% border. Method and kind tags are the squarer 5–8px radius mono form.

### Buttons
- **Primary** — accent pill, white text, `--e-accent`, hover `--accent-hover`, press
  `scale(.98)`. `.red` is `#C4676B`. `.cyan` is an alias of primary: the design collapses
  secondary-accent actions into the one green.
- **Neutral** — white pill, `--border2` hairline, `--text2`; hover darkens the border.

### Fields and focus
Inputs sit on `--input` with an 11–12px radius. Focus is
`border-color: rgba(62,181,109,.45)` plus `box-shadow: 0 0 0 3px rgba(62,181,109,.12)` —
a ring, not a colour swap.

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
