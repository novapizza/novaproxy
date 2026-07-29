# NovaProxy — an open-source Proxyman alternative

## Context

Proxyman is a best-in-class HTTP/HTTPS debugging proxy, but it is **closed-source** and strongest only on macOS. The goal is to build **NovaProxy**: an open-source, cross-platform debugging proxy that matches Proxyman's feature set and beats it on UI/UX.

Decisions locked in with the user:
- **Proxy engine:** native Rust core (mitmproxy used as *architectural reference only*, not embedded). Single static binary, full control, top performance.
- **Platform:** cross-platform via **Tauri v2** (Rust shell + web frontend) → macOS, Windows, Linux from one codebase. Cross-platform is the differentiation angle vs Proxyman's macOS-centric strength.
- **Stack:** Rust + TypeScript/React (matches the user's background, ideal for Tauri).
- **Target:** Proxyman feature **parity** as the north star. Because a from-scratch native core can't land all features at once, parity is delivered through the **phased roadmap** below.

This is a greenfield project — the working directory was empty at the start. Everything below is *new* code.

## Architecture

```
[ React/TS frontend ] ──Tauri commands──▶ [ Tauri Rust backend ]
        ▲                                          │
        └────── Tauri Channel (streamed flows) ────┘
                                                   │
                                          [ nova-core proxy engine ]
                                          hudsucker + rustls + rcgen + tokio
                                                   │
                                          intercepts HTTP/HTTPS/WS traffic
```

- **Frontend → backend:** Tauri **commands** (start/stop proxy, apply rule, resend request, install cert, toggle system proxy).
- **Backend → frontend:** Tauri **Channels** for high-frequency flow streaming (the Tauri event system is *not* built for throughput — channels are the documented choice). Batch/throttle updates (e.g. coalesce on a ~16–50ms tick) so a flood of requests can't overwhelm the UI.
- **Proxy engine** runs as an in-process async **tokio task** inside the Tauri backend (no separate sidecar — the core is already Rust).
- **Shared types:** define Rust structs once (`Flow`, `Rule`, etc.) and generate TS types with **`ts-rs`** so frontend and backend never drift.

### Engine reality — verified against hudsucker's API (prototype these first)

Two facts confirmed from hudsucker's current docs that shape the Phase 1 data model. Both are cheap to design in and expensive to retrofit — spike them before committing to the `Flow` shape.

- **Bodies are streaming, not buffered.** `handle_request(&mut self, ctx, req: Request<Body>)` and `handle_response(&mut self, ctx, res: Response<Body>)` hand you hyper **streaming** bodies. The doc's "capture req/resp, decode gzip/brotli, stream `Flow`" framing wrongly implies whole bodies. If we `.collect()` a body before forwarding, we **break the app being debugged** — SSE (every LLM API), gRPC streams, long-polls, and large downloads never reach the client until complete. Required design: **tee** each body — forward chunks immediately while copying into the flow record — and let the Inspector render a still-growing body. This dictates the `Flow` model, so it is a day-one decision.
- **No built-in request↔response correlation.** `HttpContext` is `#[non_exhaustive]` but currently exposes only `client_addr: SocketAddr` — no request ID, no flow ID. Under HTTP/2 multiplexing, pairing a response callback with its originating request is **our** job. The [`ideamans-hudsucker`](https://crates.io/crates/ideamans-hudsucker) fork exists specifically to add h2 request-response correlation, which signals upstream does not give it for free. Plan to assign flow IDs in `handle_request` and thread them through (interior state keyed per connection/stream), and be prepared to **vendor or fork** hudsucker if the stock API can't carry the correlation we need.

## Certificate management — one-click & transparent (first-class concern)

This is a make-or-break UX surface and a place to beat Proxyman. Hard truth: adding a trusted root CA to an OS trust store **always** requires exactly one OS-level authorization gate (macOS admin password, Windows UAC, Linux polkit). No tool can bypass this — it's a security boundary. So the target experience is: **one button → one native auth prompt → trusted, no terminal, no manual Keychain steps.** We reference [`mkcert`](https://github.com/FiloSottile/mkcert)'s proven per-platform install logic.

Implemented in `crates/nova-core/src/ca.rs` (generate/persist) + a `cert` command module + a frontend **Certificate** view:

- **Auto root CA on first launch:** generate a long-lived root CA with `rcgen`, persist to the app data dir; mint per-host leaf certs on the fly during interception (with an in-memory **leaf-cert cache** keyed by host — minting per connection is a known perf trap). User never thinks about this.
- **CA private key protection:** a trusted root CA key on disk can silently MITM the machine forever — treat it like mkcert treats its key. Restrictive file permissions (0600, app-data dir), never exported by any UI action, and a one-click **regenerate CA** (revoke = uninstall old + install new). Consider OS keychain storage for the key material where practical.
- **Live trust status:** detect and display whether NovaProxy's CA is currently installed & trusted in each relevant store, with one-click **Install** and **Uninstall**. Re-check after the auth prompt so the UI reflects reality.
- **One-click install per platform** (single elevated action, run transparently via privilege elevation — never ask the user to copy files or run commands):
  - **macOS:** `security add-trusted-cert -d -r trustRoot -k /Library/Keychains/System.keychain <ca.pem>`, elevated via `osascript … with administrator privileges` (triggers the native macOS password dialog). Also add to the **NSS** DB for Firefox.
  - **Windows:** `certutil -addstore -f ROOT <ca.pem>`, elevated via a `runas`/UAC-triggering launch.
  - **Linux:** distro-dependent — Debian/Ubuntu use `/usr/local/share/ca-certificates/` + `update-ca-certificates`; Fedora/RHEL/Arch use `/etc/pki/ca-trust/source/anchors/` (or `/etc/ca-certificates/trust-source/anchors/`) + `update-ca-trust`. Detect the family. Plus the **NSS** DB (`certutil -d sql:$HOME/.pki/nssdb -A …`) for Chrome/Firefox; elevate via `pkexec`. (Java apps use their own JKS trust store — out of scope for auto-install, document manually.)
- **Mobile / remote devices (later phase):** the classic flow — serve a cert-download + install-instructions page at a magic host (e.g. `nova.proxy`) reachable only through the proxy, plus a QR code, so iOS/Android devices can install the CA. Not in the first milestone but the CA design accommodates it.

The auth prompt is surfaced honestly ("NovaProxy needs your permission to install its certificate so HTTPS traffic can be decrypted"), so transparent = *no hidden steps and no surprise*, not *no prompt at all*.

### Key crates / libraries (proven, current)
- Core: **`hudsucker`** (intercepting MITM proxy), `rustls`, `rcgen` (CA + on-the-fly leaf certs), `tokio`, `hyper`, `serde`, `ts-rs`.
- Compression/decode: `flate2` (gzip/deflate), `brotli`, later `prost` (protobuf).
- Scripting (Phase 3): **`rquickjs`** (QuickJS bindings — fast, small) to expose a JS request/response API like Proxyman's.
- Frontend: **React 19 + TypeScript + Vite**, `zustand` (store fed by the channel), **`@tanstack/react-virtual`** (virtualized flow list for thousands of rows), **CodeMirror 6** (JSON/XML/HTML/text body viewer with folding + search), Radix UI primitives, Tailwind (or CSS modules).

## Proposed repo layout

```
novaproxy/
  src-tauri/                # Tauri backend (shell + commands + app state)
    src/{main,lib,commands,state}.rs
    tauri.conf.json
  crates/
    nova-core/              # the proxy engine (hudsucker Handler impl)
      src/{lib,ca,intercept,flow}.rs
      src/rules/            # map_local, map_remote, breakpoint, block_allow
      src/scripting/        # rquickjs sandbox (Phase 3)
    nova-proto/             # shared serde types → ts-rs generated TS
  src/                      # React/TS frontend
    stores/  components/  views/  (FlowList, Inspector, RulesEditor, Composer)
  package.json  pnpm-workspace.yaml  vite.config.ts
```

## Phased roadmap toward parity

**Phase 0 — Scaffolding**
Tauri v2 + React/TS/Vite + pnpm workspace; `nova-core` and `nova-proto` crates; `ts-rs` type generation wired into the build; basic window + dev loop; CI (cargo + vitest).

**Phase 1 — Capture & inspect (foundation)**
- `nova-core`: hudsucker-based engine; **one-click certificate management** as specified in the dedicated section above (auto root CA, live trust status, elevated per-platform install/uninstall) — this ships in Phase 1 because HTTPS interception is useless without it.
- `state.rs` / `commands.rs`: start/stop proxy, **system-proxy toggle** (macOS `networksetup`, Windows registry, Linux gsettings/env).
- Implement hudsucker `HttpHandler` + `WebSocketHandler`: assign flow IDs in `handle_request` and correlate through to `handle_response`, **tee bodies** (forward chunks while capturing — never buffer-to-complete), decode gzip/brotli on a copy, stream `Flow` objects (and incremental body chunks) to the UI over a channel.
- **Body storage & memory budget** (design now, or OOM before the 10k-flow goal): per-body **size cap** with UI truncation, **spill-to-disk** for large bodies (this also gives session save/load in Phase 3 nearly for free), and an overall retention/eviction policy. Bodies do **not** all live in RAM.
- **TLS passthrough / SSL-proxying scope** (make-or-break for trust): per-host **include/exclude lists** plus **auto-fallback** — a host whose client aborts the TLS handshake (pinning, mTLS) is tunneled **without** decryption rather than hard-failing. Without this, opening the proxy breaks banking apps, Apple push/update services, and any pinned app — the #1 reason users uninstall a proxy tool. Surface a **connection-failures list** with one-click "add to passthrough."
- **Client certificates (mTLS):** allow presenting a user-supplied client cert to servers that require one; until configured, such hosts route to passthrough instead of failing. (Parity item — Proxyman supports this.)
- **System-proxy safety:** persist the machine's **pre-existing** proxy state (including corporate PAC/auto-config) before touching it, restore it on clean exit, and **restore-on-next-launch** after a crash/force-quit so the user is never left with no working internet. Round-trip existing settings; never clobber.
- Frontend: virtualized **flow list** (method/status/host/path/size/time), filters + instant search, record/pause/clear, **Inspector** tabs (Headers / Body / Preview / Hex / Timing) with CodeMirror + image render + hex view. Timing tab requires **explicitly instrumenting** DNS/connect/TLS/TTFB timestamps in `nova-core` — hudsucker does not surface these.

**Phase 2 — Traffic control**
Map Local, Map Remote, Block/Allow lists, **Breakpoints** (pause flow, edit req/resp in UI via a oneshot channel, resume), **Compose/Replay** editor. Rules live in `crates/nova-core/src/rules/` with a matcher (host/path/method/content-type).
- **Upstream proxy chaining + bypass list:** forward to an existing upstream proxy (with auth) so NovaProxy works behind corporate proxies — otherwise it's unusable in exactly the enterprise environments the parked enterprise-CA section targets. Plus a **host bypass list** (never intercept `localhost`, internal domains, etc.).
- **"Launch app through NovaProxy" helper:** spawn a target app with `HTTP_PROXY`/`HTTPS_PROXY`/`ALL_PROXY` + `NODE_EXTRA_CA_CERTS=<ca.pem>` (and optionally disable QUIC) injected into its environment. Critical for Electron/multi-layer apps — see the capture-coverage section below.

**Phase 3 — Advanced**
JS **Scripting** sandbox (`rquickjs`) exposing request/response manipulation; **Reverse proxy**; **Network conditions** (throttle/latency/bandwidth); WebSocket message inspection; **raw TCP/UDP flow view** (hex/byte stream for non-HTTP traffic through the proxy); protobuf decode (`prost`); session save/load (`.nova` files); export (HAR, `curl`).

**Phase 3.5 — Transparent / local capture (differentiator vs Proxyman)**
Capture traffic from apps that ignore the system proxy — Proxyman's weak spot. Reference mitmproxy's proven approaches: **local redirect mode** (OS network extension intercepting by app name/PID), **WireGuard mode**, and classic **transparent mode** (pf/iptables/WFP routing or a TUN interface). This is the achievable, high-value alternative to a full packet-capture tool (see evaluation below).
- **Distribution risk (macOS):** local-redirect via a macOS **Network/System Extension** requires an Apple-granted entitlement and a signed, notarized build — a real hurdle for an open-source project where users build from source. The **pf-based transparent mode** is the fallback that needs no special entitlement. Lead with WireGuard/pf; treat the Network Extension as the premium signed-build path.
- **LAN exposure:** binding the proxy to `0.0.0.0` (for mobile/remote capture) turns NovaProxy into an **open proxy** on the local network. Default to loopback; require an explicit **allow-LAN** toggle and offer client allowlisting when enabled.

**Phase 4 — Polish & differentiation (beat Proxyman on UX)**
Command palette (keyboard-first), request/response **diff view**, theming/dark mode, multi-device capture, performance hardening (10k+ flows), packaging + code-signing/notarization, auto-update.

## Revised roadmap (agreed 2026-07-28)

The phased roadmap above is the original plan and stays as the feature map. This section supersedes it for **sequencing**, and reflects what is actually built.

### Already done — do not re-plan

Phases 0–2 are complete, and much of Phase 3 landed and was verified: the **rules engine** (`rules.rs` — MapRemote / MapLocal / Block / Rewrite, persisted, live-editable), the **QuickJS scripting sandbox** (`scripting.rs` — `onRequest`/`onResponse`), **breakpoints**, **network conditions**, **WebSocket message inspection**, **TLS passthrough scope** (manual per-host), **session save/load + HAR export**, **resend/replay**, and **per-app attribution** (`procinfo.rs`). Rules and scripting in particular are finished — they are not outstanding work.

Items 1–3 of the ordered plan below have since landed as well:

- **CA install defaults to the user trust domain** (`trust.rs`). `security add-trusted-cert` without `-d`, targeting `~/Library/Keychains/login.keychain-db` — no administrator password. `trust_state()` replaced the `is_trusted` bool with a per-domain query, `CaStatus` carries `trusted_user` / `trusted_system` / `trusted` (any), and the Certificate view names the domain and offers "install for all users" as an explicit opt-in. Uninstall is driven by *presence* in each keychain, so machines that already had the system-domain cert are cleaned up — that is the migration path. **Still unanswered:** whether the user-domain dialog offers Touch ID or only a password (it is a privilege reduction either way).
- **Body spill-to-disk + retention/eviction** (`bodystore.rs`). Bodies keep a 512 KiB inline preview (RAM + IPC) and stream the remainder to `<data-dir>/bodies/<flow>-<side>.bin` as they pass, bounded by a per-body cap (64 MiB), a total disk budget (1 GiB, FIFO eviction) and a 10 000-flow retention window that deletes an evicted flow's spill files. The Inspector pulls a full body on demand via the `read_body` command; the frontend store honours the same window. Spill files hold the bytes as they were on the wire, so decoding happens on read. Related fix: a body truncated at the preview cap now keeps whatever inflated before the stream ran out instead of falling back to raw compressed bytes.
- **Real timing instrumentation** (`timing.rs`). The placeholder bars are gone. `nova-core` builds hudsucker's upstream connector itself — a timed resolver inside a timed plaintext connector inside a timed HTTPS connector — so DNS / TCP / TLS are measured per connection and claimed by the flow that opened it; the handler measures request-body streaming, TTFB and download. `Flow.timings` is all-`Option`: a phase that did not happen (no lookup for an IP literal, no handshake on plain HTTP) or cannot be attributed (a pooled connection, flagged `connection_reused`) is absent rather than invented, and the UI says so. HAR export now carries the real phases too. Note: the connector deliberately does **not** advertise HTTP/2 upstream — enabling h2 is a separate decision, not a side effect of this work.

- **MCP server + MCP-only filtering** (`src-tauri/src/mcp.rs`, `nova-core/src/mcp.rs`). An MCP server runs inside the Tauri backend on the [`rmcp`](https://crates.io/crates/rmcp) 3.0 SDK, served over **streamable HTTP on loopback** — a GUI app has no stdin to hand an agent, so the stdio transport is not usable here; `claude mcp add --transport http novaproxy http://127.0.0.1:9091/` is the wiring. Off by default (it exposes every captured request), toggled in Settings, and the choice is remembered. Nine tools: `list_flows`, `search_flows`, `get_flow`, `get_body`, `replay_request`, `list_rules`, `set_rule`, `delete_rule`, `proxy_status`. Every response caps bodies (2 KB in `get_flow`, up to 200 KB from `get_body`, which reads the spilled bytes from the body store) so a single payload cannot eat an agent's context window.
  - **MCP traffic is detected and isolatable.** `nova-core/src/mcp.rs` recognises MCP exchanges from the JSON-RPC 2.0 envelope *plus* a known MCP method (`initialize`, `ping`, `tools/*`, `resources/*`, `prompts/*`, `notifications/*`, `completion/*`, `logging/*`, `roots/*`, `sampling/*`, `elicitation/*`) over streamable HTTP and inside SSE `data:` frames. Requiring the method is what keeps the filter meaningful — an Ethereum or LSP-over-HTTP call is JSON-RPC too and is deliberately not tagged. Tagged flows carry `Flow.mcp` (method, tool, JSON-RPC id, transport), the list row reads `tools/call → read_file` instead of `POST /mcp`, and both surfaces filter on it: an "MCP only" toolbar toggle plus a `mcp:`/`mcp:<term>` search prefix in the UI, and `mcp_only` on the MCP listing tools.
  - **NovaProxy's own traffic is separated.** Calls to its own MCP endpoint (matched on loopback host + port) and replays its MCP server issues (an internal marker header, stripped before forwarding) set `Flow.internal`. Those flows are hidden from the flow list and from tool results unless `include_internal` / the list's "n own" toggle asks for them — otherwise an agent reading traffic mostly sees the echo of its own tool calls.
  - **Honest limit, documented in the server's own instructions:** stdio-transport MCP servers never touch the network, so no proxy can capture them. Only HTTP/SSE MCP traffic is visible.
- **MCP-only filtering (ships with this item, agreed 2026-07-29).** Debugging MCP is the point, so MCP traffic has to be isolatable rather than buried in everything else:
    - **Detect and tag it.** Recognise MCP exchanges from the JSON-RPC 2.0 envelope plus MCP method names (`initialize`, `tools/list`, `tools/call`, `resources/*`, `prompts/*`, `notifications/*`) over the HTTP transports we can see — streamable HTTP and SSE. Tag the flow (`Flow.mcp` / a `protocol` marker) and surface the method + tool name as the display summary, so the list reads `tools/call → search_flows` instead of `POST /mcp`.
    - **Filter both surfaces.** A one-click **MCP only** filter/view in the UI (alongside the existing app filter), and the same predicate as a parameter on `list_flows`/`search_flows` so an agent can ask for MCP traffic alone.
    - **Separate our own tool traffic.** Flows produced by NovaProxy's own MCP server and its `replay_request` calls must be distinguishable (and excludable) — otherwise an agent watching traffic sees mostly itself, and its own replays pollute the very list it is reading.
    - **Honest limit to document:** stdio-transport MCP servers never touch the network, so a proxy cannot see them. Only HTTP/SSE MCP traffic is capturable — the launch-through-NovaProxy helper is the answer for locally-spawned HTTP servers, and stdio needs the Phase 3.5 local-capture work or nothing.
  - Supporting refactor: the retained-flow store moved out of the engine into `nova-core/src/flowstore.rs`, owned by `AppState`. The UI, the commands and the MCP server now read one truth, and captured flows survive stopping and restarting the proxy. The UI's Clear now clears the engine store too (`clear_flows`), so an agent cannot read flows the user believes they discarded.

- **Privileged helper + Settings › General** (2026-07-29). The recurring macOS password prompt is gone: a root LaunchDaemon (`crates/nova-helper`, protocol in `crates/nova-os/src/helper.rs`) applies system-proxy changes after a single install prompt, and crash recovery no longer raises a dialog during launch. Settings became tabbed, with a **General** tab holding the flow-list default grouping, the "system proxy at launch" default (`none`, so the OS is left alone unless asked), and the helper's install/remove card; both preferences live in `src/prefs.ts`. The flow list and the Inspector are now separated by a draggable, keyboard-resizable splitter whose width persists. Full detail: *Eliminating the repeated password prompt* below.
  - Supporting refactor: `oscmd.rs` and `sysproxy.rs` moved out of `nova-core` into a new lean **`nova-os`** crate, because the root daemon links them and must not link the MITM engine, its TLS stack or its JS runtime. `nova-core` re-exports both, so `nova_core::sysproxy::…` still resolves.

- **Cross-platform: Windows + Linux trust stores and system proxy** (`nova-core/src/{oscmd,trust,sysproxy}.rs` — `oscmd`/`sysproxy` now live in `nova-os`, re-exported). Both OS-integration surfaces now have real implementations on all three platforms, and the least-privilege default carries over: the CA installs into the **user domain** everywhere.
  - **CA trust.** Windows: `certutil -user -addstore -f ROOT` for the per-user store (no prompt at all), `certutil -addstore` behind one UAC prompt for the machine store, uninstall by SHA-1 thumbprint so a cert from an earlier CA generation is never deleted by mistake. Linux: the user domain is the **NSS databases** — Chrome/Chromium's `~/.pki/nssdb` plus every launched Firefox profile (including Snap and Flatpak locations) — and the system domain copies an anchor into the detected family's directory (Debian/Ubuntu, Fedora/RHEL, Arch) and runs `update-ca-certificates`/`update-ca-trust` behind one `pkexec` prompt. `CaId` carries both SHA-256 and SHA-1 digests because the platforms name certificates differently; every presence check matches our *exact* certificate.
  - **Honest platform difference, surfaced in the UI:** Linux has no per-user OpenSSL trust store, so a user-domain install covers browsers but not `curl`/Python/Go. `CaStatus.platform` drives that wording instead of the macOS phrasing being reused everywhere.
  - **System proxy.** Windows writes `HKCU\…\Internet Settings` via `reg.exe` (`ProxyServer`, `ProxyEnable`, `ProxyOverride`), and clears an existing `AutoConfigURL` while active because a PAC URL outranks manual settings in WinINET — restoring it on disable. Linux uses `gsettings org.gnome.system.proxy`, setting hosts before switching `mode` to `manual` so traffic is never pointed at an unset host. **Neither needs elevation** (both are per-user), which makes macOS the only platform where the recurring password prompt exists at all.
  - **The snapshot/restore contract is honoured on every platform**, including restoring a corporate PAC setup rather than a blanket "off", and *removing* values that did not exist before. `Backup` grew per-platform fields while keeping macOS's `services` at the top level, so a snapshot written by an older build still deserializes and a mid-session upgrade cannot orphan a pending restore.
  - **How this is verified without those machines:** all platform behaviour is expressed as pure command *plans* (`oscmd::Plan`), so every Windows and Linux command line, elevation choice and restore decision is asserted by unit tests that run here on macOS — 60+ of them. Only `Plan::run` is platform-gated. What remains untested is execution against real Windows/Linux hosts, which needs those machines.

### Ordered plan

1. **UI/UX pass.** Includes splitting `App.tsx` (currently ~1,300 lines holding the whole frontend) into views — every later UI change gets cheaper afterward. Plus the Phase 4 items: diff view, theming, 10k-flow list performance.
2. **Packaging + a signing guide + license.** Build the distributable bundles per platform and **write the signing/notarization guide the user follows themselves** — Developer ID signing, notarization, Windows Authenticode. Per the user's call (2026-07-29) implementation does *not* wait on signing and we do not automate it; the deliverable here is working bundles plus copy-paste steps. Also **pick the license** (still open: MIT/Apache-2.0 for adoption vs copyleft to keep forks open) — an open-source release cannot happen without it.
3. ~~**Privileged helper (removes the recurring password prompt).**~~ **Done, 2026-07-29, and it did not need item 2.** The blocker recorded here was `SMAppService`, which does require a signed bundle — but a plain **LaunchDaemon** installed behind one `osascript` prompt does not, and that is what shipped. Signing is still what closes the caller-verification gap; see *Eliminating the repeated password prompt* below for what was built, what it costs, and the signed end state.
4. **Roll out.**

**Sequencing note (superseded).** This said the helper was gated on a signed bundle and belonged after item 2. That was true of the `SMAppService` design and false of the mechanism generally — the LaunchDaemon variant works from an unsigned local build, so item 3 landed first. Item 2 still gates *distribution* and the caller code-signing check.

**Scope note.** The certificate prompt is a once-per-machine cost; the prompt that recurred on every app start was the **system-proxy toggle**, which needs root regardless of which keychain the CA lives in. The user-domain CA install did not fix that — the privileged helper (item 3) did — and note that Windows and Linux never had this problem, since their system-proxy settings are per-user.

## Cross-platform — a separate workstream, tracked from now

**Status: built (2026-07-29).** Both OS-integration surfaces — `trust.rs` and `sysproxy.rs` — now have real Windows and Linux implementations; the engine itself (`nova-core`, hudsucker/rustls/tokio) was already portable. What has *not* happened is execution on real Windows and Linux hosts: the logic is unit-tested from macOS via pure command plans (`oscmd.rs`), so the remaining risk is environmental (tool availability, distro quirks, UAC behaviour), not structural. Run the e2e walkthrough on each platform before claiming support publicly.

The table below is what was built.

**What has to be built, per platform:**

| Surface | Windows | Linux |
|---|---|---|
| CA trust (`trust.rs`) | `certutil -addstore -f ROOT <ca.pem>`, elevated via a UAC-triggering `runas` launch | Debian/Ubuntu: `/usr/local/share/ca-certificates/` + `update-ca-certificates`; Fedora/RHEL/Arch: `/etc/pki/ca-trust/source/anchors/` (or `/etc/ca-certificates/trust-source/anchors/`) + `update-ca-trust`. Detect the family; elevate via `pkexec` |
| System proxy (`sysproxy.rs`) | Registry (`Internet Settings`), with the same snapshot-and-restore safety contract as the macOS path | `gsettings` for GNOME plus the `HTTP(S)_PROXY` environment convention; no single universal mechanism — document what is covered |
| Browser trust | **NSS DB** for Firefox (`certutil -d sql:$HOME/.pki/nssdb -A …`) | Same NSS DB, also used by Chrome on Linux |

**Cross-cutting requirements.** The snapshot / restore-on-clean-exit / restore-on-next-launch-after-crash contract that `sysproxy.rs` implements for macOS is not macOS-specific — every platform variant must honour it, or a crash leaves the user with no working internet. The per-domain trust model already built for macOS is macOS-shaped; Windows and Linux have their own user-vs-machine store distinction (`certutil -user` vs machine store; per-user NSS DB vs system anchors) and should follow the same least-privilege default where the OS allows it.

**Positioning consequence.** The variants exist, so the launch can lead with cross-platform — but only once each has been exercised on a real machine. Until then the README should say "macOS tested; Windows and Linux implemented, testing wanted" rather than a flat claim.

**Neither variant needs code signing** to build, run or test — UAC elevation on Windows and `pkexec` on Linux work from an unsigned local build, exactly like `osascript` does on macOS. Signing only ever gates distribution and the macOS privileged helper.

## Verification

- **Core unit tests:** `cargo test` in `nova-core` — CA/leaf cert generation, rule matching (map local/remote, block/allow), gzip/brotli decode, breakpoint resume logic (against synthetic flows), MCP detection, body spill/retention, timing phase splitting. `nova-os` covers command plans, per-platform system-proxy behaviour and the helper's validation rules.
- **Privileged helper end-to-end (automated, no root):** `crates/nova-os/tests/helper_ipc.rs` runs the real daemon loop on a relocated socket and asserts what it refuses.
- **Engine end-to-end (automated):** `crates/nova-core/tests/` drives real traffic through a real proxy over real sockets — capture, traffic control, breakpoints, WebSockets, TLS scope, timing + body spill (`timing_and_spill.rs`), MCP tagging (`mcp_capture.rs`).
- **MCP endpoint end-to-end (automated):** `src-tauri/tests/mcp_endpoint.rs` starts the real server and speaks JSON-RPC to it: handshake, `tools/list`, every filter, rule round-trip, and the error cases.
- **End-to-end (manual, per phase):** launch the app → run cert-install wizard → toggle system proxy → drive traffic with `curl -x http://127.0.0.1:<port>` and a real browser → confirm flows stream into the list and the Inspector renders bodies correctly. For HTTPS, confirm the installed CA makes TLS interception work without warnings.
- **Frontend:** `vitest` for store/reducer logic; optional WebdriverIO/Playwright smoke test that the flow list virtualizes and the Inspector tabs render.
- **Phase gate:** each phase ends with the e2e walkthrough above plus its new feature exercised against live traffic.

## Open considerations (decide as we build)
- **HTTP/3 / QUIC** is not yet first-class in hudsucker — treat as a later add (Phase 3+), HTTP/1.1 + HTTP/2 first.
- Cert trust-store install is the most platform-specific surface — macOS landed first, as expected. Now tracked in *Cross-platform — a separate workstream* above.
- **License: still undecided, and now a rollout blocker** (item 2 of the revised roadmap — the original "pick before first commit" deadline has passed). For an explicitly open-source project meant to out-compete a closed tool, decide MIT/Apache-2.0 (permissive, max adoption) vs. a copyleft/GPL stance (keeps forks open). Also check that all key crates' licenses are compatible with the choice.

## Capture coverage — multi-layer desktop apps (e.g. Electron)

A single Electron app is **3+ independent network stacks**, and they do not all behave alike. Designing for this is a core differentiator — relying on the system proxy alone silently misses two of the three layers.

| Layer | Stack | Respects system proxy | Trusts OS-installed CA | How NovaProxy catches it |
|---|---|---|---|---|
| Renderer (web `fetch`/XHR, **WebSocket**, **GraphQL**) | Chromium net stack | Yes | Yes (OS trust store) | **System proxy** (Phase 1) |
| Main process (Node.js: `http`/`https`/`undici`/`ws`) | Node networking | **No** | **No** (Node's own CA bundle) | **Env injection** via the launch helper: `HTTP(S)_PROXY` + `NODE_EXTRA_CA_CERTS` |
| Bundled native `.exe` / child process | its own | usually no | usually no | **Transparent / local-redirect capture** (Phase 3.5) |

**Coverage strategy:** (1) system proxy for the renderer, (2) the **launch-through-NovaProxy helper** (Phase 2) to route + trust-CA the Node main process and proxy-aware libs, (3) **local-redirect capture** (Phase 3.5) as the guaranteed catch-all for everything else. Together these capture renderer + main-process JS + local exe under one app.

**Honest limits (universal — Proxyman has them too):** certificate **pinning** defeats TLS decryption regardless of routing; **HTTP/3/QUIC** and **DoH** can bypass unless disabled or caught at the transparent layer.

**vs Proxyman:** Proxyman is a **system-proxy + MITM-CA** tool — `networksetup` system proxy + a self-signed CA trusted in the System Keychain, and for Node it opens a pre-configured terminal exporting `HTTP(S)_PROXY` + `NODE_EXTRA_CA_CERTS`. It does **not** do transparent/local-redirect capture on macOS, so it shares the bundled-exe gap above. NovaProxy matches layers 1–2 the same way and **exceeds** Proxyman with Phase 3.5 local-redirect capture (the mitmproxy technique) for layer 3.

## Evaluation — Wireshark-style packet capture (recommendation: don't clone, do interoperate)

Wireshark captures at the **link/network layer** (libpcap/npcap, all protocols, root privilege) but TLS payloads stay encrypted; NovaProxy's proxy MITM captures at the **application layer** with decrypted HTTP semantics but only for traffic routed through it. **Complementary, not competing.**

- **Do NOT build a Wireshark clone** (full libpcap capture + multi-protocol dissectors). It's a different, decades-deep product that is already free/open-source — unwinnable on its turf and a distraction from "best HTTP debugger." Possible in Rust (`pcap`, `pnet`, `etherparse`) but very low ROI for this goal.
- **Better alternative = transparent/local capture** (Phase 3.5 above): solves the real Proxyman gap (apps that bypass the system proxy) while keeping decrypted HTTP semantics. This is the recommended investment.
- **Interoperate, cheaply:** **import `.pcap`** files and support **`SSLKEYLOGFILE`** so users can bring Wireshark/tcpdump captures into NovaProxy's UI and decrypt TLS — makes the two tools a workflow rather than rivals.
- **Parked, optional, much later:** a low-level packet/pcap viewer as a separate "Network" module for power users. Flagged as probably-not-worth-it; revisit only if there's clear demand.

## Eliminating the repeated password prompt (privileged helper) — **built 2026-07-29**

> Built, and **not** the way this section previously planned it. The plan assumed `SMAppService`, which needs a signed bundle; a plain LaunchDaemon does not, so the helper shipped without waiting on signing. The old design is preserved below the line as the target end state once the app *is* signed.

**The problem.** Every privileged action shelled out to `osascript … with administrator privileges` — `trust.rs` (CA install/uninstall) and `sysproxy.rs` (system-proxy enable/disable). That dialog is **password-only by Apple's design**: it never offers Touch ID, and macOS provides no way to remember the grant. So the user re-typed their password on *every* proxy toggle. The worst instance was crash recovery: a leftover `sysproxy_backup.json` made the app restore the OS proxy **during launch**, raising a password dialog before any window existed — the bug that prompted this work.

**What was built** (`crates/nova-os/src/helper.rs`, `crates/nova-helper/`):

- A small root daemon, `nova-helper`, run by launchd from `/Library/LaunchDaemons/dev.novaproxy.helper.plist`. Installing it costs **one** admin prompt (`install(1)` + `launchctl bootstrap`, batched into a single `osascript`); afterwards every enable / disable / crash-restore is silent, across app restarts and reboots.
- The app talks to it over a **unix socket** (`/var/run/novaproxy-helper.sock`) with a line-delimited JSON protocol of exactly three operations — `Ping`, `Enable{host,port,backup}`, `Disable{backup}`. There is no "run this command" endpoint.
- `sysproxy::enable`/`disable` prefer the helper and fall back to the prompting plan when none is installed, so the feature works before setup and degrades honestly after removal.
- **Crash recovery no longer prompts.** With a helper: restore silently at launch. Without: set `ProxyStatus.pending_restore` and let the UI offer a **Restore settings** button. The app never raises the admin dialog on its own.
- **Correctness fix found alongside it:** re-enabling the system proxy while a backup file existed used to snapshot the *already-proxied* state as the thing to return to, stranding the user's settings permanently. The existing backup is now reused instead.
- Settings › General installs, reinstalls (on a protocol-version mismatch) and removes it.

**Trust boundary — what replaces XPC's code-signing check.** The daemon is root and treats its input as hostile: the socket is owned by the installing uid at mode `0600` *and* every connection re-checks the peer with `getpeereid`; hosts must be loopback literals; ports must be non-zero; service names must match services the machine actually has; commands run as argv, never through a shell.

**The accepted gap, stated plainly:** a unix socket cannot verify the *caller's code signature* the way `xpc_connection_set_peer_code_signing_requirement` can, so any process running as the same user can ask the helper to point the system proxy at a loopback port. That is a real new capability for same-user malware — bounded to loopback proxy redirection (never CA installs, never arbitrary commands), but real. Closing it needs the signed-bundle path below: take the peer pid via `LOCAL_PEERPID` and check it against a designated requirement with `SecCodeCheckValidity`. Until then, this is a considered trade: strictly narrower than the sudoers alternative, strictly wider than XPC-with-signing.

**Testing.** `NOVAPROXY_HELPER_SOCKET` relocates the socket **in debug builds only**, so `crates/nova-os/tests/helper_ipc.rs` runs the real server loop, peer check and client as an ordinary user — asserting that pings work and that malformed JSON, unknown operations, non-loopback hosts and unknown network services are all refused without the daemon dying. Every request in that test is one the helper must reject; a test that reached `networksetup` would reconfigure the machine running it.

**Packaging note.** In a dev tree the app finds the helper next to its own executable (`cargo build -p nova-helper`). Bundling it is *not* wired into `tauri.conf.json`: Tauri validates `bundle.resources` paths at compile time, so a committed entry breaks `cargo build`/`cargo test` on a clean checkout. To package: build `cargo build --release -p nova-helper`, then add `"resources": { "../target/release/nova-helper": "nova-helper" }` before `cargo tauri build` — `source_binary()` already looks in `Contents/Resources/`.

---

**Target end state once the app is signed (the original plan).**

- Register the daemon with **`SMAppService.daemon`** (macOS 13+; `SMJobBless` for older) instead of a hand-installed plist.
- Verify the caller's code-signing requirement on every connection, closing the gap described above.
- Keep the narrow, typed API — that part of the design survives unchanged.

**Alternatives considered and rejected as the primary path:**
- *sudoers drop-in* (`/etc/sudoers.d/novaproxy` with `NOPASSWD` for `networksetup`/`security`): ~30 lines, one password ever, works today with no bundle. But it lets **any** process running as the user silently change system proxies and install trusted roots. Acceptable as a personal-machine escape hatch; not acceptable for distributed software.
- *Touch ID for sudo* (`pam_tid` in `/etc/pam.d/sudo_local`): only affects `sudo`, not the osascript admin dialog, and GUI-context Touch ID prompts are unreliable. Not a solution on its own.

**Cheaper partial win — user trust domain instead of System keychain. (Built — this is now the default.)** `trust.rs` used to install only to `/Library/Keychains/System.keychain` with `-d` (admin domain), which *requires* admin rights. Installing to the **login keychain in the user trust domain** (`security add-trusted-cert -k ~/Library/Keychains/login.keychain-db`, no `-d`) needs **no admin password** — it raises the ordinary keychain-authorization dialog instead, which on Touch ID Macs can be satisfied by fingerprint. SecTrust evaluates the user domain alongside admin and system, so Safari/Chrome/curl still honor it; the loss is trust for other users on the machine and for root-owned daemons. This removes the *cert* prompt cheaply but does nothing for the *system-proxy* prompt, which still needs the helper. `trust_state()` now queries both domains (`dump-trust-settings` for user, `-d` for admin) and `CaStatus` reports them separately; the remaining unknown is whether the user-domain dialog offers Touch ID.

## Future / parked — Enterprise certificate deployment (not in current scope)

> Confirmed possible; revisit later. This is the fleet-wide deployment story for installing NovaProxy's CA across an organization **without** per-user prompts. Captured here so the early CA design doesn't paint us into a corner — decisions on tooling/CA strategy are deferred.

The core enabler is supporting **one shared organizational CA** (IT-provided) instead of always auto-generating a per-machine CA, so the whole fleet trusts a single root that IT controls and can revoke centrally. Likely scope when we pick this up:
- **Import/use an external CA:** load an org-provided root CA + key (PEM/PKCS#12) and mint leaf certs from it, as an alternative to the auto-generated per-machine CA. Optionally an org-root → per-machine-intermediate model for stronger isolation/revocation.
- **Export in every deployment format:** PEM, DER/`.crt`, and a signed Apple **`.mobileconfig`** configuration profile.
- **Managed deployment paths** (write a per-tool admin manual): Microsoft **Intune**, **Jamf/Kandji** (Apple MDM via `.mobileconfig`), Active Directory **GPO** (Trusted Root store), and **Ansible/Puppet/Chef**/scripted install for Linux fleets/servers.
- **Admin guide doc** shipped in-repo with copy-paste steps per tool.
- *Open questions to answer before starting:* which fleet tooling the target orgs actually use, and which CA strategy (shared org CA vs org-signed per-machine intermediates).