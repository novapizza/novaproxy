# NovaProxy

An open-source, cross-platform HTTP/HTTPS debugging proxy — a Proxyman alternative
built on a native Rust core (hudsucker) inside a Tauri v2 shell with a React/TS UI.

See [NovaProxy.md](./NovaProxy.md) for the full design and phased roadmap.

## Status — Phase 0 + Phase 1 foundation (runnable)

Implemented and verified end-to-end:

- **Native Rust proxy engine** (`crates/nova-core`) on hudsucker 0.24 + rustls.
  - HTTP **and** HTTPS interception (MITM) with a generated root CA and
    **on-the-fly per-host leaf certs** (cached).
  - **Streaming body tee** — chunks are forwarded downstream immediately while a
    capped copy is captured, so SSE / gRPC / large downloads are never stalled.
  - **Request↔response correlation** (hudsucker gives us none) via a
    per-connection FIFO; `CONNECT` tunnels are excluded.
  - gzip / deflate / brotli decode on the captured copy; text vs. binary sniffing.
- **One-click certificate management** (`crates/nova-core/src/{ca,trust}.rs` +
  Tauri commands): auto root CA (0600 key), live trust status, and
  install / uninstall / regenerate behind one native macOS auth prompt.
- **Tauri backend** (`src-tauri`): start/stop proxy, CA commands, and flow
  streaming to the UI over a Tauri **Channel**.
- **Shared types** (`crates/nova-proto`) generated to TypeScript with `ts-rs`.
- **React UI** (`src/`): Proxyman-style dense dark dashboard — content-type
  tabs, host sidebar, request list, request/response inspector (headers / query
  / body / raw, JSON pretty-print, image preview), cert panel, setup help.

### Phase 2 — traffic control (landed)

- **Rules engine** (`crates/nova-core/src/rules.rs`): wildcard-matched **Map
  Remote**, **Map Local**, **Block**, and header **Rewrite**, applied to live
  traffic and persisted to `rules.json`. Editable in the Rules section; edits
  take effect without restarting the engine. (Map Remote fully redirects plain
  HTTP; for already-tunneled HTTPS only the Host header changes.)
- **Resend / Replay**: re-issues a captured flow *through* the proxy so it is
  recaptured (tagged `resent`).
- **System-proxy toggle** (`crates/nova-core/src/sysproxy.rs`, macOS): points
  every network service's HTTP/HTTPS proxy at NovaProxy behind one auth prompt,
  **snapshots prior state**, restores it on disable, and **restores after an
  unclean exit** on next launch.
- **Breakpoints** (`crates/nova-core/src/breakpoint.rs`): arm a URL-glob
  breakpoint; the next matching request pauses mid-flight (its task `.await`s a
  oneshot while the connection is held open), raising the **intercept modal**
  where you edit request headers and **Continue** or **Abort**. One-shot arming.

### Phase 3 — scripting (landed)

- **JavaScript sandbox** (`crates/nova-core/src/scripting.rs`): a QuickJS
  interpreter (rquickjs) on a dedicated thread — the async handler ships a flow
  snapshot over channels and awaits the result. User scripts define
  `onRequest(flow)` / `onResponse(flow)`; each `flow` exposes `method`, `host`,
  `path`, `url`, `status`, a mutable `headers` map, and `abort()`. Header edits
  are applied and `abort()` short-circuits with a 403. Editable in the Scripts
  section, persisted to `script.js`, toggled on/off live. A throwing hook can't
  wedge traffic (errors leave the flow unchanged).

- **Network conditions** (`crates/nova-core/src/intercept.rs`): simulate
  latency (delay before each response) and a downlink throttle (paces response
  body chunks to a kbps cap), configured in Settings and persisted.
- **Session save/load + HAR export** (`src/session.ts`): save all captured
  flows to a `.nova` file and reload them, or export to HAR 1.2 — via the native
  file dialog (commands in the palette). Per-flow cURL export was already there.

Not yet built (rest of Phase 3+): reverse proxy, WebSocket message inspection,
raw TCP/UDP view, protobuf decode; plus TLS passthrough auto-fallback, body
spill-to-disk, real timing instrumentation, transparent capture.

## Layout

```
crates/nova-proto   shared serde types → src/bindings/*.ts (ts-rs)
crates/nova-core    proxy engine: ca.rs, trust.rs, intercept.rs, flow.rs, lib.rs
src-tauri           Tauri shell: state.rs, commands.rs, lib.rs
src/                React frontend (App.tsx, store.ts, api.ts, styles.css)
```

## Develop

Prereqs: Rust (stable), Node 20+, and the Tauri v2 macOS deps (Xcode CLT).

```bash
npm install
npm run gen:types      # regenerate src/bindings from Rust (also runs in tests)
npm run app            # tauri dev — launches the desktop app + vite
```

In the app: **Start proxy** → open **Certificate** → *Install & trust* → route a
client through the proxy:

```bash
curl -x http://127.0.0.1:9090 --cacert "<ca.pem>" https://example.com
```

Flows stream into the list in real time.

## Updates

Released builds check for a new version themselves. **Settings → General →
Updates** shows the current version, checks on demand, and installs — and the
native menu asks for that same check: **NovaProxy → Check for Updates…** on
macOS, **Help → Check for Updates…** on Windows and Linux, which opens Settings
on the card rather than answering in a dialog of its own. The check also runs
once at launch (switchable there) but only ever *reports* — installing
replaces a binary that holds a root CA and closes the window (an exec into the
new build on macOS, an exit into the NSIS installer on Windows), so it stays a
click.

The plumbing:

```
tag push → CI builds + signs → NovaProxy_<ver>_<arch>.app.tar.gz + .sig
                             → latest.json on R2  ← tauri-plugin-updater polls
                             → latest.yml  on R2  ← download page reads
```

`crates/nova-proto` carries `UpdateStatus` / `UpdateProgress`, `src-tauri/src/update.rs`
owns the two commands, `src-tauri/src/menu.rs` holds the menu item (which only
emits an event — the card does the work), and `src/update.ts` holds the card's
state machine (tested in `src/update.test.ts`).

A development build has **no** updater endpoint — the endpoint and public key are
injected at release time — and Settings says so rather than offering a button
that can only fail.

### Releasing an updatable build

One-time, to create the signing keypair:

```bash
npx tauri signer generate -w ~/.tauri/novaproxy.key
```

Then set, in the repo's `Production` environment:

| Secret | What |
|---|---|
| `TAURI_SIGNING_PRIVATE_KEY` | contents of the private key file |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | its password |
| `TAURI_UPDATER_PUBKEY` | the public key printed by `signer generate` |
| `R2_PUBLIC_BASE_URL` | where the manifests are served, e.g. the bucket's `r2.dev` domain |

Miss any of them and the release still builds — it just cannot be an update
source, which CI says with a warning rather than a failure.

## Verify

```bash
cargo test -p nova-core          # end-to-end capture test over a real socket
cargo run -p nova-core --example serve   # headless engine on :39190 for manual curl
npx tsc --noEmit                 # frontend typecheck
```
