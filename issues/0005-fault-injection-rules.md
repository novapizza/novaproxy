# 0005 — Fault injection trong Rules (giả lập timeout / HTTP-0)

*Trạng thái: đã làm feature (2026-11-17), branch `feat/fault-injection`.*
Spike: `crates/nova-core/examples/fault_spike.rs`; test e2e: `crates/nova-core/tests/fault_injection.rs`.

## 1. Vấn đề

Cần giả lập các lỗi mạng mà app thấy thành status 0 (HTTP-0) hoặc timeout, theo
**từng endpoint**, để test xử lý timeout / retry / verify-before-retry của các
desktop app. Hiện NovaProxy chỉ có latency toàn cục, Block (403 cố định), Map
Local (status không chọn được) và hook JS (header / abort → 502).

## 2. Đề xuất: Fault là một kind trong Rules, kịch bản là group

Không thêm tab mới. Mọi thứ nằm trong trang Rules hiện có.

### 2.1 Rule

- `RuleKind` thêm `Fault`.
- `Rule` thêm (đều tuỳ chọn, rule cũ không đổi):
  - `method: Option<String>`: khớp thêm theo method (null = mọi method).
  - `group: Option<String>`: tên kịch bản, vd. `LH-54530 · A3 pass-cut`.
  - `times: Option<u32>`: số lần áp (null = luôn, 1 = chỉ lần đầu).
  - `fault: Option<Fault>`, với `Fault { mode, delay_ms, status, body, content_type, cut_after_bytes }`.
- Bộ đếm `hits` giữ trong bộ nhớ của engine (không ghi vào `rules.json`), reset được.
- `Edits` thêm `fault: Option<String>` (tên mode) để flow bị chèn lỗi có badge.

### 2.2 Mode

| Mode | Phase | Làm gì | Client thấy |
|---|---|---|---|
| `drop_before` | request | đóng kết nối, không gửi lên server | không có response (HTTP-0) |
| `hold_close` | request | giữ `delay_ms`, rồi đóng | HTTP-0 sau `delay_ms` (hoặc client tự timeout trước) |
| `hold_status` | request | giữ `delay_ms`, rồi trả `status` + `body` (vd. 504) | status đó |
| `fake_response` | request | trả ngay `status` + `body` + `content_type` | status đó |
| `waf_403` | request | 403 + HTML giống trang WAF (preset của `fake_response`) | 403 HTML |
| `proxy_auth_407` | CONNECT | trả 407 + `Proxy-Authenticate` cho CONNECT tới host khớp | 407 ở tunnel |
| `pass_cut` | response | gửi lên server (server **có** xử lý), bỏ response, đóng | HTTP-0, nhưng write đã xảy ra |
| `cut_body` | response | trả head + `cut_after_bytes` byte, rồi đóng | status 200, body đọc lỗi |
| `delay` | response | giữ response `delay_ms` rồi trả nguyên vẹn | response chậm |

Khớp rule: URL glob (như cũ) + method + host phải nằm trong **allowlist fault**
(xem 2.5). `proxy_auth_407` chỉ so host (CONNECT không có path).

### 2.3 UI (trang Rules)

- Dropdown kind có thêm **Fault**. Đầu card thêm chọn **Method** cạnh Match URL.
- Kind Fault hiện: **Mode**, rồi ô theo mode (`delay ms`, `status`, `body`,
  `content type`, `cut after bytes`), và **Áp dụng**: luôn / lần đầu / N lần.
- Đầu card: badge `hits: N` + nút reset.
- Rule có `group` được gom dưới header thu gọn được: công tắc cả nhóm, reset
  hits, export JSON. Rule không có group ở mục "Ungrouped" như hiện nay.
- Đầu trang: nút **Import** (file JSON nhiều rule, có group) cạnh "New rule";
  ô lọc theo tên group khi danh sách dài.
- Khi có rule Fault đang bật: banner nhắc trên thanh trên cùng ("Fault
  injection đang bật: N rule"), để không quên tắt.
- Bảng flow: badge `fault:<mode>` ở flow bị chèn lỗi (từ `edits.fault`).

### 2.4 MCP

- `set_rule` nhận thêm `method`, `group`, `times`, `fault` và kind `Fault`.
- Thêm `toggle_group {group, enabled}`, `get_rule_hits {rule_id? | group?}`,
  `reset_hits {rule_id? | group?}`, `import_rules {rules}`.
- Harness test (bật nhóm → chạy flow trong app → đọc hits + flows) đi hết qua MCP.

### 2.5 An toàn

- Allowlist host cho Fault (Settings), **mặc định trống**: rule Fault không khớp
  host ngoài list, dù glob khớp. Người dùng tự thêm host môi trường test.
- Fault không bao giờ tự sinh hay lặp request; `pass_cut` gửi đúng 1 lần.
- Hits và badge giúp chứng minh "proxy chỉ thấy 1 request".

## 3. Kết quả spike

Mục tiêu: hudsucker 0.24 có làm được "đóng kết nối không response" và 407 ở
CONNECT mà không patch không. **Có, cả hai, không cần patch.**

Cách làm:
- **Đóng không response:** handler trả `Response` có body lỗi ngay lần poll
  đầu (`Body::from_stream(once(Err(..)))`). hyper chỉ ghi head vào buffer, chưa
  flush; body lỗi → hyper huỷ connection → client nhận 0 byte.
- **`pass_cut`:** forward request, ở `handle_response` thay response bằng body lỗi như trên.
- **`cut_body`:** cho qua N byte, rồi **chờ ~200 ms** mới trả lỗi. Không chờ
  thì head + N byte vẫn nằm trong buffer, client nhận 0 byte (giống `drop_before`).
- **407:** `handle_request` cũng được gọi cho CONNECT; trả 407 thì không có tunnel.
- Phía client giải mã chỉ chạy HTTP/1.1 (hudsucker không bật feature `http2`),
  nên không gặp trường hợp h2 gửi HEADERS trước body.

Đo bằng curl và socket thô qua proxy, upstream là server local:

| Mode | curl | Socket thô | Ghi chú |
|---|---|---|---|
| abort (HTTPS MITM) | exit 52 *Empty reply*, 0 byte | — | |
| hold-close 2 s (HTTPS) | exit 52 sau 2,00 s | 0 byte sau 505 ms (ms=500) | |
| hold-504 1 s | 504, 1,01 s | — | |
| waf-403 | 403 HTML | — | |
| pass-cut POST | exit 52, 0 byte | 0 byte | upstream **có nhận** POST |
| cut-body n=10 | 200, `Content-Length: 100`, exit 18 *90 bytes remaining* | 132 byte | |
| CONNECT 407 | exit 56, `407` + `Proxy-Authenticate` | — | |
| request thường | 200, 100 byte | 222 byte | không ảnh hưởng |

## 4. Rủi ro / cần kiểm khi làm feature

1. **Chromium tự gửi lại:** khi socket keep-alive dùng lại bị đóng không
   response, Chromium (Electron `net.fetch`) có thể **tự gửi lại request một
   lần**. Với `drop_before` / `pass_cut` trên request ghi, hits có thể là 2 mà
   app không hề retry. Cần kiểm trên app thật. Cách giảm: chỉ lần đầu
   (`times = 1`) + đọc hits / flows để tách retry của Chromium với retry của app.
2. Cơ chế "không flush" dựa vào hành vi hiện tại của hyper 1.x (buffer head tới
   lần poll body đầu). Thêm test e2e trong `tests/` khoá lại hành vi này để
   biết ngay khi bump hyper / hudsucker.
3. `handle_request` giữ `delay_ms` ngay trong task của request, không chặn
   request khác (mỗi request một task). Cần giới hạn `delay_ms` (vd. ≤ 600 s).
4. Flow bị fault phải được ghi `FlowState::Error` + `edits.fault`, kể cả
   `drop_before` (chưa có response upstream).

## 5. Bước tiếp theo

1. `nova-proto`: `RuleKind::Fault`, `Fault`, field mới của `Rule`, `Edits.fault`;
   regen bindings TS.
2. `nova-core`: `rules.rs` khớp method / times / allowlist + hits; `intercept.rs`
   các mode như mục 3; test e2e cho từng mode (dựa trên `fault_spike`).
3. MCP + UI Rules như 2.3, 2.4.
4. Bộ rule mẫu (JSON import) cho các kịch bản test timeout.
