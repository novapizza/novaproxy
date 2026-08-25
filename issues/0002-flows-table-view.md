# 0002 — Flows: table view theo design mới (Proxyman-style)

*Trạng thái: **chưa code gì**. Review + lộ trình design.
Đã chốt hết quyết định sau khi đối chiếu UI Proxyman thật (§8).*

Nguồn: `~/Downloads/NovaProxy.html` (design canvas bundle, 5 screen:
`flows`, `table`, `rules`, `scripts`, `certs`). Screen cần bàn là **`table`** —
"flows tab 2". Reference: Proxyman.

## 1. Design mới là gì

Screen `flows` (tab 1) gần như **trùng với app hiện tại**: stat strip + sparkline,
search + chips, list group theo host, detail 5 tab. Không có gì mới ở đó.

Screen `table` (tab 2) là một **layout khác hẳn**, 5 vùng:

| Vùng | Nội dung |
|---|---|
| Sidebar 252px | Favorites (Pinned / Saved) · "All traffic" · **Apps** (name + count) · **Domains** (tree) · footer "Filter tree ⌘⇧F" |
| Filter bar | chips single-select: `All HTTP HTTPS JSON GraphQL Media Other 2xx 4xx 5xx` + "Reset filters" |
| Table | sticky header, row **34px** fixed, zebra, 10 column, `min-width: 1140px` → scroll ngang, selected row có border-left accent |
| Summary bar | method + status + url của row đang chọn, phải là `N rows · 1 selected` |
| Dual pane 42% | **Request | Response** cạnh nhau, mỗi bên có tab riêng (`Header Query Body Raw Summary` / `Header …`) + nút collapse, body là grid key/value |

Status bar dưới cùng giống app hiện tại (`recording · N flows · M hosts · upstream:
direct · CA trusted · 127.0.0.1:9090`) — không phải làm lại.

**Điểm khác biệt về triết lý:** tab 1 là "đọc từng flow" (list dày thông tin, 1 pane
detail). Tab 2 là "quét cả capture" (mật độ cao, so sánh theo column, filter theo
cây scope). Đây là 2 use case thật, không phải 2 phiên bản của cùng một thứ.

## 2. Feasibility: cao — data gần như có đủ

`Flow` (`src/bindings/Flow.ts`) đã có gần hết những gì design cần.

### Column của table

| Column | Nguồn | Trạng thái |
|---|---|---|
| `#` | `seq` | ✅ |
| URL | `url` | ✅ |
| Client | `process` (+ `pid`) — resolve trong `crates/nova-core/src/procinfo.rs` | ✅ (icon app thật thì ❌, xem §3) |
| Method | `method` | ✅ |
| Status | `status` + `error` | ✅ |
| Time | `started_at` | ✅ data, ❌ formatter `HH:mm:ss.mmm` (hiện chỉ có `formatAgo`) |
| Duration | `duration_ms` | ✅ |
| Request / Response | `request_size` / `response_size` | ✅ |
| SSL | `scheme` + `tunneled` | ✅ (lock / lock-open / tunneled) |

Miễn phí thêm nếu muốn column: `http_version`, `is_websocket`, `mcp`,
`mapped_from`, `resent`, `content_type`.

### Sidebar tree

| Mục | Trạng thái |
|---|---|
| Apps + count | ✅ `distinctApps()` đã có, count derive được |
| Domains (+ tree con theo path như Proxyman) | ✅ derive từ `host`/`path` |
| Pinned | ❌ **chưa có** — cần state per-flow mới + quyết định persist |
| Saved | ❌ **chưa có** — saved filter, cần pref mới |

### Type chips

| Chip | Trạng thái |
|---|---|
| HTTP / HTTPS | ✅ `scheme` |
| 2xx / 4xx / 5xx | ✅ `status` |
| JSON / Media / Other | ✅ `content_type` — cần classifier ở FE (Rust đã có heuristic tương tự trong `flow.rs:looks_textual`) |
| GraphQL | ❌ **chưa detect** — cần heuristic (path `*/graphql`, `content_type: application/graphql`, hoặc body có `query`/`operationName`) |

### Dual pane Request/Response

| Tab | Trạng thái |
|---|---|
| Header | ✅ `request_headers` / `response_headers` |
| Body | ✅ `BodyPreview` + `read_body` on-demand (`useBodyBytes` đã làm đúng pattern này) |
| Query | ✅ derive từ `url` — không cần backend |
| Summary | ✅ derive — gần như tab Overview hiện tại |
| Raw | ⚠️ **reconstruct được, không byte-exact**: engine không giữ raw wire bytes. Dựng lại từ `method/path/http_version` + headers + body preview |

### Kết luận feasibility
~85–90% data đã có. **Không cần đổi engine Rust** (trừ 2 việc optional: classify
GraphQL, icon app thật). Chi phí thật nằm ở **frontend architecture**, không ở data.

## 3. Phần còn thiếu — theo mức độ việc

**Nhóm A — primitive UI chưa tồn tại**
1. **Table component**: column model (width/align/formatter), sticky header, scroll
   ngang, zebra. `src/virtual.ts` hiện là windowing theo *group, row cao biến đổi*;
   table là *flat, row fixed 34px* → thêm `sliceFlat()` (đơn giản hơn cái đang có).
2. **Sort theo column**: ❌ chưa có gì. Store là insertion-order newest-first.
   Risk: sort 10k row mỗi rAF batch. Hướng: sort trên array đã filter, memo,
   và giữ hành vi "follow tail" khi chưa sort.
3. **Column resize / reorder / show-hide** + persist.
4. **Sidebar tree** + scope selection model (all / app / domain / path prefix /
   pinned / saved) + count.
5. **Dual-pane inspector** có tab độc lập, collapse, kéo cao — hiện `Detail` là
   một panel tab đơn.
6. **Keyboard**: ⌘⇧F cho filter tree, ↑/↓ chạy row (Proxyman có, app hiện chưa).

**Nhóm B — refactor bắt buộc**
7. `src/App.tsx` đang **2604 dòng**. Nhét thêm table + tree + dual-pane vào đây là
   một diff ~1500 dòng không ai review được. Phải tách `src/flows/*`
   (`FlowTable`, `ScopeTree`, `FilterBar`, `Inspector`) và **decompose `Detail`**
   thành panel dùng chung cho cả 2 view (`HeaderTable`, `BodyBlock`, `QueryTable`,
   `SummaryPanel`).
8. `src/filter.ts` hiện là `filterFlows(flows, query, {app, chip, includeInternal})`.
   Screen mới cần predicate hợp thành: `scope ∧ types ∧ statusClass ∧ query`.
   Refactor thành `FlowFilter` + `buildPredicate()`, giữ nguyên chips của view cũ.
   `filter.test.ts` đã có → mở rộng.
9. `src/prefs.ts` cần thêm: `viewMode`, columns, sort, sidebar width, pane height,
   collapsed tree nodes, saved filters → `normalizePrefs` + test.
10. Telemetry: vocabulary `UiEvent`/`UiEventName` **fix cứng cả 2 phía**
    (`src/api.ts` ↔ `commands.rs`). Event mới (view mode, sort, scope, pane tab)
    không thêm ở Rust là fold thành `ui.other`.
11. `design.md` §4.1 phải viết lại. Design HTML dùng hex thô; repo có rule
    "không hardcode hex ngoài token" → map hết về token, thêm token mới cho
    zebra row (`rgba(242,246,243,.55)`), row height, sticky header bg.

**Nhóm C — feature mới (không phải layout)**
12. **Pinned** flow: state mới. Persist qua Clear? Qua restart? (flow id không
    bền qua restart → pin phải theo url+seq hoặc chấp nhận mất).
13. **Saved** filter: pref mới + UI đặt tên.
14. **Multi-select row** — design ghi `1 selected`, tức là có multi-select
    (Proxyman dùng cho bulk export/block).
15. **Icon app thật**: cần đọc bundle icon qua NSWorkspace ở Rust. Design đang
    dùng lucide generic (`globe`, `git-branch`) — chấp nhận được ở phase 1.

## 4. Vấn đề của design cần sửa trước khi code

1. **Chips trộn 3 trục khác nhau** (protocol / content type / status class) nhưng
   lại single-select (`filterType` trong design script). Nghĩa là không thể xem
   "JSON + 4xx". Proxyman tách các trục ra. → nên tách 3 nhóm, mỗi nhóm
   multi-select, hoặc ít nhất status tách khỏi type.
2. **Không có search box** ở screen `table` (chỉ tab 1 có). Mất `method:`/`status:`/
   `host:`/`app:`/`mcp:` query đang có trong `filter.ts` là một bước lùi.
3. **Column overflow**: `min-width: 1140px` + sidebar 252 + rail 78 = cần window
   ~1470px mới không scroll ngang. Trên 1280px scroll ngang ngay từ đầu, và
   `tauri.conf.json` cho `minWidth: 940` → ở kích thước nhỏ nhất table chỉ còn
   **~610px**. → bắt buộc có default column set gọn (7 column) + column picker.
4. **Rail có 2 item cùng label "Flows"** — đây là artifact của design canvas
   (2 variant cạnh nhau), không phải 2 nav item để ship. Cần quyết (§6.1).
5. **Chips mới không có chỗ cho MCP / Errors / Slow / WebSocket.** App hiện có
   4 chip `All Errors Slow MCP` (`FLOW_CHIPS` trong `filter.ts`); design chỉ có
   proto/type/status. Thay thẳng là **mất filter MCP** — thứ khác biệt nhất của
   NovaProxy so với Proxyman (có `McpInfo`, có MCP endpoint riêng). → phải có chỗ
   cho nhóm này, xem §6 mục còn mở.
6. **Thiếu state**: design chỉ vẽ happy path. Table cần vẽ thêm: empty (chưa có
   traffic / paused / no-match), row đang in-flight (`status = null`), row
   `tunneled` (không có body), row error, row WebSocket.

## 5. Chip model đã chốt (phương án B, mở rộng theo Proxyman)

```
Proto   [HTTP] [HTTPS] [WebSocket]
Type    [JSON] [GraphQL] [MCP] [Form] [XML] [Document] [Media] [Other]
Status  [1xx] [2xx] [3xx] [4xx] [5xx] [ERR]        Reset filters
```

- **Trong nhóm là OR, giữa nhóm là AND.** Set rỗng = cả nhóm → không cần chip `All`.
- State: 3 × `Set<string>`, không phải 1 biến `filterType`.
- `WebSocket` vào nhóm Proto — Proxyman làm đúng vậy, và ta có `is_websocket`.
- `MCP` vào nhóm Type: MCP là payload JSON-RPC nên nó **là** một content kind
  (`f.mcp != null`). Không cần scope riêng ở sidebar.
- `ERR` = `f.error != null` (flow chết, không có status). Proxyman không có chip này
  vì không tách; ta có field `error` nên phải có chỗ cho nó.
- Chip **`Errors` cũ = chọn `4xx` + `5xx`** — multi-select giải quyết, không cần
  chip riêng.
- Chip **`Slow` bị bỏ.** Proxyman không có gì tương đương; thay bằng **sort theo
  column Duration** (phase 6). Đây là mất tính năng nhỏ, cố ý — `SLOW_MS = 300` là
  ngưỡng bịa, sort thì không.
- Proxyman xếp **cả 17 chip vào 1 dòng scroll ngang** (kèm saved filter cũng là chip
  trong dòng đó). Ta chọn 3 dòng (cao thêm ~56px) để đọc được trục. Nếu sau thấy
  chật: gộp về 1 dòng scroll, semantics không đổi.
- Telemetry `ui.flow.chip` đang nhận `all|errors|slow|mcp`
  (`src-tauri/src/commands.rs:658-664`) → đổi vocabulary **cả 2 phía**, không thì
  mọi lần bấm chip fold thành `ui.other`.

## 6. Quyết định — đã chốt hết

| # | Quyết định |
|---|---|
| 1 | **Table thay hẳn tab Flows hiện tại.** Không toggle List↔Table |
| 2 | **Chips = 3 nhóm multi-select** (§5) |
| 3 | **Bỏ hẳn stat strip + sparkline.** Throughput chuyển xuống status bar (§8.3) |
| 4 | Search box: **giữ** (`method:`/`host:`/`app:`/`mcp:`) |
| 5 | Pinned / Saved: **phase 8**, pin chỉ sống trong session |
| 6 | Multi-select row: **phase 6** |
| 7 | Tab Raw: **dựng lại** + nhãn "reconstructed" |
| 8 | Icon app: lucide generic ở phase 1, icon thật ở phase 7 |
| 9 | **cURL = action, không phải tab.** Proxyman để nó ở context menu "Copy as cURL" → nút trên summary bar + command palette (telemetry `copy_curl` đã có) |
| 10 | **Timing = tab riêng trong pane Response.** Không nhét vào Summary: `Timings` của ta là đo thật, có `connection_reused` + per-phase — chôn trong Summary là phí |
| 11 | **WebSocket**: khi `f.is_websocket`, dual pane đổi thành **1 pane Messages** (đúng cách Proxyman làm) |

Tab list chốt:

```
Request   Header · Query · Body · Cookies · Raw · Summary
Response  Header · Body · Raw · Treeview · Timing · Summary
WS flow   Messages   (thay cả 2 pane)
```

- `Cookies`: derive từ header `Cookie`/`Set-Cookie` — miễn phí.
- `Treeview`: JSON tree viewer — component mới (phase 4).
- Proxyman có nút `+` cuối mỗi tab list để user tự thêm tab → **bỏ qua**, over-engineering cho phase 1.

## 7. Hệ quả của "thay hẳn tab Flows" — code phải dọn

| Thứ bị bỏ | File | Ghi chú |
|---|---|---|
| Stat strip + sparkline | `src/stats.ts`, `src/stats.test.ts` | **Chỉ `flowStats` + `sparkPath` chết.** `throughputSeries` / `throughputRate` / `formatRate` **giữ lại** — Proxyman đặt throughput ở status bar (`↑ 17 KB/s ↓ 4 KB/s · 327 MB`), ta làm y vậy |
| List group theo host | `sliceGroups` (`src/virtual.ts`) + `virtual.test.ts` | Table là flat row 34px → `sliceFlat()`, đơn giản hơn |
| Pref `flowGrouping` | `src/prefs.ts:25,48,65` | + dropdown `GROUPING_ITEMS` trong Settings→General (`App.tsx:2261,2292`). Cần migration `normalizePrefs` + test |
| Chips cũ | `src/filter.ts` (`FlowChip`, `FLOW_CHIPS`, `matchChip`, `SLOW_MS`) | Thay bằng model §5 |
| Telemetry | `src/api.ts` (`UiEventName`), `commands.rs:640-666` + `ui_track_tests` | Bỏ `group_toggle`, `all/errors/slow`; thêm event cho chip nhóm, sort, scope tree, pane tab |
| Coachmark anchor | `App.tsx:279,753,895` (`flowListRef`) | Đang trỏ `.flow-list` → trỏ vào table |
| Detail 1 pane 5 tab | `Detail`, `DETAIL_TABS` (`App.tsx:1362-1506`) | Thành dual-pane; panel con tách ra dùng chung |

Ước lượng: **~500 dòng bỏ, ~1600 dòng mới**. Tách file là bắt buộc.

## 8. Đọc thêm được từ UI Proxyman thật

### 8.1 Filter builder — phần mạnh nhất, ta không có

Dưới hàng chip là một **filter row có cấu trúc**, không phải search box:

```
[✓]  [ URL ▾ ]  [ Contains ▾ ]  [ producthunt.com      ⊗ ]  [–] [+]      [Save]
     Show: ⌘F   New: ⌘N   Remove: ⇧⌘N   Up: ⌘↑   Down: ⌘↓   On/Off: ⌘B   Hide: ESC
```

- Nhiều row xếp chồng, **mỗi row bật/tắt riêng** (checkbox), `[+]`/`[–]` thêm bớt.
- `Save` → filter thành **chip trong hàng chip** (ảnh: `slack: client.counts…`).
- → **Đây là câu trả lời cho "Saved" trong Favorites**: saved filter = chip, không
  phải một mục list riêng.
- `matchQuery` của ta (`method:`/`host:`/`app:`/`mcp:`) là bản nghèo hơn của đúng ý
  tưởng này. Giữ search box cho phase 1 (đã chốt), builder vào phase 9.

### 8.2 Column ta chưa tính

| Column | Trạng thái |
|---|---|
| dot đầu row (status class) | ✅ có |
| **Edited** | ❌ **thiếu field ở Rust.** Cho biết flow bị rule/script/breakpoint sửa. Ta có `mapped_from` và `resent` nhưng **không có flag "edited"** → cần thêm ở `nova-core` (`rules.rs` / `scripting.rs` / `intercept.rs`) |
| **Comment** | ❌ feature mới: annotation per-flow (cùng vấn đề persist như Pinned) |
| **Tools** | Proxyman: menu action per-row |

Column `#` có **sort arrow** → Proxyman cho sort mọi column (đã nằm ở phase 6).

### 8.3 Chi tiết nhỏ đáng lấy

- **Auto Select** (button ở status bar) = follow-tail, tự chọn row mới nhất.
  Bắt buộc phải có khi đã cho sort. App hiện chưa có.
- `1/10 rows selected` → multi-select, format `selected/total`.
- Status bar Proxyman: trái `Clear · Filter (toggle filter bar) · Auto Select`,
  phải `· 327 MB ↑ 17 KB/s ↓ 4 KB/s` + badge `Proxy Overridden`.
  → chỗ để throughput sau khi bỏ stat strip; badge tương đương
  `proxy.pending_restore` của ta.
- Sidebar: **icon app thật**, mỗi app **expand ra domain con**, count ở group header
  (`Apps 6` / `Domains 20`), footer có `+` và `Filter (⌘⇧F)`.
- Summary bar: URL có host highlight + segment click được.
- Size rỗng render `–`, **không phải `0 B`**.
- Proxyman **không có left rail** (dùng menu bar). Ta giữ rail 78px → content hẹp
  hơn ~78px so với Proxyman ở cùng window size → column picker (§4 mục 3) càng cần.

## 9. Lộ trình

| Phase | Nội dung | Deliverable |
|---|---|---|
| **1** | Viết lại `design.md` §4.1: anatomy table view, token mới (zebra row, sticky header, row height), column set + default + picker, **toàn bộ state** (empty / paused / no-match / pending / tunneled / error / ws) | `design.md` |
| **2** | Filter & scope model: `filter.ts` → `FlowFilter` + `buildPredicate()` theo §5, classifier content type (JSON/GraphQL/MCP/Form/XML/Document/Media/Other), derived index cho count sidebar. **Pure logic + test, chưa UI** | `src/filter.ts`, `src/scope.ts`, tests |
| **3** | Table shell: `src/flows/` (`ScopeTree`, `FilterBar`, `FlowTable` + `sliceFlat`), single-select, **Auto Select**, chưa sort. Thay chỗ `FlowsSection` | UI chạy được |
| **4** | Dual-pane inspector: decompose `Detail`; tab `Query/Cookies/Raw/Summary/Timing/Treeview`; WS → 1 pane Messages; cURL thành action | UI đủ dùng |
| **5** | Dọn dead code §7 + telemetry vocab 2 phía + prefs migration | diff sạch |
| **6** | Table power: sort column, column picker/resize, keyboard nav, multi-select + `n/m rows selected`. Chord định nghĩa ở [0003](0003-keyboard-shortcuts.md) | — |
| **7** | Rust: `Edits` trên `Flow` (rule/script/breakpoint, chỉ set khi *thật sự* đổi); icon app thật — không qua NSWorkspace mà qua `.icns` + `sips`, để không thêm dependency | `Edits.ts`, command `app_icon` |
| **8** | Favorites: Pinned + Comment + Saved filter (saved → chip) | — |
| **9** | Filter builder (field/operator/value, stackable, save, ⌘F/⌘N/⌘B) | — |
| **10** | Polish: README, design.md, test coverage | — |

Phase 2 trước Phase 3 là có chủ ý: filter/scope là chỗ dễ sai và dễ test nhất; xong
rồi UI chỉ còn là render. Phase 5 tách riêng để phase 3–4 không vừa thêm vừa xoá
trong cùng một diff.
