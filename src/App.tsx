import { useEffect, useMemo, useRef, useState } from "react";
import {
  api,
  Channel,
  type Flow,
  type CaStatus,
  type Rule,
  type RuleKind,
  type Interception,
  type Header,
  type NetworkConditions,
  type WsMessage,
  type TlsScope,
} from "./api";
import { useStore } from "./store";
import { exportSession, exportHar, importSession } from "./session";

/* ------------------------------- helpers ------------------------------- */

const num = (n: number | bigint | null | undefined) => (n == null ? 0 : Number(n));

function formatBytes(n: number | bigint) {
  const v = num(n);
  if (!v) return "—";
  if (v < 1024) return `${v} B`;
  if (v < 1024 * 1024) return `${(v / 1024).toFixed(1)} KB`;
  return `${(v / 1024 / 1024).toFixed(2)} MB`;
}

function formatAgo(ms: number) {
  const s = Math.max(0, (Date.now() - ms) / 1000);
  if (s < 1) return "just now";
  if (s < 60) return `${Math.floor(s)}s ago`;
  if (s < 3600) return `${Math.floor(s / 60)}m ago`;
  return `${Math.floor(s / 3600)}h ago`;
}

const KNOWN_METHODS = ["GET", "POST", "PUT", "DELETE", "PATCH"];
const methodClass = (m: string) => (KNOWN_METHODS.includes(m) ? `m-${m}` : "m-OTHER");

function statusClass(status: number | null, error: string | null) {
  if (error) return "s-err";
  if (!status) return "s-pending";
  const b = Math.floor(status / 100);
  return b === 1 ? "s-1xx" : b === 2 ? "s-2xx" : b === 3 ? "s-3xx" : b === 4 ? "s-4xx" : "s-5xx";
}

const statusText = (status: number | null, error: string | null) => (error ? "ERR" : status ?? "···");

function buildCurl(f: Flow): string {
  let s = `curl -X ${f.method} '${f.url}'`;
  for (const h of f.request_headers) s += ` \\\n  -H '${h.name}: ${h.value}'`;
  if (f.request_body?.text) s += ` \\\n  --data '${f.request_body.text}'`;
  return s;
}

function bodyToText(body: Flow["request_body"]): string | null {
  if (!body || body.text == null) return null;
  const ct = (body.media_type ?? "").toLowerCase();
  if (ct.includes("json")) {
    try {
      return JSON.stringify(JSON.parse(body.text), null, 2);
    } catch {
      return body.text;
    }
  }
  return body.text;
}

type Section = "flows" | "rules" | "break" | "scripts" | "certs";
type DetailTab = "overview" | "request" | "response" | "timing" | "curl" | "ws";

const RAIL: { id: Section; icon: string; label: string }[] = [
  { id: "flows", icon: "≋", label: "Flows" },
  { id: "rules", icon: "⤳", label: "Rules" },
  { id: "break", icon: "⏸", label: "Break" },
  { id: "scripts", icon: "{ }", label: "Scripts" },
  { id: "certs", icon: "🔒", label: "Certs" },
];

const ACCENTS = ["#7c6cff", "#2b8fff", "#12b886", "#f2622a"];

const DEFAULT_SCRIPT = `// Runs against every intercepted flow.
// Edit flow.headers, or call flow.abort() to block the request.
export function onRequest(flow) {
  flow.headers["x-nova-debug"] = "1";
  if (flow.host.includes("telemetry")) flow.abort();
}

export function onResponse(flow) {
  // flow.status, flow.headers are available here
}
`;

const RULE_KINDS: RuleKind[] = ["MapRemote", "MapLocal", "Block", "Rewrite"];
const RULE_KIND_LABEL: Record<RuleKind, string> = {
  MapRemote: "Map Remote",
  MapLocal: "Map Local",
  Block: "Block",
  Rewrite: "Rewrite",
};

/* -------------------------------- App -------------------------------- */

export function App() {
  const { flows, recording, selectedId, proxy, ca, setRecording, clear, select } = useStore();

  const [section, setSection] = useState<Section>("flows");
  const [theme, setTheme] = useState<"dark" | "light">("dark");
  const [accent, setAccent] = useState(ACCENTS[1]);
  const [query, setQuery] = useState("");
  const [appFilter, setAppFilter] = useState("");
  const [groupByHost, setGroupByHost] = useState(true);
  const [detailTab, setDetailTab] = useState<DetailTab>("overview");

  const [paletteOpen, setPaletteOpen] = useState(false);
  const [paletteQuery, setPaletteQuery] = useState("");
  const [palIndex, setPalIndex] = useState(0);
  const [toast, setToastState] = useState("");
  const toastTimer = useRef<number | undefined>(undefined);

  const [rules, setRulesState] = useState<Rule[]>([]);
  const [scriptSource, setScriptSource] = useState(DEFAULT_SCRIPT);
  const [scriptEnabled, setScriptEnabled] = useState(false);
  const [bpArmed, setBpArmed] = useState(false);
  const [intercept, setIntercept] = useState<Interception | null>(null);
  const [net, setNet] = useState<NetworkConditions>({ enabled: false, latency_ms: 0, down_kbps: 0 });
  const [settingsOpen, setSettingsOpen] = useState(false);

  const saveNet = (next: NetworkConditions) => {
    setNet(next);
    api.setNetworkConditions(next).catch((e) => showToast(String(e)));
  };

  async function doExportSession() {
    try {
      if (await exportSession(useStore.getState().flows)) showToast("Session saved");
    } catch (e) { showToast(String(e)); }
  }
  async function doExportHar() {
    try {
      if (await exportHar(useStore.getState().flows)) showToast("HAR exported");
    } catch (e) { showToast(String(e)); }
  }
  async function doImportSession() {
    try {
      const flows = await importSession();
      if (flows) { useStore.getState().loadFlows(flows); showToast(`Imported ${flows.length} flows`); }
    } catch (e) { showToast(String(e)); }
  }

  // Persist rule edits to the backend (which updates the live engine set).
  const saveRules = (next: Rule[]) => {
    setRulesState(next);
    api.setRules(next).catch((e) => showToast(String(e)));
  };

  // Arm/disarm the breakpoint (backend is one-shot: it disarms after a hit).
  const armBreakpoint = (armed: boolean, pattern?: string) => {
    setBpArmed(armed);
    api.setBreakpoint(armed, pattern).catch((e) => showToast(String(e)));
  };

  const showToast = (t: string, ms?: number) => {
    setToastState(t);
    window.clearTimeout(toastTimer.current);
    // Longer messages (typically errors) need more reading time: scale with
    // length, clamped to a sane range, unless an explicit duration is given.
    const dur = ms ?? Math.min(9000, Math.max(2600, 2000 + t.length * 55));
    toastTimer.current = window.setTimeout(() => setToastState(""), dur);
  };

  // Wire the streaming channel + initial status once.
  useEffect(() => {
    const channel = new Channel<Flow>();
    channel.onmessage = (flow) => useStore.getState().upsertFlow(flow);
    api.subscribeFlows(channel);

    const bpChannel = new Channel<Interception>();
    bpChannel.onmessage = (i) => {
      setIntercept(i);
      setBpArmed(false); // one-shot: the backend disarmed on this hit
    };
    api.subscribeBreakpoints(bpChannel);

    const wsChannel = new Channel<WsMessage>();
    wsChannel.onmessage = (m) => useStore.getState().addWsMessage(m);
    api.subscribeWs(wsChannel);

    api.proxyStatus().then((p) => useStore.getState().setProxy(p));
    api.caStatus().then((c) => useStore.getState().setCa(c)).catch(() => {});
    api.getRules().then(setRulesState).catch(() => {});
    api.getScript().then((s) => { if (s.trim()) setScriptSource(s); }).catch(() => {});
    api.getNetworkConditions().then(setNet).catch(() => {});
  }, []);

  // Refresh the captured counter while running.
  useEffect(() => {
    if (!proxy.running) return;
    const t = setInterval(() => api.proxyStatus().then((p) => useStore.getState().setProxy(p)), 2000);
    return () => clearInterval(t);
  }, [proxy.running]);

  async function toggleProxy() {
    try {
      const next = await api.setSystemProxy(!proxy.system_proxy);
      useStore.getState().setProxy(next);
      showToast(next.system_proxy ? "System proxy enabled" : "System proxy disabled");
    } catch (e) {
      showToast(String(e));
    }
  }

  async function resendSelected() {
    if (!selected) return showToast("No flow selected");
    try {
      await api.resendFlow(selected);
      showToast("Request resent through the proxy");
    } catch (e) {
      showToast(String(e));
    }
  }

  const selected = useMemo(() => flows.find((f) => f.id === selectedId) ?? null, [flows, selectedId]);

  function copyCurl() {
    if (!selected) return showToast("No flow selected");
    navigator.clipboard.writeText(buildCurl(selected));
    showToast("cURL copied to clipboard");
  }

  /* command palette */
  const commands = useMemo(
    () => [
      { id: "rec", icon: "⏺", label: recording ? "Pause capture" : "Resume capture", run: () => setRecording(!recording) },
      { id: "clear", icon: "🗑", label: "Clear all flows", run: () => clear() },
      { id: "proxy", icon: "⇄", label: proxy.system_proxy ? "Disable system proxy" : "Enable system proxy", run: () => void toggleProxy() },
      { id: "resend", icon: "↻", label: "Resend selected flow", run: () => void resendSelected() },
      { id: "curl", icon: "⌗", label: "Copy selected as cURL", kbd: "↵", run: () => copyCurl() },
      { id: "save", icon: "⇩", label: "Save session (.nova)", run: () => void doExportSession() },
      { id: "open", icon: "⇧", label: "Open session (.nova)", run: () => void doImportSession() },
      { id: "har", icon: "⤓", label: "Export as HAR", run: () => void doExportHar() },
      { id: "bp", icon: "⏸", label: "Arm breakpoint on next request", run: () => { armBreakpoint(true); setSection("break"); showToast("Breakpoint armed"); } },
      { id: "rules", icon: "⤳", label: "Open Rules", run: () => setSection("rules") },
      { id: "scripts", icon: "{ }", label: "Open Scripts", run: () => setSection("scripts") },
      { id: "certs", icon: "🔒", label: "Open Certificate", run: () => setSection("certs") },
      { id: "theme", icon: "◑", label: theme === "dark" ? "Switch to light theme" : "Switch to dark theme", run: () => setTheme(theme === "dark" ? "light" : "dark") },
    ],
    [recording, proxy.running, proxy.system_proxy, theme, selected],
  );
  const palFiltered = useMemo(() => {
    const q = paletteQuery.toLowerCase();
    return commands.filter((c) => c.label.toLowerCase().includes(q));
  }, [commands, paletteQuery]);

  const openPalette = () => { setPaletteOpen(true); setPaletteQuery(""); setPalIndex(0); };
  const closePalette = () => setPaletteOpen(false);
  const runCommand = (c: (typeof commands)[number]) => { setPaletteOpen(false); setTimeout(() => c.run(), 0); };

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && (e.key === "k" || e.key === "K")) {
        e.preventDefault();
        paletteOpen ? closePalette() : openPalette();
        return;
      }
      if (!paletteOpen) return;
      if (e.key === "Escape") { e.preventDefault(); closePalette(); }
      else if (e.key === "ArrowDown") { e.preventDefault(); setPalIndex((i) => Math.min(palFiltered.length - 1, i + 1)); }
      else if (e.key === "ArrowUp") { e.preventDefault(); setPalIndex((i) => Math.max(0, i - 1)); }
      else if (e.key === "Enter") { e.preventDefault(); const c = palFiltered[palIndex]; if (c) runCommand(c); }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [paletteOpen, palFiltered, palIndex]);

  const hostCount = useMemo(() => new Set(flows.map((f) => f.host)).size, [flows]);

  // Distinct originating apps observed in captured traffic, for the app filter.
  const apps = useMemo(() => {
    const set = new Set<string>();
    for (const f of flows) if (f.process) set.add(f.process);
    return [...set].sort((a, b) => a.localeCompare(b));
  }, [flows]);

  return (
    <div className="nova" data-nova-theme={theme} style={{ ["--accent" as string]: accent }}>
      {/* titlebar */}
      <div className="titlebar">
        <div className="traffic">
          <span style={{ background: "#ff5f57" }} />
          <span style={{ background: "#febc2e" }} />
          <span style={{ background: "#28c840" }} />
        </div>
        <div className="tb-title">
          <span className="tb-logo" />
          NovaProxy <span className="tb-sub">— default workspace</span>
        </div>
        <div className="tb-spacer" />
      </div>

      <div className="body">
        {/* rail */}
        <div className="rail">
          {RAIL.map((r) => (
            <div
              key={r.id}
              className={`rail-item ${section === r.id ? "active" : ""}`}
              title={r.label}
              onClick={() => setSection(r.id)}
            >
              <span className="icon">{r.icon}</span>
              <span className="label">{r.label}</span>
            </div>
          ))}
          <div className="spacer" />
          <div className="rail-gear" title="Settings" onClick={() => setSettingsOpen(true)}>⚙</div>
        </div>

        {/* content */}
        <div className="content">
          {/* toolbar */}
          <div className="toolbar">
            <div className={`tool-btn rec-btn ${recording ? "on" : ""}`} onClick={() => setRecording(!recording)}>
              <span className="rec-dot" />
              {recording ? "Recording" : "Paused"}
            </div>
            <div className="tool-btn" onClick={() => clear()}>Clear</div>
            <div className="tool-sep" />
            <div className="search">
              <span className="mag">⌕</span>
              <input
                value={query}
                onChange={(e) => setQuery(e.target.value)}
                placeholder="Filter by host, path, method:GET, status:401…"
              />
              {query && <span className="clear" onClick={() => setQuery("")}>✕</span>}
            </div>
            {section === "flows" && (
              <select
                className="app-filter"
                value={appFilter}
                onChange={(e) => setAppFilter(e.target.value)}
                title="Capture only requests from the selected app"
              >
                <option value="">All apps</option>
                {appFilter && !apps.includes(appFilter) && <option value={appFilter}>{appFilter}</option>}
                {apps.map((a) => <option key={a} value={a}>{a}</option>)}
              </select>
            )}
            <div className="spacer" />
            <div className="cmd-btn" onClick={openPalette}>
              <span>Commands</span>
              <span className="kbd">⌘K</span>
            </div>
            <div className="proxy-toggle" onClick={() => void toggleProxy()}>
              <span className={`switch sm ${proxy.system_proxy ? "on" : ""}`}><span className="knob" /></span>
              System Proxy
            </div>
          </div>

          {section === "flows" && (
            <FlowsSection
              flows={flows}
              query={query}
              appFilter={appFilter}
              groupByHost={groupByHost}
              toggleGroup={() => setGroupByHost((v) => !v)}
              recording={recording}
              selected={selected}
              select={select}
              detailTab={detailTab}
              setDetailTab={setDetailTab}
              onResend={() => void resendSelected()}
              onCopyCurl={copyCurl}
              openPalette={openPalette}
            />
          )}
          {section === "rules" && <RulesSection rules={rules} saveRules={saveRules} />}
          {section === "break" && <BreakSection armed={bpArmed} onArm={armBreakpoint} />}
          {section === "scripts" && (
            <ScriptsSection
              source={scriptSource}
              setSource={setScriptSource}
              enabled={scriptEnabled}
              onApply={(src, en) => {
                setScriptEnabled(en);
                api.setScript(src, en)
                  .then(() => showToast(en ? "Script applied & enabled" : "Script saved (disabled)"))
                  .catch((e) => showToast(String(e)));
              }}
            />
          )}
          {section === "certs" && <CertsSection ca={ca} showToast={showToast} />}
        </div>
      </div>

      {/* status bar */}
      <div className="statusbar">
        <span className={`live ${recording && proxy.running ? "on" : ""}`}>
          {!proxy.running ? "❚❚ stopped" : recording ? "● live" : "❚❚ paused"}
        </span>
        <span>{flows.length} flows · {hostCount} hosts</span>
        <span className="spacer" />
        <span>upstream: direct</span>
        <span className={ca?.trusted ? "foot-ok" : "foot-warn"}>CA {ca?.trusted ? "trusted" : "not installed"}</span>
        <span>{proxy.running ? `${proxy.host}:${proxy.port}` : "127.0.0.1:9090"}</span>
      </div>

      {/* command palette */}
      {paletteOpen && (
        <>
          <div className="scrim" onClick={closePalette} />
          <div className="palette">
            <div className="palette-input">
              <span className="glyph">⌘</span>
              <input
                autoFocus
                value={paletteQuery}
                onChange={(e) => { setPaletteQuery(e.target.value); setPalIndex(0); }}
                placeholder="Type a command…"
              />
              <span className="esc">ESC</span>
            </div>
            <div className="palette-list">
              {palFiltered.length === 0 && <div className="palette-empty">No commands match</div>}
              {palFiltered.map((c, i) => (
                <button
                  key={c.id}
                  className={`palette-item ${i === palIndex ? "active" : ""}`}
                  onMouseEnter={() => setPalIndex(i)}
                  onClick={() => runCommand(c)}
                >
                  <span className="picon">{c.icon}</span>
                  <span className="plabel">{c.label}</span>
                  {"kbd" in c && c.kbd && <span className="pkbd">{c.kbd}</span>}
                </button>
              ))}
            </div>
          </div>
        </>
      )}

      {/* settings modal */}
      {settingsOpen && (
        <SettingsModal
          theme={theme} setTheme={setTheme}
          accent={accent} setAccent={setAccent}
          port={proxy.port ?? 9090} ca={ca}
          net={net} setNet={saveNet}
          onClose={() => setSettingsOpen(false)}
        />
      )}

      {/* intercept modal (paused at breakpoint) */}
      {intercept && (
        <InterceptModal
          interception={intercept}
          onResume={(cont, headers) => {
            api.resumeBreakpoint(intercept.id, cont, headers).catch((e) => showToast(String(e)));
            setIntercept(null);
            showToast(cont ? "Request continued" : "Request aborted");
          }}
        />
      )}

      {/* toast */}
      {toast && (
        <div className="toast"><span className="ok">✓</span>{toast}</div>
      )}
    </div>
  );
}

/* ------------------------------ flows section ------------------------------ */

function matchQuery(f: Flow, q: string): boolean {
  q = q.trim().toLowerCase();
  if (!q) return true;
  if (q.startsWith("method:")) return f.method.toLowerCase() === q.slice(7).trim();
  if (q.startsWith("status:")) return String(f.status ?? "") === q.slice(7).trim();
  if (q.startsWith("host:")) return f.host.toLowerCase().includes(q.slice(5).trim());
  if (q.startsWith("app:")) return (f.process ?? "").toLowerCase().includes(q.slice(4).trim());
  return `${f.host} ${f.path} ${f.method} ${f.status ?? ""} ${f.process ?? ""}`.toLowerCase().includes(q);
}

function FlowsSection(props: {
  flows: Flow[];
  query: string;
  appFilter: string;
  groupByHost: boolean;
  toggleGroup: () => void;
  recording: boolean;
  selected: Flow | null;
  select: (id: string | null) => void;
  detailTab: DetailTab;
  setDetailTab: (t: DetailTab) => void;
  onResend: () => void;
  onCopyCurl: () => void;
  openPalette: () => void;
}) {
  const { flows, query, appFilter, groupByHost, selected, select } = props;

  const filtered = useMemo(
    () => flows.filter((f) => (!appFilter || f.process === appFilter) && matchQuery(f, query)),
    [flows, query, appFilter],
  );

  const groups = useMemo(() => {
    if (!groupByHost) {
      return [{ key: "all", host: "", tls: false, showHeader: false, flows: filtered }];
    }
    const map = new Map<string, Flow[]>();
    for (const f of filtered) {
      if (!map.has(f.host)) map.set(f.host, []);
      map.get(f.host)!.push(f);
    }
    return [...map.entries()].map(([host, fl]) => ({
      key: host, host, tls: fl[0].scheme === "https", showHeader: true, flows: fl,
    }));
  }, [filtered, groupByHost]);

  const emptyMsg =
    flows.length === 0
      ? props.recording ? "Waiting for traffic…" : "Recording paused — no flows captured"
      : "No flows match your filter";

  return (
    <div className="flows">
      <div className="flow-list">
        <div className="flow-list-head">
          <span>{filtered.length} flow{filtered.length === 1 ? "" : "s"}</span>
          <span className="grouptog" onClick={props.toggleGroup}>{groupByHost ? "▾ grouped" : "≡ flat"}</span>
        </div>
        <div className="flow-scroll">
          {filtered.length === 0 && <div className="list-empty">{emptyMsg}</div>}
          {groups.map((g) => (
            <div key={g.key}>
              {g.showHeader && (
                <div className="group-head">
                  <span className="hdot" />
                  <span className="hname">{g.host}</span>
                  {g.tls && <span className="tls-chip">TLS</span>}
                  <span className="spacer" />
                  <span className="hcount">{g.flows.length}</span>
                </div>
              )}
              {g.flows.map((f) => (
                <button
                  key={f.id}
                  className={`flow-row ${f.id === selected?.id ? "sel" : ""}`}
                  onClick={() => select(f.id)}
                >
                  <span className={`badge ${methodClass(f.method)}`}>{f.method}</span>
                  <span className="col">
                    <div className="fpath">{f.path}</div>
                    <div className="fsub">
                      {f.mapped_from && <span className="fmap">⤳ </span>}
                      {f.host}
                      {f.resent && <span className="fresent"> · resent</span>}
                    </div>
                  </span>
                  <span className="fright">
                    <div className={`fstatus ${statusClass(f.status, f.error)}`}>{statusText(f.status, f.error)}</div>
                    <div className="ftime">{f.duration_ms != null ? `${Math.round(f.duration_ms)}ms` : "—"}</div>
                  </span>
                </button>
              ))}
            </div>
          ))}
        </div>
      </div>

      <div className="detail">
        {!selected ? (
          <div className="detail-empty">
            <div className="glyph">≋</div>
            <div className="big">Select a flow to inspect</div>
            <div className="hint">or press <span className="kbd">⌘K</span> for commands</div>
          </div>
        ) : (
          <Detail
            flow={selected}
            tab={props.detailTab}
            setTab={props.setDetailTab}
            onResend={props.onResend}
            onCopyCurl={props.onCopyCurl}
          />
        )}
      </div>
    </div>
  );
}

const DETAIL_TABS: { id: DetailTab; label: string }[] = [
  { id: "overview", label: "Overview" },
  { id: "request", label: "Request" },
  { id: "response", label: "Response" },
  { id: "timing", label: "Timing" },
  { id: "curl", label: "cURL" },
];

function Detail({
  flow, tab, setTab, onResend, onCopyCurl,
}: {
  flow: Flow;
  tab: DetailTab;
  setTab: (t: DetailTab) => void;
  onResend: () => void;
  onCopyCurl: () => void;
}) {
  const wsMessages = useStore((s) => s.wsMessages[flow.id]);
  const tabs = flow.is_websocket
    ? [...DETAIL_TABS, { id: "ws" as DetailTab, label: `WebSocket${wsMessages ? ` (${wsMessages.length})` : ""}` }]
    : DETAIL_TABS;
  const totalSize = num(flow.request_size) + num(flow.response_size);
  const facts = [
    { k: "Method", v: flow.method },
    { k: "Status", v: flow.error ? "error" : String(flow.status ?? "pending") },
    { k: "Protocol", v: flow.http_version },
    { k: "Scheme", v: flow.scheme.toUpperCase() },
    { k: "Remote host", v: flow.host },
    { k: "App", v: flow.process ? `${flow.process}${flow.pid != null ? ` (${flow.pid})` : ""}` : "—" },
    { k: "Duration", v: flow.duration_ms != null ? `${Math.round(flow.duration_ms)} ms` : "—" },
    { k: "Size", v: formatBytes(totalSize) },
    { k: "Started", v: formatAgo(flow.started_at) },
  ];

  return (
    <>
      <div className="detail-head">
        <div className="detail-url">
          <span className={`badge ${methodClass(flow.method)}`}>{flow.method}</span>
          <span className="u">{flow.url}</span>
          <span className={`status-pill ${statusClass(flow.status, flow.error)}`}>{statusText(flow.status, flow.error)}</span>
          <div className="resend" onClick={onResend}>↻ Resend</div>
        </div>
        <div className="detail-tabs">
          {tabs.map((t) => (
            <div key={t.id} className={`dtab ${tab === t.id ? "active" : ""}`} onClick={() => setTab(t.id)}>{t.label}</div>
          ))}
        </div>
      </div>

      <div className="detail-body">
        {tab === "overview" && (
          <>
            <div className="fact-grid">
              {facts.map((f) => (
                <div className="fact" key={f.k}>
                  <div className="k">{f.k}</div>
                  <div className="v">{f.v}</div>
                </div>
              ))}
            </div>
            <div className="chips">
              {flow.scheme === "https" ? (
                <span className="chip green">🔒 TLS · decrypted</span>
              ) : (
                <span className="chip blue">plaintext</span>
              )}
              <span className="chip blue">{flow.http_version}</span>
              {flow.is_websocket && <span className="chip cyan">≋ WebSocket</span>}
              {flow.tunneled && <span className="chip amber">⇅ tunneled · not decrypted</span>}
              {flow.mapped_from && <span className="chip violet">⤳ mapped from {flow.mapped_from}</span>}
              {flow.resent && <span className="chip cyan">↻ resent</span>}
              {flow.error && <span className="chip red">⚠ {flow.error}</span>}
            </div>
          </>
        )}

        {tab === "request" && (
          <>
            <div className="sec-label">Request headers</div>
            {flow.request_headers.length === 0 && <div className="sec-label metaval">— no headers —</div>}
            {flow.request_headers.map((h, i) => (
              <div className="hrow" key={i}><span className="hk">{h.name}</span><span className="hv">{h.value}</span></div>
            ))}
            <div className="sec-label">Body</div>
            <BodyBlock body={flow.request_body} kind="req" />
          </>
        )}

        {tab === "response" && (
          <>
            <div className="sec-label">Response headers</div>
            {flow.response_headers.length === 0 && <div className="sec-label metaval">— no headers —</div>}
            {flow.response_headers.map((h, i) => (
              <div className="hrow" key={i}><span className="hk">{h.name}</span><span className="hv">{h.value}</span></div>
            ))}
            <div className="sec-label meta">
              Body
              <span className="metaval">{(flow.content_type ?? "—")} · {formatBytes(flow.response_size)}</span>
            </div>
            <BodyBlock body={flow.response_body} kind="res" status={flow.status} />
          </>
        )}

        {tab === "timing" && (
          <div className="timing">
            <div className="timing-row">
              <span className="tl">Total</span>
              <div className="timing-bar"><span style={{ width: "100%", background: "var(--c-amber)" }} /></div>
              <span className="tv">{flow.duration_ms != null ? `${Math.round(flow.duration_ms)}ms` : "—"}</span>
            </div>
            <div className="timing-row">
              <span className="tl">Request</span>
              <div className="timing-bar"><span style={{ width: "100%", background: "var(--c-blue)", opacity: 0.5 }} /></div>
              <span className="tv">{formatBytes(flow.request_size)}</span>
            </div>
            <div className="timing-row">
              <span className="tl">Response</span>
              <div className="timing-bar"><span style={{ width: "100%", background: "var(--c-green)", opacity: 0.5 }} /></div>
              <span className="tv">{formatBytes(flow.response_size)}</span>
            </div>
            <div className="timing-total">
              <span>Started</span>
              <span className="mono">{formatAgo(flow.started_at)}</span>
            </div>
          </div>
        )}

        {tab === "curl" && (
          <>
            <div className="sec-label meta">
              Export as cURL
              <span className="copy" onClick={onCopyCurl}>Copy</span>
            </div>
            <pre className="code curl">{buildCurl(flow)}</pre>
          </>
        )}

        {tab === "ws" && <WsPanel messages={wsMessages} />}
      </div>
    </>
  );
}

function WsPanel({ messages }: { messages: WsMessage[] | undefined }) {
  if (!messages || messages.length === 0) {
    return <pre className="code res">— no WebSocket frames captured yet —</pre>;
  }
  return (
    <div className="ws-log">
      {messages.map((m) => {
        const sent = m.direction === "Sent";
        const label = m.opcode.toLowerCase();
        const payload =
          m.text != null
            ? m.text
            : m.base64 != null
            ? `[binary ${formatBytes(m.size)}]`
            : m.opcode === "Close"
            ? "(closed)"
            : "";
        return (
          <div className={`ws-frame ${sent ? "sent" : "recv"}`} key={m.flow_id + "-" + String(m.seq)}>
            <span className={`ws-dir ${sent ? "sent" : "recv"}`}>{sent ? "▲ sent" : "▼ recv"}</span>
            <span className="ws-op">{label}</span>
            <span className="ws-payload">{payload}{m.truncated ? " …(truncated)" : ""}</span>
            <span className="ws-meta">{formatBytes(m.size)} · {formatAgo(m.at)}</span>
          </div>
        );
      })}
    </div>
  );
}

function BodyBlock({ body, kind, status }: { body: Flow["request_body"]; kind: "req" | "res"; status?: number | null }) {
  if (!body) {
    return <pre className={`code ${kind}`}>{status === 204 ? "— no content (204) —" : "— no body —"}</pre>;
  }
  const ct = (body.media_type ?? "").toLowerCase();
  if (body.base64 && ct.startsWith("image/")) {
    return (
      <div className={`code ${kind}`}>
        <img src={`data:${body.media_type};base64,${body.base64}`} alt="body preview" />
      </div>
    );
  }
  if (body.base64) {
    return <pre className={`code ${kind}`}>Binary body — {formatBytes(body.size)} ({body.media_type ?? "unknown"}){body.truncated ? ", truncated" : ""}</pre>;
  }
  const text = bodyToText(body);
  return (
    <pre className={`code ${kind}`}>
      {text ?? "— empty body —"}
      {body.truncated ? "\n… preview truncated at capture cap" : ""}
    </pre>
  );
}

/* ------------------------------ rules section ------------------------------ */

function RulesSection({ rules, saveRules }: { rules: Rule[]; saveRules: (r: Rule[]) => void }) {
  function addRule() {
    const id = "r" + Date.now();
    saveRules([
      ...rules,
      {
        id, enabled: true, kind: "MapRemote", name: "new rule",
        pattern: "https://api.example.com/*",
        target: "https://staging.example.com",
        header_name: null, header_value: null,
      },
    ]);
  }
  const update = (id: string, patch: Partial<Rule>) =>
    saveRules(rules.map((r) => (r.id === id ? { ...r, ...patch } : r)));
  const del = (id: string) => saveRules(rules.filter((r) => r.id !== id));

  return (
    <div className="page">
      <div className="page-inner w720">
        <div className="page-head">
          <h2 className="page-title">Rules</h2>
          <div className="btn-primary" onClick={addRule}>+ New rule</div>
        </div>
        <p className="page-sub">
          Map Remote, Map Local, Block and header Rewrite are applied to matching live traffic.
          Patterns match <span className="mono">scheme://host/path</span> with <span className="mono">*</span> wildcards.
        </p>
        {rules.length === 0 && <p className="page-sub">No rules yet — add one to reshape traffic.</p>}
        {rules.map((r) => (
          <div className="rule" key={r.id}>
            <div className="rule-head">
              <span className={`switch ${r.enabled ? "on" : ""}`} onClick={() => update(r.id, { enabled: !r.enabled })}><span className="knob" /></span>
              <select className="rule-kind" value={r.kind} onChange={(e) => update(r.id, { kind: e.target.value as RuleKind })}>
                {RULE_KINDS.map((k) => <option key={k} value={k}>{RULE_KIND_LABEL[k]}</option>)}
              </select>
              <input className="rule-name-input" value={r.name} onChange={(e) => update(r.id, { name: e.target.value })} placeholder="rule name" />
              <span className="spacer" />
              <span className="rule-del" onClick={() => del(r.id)}>🗑</span>
            </div>
            <div className="rule-body">
              <div className="rule-field">
                <div className="k">Match URL</div>
                <input className="rule-input" value={r.pattern} onChange={(e) => update(r.id, { pattern: e.target.value })} placeholder="https://host/path/*" />
              </div>
              {r.kind === "MapRemote" && (
                <div className="rule-field">
                  <div className="k">Redirect to</div>
                  <input className="rule-input" value={r.target ?? ""} onChange={(e) => update(r.id, { target: e.target.value })} placeholder="https://other-host" />
                </div>
              )}
              {r.kind === "MapLocal" && (
                <div className="rule-field">
                  <div className="k">Serve file</div>
                  <input className="rule-input" value={r.target ?? ""} onChange={(e) => update(r.id, { target: e.target.value })} placeholder="/absolute/path/response.json" />
                </div>
              )}
              {r.kind === "Block" && (
                <div className="rule-field">
                  <div className="k">Action</div>
                  <div className="v">Respond <span className="mono">403</span> to matching requests</div>
                </div>
              )}
              {r.kind === "Rewrite" && (
                <>
                  <div className="rule-field">
                    <div className="k">Header name</div>
                    <input className="rule-input" value={r.header_name ?? ""} onChange={(e) => update(r.id, { header_name: e.target.value })} placeholder="x-debug" />
                  </div>
                  <div className="rule-field">
                    <div className="k">Header value</div>
                    <input className="rule-input" value={r.header_value ?? ""} onChange={(e) => update(r.id, { header_value: e.target.value })} placeholder="true" />
                  </div>
                </>
              )}
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}

/* ------------------------------ breakpoints section ------------------------------ */

function BreakSection({ armed, onArm }: { armed: boolean; onArm: (armed: boolean, pattern?: string) => void }) {
  const [pattern, setPattern] = useState("*");
  return (
    <div className="page">
      <div className="page-inner w720">
        <h2 className="page-title">Breakpoints</h2>
        <p className="page-sub">
          Arm a breakpoint to pause the next request whose URL matches the glob below, mid-flight,
          so you can edit its headers and continue — or abort it.
        </p>
        <div className="bp-card">
          <div className={`bp-icon ${armed ? "armed" : ""}`}>⏸</div>
          <div style={{ flex: 1 }}>
            <div className="t">{armed ? "Breakpoint armed" : "Breakpoint idle"}</div>
            <div className="s">
              {armed
                ? "The next matching request will pause for inspection."
                : "Arm to intercept the next matching request."}
            </div>
          </div>
          <div
            className={`btn-primary ${armed ? "cyan" : ""}`}
            onClick={() => onArm(!armed, pattern)}
          >
            {armed ? "Disarm" : "Arm breakpoint"}
          </div>
        </div>
        <div className="rule-field" style={{ marginTop: 14 }}>
          <div className="k">Match URL</div>
          <input
            className="rule-input"
            value={pattern}
            onChange={(e) => setPattern(e.target.value)}
            placeholder="* (all) or https://api.example.com/*"
            disabled={armed}
          />
        </div>
      </div>
    </div>
  );
}

/* ------------------------------ intercept modal ------------------------------ */

function InterceptModal({
  interception,
  onResume,
}: {
  interception: Interception;
  onResume: (cont: boolean, headers: Header[]) => void;
}) {
  const initial = interception.request_headers.map((h) => `${h.name}: ${h.value}`).join("\n");
  const [text, setText] = useState(initial);

  const parseHeaders = (): Header[] =>
    text
      .split("\n")
      .map((line) => {
        const idx = line.indexOf(":");
        if (idx === -1) return null;
        return { name: line.slice(0, idx).trim(), value: line.slice(idx + 1).trim() };
      })
      .filter((h): h is Header => !!h && h.name.length > 0);

  return (
    <>
      <div className="scrim modal-scrim" />
      <div className="intercept">
        <div className="intercept-head">
          <span className="bp-dot" />
          Request paused at breakpoint
        </div>
        <div className="intercept-url">
          <span className={`badge ${methodClass(interception.method)}`}>{interception.method}</span>
          <span className="u mono">{interception.url}</span>
        </div>
        <div className="sec-label">Edit request headers</div>
        <textarea
          className="intercept-headers"
          value={text}
          onChange={(e) => setText(e.target.value)}
          spellCheck={false}
        />
        <div className="intercept-actions">
          <div className="btn-neutral danger" onClick={() => onResume(false, [])}>Abort</div>
          <div className="btn-primary" onClick={() => onResume(true, parseHeaders())}>Continue →</div>
        </div>
      </div>
    </>
  );
}

/* ------------------------------ scripts section ------------------------------ */

function ScriptsSection({
  source,
  setSource,
  enabled,
  onApply,
}: {
  source: string;
  setSource: (s: string) => void;
  enabled: boolean;
  onApply: (source: string, enabled: boolean) => void;
}) {
  return (
    <div className="page">
      <div className="page-inner w820">
        <div className="page-head">
          <h2 className="page-title">Scripts</h2>
          <div className="page-head-actions">
            <span className="script-toggle" onClick={() => onApply(source, !enabled)}>
              <span className={`switch ${enabled ? "on" : ""}`}><span className="knob" /></span>
              {enabled ? "Enabled" : "Disabled"}
            </span>
            <div className="btn-primary cyan" onClick={() => onApply(source, enabled)}>Save &amp; apply</div>
          </div>
        </div>
        <p className="page-sub">
          A QuickJS sandbox runs these hooks against every intercepted flow.{" "}
          <span className="accent">onRequest(flow)</span> / <span className="accent">onResponse(flow)</span> — edit{" "}
          <span className="mono">flow.headers</span> or call <span className="mono">flow.abort()</span>.
        </p>
        <div className="editor">
          <div className="editor-tab">tamper.js</div>
          <textarea
            className="editor-area"
            value={source}
            onChange={(e) => setSource(e.target.value)}
            spellCheck={false}
          />
        </div>
      </div>
    </div>
  );
}

/* ------------------------------ certificate section ------------------------------ */

function CertsSection({ ca, showToast }: { ca: CaStatus | null; showToast: (t: string) => void }) {
  const [busy, setBusy] = useState<string | null>(null);
  const setCa = useStore.getState().setCa;

  async function run(kind: "install" | "uninstall" | "regen") {
    setBusy(kind);
    try {
      const next =
        kind === "install" ? await api.installCa()
        : kind === "uninstall" ? await api.uninstallCa()
        : await api.regenerateCa();
      setCa(next);
      showToast(kind === "install" ? "Certificate installed & trusted" : kind === "uninstall" ? "Certificate removed" : "Root CA regenerated");
    } catch (e) {
      showToast(String(e));
    } finally {
      setBusy(null);
    }
  }

  const trusted = !!ca?.trusted;

  return (
    <div className="page">
      <div className="page-inner w640">
        <h2 className="page-title">Certificate</h2>
        <p className="page-sub">NovaProxy uses a locally-generated root CA to decrypt HTTPS. Install &amp; trust it to inspect TLS traffic.</p>
        <div className="cert-card">
          {!ca ? (
            <div className="s">Certificate authority not initialized.</div>
          ) : (
            <>
              <div className="cert-row">
                <div className={`cert-icon ${trusted ? "trusted" : ""}`}>🔒</div>
                <div style={{ flex: 1 }}>
                  <div className="cert-name">{ca.subject || "NovaProxy Root CA"}</div>
                  <div className="cert-fp">SHA-256 · {ca.fingerprint}</div>
                </div>
                <span className={`cert-status ${trusted ? "trusted" : "untrusted"}`}>{trusted ? "Trusted" : "Not installed"}</span>
              </div>
              <div className="cert-meta">
                <div><div className="k">Path</div><div className="v">{ca.cert_path}</div></div>
              </div>
              <div className="cert-actions">
                {!trusted ? (
                  <div className="btn-primary" onClick={() => !busy && run("install")}>{busy === "install" ? "Installing…" : "Install & trust"}</div>
                ) : (
                  <div className="btn-primary red" onClick={() => !busy && run("uninstall")}>{busy === "uninstall" ? "Removing…" : "Remove certificate"}</div>
                )}
                <div className="btn-neutral" onClick={() => { navigator.clipboard.writeText(ca.cert_path); showToast("Certificate path copied"); }}>Export .pem</div>
                <div className="btn-neutral" onClick={() => !busy && run("regen")}>{busy === "regen" ? "Regenerating…" : "Regenerate CA"}</div>
              </div>
            </>
          )}
        </div>

        <TlsScopeCard showToast={showToast} />
      </div>
    </div>
  );
}

function TlsScopeCard({ showToast }: { showToast: (t: string) => void }) {
  const [scope, setScope] = useState<TlsScope | null>(null);
  const [dirty, setDirty] = useState(false);

  useEffect(() => {
    api.getTlsScope().then(setScope).catch(() => {});
  }, []);

  if (!scope) return null;

  const update = (patch: Partial<TlsScope>) => {
    setScope({ ...scope, ...patch });
    setDirty(true);
  };

  const save = () => {
    // One host glob per line while editing; trim + drop blanks on save.
    const clean = (lines: string[]) => lines.map((s) => s.trim()).filter(Boolean);
    const cleaned: TlsScope = {
      ...scope,
      include: clean(scope.include),
      exclude: clean(scope.exclude),
    };
    setScope(cleaned);
    api.setTlsScope(cleaned).then(() => { setDirty(false); showToast("SSL proxying scope saved"); }).catch((e) => showToast(String(e)));
  };

  return (
    <div className="cert-card" style={{ marginTop: 16 }}>
      <div className="sec-label meta" style={{ margin: "2px 0 10px" }}>
        SSL Proxying scope
        {dirty && <span className="copy" onClick={save}>Save</span>}
      </div>
      <p className="page-sub" style={{ margin: "0 0 12px" }}>
        Hosts that pin certificates or require client certs can't be decrypted — tunnel them so the app keeps working.
      </p>
      <div className="scope-toggle" onClick={() => update({ intercept_all: !scope.intercept_all })}>
        <span className={`switch sm ${scope.intercept_all ? "on" : ""}`}><span className="knob" /></span>
        <span>{scope.intercept_all ? "Decrypt all HTTPS, except the hosts below" : "Decrypt only the hosts below"}</span>
      </div>
      {scope.intercept_all ? (
        <>
          <div className="sec-label">Tunnel (don't decrypt) — one host glob per line</div>
          <textarea
            className="intercept-headers"
            value={scope.exclude.join("\n")}
            onChange={(e) => update({ exclude: e.target.value.split("\n") })}
            placeholder={"*.apple.com\npinned.example.com"}
            spellCheck={false}
          />
        </>
      ) : (
        <>
          <div className="sec-label">Decrypt only these — one host glob per line</div>
          <textarea
            className="intercept-headers"
            value={scope.include.join("\n")}
            onChange={(e) => update({ include: e.target.value.split("\n") })}
            placeholder={"api.example.com\n*.mysite.dev"}
            spellCheck={false}
          />
        </>
      )}
      {dirty && (
        <div className="cert-actions">
          <div className="btn-primary" onClick={save}>Save scope</div>
        </div>
      )}
    </div>
  );
}

/* ------------------------------ settings modal ------------------------------ */

function SettingsModal({
  theme, setTheme, accent, setAccent, port, ca, net, setNet, onClose,
}: {
  theme: "dark" | "light";
  setTheme: (t: "dark" | "light") => void;
  accent: string;
  setAccent: (a: string) => void;
  port: number;
  ca: CaStatus | null;
  net: NetworkConditions;
  setNet: (n: NetworkConditions) => void;
  onClose: () => void;
}) {
  return (
    <>
      <div className="scrim modal-scrim" onClick={onClose}>
        <div className="modal" onClick={(e) => e.stopPropagation()}>
          <div className="modal-head">
            <h2>Settings</h2>
            <span className="modal-x" onClick={onClose}>✕</span>
          </div>
          <div className="modal-body">
            <h3>Appearance</h3>
            <div className="field-group">
              <div className={`switch ${theme === "light" ? "on" : ""}`} onClick={() => setTheme(theme === "dark" ? "light" : "dark")}><span className="knob" /></div>
              <span style={{ color: "var(--text2)" }}>{theme === "dark" ? "Dark theme" : "Light theme"}</span>
            </div>
            <div className="field-group">
              {ACCENTS.map((a) => (
                <span
                  key={a}
                  className={`swatch ${accent === a ? "sel" : ""}`}
                  style={{ background: a }}
                  onClick={() => setAccent(a)}
                />
              ))}
            </div>

            <h3>Network conditions</h3>
            <div className="field-group">
              <div
                className={`switch ${net.enabled ? "on" : ""}`}
                onClick={() => setNet({ ...net, enabled: !net.enabled })}
              >
                <span className="knob" />
              </div>
              <span style={{ color: "var(--text2)" }}>
                {net.enabled ? "Throttling active" : "Throttling off"}
              </span>
            </div>
            <div className="net-grid">
              <label className="net-field">
                <span className="k">Latency (ms)</span>
                <input
                  className="rule-input"
                  type="number"
                  min={0}
                  value={net.latency_ms}
                  onChange={(e) => setNet({ ...net, latency_ms: Math.max(0, +e.target.value || 0) })}
                />
              </label>
              <label className="net-field">
                <span className="k">Downlink (kbps, 0 = ∞)</span>
                <input
                  className="rule-input"
                  type="number"
                  min={0}
                  value={net.down_kbps}
                  onChange={(e) => setNet({ ...net, down_kbps: Math.max(0, +e.target.value || 0) })}
                />
              </label>
            </div>

            <h3>1. Route traffic through the proxy</h3>
            <CodeSnippet text={`curl -x http://127.0.0.1:${port} https://example.com`} />

            <h3>2. Trust the root certificate</h3>
            <p>Open the Certs panel and click “Install &amp; trust” so HTTPS decrypts cleanly.</p>
            <CodeSnippet
              text={`export HTTP_PROXY=http://127.0.0.1:${port}
export HTTPS_PROXY=http://127.0.0.1:${port}
export NODE_EXTRA_CA_CERTS="${ca?.cert_path ?? "<ca.pem path>"}"`}
            />
          </div>
        </div>
      </div>
    </>
  );
}

function CodeSnippet({ text }: { text: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <div className="code-block">
      <pre>{text}</pre>
      <button
        className="cb-copy"
        onClick={() => { navigator.clipboard.writeText(text); setCopied(true); setTimeout(() => setCopied(false), 1200); }}
      >
        {copied ? "Copied" : "Copy"}
      </button>
    </div>
  );
}
