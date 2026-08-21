# 0002 — Setup build/deploy cho NovaProxy (GitHub Actions)

*Trạng thái: 2 workflow đã commit (`88eef9e`, branch `fix/flow-list-windowing`).
Chưa push, chưa chạy lần nào. Các việc dưới đây phải làm trước khi push tag đầu tiên.*

## Đã có gì

| File | Trigger | Làm gì |
|---|---|---|
| `.github/workflows/ci.yml` | push/PR vào `main` | `npm ci` → `npm run build` → `npm test` → `cargo test --workspace` → clippy (non-blocking) |
| `.github/workflows/release.yml` | tag `v*.*.*` | guard version → import cert → sign `nova-helper` → `tauri-action@v1` (sign + notarize + staple + draft release) → verify → xoá keychain |

Scope đã chọn: **chỉ `.dmg`**, **chỉ arm64**, publish lên **GitHub Releases** (draft).
Không có `.pkg` trong CI (`scripts/build-pkg.sh` vẫn dùng cho local), không R2,
không auto-update.

## Việc cần làm

### 1. Tạo environment `Production` ở repo novaproxy — **bắt buộc**

`release.yml` khai `environment: Production`. Repo novaproxy hiện **chưa có**
environment nào (novapad có `Production` + `github-pages`). Nếu không tạo, job sẽ
không chạy.

```
gh api -X PUT repos/novapizza/novaproxy/environments/Production
```

Hoặc bỏ dòng `environment: Production` khỏi `release.yml` nếu không cần
protection rule.

### 2. Xác nhận org secrets nhìn thấy được từ novaproxy — **bắt buộc**

Workflow dùng đúng tên secret novapad đang dùng, **không cần tạo secret mới**:

| Tauri env | Secret |
|---|---|
| `APPLE_CERTIFICATE` | `CSC_LINK` |
| `APPLE_CERTIFICATE_PASSWORD` | `CSC_KEY_PASSWORD` |
| `APPLE_PASSWORD` | `APPLE_APP_SPECIFIC_PASSWORD` |
| `APPLE_ID` | `APPLE_ID` |
| `APPLE_TEAM_ID` | `APPLE_TEAM_ID` |

Repo novapad có **0 repo-level secret**, env `Production` chỉ có `R2_RELEASES_*`
+ `SLACK_WEBHOOK_URL` → 5 secret trên chắc chắn là **org-level**. Chưa kiểm tra
được visibility (cần scope `admin:org`). Nếu là "selected repositories" thì phải
add `novaproxy` vào danh sách.

Dấu hiệu nhận biết: `CSC_LINK` trống thì workflow **warning rồi build dmg
unsigned**, không fail. Đọc annotation của run là biết.

### 3. Push branch + mở PR để CI chạy lần đầu

`ci.yml` chỉ trigger trên `main`, nên phải mở PR vào `main` mới thấy nó chạy.
Chưa biết `cargo test --workspace` có xanh trên runner không —
`crates/nova-os/tests/helper_ipc.rs` đáng nghi nhất (unix socket + launchd path).
Nếu đỏ thì `--skip` test đó kèm ghi chú lý do.

### 4. Bump version rồi mới tag

Guard đầu `release.yml` fail nếu tag lệch `tauri.conf.json`. Hiện cả 3 chỗ đều
`0.1.0`:

- `src-tauri/tauri.conf.json` → `version`
- `Cargo.toml` → `workspace.package.version`
- `package.json` → `version`

```
git tag v0.1.0 && git push origin v0.1.0
```

Release ra ở dạng **draft** (giống novapad) — phải publish tay.

### 5. Sau lần release đầu — kiểm tra thủ công

Tải dmg về máy sạch (hoặc `xattr -w com.apple.quarantine` để giả lập) và mở:

```
spctl --assess --type execute -vv /Applications/NovaProxy.app
codesign -dv --verbose=4 /Applications/NovaProxy.app/Contents/Resources/nova-helper
```

Rồi bật system-proxy toggle để chắc helper install được từ bundle đã sign +
notarize — đây là đường dễ vỡ nhất, xem mục "Vì sao phải sign helper tay" dưới.

## Vì sao phải sign `nova-helper` bằng tay

`tauri-bundler/src/bundle/macos/app.rs` gom frameworks, `externalBin` và
`Contents/MacOS` vào `sign_paths`, nhưng kết quả của `settings.copy_resources()`
**không** bao giờ vào `sign_paths`. `nova-helper` là Mach-O executable được bundle
dạng `resources` (`tauri.conf.json` → `bundle.resources`), nên notary service sẽ
reject cả app vì có nested binary chưa sign.

Nên `release.yml` tự import cert vào keychain riêng, sign helper với hardened
runtime **trước** `tauri build`. `beforeBuildCommand` chạy lại
`cargo build --release -p nova-helper` nhưng cargo thấy fresh nên no-op →
signature còn nguyên → Tauri copy vào Resources rồi sign vỏ ngoài.

Chỗ này dựa vào cargo freshness, nên có step verify đọc lại signature từ
`Contents/Resources/nova-helper` **bên trong** `.app`. Nếu step đó fail thì giả
định trên đã vỡ — cách sửa dứt điểm là chuyển helper sang `bundle.externalBin`
(Tauri sign luôn), nhưng phải sửa `helper::source_binary` trong
`crates/nova-os/src/helper.rs` vì externalBin lands ở `Contents/MacOS` với suffix
`-{triple}`.

## Chưa làm (nếu sau này cần)

- **`.pkg`**: cần cert **Developer ID Installer** (khác `Developer ID Application`
  novapad đang dùng), và `scripts/build-pkg.sh` chưa có bước
  `xcrun notarytool` + `stapler`.
- **x86_64 / universal**: không truyền `--target` là cố ý — `--target` đổi path
  thành `target/<triple>/release/`, chỗ `bundle.resources` trong tauri.conf không
  còn tìm thấy helper. Muốn 2 arch thì dùng 2 runner khác nhau, đừng dùng
  `--target`.
- **Auto-update**: app chưa có `tauri-plugin-updater`; cần thêm plugin +
  `latest.json` + keypair `TAURI_SIGNING_PRIVATE_KEY`.
- **Publish lên R2** như novapad (`R2_RELEASES_*` + `prune-r2`).
- **`cargo fmt --check`** và `clippy -D warnings`: rustfmt chưa từng được cài với
  toolchain này nên tree gần như chắc chắn chưa clean. Dọn rồi mới bật.
- **`audit.yml`**: novapad có `pnpm audit` chạy tuần; bản novaproxy sẽ là
  `npm audit` + `cargo audit`.
