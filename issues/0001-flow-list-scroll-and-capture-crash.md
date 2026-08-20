# 0001 — Flow list không virtual scroll, record một lúc thì UI crash & reload

*Trạng thái: đã sửa phần chính (chưa commit) · Còn phần cần review ở cuối.*

## Tóm tắt

Section **Flows** render toàn bộ flow ra DOM và cập nhật store theo từng snapshot.
Record một lúc là scroll giật, rồi renderer của webview bị kill → UI tự reload
(mất list đang xem).

Ba nguyên nhân độc lập, cộng dồn:

1. **Không có windowing.** `FlowsSection` map thẳng `filtered` ra DOM. Retention là
   `MAX_FLOWS = 10_000`, mỗi row là một `<button>` với ~10 node con → **hơn 100k
   DOM node**. Mỗi snapshot React phải reconcile toàn bộ.
2. **Mỗi snapshot là một store update.** Một flow phát 3 snapshot (started →
   response → completed); traffic nặng là hàng trăm message/giây, mỗi cái
   re-render cả list. Cộng với (1) là render storm.
3. **Store giữ toàn bộ body preview.** Engine cap preview ở
   `DEFAULT_INLINE_CAP = 512 KB` mỗi body (`crates/nova-core/src/bodystore.rs:33`),
   list giữ 10.000 flow × (request + response). JS string là UTF-16 nên body ASCII
   tốn gấp đôi → heap của webview phình tới hàng GB. Đây là phần thực sự làm
   process chết, (1) và (2) làm nó tới nhanh hơn.

Phụ: `wsMessages` **không có cap** per flow và `WsPanel` render mọi frame — một
socket chatty tái hiện đúng vấn đề (1) + (3) bên trong tab WebSocket.

## Các phần đã làm

### 1. Windowed flow list

| File | Nội dung |
|---|---|
| `src/virtual.ts` (mới) | `sliceGroups()` — hình học thuần: group nào on-screen, render row `[from, to)`, spacer `padTop`/`padBottom` giữ chỗ phần còn lại |
| `src/virtual.test.ts` (mới) | 13 test |
| `src/App.tsx` | `FlowList` + `FlowRow` (memo) thay khối markup cũ |
| `src/styles.css` | bỏ `animation: novaflow` trên `.flow-row` |

Quyết định đáng lưu:

- **Slice theo từng group, không flatten thành một mảng row.** Header host là
  `position: sticky`; nó chỉ sticky đúng theo từng host khi mỗi group còn là
  containing block riêng. Flatten thì các header dồn lên nhau ở đỉnh.
- **Chiều cao row/header đo từ DOM** (`getBoundingClientRect`, giữ phần thập
  phân) chứ không hardcode: chiều cao đi theo font, và window mà số học lệch
  layout thì spacer cộng không khớp → scrollbar trượt dần.
- **Bỏ animation mount của row**: list windowed thì row mount lại mỗi lần scroll
  vào viewport, animation sẽ nháy liên tục.
- Viewport đo trong `useLayoutEffect` + `ResizeObserver` (resize cửa sổ / kéo
  splitter là window lại). Scroll chỉ re-render `FlowList`, không kéo theo
  `App`/`Detail`.

Kết quả: số row trong DOM là **hằng số** (~33 row ≈ 350 node) dù list 100 hay
100.000 flow — có test khẳng định (`keeps the row count bounded however long the
list gets`).

### 2. Gộp snapshot theo animation frame

- `src/App.tsx`: channel flow và channel WS đẩy message vào queue, flush 1 lần
  mỗi `requestAnimationFrame`.
- `src/store.ts`: `upsertFlows(batch)` / `addWsMessages(batch)`.
- Trong batch, flow tìm qua **index map dựng một lần**, đọc qua `shift` (mỗi
  prepend dịch index đi 1) và **đối chiếu id trước khi ghi** — snapshot của flow
  đã bị evict không bao giờ ghi đè row của flow khác.
- Số lần re-render chặn ở tần số màn hình, bất kể traffic bao nhiêu.

### 3. Body: refetch on demand (không giữ bytes trong webview)

**Backend** — `src-tauri/src/commands.rs`:

- `read_body` có hai nguồn: disk body store như trước, còn body chưa từng spill
  thì lấy từ **flow đang được engine retain**. Preview giữ `spilled` đúng sự thật
  (không spill → `false`), nên inspector không mời "load full body" từ store
  không có nó.
- Tách `read_body_from(&AppState, …)` để test gọi được mà không cần `State`.
- Command mới `retained_flows` → mọi flow engine đang giữ, newest-first, kèm body.
- `resend_flow` hydrate request body trước khi replay: UI gửi flow nó đang giữ
  (không bytes), body lấy lại từ store → replay vẫn gửi đúng body đã capture.

**Store** — `src/store.ts`:

- `withoutBodies()` cắt `text`/`base64` khi flow vào store, **giữ nguyên
  metadata** (size, media type, `truncated`, `spilled`) để inspector biết có gì và
  fetch được.
- **Flow import từ `.nova` được miễn**: chúng không tồn tại ở đâu khác, `loadFlows`
  giữ nguyên bytes.

**UI** — `src/App.tsx`:

- Hook `useBodyBytes` fetch bytes khi store không giữ. Copy đã fetch được **tag
  theo flow + side** → đổi flow không bao giờ hiện body của flow này dưới header
  của flow khác. (Sửa luôn bug tiềm ẩn của `BodyBlock` cũ: nó reset trong
  `useEffect`, nên có một frame hiện body của flow trước.)
- Tab cURL tách thành `CurlPanel` để chỉ fetch khi tab mở; `copyCurl` hydrate
  trước khi ghi clipboard — cURL thiếu `--data` thì không còn là request đó.
- Export session/HAR đọc qua `retained_flows`, fallback về list cho flow đã
  import → file xuất ra đầy đủ body như trước.

### 4. WebSocket frames

- Cap **2.000 frame/socket** (`MAX_WS_FRAMES`), bỏ frame cũ nhất, đếm số đã bỏ
  per socket (`wsDropped`; xoá cùng flow khi flow bị evict).
- `WsPanel` render **400 frame mới nhất**, kèm dòng ghi rõ *"N earlier frames
  dropped at the 2.000-frame cap"* + link "Show N earlier retained frames".
- **Chỗ này đi lệch khỏi phương án "virtualize"**: `.ws-payload` là
  `white-space: pre-wrap; word-break: break-word` → chiều cao mỗi row biến thiên
  theo payload. Windowing kiểu row cố định sẽ hoặc lệch layout, hoặc buộc clamp
  payload về một dòng (mất khả năng đọc payload nhiều dòng đang có). Cap + cửa sổ
  400 frame chặn DOM tương đương mà không đổi cách frame hiển thị.

### Verify đã chạy

- `npm test` → **94 pass** (60 trước đó + 13 hình học + 21 store).
- `cargo test --workspace` → toàn bộ xanh, gồm **7 test mới**
  `src-tauri/tests/body_refetch.rs`: body chưa spill trả về từ flow retained và
  `spilled == false`; flow không có body báo lỗi rõ thay vì bịa; flow đã evict báo
  "no longer retained"; replay flow bị strip vẫn gửi đúng body; hydrate không đè
  body user sửa tay; body rỗng không bị coi là thiếu.
- `tsc --noEmit` + `vite build` sạch; app boot không lỗi.

## Phần cần review / làm tiếp

1. **Chưa verify với traffic thật.** Engine chỉ start qua toggle system-proxy
   (cần quyền admin) nên chưa chạy được end-to-end trong lúc sửa. Cần: bật proxy,
   record vài nghìn flow, kiểm tra (a) scroll mượt và DOM không phình,
   (b) mở flow cũ thì body load qua đường fetch mới, (c) memory của webview đứng
   yên theo thời gian.

2. **Engine cũng giữ preview trong RAM — chưa chặn.** `FlowStore` retain 10.000
   `Flow`, mỗi cái mang preview tới 512 KB (Rust UTF-8 nên bằng nửa phía JS,
   nhưng cùng bậc). Bỏ preview khỏi webview chặn được process hay chết
   (renderer), phía Rust thì chưa. Hướng chặn cả hai bên: **hạ
   `DEFAULT_INLINE_CAP`** (512 KB → 64 KB) để mọi body lớn hơn đều spill xuống
   disk và chỉ fetch khi cần. Chưa làm vì nó đổi ý nghĩa "preview" trên toàn
   engine, ảnh hưởng cả payload MCP → cần quyết định về mặt sản phẩm.

3. **Virtualize thật cho `WsPanel`** (nếu muốn): cần windowing có đo từng row
   (measurement cache) vì payload wrap nhiều dòng. Xem mục 4 ở trên.

4. **Hành vi cũ được giữ nguyên, nhưng đáng xem lại:** snapshot đến muộn của một
   flow đã bị evict khỏi window sẽ được **prepend lại như flow mới** (nhảy lên
   đầu list, lệch thứ tự thời gian). Đây là hành vi từ trước khi sửa; batching
   chỉ đảm bảo nó không ghi đè row khác. Có test ghi lại đúng invariant đó:
   `never writes a snapshot over a different flow after an eviction`.

5. **Accessibility:** row ngoài viewport không còn tồn tại trong DOM nên không
   tab tới được — đặc tính chung của virtual list. Nếu cần điều hướng bằng bàn
   phím trong list thì phải thêm keyboard nav + scroll-into-view (hiện list không
   có keyboard nav, chỉ command palette có).

6. Diff **chưa commit**: 8 file sửa + 3 file mới (`src/virtual.ts`,
   `src/virtual.test.ts`, `src-tauri/tests/body_refetch.rs`).
