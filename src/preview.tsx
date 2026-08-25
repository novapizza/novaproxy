/* Throwaway visual harness: renders the Flows section with mock flows so the
   layout can be screenshotted without a Tauri runtime. Not shipped. */
import { createRoot } from "react-dom/client";
import { useEffect, useState } from "react";
import "@fontsource-variable/inter";
import "@fontsource-variable/jetbrains-mono";
import "@fontsource-variable/petrona";
import "./styles.css";
import type { Flow } from "./api";
import { FlowsSection } from "./flows/FlowsSection";
import { DEFAULT_COLUMNS, normalizeColumns, type ColumnId } from "./flows/columns";
import { EMPTY_FILTER, type FlowFilter, type SavedFilter } from "./filter";
import { DEFAULT_PREFS } from "./prefs";
import { KeyValueTable } from "./inspector/KeyValueTable";
import { TreeviewPanel } from "./inspector/TreeviewPanel";
import { TimingPanel } from "./inspector/TimingPanel";
import { BodyPanel, bodyToText } from "./inspector/BodyPanel";
import { parseCookies, parseQuery, rawHttp, summaryOf } from "./inspector/parts";
import { ShortcutsDialog } from "./ShortcutsDialog";
import { useStore } from "./store";

const now = Date.now();
let n = 148;
function mk(over: Partial<Flow>): Flow {
  n += 3;
  return {
    id: `f${n}`, seq: BigInt(n), method: "GET", scheme: "https",
    host: "api.creativeforce.io", path: "/v3/x", url: "https://api.creativeforce.io/v3/x",
    client_addr: "127.0.0.1:5432", pid: 4821, process: "Google Chrome",
    http_version: "HTTP/1.1", state: "Completed", status: 200,
    request_headers: [
      { name: "Host", value: "api.creativeforce.io" },
      { name: "accept", value: "application/json" },
      { name: "cookie", value: "sid=abc123; theme=dark" },
      { name: "user-agent", value: "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)" },
    ],
    response_headers: [
      { name: "content-type", value: "application/json" },
      { name: "cache-control", value: "no-store" },
      { name: "set-cookie", value: "sid=abc123; Path=/; Secure" },
    ],
    request_body: null,
    response_body: { size: 74n, truncated: false, media_type: "application/json", decoded_from: "gzip", text: '{\n  "workspace": "default",\n  "shoot": 4821,\n  "samples": [1,2,3]\n}', base64: null, spilled: false } as Flow["response_body"],
    request_size: 0n, response_size: 1240n, content_type: "application/json",
    started_at: now - 4000, duration_ms: 118,
    timings: { dns_ms: 9, connect_ms: 16, tls_ms: 26, connection_reused: false, request_ms: null, ttfb_ms: 44, download_ms: 14 },
    error: null, resent: false, mapped_from: null, is_websocket: false,
    tunneled: false, mcp: null, internal: false,
    edits: { rule: false, script: false, breakpoint: false },
    ...over,
  } as Flow;
}

const FLOWS: Flow[] = [
  mk({ path: "/v3/workspaces/current", url: "https://api.creativeforce.io/v3/workspaces/current" }),
  mk({ method: "POST", path: "/v3/shoots/4821/tasks:assign", url: "https://api.creativeforce.io/v3/shoots/4821/tasks:assign", duration_ms: 264, response_size: 812n }),
  mk({ path: "/v3/assets/9f3c1/derivatives", url: "https://api.creativeforce.io/v3/assets/9f3c1/derivatives", status: 401, duration_ms: 92, response_size: 184n }),
  mk({ method: "POST", path: "/mcp/v1/tools/call", url: "https://api.creativeforce.io/mcp/v1/tools/call", duration_ms: 412, response_size: 6400n, mcp: { method: "tools/call", tool: "read_file", id: "7", transport: "Http" } as Flow["mcp"] }),
  mk({ edits: { rule: true, script: false, breakpoint: false }, mapped_from: "api.creativeforce.io", method: "PUT", path: "/v3/reviews/1188/decision", url: "https://api.creativeforce.io/v3/reviews/1188/decision", status: 422, duration_ms: 205, response_size: 402n }),
  mk({ host: "github.com", process: "git", path: "/novapizza/novaproxy.git/info/refs", url: "https://github.com/novapizza/novaproxy.git/info/refs", status: 500, duration_ms: 512, response_size: 228n, content_type: "text/plain" }),
  mk({ host: "github.com", process: "git", method: "POST", path: "/novapizza/novaproxy.git/git-upload-pack", url: "https://github.com/novapizza/novaproxy.git/git-upload-pack", duration_ms: 325, response_size: 24800n, content_type: "application/x-git-upload-pack-result" }),
  mk({ host: "cdn.example.com", process: null, path: "/img/hero.png", url: "https://cdn.example.com/img/hero.png", content_type: "image/png", response_size: 184320n, duration_ms: 47 }),
  mk({ edits: { rule: true, script: true, breakpoint: true }, host: "browser-intake.datadoghq.com", method: "POST", path: "/api/v2/rum?ddsource=browser", url: "https://browser-intake.datadoghq.com/api/v2/rum?ddsource=browser", status: 202, duration_ms: 528, response_size: 0n, content_type: null }),
  mk({ host: "ws.productpad.io", scheme: "https", path: "/socket", url: "https://ws.productpad.io/socket", is_websocket: true, status: 101, duration_ms: 12, response_size: 0n }),
  mk({ host: "pinned.apple.com", path: "/", url: "https://pinned.apple.com/", tunneled: true, status: 200, content_type: null, response_size: 0n, response_body: null }),
  mk({ host: "api.creativeforce.io", path: "/v3/briefs?status=open", url: "https://api.creativeforce.io/v3/briefs?status=open", status: null, duration_ms: null, response_size: 0n, state: "Started", response_body: null }),
];

const REVERSED = new URLSearchParams(location.search).has("rev");
const ROWS = REVERSED ? [...FLOWS].reverse() : FLOWS;

function Harness() {
  const [columns, setColumns] = useState<ColumnId[]>(normalizeColumns([...DEFAULT_COLUMNS, "edited"]));
  const [widths, setWidths] = useState({});
  const [treeWidth, setTreeWidth] = useState(DEFAULT_PREFS.treeWidth);
  const [inspectorPct, setInspectorPct] = useState(DEFAULT_PREFS.inspectorPct);
  const [requestPct, setRequestPct] = useState(DEFAULT_PREFS.requestPct);
  const [saved, setSaved] = useState<SavedFilter[]>([
    { id: "s1", label: "JSON · 4xx · 5xx", filter: { type: ["json"], status: ["4xx", "5xx"] } },
    { id: "s2", label: "slack.com", filter: { scope: { kind: "host", host: "slack.com" } } },
  ]);
  useEffect(() => {
    const st = useStore.getState();
    st.togglePin(FLOWS[2].id);
    st.setComment(FLOWS[2].id, "401 only for this tenant");
  }, []);
  const [filter, setFilter] = useState<FlowFilter>({
    ...EMPTY_FILTER,
    clauses: [
      { id: "c1", enabled: true, field: "header", op: "contains", value: "bearer" },
      { id: "c2", enabled: false, field: "status", op: "gt", value: "400" },
    ],
  });
  const [selectedId, setSelectedId] = useState<string | null>(ROWS[REVERSED ? 0 : 3].id);
  const selected = ROWS.find((f) => f.id === selectedId) ?? null;
  return (
    <div className="nova">
      <div className="body">
        <div className="content">
          <FlowsSection
            flows={ROWS}
            filter={filter}
            patch={(p) => setFilter((f) => ({ ...f, ...p }))}
            reset={() => setFilter(EMPTY_FILTER)}
            columns={columns}
            setColumns={setColumns}
            widths={widths}
            setWidths={setWidths}
            saved={saved}
            setSaved={setSaved}
            treeWidth={treeWidth}
            setTreeWidth={setTreeWidth}
            commitTreeWidth={setTreeWidth}
            inspectorPct={inspectorPct}
            setInspectorPct={setInspectorPct}
            commitInspectorPct={setInspectorPct}
            requestPct={requestPct}
            setRequestPct={setRequestPct}
            commitRequestPct={setRequestPct}
            recording
            selected={selected}
            select={setSelectedId}
            autoSelect={false}
            onResend={() => {}}
            onCopyCurl={() => {}}
            showToast={() => {}}
          />
        </div>
      </div>
      <div className="statusbar">
        <span className="live on">recording</span>
        <span>{FLOWS.length} flows · 6 hosts</span>
        <span className="autosel"><span /> Auto Select</span>
        <span className="spacer" />
        <span>12.3 Mb/s</span>
        <span>upstream: direct</span>
        <span className="foot-ok">CA trusted</span>
        <span>127.0.0.1:9090</span>
      </div>
    </div>
  );
}

function Panels() {
  const f = FLOWS[3];
  const git = FLOWS[6];
  const box = (title: string, body: React.ReactNode) => (
    <div style={{ border: "1px solid rgba(27,26,61,.1)", borderRadius: 14, background: "#fff", padding: 12 }}>
      <div style={{ font: "600 11px Inter", letterSpacing: ".09em", textTransform: "uppercase", color: "#6B6A86", marginBottom: 8 }}>{title}</div>
      {body}
    </div>
  );
  return (
    <div className="nova" style={{ padding: 16, display: "grid", gridTemplateColumns: "1fr 1fr", gap: 14, height: "auto", overflow: "auto" }}>
      {box("Query", <KeyValueTable rows={parseQuery("https://x.com/a?id=1&id=2&q=a%20b")} empty="none" />)}
      {box("Cookies (request)", <KeyValueTable rows={parseCookies(f, "request")} empty="none" />)}
      {box("Raw (request)", <pre className="code">{rawHttp(f, "request")}</pre>)}
      {box("Raw (response)", <pre className="code">{rawHttp(f, "response")}</pre>)}
      {box("Treeview", <TreeviewPanel text={bodyToText(f.response_body)} />)}
      {box("Treeview — not json", <TreeviewPanel text={"<html>hi</html>"} />)}
      {box("Timing", <TimingPanel flow={f} />)}
      {box("Summary (response)", <KeyValueTable rows={summaryOf(git, "response")} empty="none" />)}
      {box("Body", <BodyPanel body={f.response_body} kind="res" status={200} flowId={f.id} showToast={() => {}} />)}
    </div>
  );
}

const q = new URLSearchParams(location.search);
createRoot(document.getElementById("root")!).render(
  q.has("panels") ? <Panels /> : q.has("shortcuts") ? (
    <div className="nova">
      <ShortcutsDialog onClose={() => {}} />
    </div>
  ) : (
    <Harness />
  ),
);
