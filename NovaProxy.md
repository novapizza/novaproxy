# NovaProxy — an open-source Proxyman alternative

## Context

Proxyman is a best-in-class HTTP/HTTPS debugging proxy, but it is **closed-source** and strongest only on macOS. The goal is to build **NovaProxy**: an open-source, cross-platform debugging proxy that matches Proxyman's feature set and beats it on UI/UX.

Decisions locked in with the user:
- **Proxy engine:** native Rust core (mitmproxy used as *architectural reference only*, not embedded). Single static binary, full control, top performance.
- **Platform:** cross-platform via **Tauri v2** (Rust shell + web frontend) → macOS, Windows, Linux from one codebase. Cross-platform is the differentiation angle vs Proxyman's macOS-centric strength.
- **Stack:** Rust + TypeScript/React (matches the user's background, ideal for Tauri).
- **Target:** Proxyman feature **parity** as the north star. Because a from-scratch native core can't land all features at once, parity is delivered through the **phased roadmap** below.

This is a greenfield project — the working directory `/Users/nhunglc/novaproxy` is empty. Everything below is *new* code.

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

## Verification

- **Core unit tests:** `cargo test` in `nova-core` — CA/leaf cert generation, rule matching (map local/remote, block/allow), gzip/brotli decode, breakpoint resume logic (against synthetic flows).
- **End-to-end (manual, per phase):** launch the app → run cert-install wizard → toggle system proxy → drive traffic with `curl -x http://127.0.0.1:<port>` and a real browser → confirm flows stream into the list and the Inspector renders bodies correctly. For HTTPS, confirm the installed CA makes TLS interception work without warnings.
- **Frontend:** `vitest` for store/reducer logic; optional WebdriverIO/Playwright smoke test that the flow list virtualizes and the Inspector tabs render.
- **Phase gate:** each phase ends with the e2e walkthrough above plus its new feature exercised against live traffic.

## Open considerations (decide as we build)
- **HTTP/3 / QUIC** is not yet first-class in hudsucker — treat as a later add (Phase 3+), HTTP/1.1 + HTTP/2 first.
- Cert trust-store install is the most platform-specific surface — budget extra time for Windows/Linux variants; macOS is the simplest to land first.
- **License:** pick before first commit. For an explicitly open-source project meant to out-compete a closed tool, decide MIT/Apache-2.0 (permissive, max adoption) vs. a copyleft/GPL stance (keeps forks open). Also check that all key crates' licenses are compatible with the choice.

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

## Future / parked — Enterprise certificate deployment (not in current scope)

> Confirmed possible; revisit later. This is the fleet-wide deployment story for installing NovaProxy's CA across an organization **without** per-user prompts. Captured here so the early CA design doesn't paint us into a corner — decisions on tooling/CA strategy are deferred.

The core enabler is supporting **one shared organizational CA** (IT-provided) instead of always auto-generating a per-machine CA, so the whole fleet trusts a single root that IT controls and can revoke centrally. Likely scope when we pick this up:
- **Import/use an external CA:** load an org-provided root CA + key (PEM/PKCS#12) and mint leaf certs from it, as an alternative to the auto-generated per-machine CA. Optionally an org-root → per-machine-intermediate model for stronger isolation/revocation.
- **Export in every deployment format:** PEM, DER/`.crt`, and a signed Apple **`.mobileconfig`** configuration profile.
- **Managed deployment paths** (write a per-tool admin manual): Microsoft **Intune**, **Jamf/Kandji** (Apple MDM via `.mobileconfig`), Active Directory **GPO** (Trusted Root store), and **Ansible/Puppet/Chef**/scripted install for Linux fleets/servers.
- **Admin guide doc** shipped in-repo with copy-paste steps per tool.
- *Open questions to answer before starting:* which fleet tooling the target orgs actually use, and which CA strategy (shared org CA vs org-signed per-machine intermediates).