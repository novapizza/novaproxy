# 0003 — Hệ thống phím tắt + dialog Shortcuts (Help → Shortcuts)

*Trạng thái: design đã chốt hết quyết định, chưa code.*
Liên quan: [0002](0002-flows-table-view.md) — table view mới cần `↑↓`, `⌘F`,
`⌘⇧F`; toàn bộ chord của nó định nghĩa ở đây, không định nghĩa rải trong 0002.

## 1. Hiện trạng

Đang có đúng 5 nhóm chord, nằm rải trong 3 file:

| Chord | Làm gì | Nơi xử lý |
|---|---|---|
| `⌘K` / `Ctrl+K` | toggle command palette | `src/App.tsx:627` |
| `Esc` | đóng palette / dropdown / walkthrough | `App.tsx:633`, `Dropdown.tsx:114`, `Walkthrough.tsx:69` |
| `↑` `↓` `↵` | điều hướng palette | `App.tsx:634-636` |
| `←` `→` (+`⇧`) | kéo splitter khi nó có focus | `App.tsx:1030` |
| `⌘C` `⌘V` `⌘X` `⌘A` `⌘Z` | clipboard / undo — **native menu**, không phải webview | `src-tauri/src/menu.rs` (Edit submenu) |

Ba vấn đề:

1. **Không có nguồn sự thật.** Không có danh sách nào cả → dialog không có gì để
   render, và không ai (kể cả code) biết app đang chiếm những phím nào.
2. **`metaKey || ctrlKey` là bug trên macOS** (`App.tsx:627`). Trên mac
   **`Ctrl+K` là kill-line** (emacs binding, hoạt động trong mọi text field của
   AppKit). Đang gõ trong search box mà bấm `Ctrl+K` thì được command palette
   thay vì xoá tới cuối dòng. Phải tách theo platform, không dùng `||`.
3. **Không dùng được bằng bàn phím.** Không chọn row bằng `↑↓`, không mở Settings
   bằng `⌘,` — mà table view ở 0002 cần đúng những thứ đó.

## 2. Kiến trúc: một registry, hai người đọc

```
src/shortcuts.ts          <- nguồn sự thật duy nhất
      |
      +-- useShortcuts()      -> 1 listener window.keydown, dispatch theo id
      +-- <ShortcutsDialog>   -> render bảng, KHÔNG tự viết chord
```

```ts
interface Shortcut {
  id: ShortcutId;       // stable, đồng thời là tên telemetry
  group: ShortcutGroup; // "global" | "table" | "filter" | "inspector" | "dialog"
  scope: Scope;         // ai được bắt phím này
  chord: string;        // "Mod+Shift+F" | "ArrowDown" | "Escape"
  label: string;        // "Focus the filter tree"
  when?: string;        // điều kiện, hiện làm chú thích trong dialog
}
```

`Mod` = `⌘` trên macOS, `Ctrl` ở nơi khác. `formatChord()` trả về **mảng phím** để
render badge, không trả về string — `⌘⇧F` là 3 badge.

Dialog render **từ** registry là lý do registry tồn tại: bảng phím tắt tự viết tay
là bảng sai sau commit thứ hai.

### Quy tắc matching (chỗ dễ sai nhất)

1. Focus đang trong `input` / `textarea` / `[contenteditable]` → **bỏ qua chord
   không có `Mod`**, trừ `Escape` / `↵` / `↑↓`. Không có luật này thì gõ chữ "j"
   trong search box là nhảy row.
2. **Scope stack**: modal (palette / settings / intercept / onboarding /
   shortcuts) **chặn hết** global và table.
3. Scope `table` chỉ bắn khi table thật sự có focus (`tabIndex` trên container +
   `aria-activedescendant` cho row đang chọn).
4. `preventDefault()` **chỉ khi match**. Gọi vô điều kiện là phá phím hệ thống.
5. Dùng `e.key` lowercase cho chữ. **Không dùng `e.code`** (layout khác thì vị trí
   khác: `KeyA` là `Q` trên AZERTY). **Tuyệt đối không dùng Alt/Option trong
   chord**: trên mac `Option+o` tạo dead key, `e.key` là `"ø"` chứ không phải
   `"o"` — chord kiểu đó không bấm được ở nửa thế giới.
6. Platform: `navigator.userAgent.includes("Macintosh")`. **Không thêm
   `@tauri-apps/plugin-os`** chỉ để trả lời một câu hỏi boolean (thêm dep + thêm
   permission vào `capabilities/default.json`). UA inject được nên vẫn test được.

## 3. Native menu vs webview — ai bắt phím

Điều quan trọng nhất của cả doc này: **trên macOS, chord đã có accelerator trong
native menu thì KHÔNG tới webview.** `menu.rs` đã ghi đúng chuyện đó cho clipboard
("⌘C/⌘V inside the webview are delivered *through the menu*"). Nên một chord chỉ
được nằm ở **một** chỗ, không bao giờ hai.

| Loại | Ở đâu | Vì sao |
|---|---|---|
| clipboard, `⌘W` `⌘Q` `⌘M`, fullscreen | native menu (đang có, predefined) | OS convention, không đụng |
| **Keyboard Shortcuts (`⌘/`)** | **native menu Help + accelerator** | phải mở được cả khi focus không ở webview; và người mac tìm phím tắt trong Help |
| tất cả còn lại | webview registry | một nguồn sự thật; không phải sync Rust ↔ TS |

Giá phải trả: menu bar **không** hiện chord bên cạnh các action (vì action nằm
trong webview chứ không phải menu item). Bù bằng dialog + command palette. Nếu sau
này muốn hiện trong menu thì phải **generate** accelerator từ registry, không chép
tay.

## 4. Bảng phím tắt (đã chốt)

### 4.1 Global

| macOS | Win/Linux | Làm gì | Ghi chú |
|---|---|---|---|
| `⌘K` | `Ctrl+K` | **Clear all flows** | như Proxyman. Không hỏi xác nhận — §10.1 |
| `⌘P` | `Ctrl+P` | **Command palette** | đổi từ `⌘K`. Trùng phím Print — xem §9.5 |
| `⌘/` | `Ctrl+/` | Dialog Shortcuts | accelerator trong menu Help |
| `⌘,` | `Ctrl+,` | Settings | convention mac; chưa có |
| `⌘1`…`⌘5` | `Ctrl+1`…`5` | Flows / Rules / Break / Scripts / Certs | |
| `⌘⇧R` | `Ctrl+Shift+R` | Pause / Resume capture | **không dùng `⌘R`** — webview reload |
| — | — | Toggle system proxy | **cố ý không có chord** — §10.2 |
| `⌘S` | `Ctrl+S` | Save session (`.nova`) | |
| `⌘O` | `Ctrl+O` | Open session | |
| `⌘⇧E` | `Ctrl+Shift+E` | Export HAR | |

### 4.2 Flows table — chỉ khi table có focus

| Chord | Làm gì |
|---|---|
| `↑` / `↓` | row trước / sau |
| `Home` / `End` | row mới nhất / cũ nhất (list newest-first → `Home` là mới nhất) |
| `PageUp` / `PageDown` | nhảy một trang |
| `↵` | chuyển focus sang Inspector |
| `⌘↵` | Resend flow đang chọn |
| `⌘⇧C` | Copy as cURL |
| `⌘⇧A` | Toggle **Auto Select** (follow tail) |
| `⌘A` | Select all rows (phase 6) — **đụng `select_all` của Edit menu**, xem §9.3 |

### 4.3 Filter — lấy đúng của Proxyman

| Chord | Làm gì | Proxyman |
|---|---|---|
| `⌘F` | focus filter bar | Show: `⌘F` |
| `⌘⇧F` | focus filter tree ở sidebar | `⌘⇧F` |
| `Esc` | ẩn / bỏ focus filter bar | Hide: `ESC` |
| `⌘B` | bật/tắt filter đang có (**không mất** filter) | On/Off: `⌘B` |
| `⌘N` / `⌘⇧N` | thêm / bớt filter row (phase 9) | New / Remove |
| `⌘0` | ẩn/hiện sidebar | — (§10.3) |

### 4.4 Inspector

| Chord | Làm gì |
|---|---|
| `⌘[` / `⌘]` | tab trước / sau trong pane đang focus |
| `⌘⇧←` / `⌘⇧→` | đổi pane Request ↔ Response |
| `⌘E` | collapse / expand pane đang focus |

### 4.5 Dialog & modal

| Chord | Làm gì |
|---|---|
| `Esc` | đóng |
| `↑` / `↓` | di chuyển trong palette |
| `↵` | chạy item đang chọn |
| `Tab` / `⇧Tab` | đổi focus (native, không handle) |

## 5. Reserved — không được chiếm

`⌘Q` `⌘W` `⌘M` `⌘H` `⌘⇧H` · `⌘C` `⌘V` `⌘X` `⌘Z` `⌘⇧Z` · `⌘R` (reload webview) ·
`⌘Space` (Spotlight) · `⌘Tab` · `⌘⌥I` / `F12` (devtools) · `F5` (reload trên
Windows) · `⌘⌃F` (fullscreen).

`⌘P` **không** nằm trong danh sách này: trên macOS Print là *menu item*, và menu
của ta không có item Print (`File` chỉ có `close_window` + `quit`) → chord tự do.
Vẫn phải verify webview không tự mở print dialog (§9.5).

`⌘⇧P` cũng để trống, cố ý — xem §10.2.

Luật chung: mỗi chord **tối đa `Mod` + `Shift`**. Không Alt/Option (§2 quy tắc 5),
không Ctrl trên mac (§1 vấn đề 2). Có test chặn (§8).

## 6. Dialog Shortcuts

- **Mở từ 3 chỗ**: menu **Help → Keyboard Shortcuts** (accelerator `⌘/`), chord
  `⌘/`, và một entry trong command palette.
- Dùng lại khuôn `SettingsModal` (`App.tsx:2128`): `.scrim` + panel, `Esc` đóng,
  click scrim đóng.
- Layout: header + search input (lọc theo label và theo chord); body 2 cột, group
  theo `ShortcutGroup`; mỗi dòng label bên trái, badge phím bên phải (class `.kbd`
  đã có trong `styles.css`).
- Chord render theo platform: `⌘ ⇧ F` trên mac, `Ctrl Shift F` ở nơi khác.
- Dòng có `when` thì hiện chú thích mờ ("khi đã chọn một flow").
- **Không cho đổi phím ở v1.** Registry đã tách nên thêm sau chỉ là một lớp
  override + persist vào `prefs.ts`, không phải viết lại.
- Telemetry: event mới `ui.shortcut`, name = shortcut id → phải thêm vocab **cả 2
  phía** (`src/api.ts` + `commands.rs:640-666`), không thì fold thành `ui.other`.

## 7. Việc phải làm

| File | Nội dung |
|---|---|
| `src/shortcuts.ts` (mới) | registry + `formatChord()` + `matchChord()` + `isMac()` |
| `src/shortcuts.test.ts` (mới) | §8 |
| `src/useShortcuts.ts` (mới) | 1 listener + scope stack |
| `src/ShortcutsDialog.tsx` (mới) | dialog §6 |
| `src/App.tsx` | bỏ handler `⌘K` rời (`:627`), nối vào registry; listen `menu://shortcuts`; **sửa 2 chỗ hiển thị `⌘K`** → `⌘P`: badge nút Commands (`:717`) và dòng "or press ⌘K for commands" (`:1175`) |
| `src/api.ts` | `onMenuShortcuts()` (giống `onMenuCheckUpdates`), `UiEvent` += `ui.shortcut` |
| `src-tauri/src/menu.rs` | `const SHORTCUTS = "help.shortcuts"`, item trong Help + accelerator, `Action::Shortcuts`, `pub const SHORTCUTS_EVENT = "menu://shortcuts"` |
| `src-tauri/src/commands.rs` | `ui_event` += `ui.shortcut`; `ui_event_name` += các shortcut id |
| `design.md` | mục mới: dialog Shortcuts + badge phím |

## 8. Test

- Mọi `id` unique; mọi `chord` unique **trong cùng scope**; không chord nào nằm
  trong `RESERVED`; không chord nào chứa Alt.
- `formatChord()` đúng cho mac và cho Win/Linux (inject UA).
- Matcher: single-key bị bỏ khi focus trong input, nhưng `Escape` thì không; modal
  chặn global; chord không match thì **không** `preventDefault`.
- Rust: `action_for("help.shortcuts")` trả đúng action; 3 id không trùng nhau —
  khuôn test đã có sẵn trong `menu.rs` (`every_id_we_own_maps_to_the_action_it_names`).

## 9. Cần verify khi implement (chưa chắc, không đoán)

1. **Cú pháp accelerator của Tauri/muda cho `⌘/`** — `"CmdOrCtrl+Slash"` hay
   `"CmdOrCtrl+/"`. Phải thử, không chép từ trí nhớ.
2. **`⌘R` trong build release** có reload webview không (dev thì có). Nếu có thì
   phải `preventDefault` — và đó là lý do record toggle dùng `⌘⇧R`.
3. **`⌘A`**: Edit submenu có `select_all` predefined → trên mac nó chiếm `⌘A`
   trước webview. Muốn `⌘A` = select all rows thì phải bỏ item đó khỏi Edit (mất
   Select All trong text field) hoặc chọn chord khác. Verify rồi mới chốt.
4. **`⌘,`**: macOS thường tự map Preferences… Ta không có item Preferences trong
   app menu → phải tự handle ở webview; verify OS không ăn trước.
5. **`⌘P` có bị webview biến thành print không.** WKWebView không tự bind `⌘P`
   khi app không có Print menu item, nhưng WebView2 (Windows) **có** menu chuột
   phải với Print. Phải thử trên cả 2 platform; nếu bị ăn thì `preventDefault`
   trong handler là đủ (ta bắt trước ở `window` với `capture`).

## 10. Quyết định — đã chốt hết

### 10.1 `⌘K` = Clear: không hỏi xác nhận

`⌘K` xoá toàn bộ capture và **không undo được**: `clear_flows` bỏ retention ở
engine và ở body store, nên khôi phục ở frontend chỉ dựng lại được list *không có
body* — undo giả, tệ hơn là không có undo.

Chốt: **không modal xác nhận**, đúng chuẩn Proxyman và chuẩn tool loại này. Bù bằng
hai thứ:

- Toast sau khi clear nói rõ đã mất gì: `Cleared 148 flows` (dùng
  `toastDuration()` đang có trong `filter.ts`), không phải "Cleared".
- Dòng chú thích trong dialog Shortcuts ở ngay hàng `⌘K`: *"Không hoàn tác được —
  `⌘S` lưu session trước."*

Phương án "nút không hỏi, chord thì hỏi" bị loại: cùng một action không được có hai
hành vi.

### 10.2 Toggle system proxy: không có phím tắt

`⌘⇧P` là chord command palette trong VS Code / Sublime. Đặt "toggle system proxy"
vào đó nghĩa là người quen editor bấm `⌘⇧P` để mở palette sẽ **ghi lại cấu hình
proxy của cả máy** (`sysproxy.rs`, cần `restore_system_proxy` để dọn).

Chốt: action này **không có chord**. Giữ ở toolbar + command palette. Hiếm dùng,
hệ quả nặng, một chord là quá gần. `⌘⇧P` để trống, không cấp cho việc khác.

### 10.3 Sidebar và filter

Chốt theo Proxyman cho phần filter, `⌘0` cho sidebar:

| Chord | Làm gì |
|---|---|
| `⌘B` | bật/tắt filter đang có, **không mất** filter (Proxyman: On/Off) |
| `⌘0` | ẩn/hiện sidebar tree |

Lệch với VS Code (`⌘B` = sidebar) là cố ý: doc 0002 clone mô hình filter của
Proxyman, nên chord của filter cũng lấy của Proxyman — người dùng chuyển từ
Proxyman sang không phải học lại.

### 10.4 Không cho đổi phím tắt ở v1

Registry (§2) đã tách sẵn nên thêm sau chỉ là một lớp override + persist vào
`prefs.ts`. Làm ngay ở v1 thì phải giải quyết luôn conflict detection, reset,
migration khi chord mặc định đổi — chưa đáng.
