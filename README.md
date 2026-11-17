# NovaProxy

An open-source HTTP/HTTPS debugging proxy: a Rust engine (hudsucker + rustls)
in a Tauri v2 desktop app with a React/TypeScript UI. It captures and decrypts
traffic, lets you change it on the way through, finds anti-patterns in it, and
hands it to AI agents over MCP.

Platforms: **macOS (Apple Silicon)** and **Windows**, both signed and
auto-updating. Linux builds in CI but is not shipped yet.

The original design and roadmap are in [NovaProxy.md](./NovaProxy.md); review
notes and plans are in [notes/](./notes/).

## What it does

**Capture**
- HTTP and HTTPS interception with a generated root CA and per-host leaf
  certificates. Upstream connections trust Mozilla's roots plus the OS store
  (so corporate re-signing proxies work), never NovaProxy's own CA.
- HTTP/2 to servers, negotiated by ALPN; apps can talk HTTP/1.1 to the proxy
  either way. The Protocol column shows what was spoken with the server.
- **SSL proxying scope**: decrypt all hosts, or tunnel some untouched (pinned
  apps, mTLS).
- **System proxy** on macOS and Windows, with a snapshot of the previous
  settings that is restored on disable, on quit, and after a crash.
- **Keep apps on TCP**: HTTP/3 advertisements (`Alt-Svc: h3`) are taken out of
  responses so apps do not switch to QUIC, which no HTTP proxy can see.
- Each flow is attributed to the app that sent it, with its icon.
- WebSocket capture: frames are kept by the engine and streamed to the UI as
  previews; a frame's full payload loads when you expand it.
- Real timing per flow (DNS, connect, TLS, sending, server wait, download) and
  a slow-server marker (🐢) in the table.

**Memory and disk stay bounded on long captures**: 10,000 flows retained;
64 KB body previews held compressed, larger bodies spilled to disk (64 MB each,
1 GB total); WebSocket payloads under a 64 MB budget; no body bytes on the
engine → UI stream; exports written to disk one flow at a time.

**Change traffic**
- Rules: Map Remote, Map Local, Block, header Rewrite — live, persisted.
- Breakpoints: pause a matching request, edit it, continue or abort.
- JavaScript hooks (`onRequest` / `onResponse`) in a QuickJS sandbox.
- Network conditions: added latency and a download throttle.
- Resend any flow; copy as cURL.

**Understand it**
- **Insights**: 23 fixed rules over the capture — request bursts and N+1
  loops, polling, retry storms, rate limiting, error rates, uncompressed or
  oversized responses, caching mistakes, redirect chains, slow servers, CORS
  preflight cost, credentials in URLs or over plain HTTP, insecure cookies,
  heavy images, mixed content, missing security headers, broken assets, empty
  responses, slow connection setup, CDN misses. Each finding has numbers,
  advice and example flows; mute what you don't care about. Exports as
  Markdown.
- Filters by app, host, protocol, payload kind and status, plus a condition
  builder; save filters and app/host selections.

**Share it**
- Save a session (`.nova`) and reopen it later.
- Export HAR 1.2. Credentials (auth headers, cookies, API keys, password and
  token fields) are hidden by default — **Settings → General → Exports**.

**MCP server for AI agents** (off by default; **Settings → MCP**)

Loopback only, and a client must send the access token shown in Settings. The
command to copy there looks like:

```bash
claude mcp add --transport http novaproxy http://127.0.0.1:9091/ \
  --header "Authorization: Bearer <token>"
```

Tools: `list_flows`, `search_flows`, `get_flow`, `get_body`,
`summarize_session`, `get_insights`, `explain_failure`, `diff_flows`,
`replay_request`, `list_rules`, `set_rule`, `delete_rule`, `import_rules`,
`toggle_group`, `get_rule_hits`, `reset_hits`, `get_fault_settings`, `proxy_status`.
Credentials are hidden in what the endpoint returns unless you turn that off;
replays still send the real values. The token can be rotated in Settings.

## Layout

```
crates/nova-proto   shared serde types → src/bindings/*.ts (ts-rs)
crates/nova-core    the engine: intercept.rs (capture, tee, WebSocket),
                    insights.rs, redact.rs, timing.rs, bodystore.rs,
                    flowstore.rs, wsstore.rs, rules.rs, scripting.rs, ca.rs,
                    trust.rs, upstream.rs
crates/nova-os      OS surfaces: system proxy, trust store, helper IPC, app icons
crates/nova-helper  the macOS privileged helper (system proxy, trust)
src-tauri           the app: state.rs (stream to the UI), commands.rs, mcp.rs,
                    analysis.rs (MCP summary/diff/explain), export.rs, menu.rs
src/                React frontend
  flows/            scope tree, filter bar, table, columns
  inspector/        request and response panes and their panels
  shortcuts.ts      every chord, read by both the dispatcher and the dialog
```

**Keyboard**: `⌘/` (or **Help → Keyboard Shortcuts**) lists every chord. `⌘P`
opens the command palette; `⌘K` clears the capture.

**Visual harness**: `preview.html` renders the Flows section against mock
flows — `npm run dev`, then open `/preview.html` (`?panels=1` for every
inspector panel, `?shortcuts=1` for the dialog). No proxy, CA or traffic
needed.

## Develop

Prerequisites: Rust (stable), Node 20+, and the Tauri v2 prerequisites for your
platform (Xcode command line tools on macOS; on Windows, the MSVC build tools
and NASM, which aws-lc needs).

```bash
npm install
npm run gen:types      # regenerate src/bindings from Rust
npm run app            # tauri dev — the desktop app plus vite
```

In the app: **Start proxy** → **Certificate** → *Install & trust*, then turn on
the system proxy or point a client at it:

```bash
curl -x http://127.0.0.1:9090 --cacert "<ca.pem>" https://example.com
```

(The Certificate page shows where `ca.pem` is.)

## Check

What CI runs on every pull request (`.github/workflows/ci.yml`):

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace            # unit tests plus end-to-end tests over real sockets
npm run lint                      # ESLint: errors fail, hook-dependency warnings do not
npx tsc --noEmit
npx vitest run
```

Tests and clippy run on Windows and macOS; a Linux job builds and tests too
but does not block a merge yet. For a headless engine to try by hand:

```bash
cargo run -p nova-core --example serve    # proxy on :39190, prints each flow
```

## Updates

Released builds check for a new version themselves. **Settings → General →
Updates** shows the current version, checks on demand, and installs — and the
native menu asks for that same check: **NovaProxy → Check for Updates…** on
macOS, **Help → Check for Updates…** on Windows, which opens Settings on the
card rather than answering in a dialog of its own. The check also runs once at
launch (switchable there) but only ever *reports* — installing replaces a
binary that holds a root CA and closes the window (an exec into the new build
on macOS, an exit into the NSIS installer on Windows), so it stays a click.

The plumbing:

```
tag push → CI builds + signs → NovaProxy_<ver>_<arch>.app.tar.gz + .sig
                             → latest.json on R2  ← tauri-plugin-updater polls
                             → latest.yml  on R2  ← download page reads
```

`crates/nova-proto` carries `UpdateStatus` / `UpdateProgress`,
`src-tauri/src/update.rs` owns the two commands, `src-tauri/src/menu.rs` holds
the menu item (which only emits an event — the card does the work), and
`src/update.ts` holds the card's state machine (tested in `src/update.test.ts`).

A development build has **no** updater endpoint — the endpoint and public key
are injected at release time — and Settings says so rather than offering a
button that can only fail.

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
