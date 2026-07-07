# NovaProxy — Design Reference

> Extracted from `claude-design/NovaProxy Prototype.html`. This documents the visual
> language, UI structure, CSS system, and interaction behaviors so any session can
> rebuild or extend the UI consistently.

NovaProxy is a **desktop HTTPS-debugging proxy** (Charles/Proxyman-style) — it captures,
inspects, and manipulates live network traffic. The prototype presents a macOS-style
desktop app window with a left icon rail, a flow list, a detail inspector, and several
settings sections, plus a command palette and modal overlays.

---

## 1. Look & Feel

- **Tone:** Professional developer tool. Dense but calm. Dark-first, with a full light theme.
- **Window chrome:** Emulates a native macOS window — 38px titlebar with the three traffic-light
  dots (`#ff5f57` red, `#febc2e` yellow, `#28c840` green), centered app title with a small
  gradient logo chip, and a bottom status bar.
- **Typography:**
  - UI text: `-apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Helvetica Neue', sans-serif`
  - Monospace (paths, headers, code, cURL, badges): `ui-monospace, 'SF Mono', Menlo, Monaco, monospace`
- **Shape language:** Rounded throughout — cards/modals `12–16px`, buttons/inputs `8–11px`,
  badges/pills/tags `5–7px`, toggle tracks `11px` (pill).
- **Density:** Small type (10–15px), tight paddings. Uppercase micro-labels with letterspacing
  (`.05–.06em`) for section/field headers.
- **Motion:** Subtle, fast. Fade/slide-in on new rows and overlays; a pulsing dot for the
  live "Recording" state and the paused-breakpoint indicator.
- **Accent-driven:** A single user-configurable `--accent` color drives active nav, primary
  buttons, toggles, focus rings, and selection highlights.

---

## 2. Theming & CSS Variables

Theme is switched via a `data-nova-theme="dark|light"` attribute on the root container.
The accent is injected as an inline `--accent` custom property on the same element.

### Dark theme (default feel)
```css
[data-nova-theme="dark"]{
  --bg:#0d0f13; --panel:#111419; --list:#0f1217; --card:#12151b; --input:#161a21; --hover:#1b2029;
  --border:#20252e; --border2:#2a2f39; --bsoft:#171b22;
  --text:#e6e8ec; --text2:#b6bcc6; --muted:#8b929e; --faint:#5b6270;
  --shadow:rgba(0,0,0,.6); --overlay:rgba(6,8,11,.62);
  --c-green:#5fe0a8; --c-blue:#7db9ff; --c-amber:#f7c86b; --c-red:#ff8a8a;
  --c-violet:#c3a3ff; --c-pink:#f79bde; --c-cyan:#38d6c8;
}
```

### Light theme
```css
[data-nova-theme="light"]{
  --bg:#eef0f4; --panel:#ffffff; --list:#f7f8fb; --card:#ffffff; --input:#eef1f5; --hover:#e9ecf2;
  --border:#e2e5ec; --border2:#d2d7e0; --bsoft:#edeff4;
  --text:#1b1e26; --text2:#4a515f; --muted:#6d7484; --faint:#a2a8b5;
  --shadow:rgba(30,42,66,.16); --overlay:rgba(28,38,55,.34);
  --c-green:#0f9d63; --c-blue:#2b6fed; --c-amber:#b0761a; --c-red:#d63b3b;
  --c-violet:#7a4fd6; --c-pink:#c43a9e; --c-cyan:#0c9c90;
}
```

### Variable roles
| Group | Vars | Use |
|---|---|---|
| Surfaces | `--bg`, `--panel`, `--list`, `--card`, `--input`, `--hover` | Page bg, chrome bars, list column, cards/detail, form fields, hover state |
| Borders | `--border`, `--border2`, `--bsoft` | Standard dividers, stronger/interactive borders, subtle inner row separators |
| Text | `--text`, `--text2`, `--muted`, `--faint` | Primary, secondary, labels, hints/placeholders (4-level hierarchy) |
| Depth | `--shadow`, `--overlay` | Modal shadows, backdrop scrims |
| Semantic | `--c-green/blue/amber/red/violet/pink/cyan` | Status codes, HTTP methods, tags, accents (theme-adjusted for contrast) |
| Injected | `--accent` | Primary actions, active nav, toggles-on, focus, selection |

> **Semantic colors are theme-aware.** Always reference them via the CSS var (e.g.
> `color:var(--c-green)`) so both themes stay legible. Raw rgba tints (backgrounds/borders
> like `rgba(56,217,150,.1)`) are reused across both themes intentionally.

### Accent tinting helper
Translucent accent shades use `color-mix`:
```
color-mix(in srgb, var(--accent) <pct>%, transparent)
```
Common percentages: 13% (row selection bg), 16% (palette hover), 20% (active nav bg), 42% (active nav border).

### Scrollbars (custom, thin)
```css
::-webkit-scrollbar{width:10px;height:10px}
::-webkit-scrollbar-thumb{background:rgba(128,136,150,.4);border-radius:6px;border:2px solid transparent;background-clip:padding-box}
::-webkit-scrollbar-track{background:transparent}
```

---

## 3. Layout Structure

Root is a full-viewport (`100vh`) vertical flex column, `overflow:hidden`, `position:relative`
(so overlays anchor to it).

```
┌───────────────────────────────────────────────── titlebar (38px) ──┐
│ ● ● ●        ▪ NovaProxy — default workspace                        │
├──────┬──────────────────────────────────────────────────────────────┤
│ rail │  toolbar (52px): Recording | Clear | search | ⌘K | SysProxy   │
│ 64px ├──────────────────────────────────────────────────────────────┤
│      │                     active section body                        │
│ ≋    │  ┌─ flow list (412px) ─┬─ detail inspector (flex) ─┐          │
│ ⤳    │  │  grouped by host    │  header + tabs + body     │          │
│ ⏸    │  │  method/path/status │  Overview/Req/Res/Timing  │          │
│ { }  │  │                     │  /cURL                    │          │
│ 🔒   │  └─────────────────────┴───────────────────────────┘          │
│ ⚙    │                                                                │
├──────┴──────────────────────────────────────────────────────────────┤
│ ● live   N flows · M hosts        upstream: direct  CA…  127.0.0.1:9090│  status bar (26px)
└──────────────────────────────────────────────────────────────────────┘
```

### 3.1 Titlebar (38px)
`background:var(--panel)`, bottom border. Left: three 12px traffic-light dots. Center:
600/13px title with a 14px gradient chip `linear-gradient(135deg,var(--accent),var(--c-cyan))`
and a faint "— default workspace" suffix. Right: 56px spacer for symmetry.

### 3.2 Left rail (64px)
Vertical, centered, `background:var(--panel)`, right border. Nav items are 44×44 rounded
(`11px`) squares, icon (18px) over 10px uppercase label. Sections: **Flows ≋, Rules ⤳,
Break ⏸, Scripts { }, Certs 🔒**. A flex spacer pushes a settings gear (⚙) to the bottom.
- Active item: `background:color-mix(accent 20%)`, `color:var(--accent)`, `border:1px accent 42%`.
- Inactive: `color:var(--muted)`, transparent border; hover → `--hover` bg + `--text2`.

### 3.3 Toolbar (52px)
`background:var(--panel)`, bottom border, horizontal flex with 11px gap. Contents:
- **Record/Pause button** — toggles capture. When recording: red text/border/tint
  (`--c-red`, `rgba(255,107,107,…)`) with a pulsing red dot. When paused: muted + `--input`.
- **Clear button** — neutral `--input` bordered button.
- Divider (1px × 22px).
- **Search input** — `--input` field, `⌕` icon, placeholder
  `"Filter by host, path, method:GET, status:401…"`, focus ring = accent border, `✕` clear
  affordance when non-empty. Flex-grows to `max-width:520px`.
- Flex spacer.
- **Commands button** — opens palette; shows a `⌘K` keycap chip.
- **System Proxy toggle** — pill switch + "System Proxy" label.

### 3.4 Status bar (26px)
`background:var(--panel)`, top border, 11px monospace muted text. Left: live/paused indicator
(green `● live` / muted `❚❚ paused`), then `N flows · M hosts`. Right: `upstream: direct`,
CA trust state (green/amber), and bind address `127.0.0.1:9090`.

---

## 4. Sections (rail-switched, one visible at a time)

### 4.1 Flows (default) — two-pane
**Flow list (412px, `background:var(--list)`):**
- 36px header row: count label (`"N flows"`) + a group toggle (`▾ grouped` / `≡ flat`).
- Group headers (when grouped by host): sticky, 700/12px, a cyan host dot, host name
  (ellipsized), a green **TLS** chip, and a per-host count. `position:sticky;top:0`.
- **Flow rows:** method badge + (path in monospace / subline = host, with `⤳` prefix if
  mapped and `· resent` suffix) + right-aligned status code (semantic color) & relative time.
  Selected row: 2px accent left-border and `color-mix(accent 13%)` bg. New rows fade in
  (`novaflow .22s`). Hover → `--hover`.
- Empty states: centered faint text — "Waiting for traffic…", "Recording paused — no flows
  captured", or "No flows match your filter".

**Detail inspector (flex, `background:var(--bg)`):**
- Empty state: dashed rounded icon (`≋`) + "Select a flow to inspect" + ⌘K hint.
- Header: method badge, full URL (monospace, break-all), a status pill, and a primary
  **↻ Resend** button (accent bg, white text).
- **Tab bar:** Overview / Request / Response / Timing / cURL. Active tab = `--text` with a
  2px accent bottom-border; inactive = `--muted`.
- **Overview:** a 2-column fact grid (Method, Status, Protocol, Scheme, Remote host,
  Duration, Size, Started) rendered as bordered cells (1px `--border` gaps over `--card`),
  followed by inline pill chips: green TLS (`🔒 TLS 1.3 · decrypted`), blue protocol
  (`HTTP/2`), and (if mapped) a violet `⤳ mapped from <host>` chip.
- **Request / Response:** uppercase "headers" label → key/value rows (150px min key column,
  bottom-bordered by `--bsoft`) → a "Body" `<pre>` block. Bodies use a dark code surface
  `#0f1217`, `1px var(--border)`, rounded 9px; request body text `#a9c8ee`, response body
  `#c8e6c2`. Response body shows a `content-type · size` meta line.
- **Timing:** waterfall of phases (DNS, Connect, TLS, TTFB, Download), each a labeled
  horizontal bar (`--input` track, colored fill per phase) with a right-aligned ms value,
  then a bold Total row.
- **cURL:** "Export as cURL" label + Copy button, then a `<pre>` code block (`#dfe3e9` text)
  with the generated `curl -X … -H … --data …` command.

### 4.2 Rules — centered column (max 720px)
Header "Rules" + accent **+ New rule** button. Subtitle explains Map Local / Map Remote /
header rewrites on live traffic. Each rule is a `--card` bordered box:
- Header row: enable toggle (pill switch) + a violet **type tag** (uppercase) + rule label +
  trash icon (hover → red).
- Body: field rows — uppercase field key + a value shown as monospace blue text in an
  `--input` pill (`border-radius:7px`).
- Rule types: **Map Remote**, **Map Local**, **Rewrite**.

### 4.3 Breakpoints — centered column (max 720px)
Header + subtitle. Single `--card` panel: a large `⏸` state icon (amber tint when armed,
neutral when idle), a title/subtitle describing armed vs idle, and an **Arm breakpoint /
Disarm** button (cyan when armed, accent when idle). Arming pauses the next matching request.

### 4.4 Scripts — centered column (max 820px)
Header + accent-cyan **▶ Run on next flow** button. Subtitle mentions `onRequest`/`onResponse`
JS hooks (accent-colored). A faux editor: a `tamper.js` tab bar over a syntax-highlighted
`<pre>` (dark `#0f1217`). Highlight palette in code: comments `#5b6270`, strings `#c8e6c2`,
keywords `#c3a3ff`, numbers `#f7c86b`, base text `#dfe3e9`.

### 4.5 Certificate — centered column (max 640px)
Header + subtitle about the local root CA for HTTPS decryption. A `--card` panel: a `🔒` icon
(green tint when trusted), "NovaProxy Root CA" with a SHA-256 fingerprint, a status pill
(green **Trusted** / amber **Not installed**), and buttons **Install & trust** (accent; becomes
red **Remove certificate** when installed) and **Export .pem** (neutral).

---

## 5. Reusable Component Patterns

### HTTP method badge
Fixed-scheme per method, `min-width:44px`, `border-radius:6px`, 700/10.5px monospace, colored
text over a translucent tint + border:
| Method | Color var | tint bg | border |
|---|---|---|---|
| GET | `--c-blue` | `rgba(78,161,255,.13)` | `rgba(78,161,255,.32)` |
| POST | `--c-green` | `rgba(56,217,150,.13)` | `rgba(56,217,150,.32)` |
| PUT | `--c-amber` | `rgba(247,185,85,.14)` | `rgba(247,185,85,.34)` |
| PATCH | `--c-pink` | `rgba(247,139,214,.14)` | `rgba(247,139,214,.32)` |
| DELETE | `--c-red` | `rgba(255,107,107,.14)` | `rgba(255,107,107,.34)` |
| WS | `--c-violet` | `rgba(185,140,255,.15)` | `rgba(185,140,255,.34)` |

### Status-code color scale
`101` → violet · `<300` → green · `<400` → blue · `<500` → amber · `≥500` (or aborted) → red.

### Toggle switch (pill)
Track: `width` 34/36px × `height:20px`, `border-radius:11px`; on = `var(--accent)`, off =
`var(--border2)`, `transition:background .15s`. Knob: 16px white circle, `top:2px`, slides
`left` from `2px` to `width-18px`, subtle shadow, `transition:left .15s`.

### Chips / tags / pills
Small rounded (`5–7px`), 600–700 weight, often uppercase, semantic-colored text over a
matching translucent tint with a matching-color border. Used for TLS, protocol, mapped-from,
rule type, cert status, keycaps.

### Primary vs neutral buttons
- **Primary:** `background:var(--accent)`, `color:#fff`, rounded 9px, hover `filter:brightness(1.1)`.
  Variants swap accent for `--c-cyan` (Scripts run) or `--c-red` (destructive).
- **Neutral:** `background:var(--input)`, `1px var(--border2)`, `color:var(--text2)`, hover
  → `--hover` or accent border.

### Code / body blocks
`<pre>` with `background:#0f1217`, `1px var(--border)`, `border-radius:9px`, 12.5px monospace,
`line-height:1.65–1.8`, `white-space:pre-wrap`, `overflow-x:auto`.

---

## 6. Overlays

All overlays are `position:absolute` within the root (`z-index` 40–60) over a
`var(--overlay)` scrim.

- **Intercept modal (z40):** 520px `--card` panel, centered. Amber pulsing dot + "Request
  paused at breakpoint" title, the paused method badge + URL, an editable-headers `<pre>`,
  and footer actions **Abort** (red outline/tint) + **Continue →** (accent). Enters with
  `novafade` + `novaflow`.
- **Command palette (z50/51):** click-scrim + a 540px `--card` panel `88px` from top,
  centered via `translateX(-50%)`. Header: `⌘` icon + text input ("Type a command…") + ESC
  keycap. Scrollable list (max 280px) of icon + label (+ optional keycap) rows; active/hover
  row = `color-mix(accent 16%)`. "No commands match" empty state. Enters with `novapop`.
- **Toast (z60):** bottom-center `--card` pill, `44px` from bottom, cyan `✓` + message,
  shadow, `novaflow` entry. Auto-dismisses after ~2.2s.

---

## 7. Animations (keyframes)

```css
@keyframes novapulse{0%,100%{opacity:1;transform:scale(1)}50%{opacity:.35;transform:scale(.82)}} /* live dot, paused indicator */
@keyframes novaflow {from{opacity:0;transform:translateY(-4px)}to{opacity:1;transform:translateY(0)}} /* new rows, modal, toast */
@keyframes novafade {from{opacity:0}to{opacity:1}}                                                    /* scrims, empty states */
@keyframes novapop  {from{opacity:0;transform:translate(-50%,8px) scale(.97)}to{opacity:1;transform:translate(-50%,0) scale(1)}} /* command palette */
```

Interaction transitions are short (`.15s`) on toggles; hover feedback is instant via
`style-hover` (bg/color/filter/border shifts).

---

## 8. Configurable Props (design-doc props)

The prototype exposes these tweakable inputs (drive theming/behavior):
- **theme** — enum `dark | light` (default `light` in props; component falls back to `dark`).
- **accent** — color, default `#2b8fff`; presets `#7c6cff` (violet), `#2b8fff` (blue),
  `#12b886` (green), `#f2622a` (orange).
- **streamSpeed** — range 400–3000ms (default 1600), how fast simulated flows arrive.

---

## 9. Behavior & Interaction Notes

- **Live traffic simulation:** flows stream in on an interval (`streamSpeed`), capped at 120,
  newest first. Recording can be paused; a breakpoint can pause the next request into the
  intercept modal (Continue/Abort).
- **Search grammar:** free text matches host/path/method/status; prefixes `method:`,
  `status:`, `host:` do targeted filters.
- **Grouping:** flow list groups by host (sticky headers) or flat list, toggleable.
- **Command palette:** `⌘K`/`Ctrl+K` toggles; arrow keys navigate, Enter runs, Esc closes.
  Commands: pause/record, clear, resend, copy cURL, arm breakpoint, open Rules/Certs, toggle
  system proxy.
- **Rules engine:** Map Remote actually rewrites the host of matching simulated flows (shown
  with the `⤳ mapped` indicator).
- **Resend:** clones the selected flow as a new entry (`· resent`).
- **Cert install** and **system proxy** are stateful toggles reflected in the status bar and
  cert section, with confirming toasts.

---

## 10. Notes for Rebuilding

- Everything is driven by CSS variables + one `--accent`; keep new UI referencing vars, never
  hardcode grays/text colors, so both themes and accent swaps keep working.
- Reference semantic colors via `var(--c-*)` (theme-adjusted); reuse the fixed rgba tints for
  badge/chip backgrounds.
- Match the existing density and radii scale (cards 12–16 / controls 8–11 / chips 5–7).
- Use the 4-level text hierarchy (`--text`, `--text2`, `--muted`, `--faint`) rather than
  ad-hoc opacities.
- Uppercase micro-labels (10–11px, `letter-spacing:.05–.06em`) for section/field headers.
- The prototype's markup is inline-styled (a design-doc template with `{{ }}` bindings,
  `sc-if`/`sc-for` control flow). A production rebuild would extract these into CSS classes
  keyed off the same variables and semantic tokens.
