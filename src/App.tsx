import { memo, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
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
  type McpStatus,
  type HelperStatus,
  type BodyPreview,
} from "./api";
import { MAX_WS_FRAMES, useStore } from "./store";
import { exportSession, exportHar, importSession } from "./session";
import {
  distinctApps,
  filterFlows,
  FLOW_CHIPS,
  type FlowChip,
  mcpLabel,
  toastDuration,
} from "./filter";
import { Brandmark } from "./Brandmark";
import { Dropdown, type DropdownItem } from "./Dropdown";
import { Icon, type IconName } from "./icons";
import {
  flowStats,
  formatRate,
  sparkPath,
  SPARK_WINDOW_MS,
  throughputRate,
  throughputSeries,
} from "./stats";
import { formatDuration, formatMs, timingBreakdown } from "./timing";
import { sliceGroups } from "./virtual";
import {
  clampListWidth,
  DEFAULT_PREFS,
  loadPrefs,
  savePrefs,
  MAX_LIST_WIDTH,
  MIN_LIST_WIDTH,
  type Prefs,
} from "./prefs";

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

/**
 * Whether a preview describes bytes the list is not holding: the metadata says
 * there is content, but neither the text nor the base64 came with it. The store
 * drops body bytes on ingest (see `withoutBodies`); the engine keeps them.
 */
function bytesMissing(body: Flow["request_body"]): boolean {
  return !!body && body.text == null && body.base64 == null && Number(body.size) > 0;
}

/** Put a flow's request body back, for the paths that need the bytes themselves. */
async function withRequestBody(flow: Flow): Promise<Flow> {
  const body = flow.request_body;
  if (!bytesMissing(body)) return flow;
  try {
    return {
      ...flow,
      request_body: await api.readBody(flow.id, "request", body!.media_type, body!.decoded_from),
    };
  } catch {
    return flow; // a cURL without its body still beats no cURL
  }
}

/**
 * A body preview with its bytes, fetched when the list is not holding them.
 *
 * The fetched copy is tagged with the flow and side it belongs to, so switching
 * flows can never show one flow's body under another's headers while the next
 * fetch is in flight.
 */
function useBodyBytes(
  flowId: string,
  side: "request" | "response",
  body: Flow["request_body"],
  onError?: (m: string) => void,
) {
  const [fetched, setFetched] = useState<{ key: string; body: BodyPreview } | null>(null);
  const [loading, setLoading] = useState(false);
  const key = `${flowId}:${side}`;
  const current = fetched?.key === key ? fetched.body : null;
  const missing = bytesMissing(body);

  useEffect(() => {
    if (!missing) return;
    let alive = true;
    setLoading(true);
    api
      .readBody(flowId, side, body!.media_type, body!.decoded_from)
      .then((p) => alive && setFetched({ key, body: p }))
      .catch((e) => alive && onError?.(String(e)))
      .finally(() => alive && setLoading(false));
    return () => {
      alive = false;
    };
    // `key` covers flowId and side; the rest of `body` is metadata for the fetch.
  }, [key, missing]);

  return {
    shown: current ?? body,
    fetched: current,
    loading,
    put: (p: BodyPreview) => setFetched({ key, body: p }),
    setLoading,
  };
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

const RAIL: { id: Section; icon: IconName; label: string }[] = [
  { id: "flows", icon: "activity", label: "Flows" },
  { id: "rules", icon: "git-branch", label: "Rules" },
  { id: "break", icon: "circle-pause", label: "Break" },
  { id: "scripts", icon: "braces", label: "Scripts" },
  { id: "certs", icon: "shield-check", label: "Certs" },
];

/** Header caption per section, so the bar always says where you are. */
const SECTION_TITLE: Record<Section, string> = {
  flows: "Traffic inspector",
  rules: "Rules",
  break: "Breakpoints",
  scripts: "Scripts",
  certs: "Certificate",
};

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

  // Persisted preferences. Read once: they are defaults for this session, not a
  // live binding — flipping "default grouping" must not reshuffle the list under
  // someone who has since toggled it in the toolbar.
  const [prefs, setPrefsState] = useState<Prefs>(() => loadPrefs());
  const setPrefs = (next: Prefs) => {
    setPrefsState(next);
    savePrefs(next);
  };

  const [section, setSection] = useState<Section>("flows");
  const [query, setQuery] = useState("");
  const [appFilter, setAppFilter] = useState("");
  // Which slice of the capture the list shows, and (separately) whether
  // NovaProxy's own MCP/replay traffic is part of it.
  const [chip, setChip] = useState<FlowChip>("all");
  const [showInternal, setShowInternal] = useState(false);
  const [groupByHost, setGroupByHost] = useState(prefs.flowGrouping === "grouped");
  const [detailTab, setDetailTab] = useState<DetailTab>("overview");
  const [listWidth, setListWidth] = useState(prefs.flowListWidth);

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
  const [mcp, setMcp] = useState<McpStatus | null>(null);
  const [helper, setHelper] = useState<HelperStatus | null>(null);
  const [restoreHidden, setRestoreHidden] = useState(false);

  const saveNet = (next: NetworkConditions) => {
    setNet(next);
    api.setNetworkConditions(next).catch((e) => showToast(String(e)));
  };

  /**
   * The flows to write out, with their bodies.
   *
   * The list itself keeps no body bytes (see `withoutBodies`), so an export takes
   * the engine's retained copies and falls back to the list for anything the
   * engine does not have — flows imported from a session file exist only here.
   */
  async function flowsForExport(): Promise<Flow[]> {
    const listed = useStore.getState().flows;
    try {
      const retained = new Map((await api.retainedFlows()).map((f) => [f.id, f]));
      return listed.map((f) => retained.get(f.id) ?? f);
    } catch (e) {
      // The export still has every flow, just not the bodies the list dropped —
      // said out loud, because a silently body-less export looks complete.
      showToast(`Couldn't read bodies from the engine — exporting without them (${e})`);
      return listed;
    }
  }
  async function doExportSession() {
    try {
      if (await exportSession(await flowsForExport())) showToast("Session saved");
    } catch (e) { showToast(String(e)); }
  }
  async function doExportHar() {
    try {
      if (await exportHar(await flowsForExport())) showToast("HAR exported");
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

  // Clear both sides: the engine keeps its own retained flows, which the MCP
  // server reads. Clearing only the UI would leave an agent looking at traffic
  // the user believes they discarded.
  async function clearAll() {
    clear();
    try {
      await api.clearFlows();
    } catch (e) {
      showToast(String(e));
    }
  }

  const showToast = (t: string, ms?: number) => {
    setToastState(t);
    window.clearTimeout(toastTimer.current);
    toastTimer.current = window.setTimeout(() => setToastState(""), toastDuration(t, ms));
  };

  // Wire the streaming channel + initial status once.
  useEffect(() => {
    /**
     * Snapshots and frames arrive several times per flow and, under load,
     * hundreds of times a second. Each one used to be its own store update —
     * one re-render of the whole list per message, which is what made a busy
     * capture unusable. Coalescing a frame's worth into a single update caps the
     * render rate at the display's, however fast traffic is.
     */
    let flowQueue: Flow[] = [];
    let wsQueue: WsMessage[] = [];
    let frame = 0;
    // One flush for both channels, snapshots first: the store drops frames of
    // flows it does not hold, so a socket's first frames must never be applied
    // ahead of the snapshot that introduces their flow.
    const flush = () => {
      frame = 0;
      const flows = flowQueue;
      const ws = wsQueue;
      flowQueue = [];
      wsQueue = [];
      if (flows.length) useStore.getState().upsertFlows(flows);
      if (ws.length) useStore.getState().addWsMessages(ws);
    };
    const schedule = () => {
      if (!frame) frame = requestAnimationFrame(flush);
    };

    const channel = new Channel<Flow>();
    channel.onmessage = (flow) => {
      flowQueue.push(flow);
      schedule();
    };
    api.subscribeFlows(channel);

    const bpChannel = new Channel<Interception>();
    bpChannel.onmessage = (i) => {
      setIntercept(i);
      setBpArmed(false); // one-shot: the backend disarmed on this hit
    };
    api.subscribeBreakpoints(bpChannel);

    const wsChannel = new Channel<WsMessage>();
    wsChannel.onmessage = (m) => {
      wsQueue.push(m);
      schedule();
    };
    api.subscribeWs(wsChannel);

    api.proxyStatus().then((p) => useStore.getState().setProxy(p));
    api.caStatus().then((c) => useStore.getState().setCa(c)).catch(() => {});
    api.getRules().then(setRulesState).catch(() => {});
    api.getScript().then((s) => { if (s.trim()) setScriptSource(s); }).catch(() => {});
    api.getNetworkConditions().then(setNet).catch(() => {});
    api.mcpStatus().then(setMcp).catch(() => {});
    api.helperStatus().then(setHelper).catch(() => {});

    return () => {
      if (frame) cancelAnimationFrame(frame);
    };
  }, []);

  // "System proxy at launch" — off unless the user asked for it, because it
  // rewrites an OS setting they depend on for working internet. Without the
  // privileged helper this is also the one path that can still raise a password
  // prompt at startup, which is why Settings says so.
  useEffect(() => {
    if (prefs.systemProxyAtLaunch !== "system") return;
    let cancelled = false;
    (async () => {
      try {
        const now = await api.proxyStatus();
        if (cancelled || now.system_proxy) return;
        useStore.getState().setProxy(await api.setSystemProxy(true));
        showToast("System proxy enabled (launch default)");
      } catch (e) {
        if (!cancelled) showToast(String(e));
      }
    })();
    return () => { cancelled = true; };
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

  // Put back settings a previous session left behind. Deliberately a button and
  // not something the app does by itself: without the helper the OS asks for a
  // password, and that dialog should never appear unasked.
  async function restorePrevious() {
    try {
      useStore.getState().setProxy(await api.restoreSystemProxy());
      showToast("Previous proxy settings restored");
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

  async function copyCurl() {
    if (!selected) return showToast("No flow selected");
    // The list holds no body bytes, so the request body is fetched before the
    // command is written out — a cURL without its `--data` is not the request.
    navigator.clipboard.writeText(buildCurl(await withRequestBody(selected)));
    showToast("cURL copied to clipboard");
  }

  /* command palette */
  const commands: { id: string; icon: IconName; label: string; kbd?: string; run: () => void }[] = useMemo(
    () => [
      { id: "rec", icon: recording ? "circle-pause" : "circle-dot", label: recording ? "Pause capture" : "Resume capture", run: () => setRecording(!recording) },
      { id: "clear", icon: "eraser", label: "Clear all flows", run: () => clear() },
      { id: "proxy", icon: "power", label: proxy.system_proxy ? "Disable system proxy" : "Enable system proxy", run: () => void toggleProxy() },
      { id: "resend", icon: "repeat", label: "Resend selected flow", run: () => void resendSelected() },
      { id: "curl", icon: "copy", label: "Copy selected as cURL", kbd: "↵", run: () => void copyCurl() },
      { id: "save", icon: "download", label: "Save session (.nova)", run: () => void doExportSession() },
      { id: "open", icon: "upload", label: "Open session (.nova)", run: () => void doImportSession() },
      { id: "har", icon: "file-down", label: "Export as HAR", run: () => void doExportHar() },
      { id: "mcponly", icon: "plug", label: chip === "mcp" ? "Show all traffic (clear MCP filter)" : "Show only MCP traffic", run: () => { setChip((c) => (c === "mcp" ? "all" : "mcp")); setSection("flows"); } },
      { id: "bp", icon: "circle-pause", label: "Arm breakpoint on next request", run: () => { armBreakpoint(true); setSection("break"); showToast("Breakpoint armed"); } },
      { id: "rules", icon: "git-branch", label: "Open Rules", run: () => setSection("rules") },
      { id: "scripts", icon: "braces", label: "Open Scripts", run: () => setSection("scripts") },
      { id: "certs", icon: "shield-check", label: "Open Certificate", run: () => setSection("certs") },
    ],
    [recording, proxy.running, proxy.system_proxy, chip, selected],
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
  const apps = useMemo(() => distinctApps(flows), [flows]);
  // A filter set from the palette (or from a flow that has since been evicted)
  // may name an app no longer in the capture; keep it listed so the dropdown
  // shows the filter that is actually in force.
  const appFilterItems = useMemo(
    () => (appFilter && !apps.includes(appFilter) ? [appFilter, ...apps] : apps)
      .map((a) => ({ value: a, label: a, icon: "app-window" as const })),
    [apps, appFilter],
  );

  return (
    <div className="nova">
      <div className="body">
        {/* rail */}
        <div className="rail">
          <Brandmark className="rail-logo" />
          {RAIL.map((r) => (
            <div
              key={r.id}
              className={`rail-item ${section === r.id ? "active" : ""}`}
              title={r.label}
              onClick={() => setSection(r.id)}
            >
              <span className="icon"><Icon name={r.icon} size={19} /></span>
              <span className="label">{r.label}</span>
            </div>
          ))}
          <div className="spacer" />
          <div className="rail-gear" title="Settings" onClick={() => setSettingsOpen(true)}>
            <Icon name="settings" size={18} />
          </div>
        </div>

        {/* content */}
        <div className="content">
          {/* header */}
          <div className="toolbar">
            <div className="hd-name">
              <div className="hd-title">{SECTION_TITLE[section]}</div>
              <div className="hd-sub">default workspace · {proxy.host}:{proxy.port}</div>
            </div>
            <div className="tool-sep" />
            <div className={`tool-btn rec-btn ${recording ? "on" : ""}`} onClick={() => setRecording(!recording)}>
              <span className="rec-dot" />
              {recording ? "Recording" : "Paused"}
            </div>
            <div className="tool-btn" onClick={() => void clearAll()}>
              <Icon name="eraser" />
              Clear
            </div>
            {section === "flows" && (
              <Dropdown
                className="dd-app"
                label="Filter by app"
                title="Show only requests from the selected app"
                value={appFilter}
                placeholder="All apps"
                emptyLabel="No app captured yet"
                items={appFilterItems}
                onChange={setAppFilter}
                clearLabel="All apps"
                onClear={appFilter ? () => setAppFilter("") : undefined}
              />
            )}
            <div className="spacer" />
            <div className="cmd-btn" onClick={openPalette}>
              <Icon name="command" size={13} />
              <span>Commands</span>
              <span className="kbd">⌘K</span>
            </div>
            <div className="proxy-toggle" onClick={() => void toggleProxy()}>
              <span>System proxy</span>
              <span className={`switch sm ${proxy.system_proxy ? "on" : ""}`}><span className="knob" /></span>
            </div>
          </div>

          {proxy.pending_restore && !restoreHidden && (
            <div className="restore-bar">
              <span className="rb-icon"><Icon name="triangle-alert" size={15} /></span>
              <span>
                Your system proxy still points at NovaProxy from a session that ended
                unexpectedly{helper?.supported && !helper.running ? " — restoring it needs your password once" : ""}.
              </span>
              <span className="spacer" />
              <button className="tool-btn" onClick={() => void restorePrevious()}>Restore settings</button>
              <span className="rb-x" title="Dismiss" onClick={() => setRestoreHidden(true)}>
                <Icon name="x" size={15} />
              </span>
            </div>
          )}

          {section === "flows" && (
            <FlowsSection
              flows={flows}
              query={query}
              setQuery={setQuery}
              appFilter={appFilter}
              chip={chip}
              setChip={setChip}
              showInternal={showInternal}
              toggleInternal={() => setShowInternal((v) => !v)}
              groupByHost={groupByHost}
              toggleGroup={() => setGroupByHost((v) => !v)}
              recording={recording}
              selected={selected}
              select={select}
              detailTab={detailTab}
              setDetailTab={setDetailTab}
              listWidth={listWidth}
              setListWidth={setListWidth}
              commitListWidth={(w) => setPrefs({ ...prefs, flowListWidth: w })}
              onResend={() => void resendSelected()}
              onCopyCurl={copyCurl}
              openPalette={openPalette}
              showToast={showToast}
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
          {!proxy.running ? "stopped" : recording ? "recording" : "paused"}
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
              <span className="glyph"><Icon name="search" size={15} /></span>
              <input
                autoFocus
                value={paletteQuery}
                onChange={(e) => { setPaletteQuery(e.target.value); setPalIndex(0); }}
                placeholder="Type a command…"
              />
              <span className="esc">ESC</span>
            </div>
            <div className="palette-list">
              {palFiltered.length === 0 && (
                <div className="palette-empty">No command matches “{paletteQuery}”</div>
              )}
              {palFiltered.map((c, i) => (
                <button
                  key={c.id}
                  className={`palette-item ${i === palIndex ? "active" : ""}`}
                  onMouseEnter={() => setPalIndex(i)}
                  onClick={() => runCommand(c)}
                >
                  <span className="picon"><Icon name={c.icon} size={15} /></span>
                  <span className="plabel">{c.label}</span>
                  {c.kbd && <span className="pkbd">{c.kbd}</span>}
                </button>
              ))}
            </div>
          </div>
        </>
      )}

      {/* settings modal */}
      {settingsOpen && (
        <SettingsModal
          port={proxy.port ?? 9090} ca={ca}
          net={net} setNet={saveNet}
          mcp={mcp} setMcp={setMcp}
          helper={helper} setHelper={setHelper}
          prefs={prefs} setPrefs={setPrefs}
          showToast={showToast}
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
        <div className="toast"><span className="ok"><Icon name="check" size={16} /></span>{toast}</div>
      )}
    </div>
  );
}

/* ------------------------------ flows section ------------------------------ */

function FlowsSection(props: {
  flows: Flow[];
  query: string;
  setQuery: (q: string) => void;
  appFilter: string;
  chip: FlowChip;
  setChip: (c: FlowChip) => void;
  showInternal: boolean;
  toggleInternal: () => void;
  groupByHost: boolean;
  toggleGroup: () => void;
  recording: boolean;
  selected: Flow | null;
  select: (id: string | null) => void;
  detailTab: DetailTab;
  setDetailTab: (t: DetailTab) => void;
  /** Live width of the flow list while dragging. */
  listWidth: number;
  setListWidth: (w: number) => void;
  /** Called once at the end of a drag, so a drag writes one preference, not 200. */
  commitListWidth: (w: number) => void;
  onResend: () => void;
  onCopyCurl: () => void;
  openPalette: () => void;
  showToast: (t: string) => void;
}) {
  const { flows, query, appFilter, chip, showInternal, groupByHost, selected, select } = props;
  const splitRef = useRef<HTMLDivElement | null>(null);

  const filtered = useMemo(
    () =>
      filterFlows(flows, query, {
        app: appFilter,
        chip,
        includeInternal: showInternal,
      }),
    [flows, query, appFilter, chip, showInternal],
  );

  const stats = useMemo(() => flowStats(flows, filtered), [flows, filtered]);
  // Recomputed whenever the capture changes rather than on a timer: an idle
  // proxy should not repaint the sparkline once a second forever.
  const spark = useMemo(() => {
    const series = throughputSeries(flows, Date.now());
    return { ...sparkPath(series, 220, 46), rate: throughputRate(series, SPARK_WINDOW_MS) };
  }, [flows]);
  // How much of the capture is NovaProxy's own doing, so the count can be
  // surfaced rather than silently swallowed.
  const internalCount = useMemo(() => flows.filter((f) => f.internal).length, [flows]);

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

  // An empty list has three causes, and they want three different sentences —
  // telling someone to loosen a filter they never set is worse than saying
  // nothing.
  const filtering = query.trim() !== "" || chip !== "all" || appFilter !== "";
  const empty: { icon: IconName; msg: string; hint: string } | null =
    filtered.length > 0
      ? null
      : flows.length === 0
      ? props.recording
        ? { icon: "activity", msg: "Waiting for traffic…", hint: "Flows land here as your apps make requests." }
        : { icon: "circle-pause", msg: "Recording paused", hint: "Nothing is being captured right now." }
      : filtering
      ? { icon: "search-x", msg: "No flows match", hint: `${flows.length} captured, none matching. Try a shorter filter.` }
      : { icon: "search-x", msg: "Nothing to show", hint: "Every captured flow is hidden." };

  /**
   * Drag the divider. Pointer capture (rather than window listeners) is what
   * keeps the drag alive when the cursor outruns the handle or leaves the
   * window, and it releases itself if the pointer is lost.
   */
  const startDrag = (e: React.PointerEvent<HTMLDivElement>) => {
    e.preventDefault();
    const handle = splitRef.current;
    const listEl = handle?.previousElementSibling as HTMLElement | null;
    const detailEl = handle?.nextElementSibling as HTMLElement | null;
    if (!handle || !listEl || !detailEl) return;
    // Measure from the list's own edge, not the row's: the row is padded, so
    // the two are not the same point and the cursor would drift off the handle.
    const left = listEl.getBoundingClientRect().left;
    // Never let the inspector be squeezed out of existence, however wide the
    // list is allowed to be in isolation.
    const roomForDetail =
      detailEl.getBoundingClientRect().right - left - handle.offsetWidth - 360;
    handle.setPointerCapture(e.pointerId);

    const onMove = (ev: PointerEvent) =>
      props.setListWidth(Math.min(clampListWidth(ev.clientX - left), Math.max(MIN_LIST_WIDTH, roomForDetail)));
    const onUp = (ev: PointerEvent) => {
      handle.removeEventListener("pointermove", onMove);
      handle.removeEventListener("pointerup", onUp);
      handle.removeEventListener("pointercancel", onUp);
      handle.releasePointerCapture(ev.pointerId);
      props.commitListWidth(Math.min(clampListWidth(ev.clientX - left), Math.max(MIN_LIST_WIDTH, roomForDetail)));
    };
    handle.addEventListener("pointermove", onMove);
    handle.addEventListener("pointerup", onUp);
    handle.addEventListener("pointercancel", onUp);
  };

  /** Keyboard resizing, so the divider is not mouse-only. */
  const nudge = (e: React.KeyboardEvent<HTMLDivElement>) => {
    const step = e.shiftKey ? 48 : 12;
    const delta = e.key === "ArrowLeft" ? -step : e.key === "ArrowRight" ? step : 0;
    if (!delta) return;
    e.preventDefault();
    const next = clampListWidth(props.listWidth + delta);
    props.setListWidth(next);
    props.commitListWidth(next);
  };

  return (
    <div className="flows-wrap">
      <div className="stat-row">
        <div className="stat">
          <div className="k"><span className="icon"><Icon name="list" size={15} /></span>Flows</div>
          <div className="row">
            <span className="v">{stats.visible}</span>
            <span className="u">of {stats.total}</span>
          </div>
        </div>
        <div className="stat green">
          <div className="k"><span className="icon"><Icon name="gauge" size={15} /></span>Median</div>
          <div className="row">
            <span className="v">{stats.medianMs != null ? formatDuration(stats.medianMs) : "—"}</span>
            <span className="u">ms</span>
          </div>
        </div>
        <div className="stat red">
          <div className="k"><span className="icon"><Icon name="triangle-alert" size={15} /></span>Failed</div>
          <div className="row">
            <span className="v">{stats.failed}</span>
            <span className="u">4xx / 5xx</span>
          </div>
        </div>
        <div className="stat violet">
          <div className="k"><span className="icon"><Icon name="plug" size={15} /></span>MCP calls</div>
          <div className="row">
            <span className="v">{stats.mcp}</span>
            <span className="u">tool traffic</span>
          </div>
        </div>
        <div className="stat spark">
          <div className="k">
            <span>Throughput</span>
            <span className="rate">{formatRate(spark.rate)}</span>
          </div>
          <svg viewBox="0 0 220 46" preserveAspectRatio="none" aria-hidden>
            <defs>
              <linearGradient id="npSpark" x1="0" y1="0" x2="0" y2="1">
                <stop offset="0%" stopColor="var(--accent)" stopOpacity="0.32" />
                <stop offset="100%" stopColor="var(--accent)" stopOpacity="0" />
              </linearGradient>
            </defs>
            <path d={spark.area} fill="url(#npSpark)" />
            <path
              d={spark.line}
              fill="none"
              stroke="var(--accent)"
              strokeWidth={2}
              strokeLinecap="round"
              strokeLinejoin="round"
            />
            <circle cx={spark.last.x} cy={spark.last.y} r={3.5} fill="var(--accent)" />
          </svg>
        </div>
      </div>

      <div className="flows">
      <div className="flow-list" style={{ width: props.listWidth }}>
        <div className="flow-list-head">
          <div className="search">
            <span className="mag"><Icon name="search" /></span>
            <input
              value={query}
              onChange={(e) => props.setQuery(e.target.value)}
              placeholder="host, path, method:GET, status:401…"
            />
            {query && (
              <span className="clear" title="Clear filter" onClick={() => props.setQuery("")}>
                <Icon name="x" />
              </span>
            )}
          </div>
          <div className="chip-row">
            {FLOW_CHIPS.map((c) => (
              <div
                key={c.id}
                className={`fchip ${chip === c.id ? "on" : ""}`}
                onClick={() => props.setChip(c.id)}
              >
                {c.label}
              </div>
            ))}
          </div>
          <div className="fl-meta">
            <span>{filtered.length} flow{filtered.length === 1 ? "" : "s"}</span>
            <span className="spacer" />
            {internalCount > 0 && (
              <span
                className="grouptog"
                title="NovaProxy's own MCP endpoint calls and replays"
                onClick={props.toggleInternal}
              >
                <Icon name={showInternal ? "circle-dot" : "circle"} size={12} />
                {internalCount} own
              </span>
            )}
            <span className="grouptog" onClick={props.toggleGroup}>
              <Icon name={groupByHost ? "chevron-down" : "list"} size={12} />
              {groupByHost ? "grouped" : "flat"}
            </span>
          </div>
        </div>
        <FlowList
          groups={groups}
          selectedId={selected?.id ?? null}
          select={select}
          empty={empty}
        />
      </div>

      <div
        ref={splitRef}
        className="splitter"
        role="separator"
        aria-orientation="vertical"
        aria-label="Resize the flow list"
        aria-valuenow={props.listWidth}
        aria-valuemin={MIN_LIST_WIDTH}
        aria-valuemax={MAX_LIST_WIDTH}
        tabIndex={0}
        onPointerDown={startDrag}
        onKeyDown={nudge}
        onDoubleClick={() => {
          props.setListWidth(DEFAULT_PREFS.flowListWidth);
          props.commitListWidth(DEFAULT_PREFS.flowListWidth);
        }}
      >
        <span className="grip" />
      </div>

      <div className="detail">
        {!selected ? (
          <div className="detail-empty">
            <div className="glyph"><Icon name="activity" size={21} /></div>
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
            showToast={props.showToast}
          />
        )}
      </div>
      </div>
    </div>
  );
}

/* ------------------------------ windowed list ------------------------------ */

/** One host's flows, or all of them when the list is flat. */
interface FlowGroup {
  key: string;
  host: string;
  tls: boolean;
  showHeader: boolean;
  flows: Flow[];
}

/** Rows kept rendered beyond each viewport edge, so a fast flick stays covered. */
const OVERSCAN = 8;
/** First-frame estimates only — the real heights are measured from the DOM. */
const ROW_H_GUESS = 54;
const HEADER_H_GUESS = 33;

/**
 * The flow list, windowed.
 *
 * Retention allows `MAX_FLOWS` rows, and rendering them all put well over a
 * hundred thousand nodes in the webview: scrolling stuttered, every snapshot
 * walked the lot, and a long recording session ended with the renderer dying and
 * the UI reloading itself. Only the rows overlapping the viewport are mounted
 * now; `sliceGroups` holds the rest open with spacers so the scrollbar and the
 * host headers behave exactly as they did.
 */
function FlowList({
  groups,
  selectedId,
  select,
  empty,
}: {
  groups: FlowGroup[];
  selectedId: string | null;
  select: (id: string) => void;
  empty: { icon: IconName; msg: string; hint: string } | null;
}) {
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const [scrollTop, setScrollTop] = useState(0);
  const [viewportH, setViewportH] = useState(0);
  const [rowH, setRowH] = useState(ROW_H_GUESS);
  const [headerH, setHeaderH] = useState(HEADER_H_GUESS);

  // Measure the viewport in a layout effect, so the first paint is already
  // windowed, and observe it: a window resize or a divider drag changes how many
  // rows fit.
  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const sync = () => {
      setViewportH(el.clientHeight);
      setScrollTop(el.scrollTop);
    };
    sync();
    const ro = new ResizeObserver(sync);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  /**
   * Row and header heights are read from the DOM rather than hard-coded: they
   * follow the font, and a window whose arithmetic disagrees with the layout
   * drifts. The fractional rect height is what makes the spacers add up exactly.
   */
  const measure = (current: number, set: (h: number) => void) => (el: HTMLElement | null) => {
    if (!el) return;
    const h = el.getBoundingClientRect().height;
    if (h > 0 && Math.abs(h - current) > 0.5) set(h);
  };

  const hasHeaders = groups.length > 0 && groups[0].showHeader;
  const slices = sliceGroups(
    groups.map((g) => g.flows.length),
    { rowH, headerH: hasHeaders ? headerH : 0, overscan: OVERSCAN },
    scrollTop,
    viewportH,
  );
  // Measure against the first group that is actually on screen.
  const firstOnScreen = slices.findIndex((s) => s.onScreen);

  return (
    <div
      className="flow-scroll"
      ref={scrollRef}
      onScroll={(e) => setScrollTop(e.currentTarget.scrollTop)}
    >
      {empty && (
        <div className="list-empty">
          <div className="icon"><Icon name={empty.icon} size={26} /></div>
          <div className="big">{empty.msg}</div>
          <div>{empty.hint}</div>
        </div>
      )}
      {groups.map((g, gi) => {
        const s = slices[gi];
        // An off-screen group is one spacer: no header, no rows, no cost.
        if (!s.onScreen) return <div key={g.key} style={{ height: s.height }} />;
        return (
          <div key={g.key}>
            {g.showHeader && (
              <div
                className="group-head"
                ref={gi === firstOnScreen ? measure(headerH, setHeaderH) : undefined}
              >
                <span className="hdot" />
                <span className="hname">{g.host}</span>
                {g.tls && <span className="tls-chip">TLS</span>}
                <span className="spacer" />
                <span className="hcount">{g.flows.length}</span>
              </div>
            )}
            {s.padTop > 0 && <div style={{ height: s.padTop }} />}
            {g.flows.slice(s.from, s.to).map((f, i) => (
              <FlowRow
                key={f.id}
                flow={f}
                selected={f.id === selectedId}
                select={select}
                measure={gi === firstOnScreen && i === 0 ? measure(rowH, setRowH) : undefined}
              />
            ))}
            {s.padBottom > 0 && <div style={{ height: s.padBottom }} />}
          </div>
        );
      })}
    </div>
  );
}

/** One row. Memoised: a snapshot for one flow must not re-render its neighbours. */
const FlowRow = memo(function FlowRow({
  flow: f,
  selected,
  select,
  measure,
}: {
  flow: Flow;
  selected: boolean;
  select: (id: string) => void;
  measure?: (el: HTMLElement | null) => void;
}) {
  return (
    <button
      ref={measure}
      className={`flow-row ${selected ? "sel" : ""}`}
      onClick={() => select(f.id)}
    >
      <span className={`badge ${methodClass(f.method)}`}>{f.method}</span>
      <span className="col">
        <div className="fpath">{f.mcp ? <span className="fmcp">{mcpLabel(f)}</span> : f.path}</div>
        <div className="fsub">
          {f.mapped_from && <span className="fmap" title={`mapped from ${f.mapped_from}`}><Icon name="git-branch" size={11} /></span>}
          {f.host}
          {f.mcp && <span className="fdim"> · {f.path}</span>}
          {f.resent && <span className="fresent"> · resent</span>}
          {f.internal && <span className="fdim"> · NovaProxy</span>}
        </div>
      </span>
      <span className="fright">
        <div className={`fstatus ${statusClass(f.status, f.error)}`}>
          {statusText(f.status, f.error)}
        </div>
        <div className="ftime">{f.duration_ms != null ? formatMs(f.duration_ms) : "—"}</div>
      </span>
    </button>
  );
});

const DETAIL_TABS: { id: DetailTab; label: string }[] = [
  { id: "overview", label: "Overview" },
  { id: "request", label: "Request" },
  { id: "response", label: "Response" },
  { id: "timing", label: "Timing" },
  { id: "curl", label: "cURL" },
];

function Detail({
  flow, tab, setTab, onResend, onCopyCurl, showToast,
}: {
  flow: Flow;
  tab: DetailTab;
  setTab: (t: DetailTab) => void;
  onResend: () => void;
  onCopyCurl: () => void;
  showToast: (t: string) => void;
}) {
  const wsMessages = useStore((s) => s.wsMessages[flow.id]);
  const wsDropped = useStore((s) => s.wsDropped[flow.id] ?? 0);
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
    { k: "Duration", v: flow.duration_ms != null ? `${formatDuration(flow.duration_ms)} ms` : "—" },
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
          <div className="resend" onClick={onResend}>
            <Icon name="repeat" size={13} />
            Resend
          </div>
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
            {flow.mcp && (
              <div className="fact-grid">
                <div className="fact"><div className="k">MCP method</div><div className="v">{flow.mcp.method ?? "—"}</div></div>
                <div className="fact"><div className="k">MCP tool</div><div className="v">{flow.mcp.tool ?? "—"}</div></div>
                <div className="fact"><div className="k">JSON-RPC id</div><div className="v">{flow.mcp.id ?? "notification"}</div></div>
                <div className="fact"><div className="k">Transport</div><div className="v">{flow.mcp.transport === "Sse" ? "SSE" : "HTTP"}</div></div>
              </div>
            )}
            <div className="chips">
              {flow.scheme === "https" ? (
                <span className="chip green"><Icon name="lock" size={12} /> TLS · decrypted</span>
              ) : (
                <span className="chip blue">plaintext</span>
              )}
              <span className="chip blue">{flow.http_version}</span>
              {flow.is_websocket && <span className="chip cyan"><Icon name="activity" size={12} /> WebSocket</span>}
              {flow.tunneled && <span className="chip amber"><Icon name="arrow-up-down" size={12} /> tunneled · not decrypted</span>}
              {flow.mapped_from && <span className="chip violet"><Icon name="git-branch" size={12} /> mapped from {flow.mapped_from}</span>}
              {flow.mcp && <span className="chip violet"><Icon name="plug" size={12} /> MCP · {mcpLabel(flow)}</span>}
              {flow.internal && <span className="chip amber">NovaProxy's own traffic</span>}
              {flow.resent && <span className="chip cyan"><Icon name="repeat" size={12} /> resent</span>}
              {flow.error && <span className="chip red"><Icon name="triangle-alert" size={12} /> {flow.error}</span>}
            </div>
          </>
        )}

        {tab === "request" && (
          <>
            <div className="sec-label">Request headers</div>
            {flow.request_headers.length === 0 ? (
              <div className="hlist-empty">— no headers —</div>
            ) : (
              <div className="hlist">
                {flow.request_headers.map((h, i) => (
                  <div className="hrow" key={i}><span className="hk">{h.name}</span><span className="hv">{h.value}</span></div>
                ))}
              </div>
            )}
            <div className="sec-label">Body</div>
            <BodyBlock body={flow.request_body} kind="req" flowId={flow.id} showToast={showToast} />
          </>
        )}

        {tab === "response" && (
          <>
            <div className="sec-label">Response headers</div>
            {flow.response_headers.length === 0 ? (
              <div className="hlist-empty">— no headers —</div>
            ) : (
              <div className="hlist">
                {flow.response_headers.map((h, i) => (
                  <div className="hrow" key={i}><span className="hk">{h.name}</span><span className="hv">{h.value}</span></div>
                ))}
              </div>
            )}
            <div className="sec-label meta">
              Body
              <span className="metaval">{(flow.content_type ?? "—")} · {formatBytes(flow.response_size)}</span>
            </div>
            <BodyBlock body={flow.response_body} kind="res" status={flow.status} flowId={flow.id} showToast={showToast} />
          </>
        )}

        {tab === "timing" && <TimingPanel flow={flow} />}

        {tab === "curl" && <CurlPanel flow={flow} onCopy={onCopyCurl} showToast={showToast} />}

        {tab === "ws" && <WsPanel key={flow.id} messages={wsMessages} dropped={wsDropped} />}
      </div>
    </>
  );
}

/**
 * Waterfall of the phases the engine actually measured. Every bar here comes
 * from an instrumented timer — phases that did not happen (no DNS lookup for an
 * IP literal, no handshake on plain HTTP) or could not be attributed (a reused
 * connection) are stated as such instead of being drawn.
 */
function TimingPanel({ flow }: { flow: Flow }) {
  const b = timingBreakdown(flow);

  if (b.empty) {
    return (
      <div className="timing">
        <div className="timing-note">
          No timing was measured for this flow.
          {flow.tunneled
            ? " It was tunneled without decryption, so only the CONNECT is visible."
            : flow.state === "Started"
            ? " It is still in flight."
            : " Rule-served and imported flows carry no measurements."}
        </div>
      </div>
    );
  }

  return (
    <div className="timing">
      {b.phases.map((p) => (
        <div className="timing-row" key={p.key}>
          <span className="tl">{p.label}</span>
          <div className="timing-bar">
            <span
              style={{
                left: `${(p.startMs / b.spanMs) * 100}%`,
                // Keep a hairline visible for phases that rounded to ~0ms.
                width: `${Math.max((p.ms / b.spanMs) * 100, 0.5)}%`,
                background: p.color,
              }}
            />
          </div>
          <span className="tv">{formatMs(p.ms)}</span>
        </div>
      ))}

      {b.reused && (
        <div className="timing-note">
          Reused an open connection — no DNS, connect or TLS cost belongs to this request.
        </div>
      )}
      {b.requestMs != null && (
        <div className="timing-note">
          Request body streamed upstream in {formatMs(b.requestMs)} ({formatBytes(flow.request_size)}).
        </div>
      )}

      <div className="timing-total">
        <span>Total</span>
        <span className="mono">{b.totalMs != null ? formatMs(b.totalMs) : "in flight"}</span>
      </div>
      <div className="timing-total">
        <span>Transferred</span>
        <span className="mono">{formatBytes(num(flow.request_size) + num(flow.response_size))}</span>
      </div>
      <div className="timing-total">
        <span>Started</span>
        <span className="mono">{formatAgo(flow.started_at)}</span>
      </div>
    </div>
  );
}

/**
 * Frames rendered at once. A busy socket fills its retention window in seconds,
 * and every frame is a DOM row — the rest stay one click away rather than being
 * mounted where nobody is looking.
 */
const WS_PAGE = 400;

function WsPanel({ messages, dropped }: { messages: WsMessage[] | undefined; dropped: number }) {
  const [showAll, setShowAll] = useState(false);
  if (!messages || messages.length === 0) {
    return <pre className="code res">— no WebSocket frames captured yet —</pre>;
  }
  // Newest frames are the ones being read, so the window is the tail.
  const visible = showAll ? messages : messages.slice(Math.max(0, messages.length - WS_PAGE));
  const earlier = messages.length - visible.length;
  return (
    <>
      {(dropped > 0 || earlier > 0) && (
        <div className="ws-note">
          {dropped > 0 && (
            <span>
              {dropped.toLocaleString()} earlier frame{dropped === 1 ? "" : "s"} dropped at the{" "}
              {MAX_WS_FRAMES.toLocaleString()}-frame cap.
            </span>
          )}
          {earlier > 0 && (
            <span className="ws-more" onClick={() => setShowAll(true)}>
              Show {earlier.toLocaleString()} earlier retained frame{earlier === 1 ? "" : "s"}
            </span>
          )}
        </div>
      )}
      <div className="ws-log">
      {visible.map((m) => {
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
            <span className={`ws-dir ${sent ? "sent" : "recv"}`}>
              <Icon name={sent ? "arrow-up" : "arrow-down"} size={11} />
              {sent ? "sent" : "recv"}
            </span>
            <span className="ws-op">{label}</span>
            <span className="ws-payload">{payload}{m.truncated ? " …(truncated)" : ""}</span>
            <span className="ws-meta">{formatBytes(m.size)} · {formatAgo(m.at)}</span>
          </div>
        );
      })}
      </div>
    </>
  );
}

/**
 * The cURL tab. Its own component so the request body is fetched when the tab is
 * actually open, rather than on every flow selection.
 */
function CurlPanel({
  flow,
  onCopy,
  showToast,
}: {
  flow: Flow;
  onCopy: () => void;
  showToast: (t: string) => void;
}) {
  const { shown, loading, fetched } = useBodyBytes(flow.id, "request", flow.request_body, showToast);
  return (
    <>
      <div className="sec-label meta">
        Export as cURL
        <span className="copy" onClick={onCopy}>Copy</span>
      </div>
      <pre className="code curl">
        {buildCurl(loading && !fetched ? flow : { ...flow, request_body: shown })}
      </pre>
    </>
  );
}

function BodyBlock({
  body, kind, status, flowId, showToast,
}: {
  body: Flow["request_body"];
  kind: "req" | "res";
  status?: number | null;
  flowId: string;
  showToast?: (t: string) => void;
}) {
  // Bodies are not held in the list. The bytes of the one on screen are fetched
  // here — from the on-disk store when the body was too large to preview in
  // full, otherwise from the flow the engine retains.
  const side = kind === "req" ? "request" : "response";
  const { shown, fetched: full, loading, put, setLoading } = useBodyBytes(flowId, side, body, showToast);

  if (!shown) {
    return <pre className={`code ${kind}`}>{status === 204 ? "— no content (204) —" : "— no body —"}</pre>;
  }
  if (loading && !full) {
    return <pre className={`code ${kind}`}>Loading body ({formatBytes(shown.size)})…</pre>;
  }

  async function loadFull() {
    if (!body) return;
    setLoading(true);
    try {
      put(await api.readBody(flowId, side, body.media_type, body.decoded_from));
    } catch (e) {
      showToast?.(String(e));
    } finally {
      setLoading(false);
    }
  }

  const loadMore =
    body?.spilled && !full ? (
      <div className="body-more" onClick={() => !loading && loadFull()}>
        {loading ? "Loading…" : `Load full body (${formatBytes(body.size)})`}
      </div>
    ) : null;

  const ct = (shown.media_type ?? "").toLowerCase();
  if (shown.base64 && ct.startsWith("image/")) {
    return (
      <div className={`code ${kind}`}>
        <img src={`data:${shown.media_type};base64,${shown.base64}`} alt="body preview" />
        {loadMore}
      </div>
    );
  }
  if (shown.base64) {
    return (
      <>
        <pre className={`code ${kind}`}>Binary body — {formatBytes(shown.size)} ({shown.media_type ?? "unknown"}){shown.truncated ? ", truncated" : ""}</pre>
        {loadMore}
      </>
    );
  }
  const text = bodyToText(shown);
  return (
    <>
      <pre className={`code ${kind}`}>
        {text ?? "— empty body —"}
        {shown.truncated
          ? shown.spilled && !full
            ? "\n… preview truncated — the full body is stored on disk"
            : "\n… truncated at the capture cap"
          : ""}
      </pre>
      {loadMore}
    </>
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
          <div className="btn-primary" onClick={addRule}>
            <Icon name="plus" />
            New rule
          </div>
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
              <span className="rule-del" title="Delete rule" onClick={() => del(r.id)}><Icon name="trash-2" size={15} /></span>
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
          <div className={`bp-icon ${armed ? "armed" : ""}`}><Icon name="circle-pause" size={21} /></div>
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
          <div className="btn-primary" onClick={() => onResume(true, parseHeaders())}>Continue<Icon name="arrow-right" size={13} /></div>
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
          <div className="editor-tab">
            <span className="icon"><Icon name="file-code" /></span>
            tamper.js
          </div>
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

/**
 * Human-readable trust state. Four states are reachable because the CA can be
 * trusted for this user, for the whole machine, both, or neither.
 */
/**
 * What the user trust domain actually covers, which differs by platform: on
 * Linux there is no per-user OpenSSL store, so a user-domain install reaches
 * browsers only and `curl`/Python/Go still reject our leaf certs.
 */
export function userDomainLabel(platform: string | undefined): string {
  return platform === "linux" ? "browsers" : "this user";
}

export function trustLabel(ca: CaStatus | null): { text: string; kind: "trusted" | "untrusted" } {
  if (!ca?.trusted) return { text: "Not installed", kind: "untrusted" };
  const user = userDomainLabel(ca.platform);
  if (ca.trusted_user && ca.trusted_system) return { text: `Trusted · ${user} + all users`, kind: "trusted" };
  if (ca.trusted_system) return { text: "Trusted · all users", kind: "trusted" };
  return { text: `Trusted · ${user}`, kind: "trusted" };
}

/** How each platform describes the no-admin install and what it costs. */
export function trustHint(ca: CaStatus): string {
  if (ca.trusted && ca.trusted_system) {
    return "Trusted machine-wide: every user account, command-line tool and root-owned daemon accepts it.";
  }
  if (ca.trusted) {
    return ca.platform === "linux"
      ? "Trusted in your browser certificate databases only — no password was needed. Command-line tools (curl, Python, Go) will still reject it until you install for all users."
      : "Trusted for your login only — no administrator password was needed. Other user accounts and root-owned daemons will not accept it.";
  }
  if (ca.platform === "windows") {
    return "Installing for you writes your personal certificate store and needs no prompt at all. Choose “all users” (one UAC prompt) if other accounts or services need to trust it.";
  }
  if (ca.platform === "linux") {
    return "Installing for you adds the CA to your browser certificate databases (Chrome, Firefox) — no password needed, but command-line tools are not covered. Choose “all users” (one polkit prompt) to add a system trust anchor.";
  }
  return "Installing for your user only needs a keychain confirmation, not an administrator password. Choose “all users” if you need root-owned daemons or other accounts to trust it too.";
}

function CertsSection({ ca, showToast }: { ca: CaStatus | null; showToast: (t: string) => void }) {
  const [busy, setBusy] = useState<string | null>(null);
  const setCa = useStore.getState().setCa;

  async function run(kind: "install" | "install-all" | "uninstall" | "regen") {
    setBusy(kind);
    try {
      const next =
        kind === "install" ? await api.installCa()
        : kind === "install-all" ? await api.installCa(true)
        : kind === "uninstall" ? await api.uninstallCa()
        : await api.regenerateCa();
      setCa(next);
      showToast(
        kind === "install" ? "Certificate installed & trusted for your user"
        : kind === "install-all" ? "Certificate installed & trusted for all users"
        : kind === "uninstall" ? "Certificate removed"
        : "Root CA regenerated",
      );
    } catch (e) {
      showToast(String(e));
    } finally {
      setBusy(null);
    }
  }

  const trusted = !!ca?.trusted;
  const status = trustLabel(ca);

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
                <div className={`cert-icon ${trusted ? "trusted" : ""}`}><Icon name="shield-check" size={24} /></div>
                <div style={{ flex: 1 }}>
                  <div className="cert-name">{ca.subject || "NovaProxy Root CA"}</div>
                  <div className="cert-fp">SHA-256 · {ca.fingerprint}</div>
                </div>
                <span className={`cert-status ${status.kind}`}>{status.text}</span>
              </div>
              <div className="cert-meta">
                <div><div className="k">Path</div><div className="v">{ca.cert_path}</div></div>
              </div>
              <div className="cert-actions">
                {!trusted ? (
                  <>
                    <div className="btn-primary" onClick={() => !busy && run("install")}><Icon name="shield-check" />{busy === "install" ? "Installing…" : "Install & trust"}</div>
                    <div className="btn-neutral" onClick={() => !busy && run("install-all")}><Icon name="users" />{busy === "install-all" ? "Installing…" : "Install for all users"}</div>
                  </>
                ) : (
                  <>
                    <div className="btn-primary red" onClick={() => !busy && run("uninstall")}><Icon name="trash-2" />{busy === "uninstall" ? "Removing…" : "Remove certificate"}</div>
                    {!ca.trusted_system && (
                      <div className="btn-neutral" onClick={() => !busy && run("install-all")}><Icon name="users" />{busy === "install-all" ? "Installing…" : "Also trust for all users"}</div>
                    )}
                  </>
                )}
                <div className="btn-neutral" onClick={() => { navigator.clipboard.writeText(ca.cert_path); showToast("Certificate path copied"); }}><Icon name="download" />Export .pem</div>
                <div className="btn-neutral" onClick={() => !busy && run("regen")}><Icon name="refresh-cw" />{busy === "regen" ? "Regenerating…" : "Regenerate CA"}</div>
              </div>
              <div className="cert-hint">{trustHint(ca)}</div>
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
      <div className="sec-label meta">
        SSL Proxying scope
        {dirty && <span className="copy" onClick={save}>Save</span>}
      </div>
      <p className="page-sub">
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

type SettingsTab = "general" | "network" | "mcp" | "setup";

const SETTINGS_TABS: { id: SettingsTab; label: string }[] = [
  { id: "general", label: "General" },
  { id: "network", label: "Network" },
  { id: "mcp", label: "MCP" },
  { id: "setup", label: "Getting started" },
];

function SettingsModal({
  port, ca, net, setNet, mcp, setMcp,
  helper, setHelper, prefs, setPrefs, showToast, onClose,
}: {
  port: number;
  ca: CaStatus | null;
  net: NetworkConditions;
  setNet: (n: NetworkConditions) => void;
  mcp: McpStatus | null;
  setMcp: (m: McpStatus) => void;
  helper: HelperStatus | null;
  setHelper: (h: HelperStatus) => void;
  prefs: Prefs;
  setPrefs: (p: Prefs) => void;
  showToast: (t: string) => void;
  onClose: () => void;
}) {
  const [tab, setTab] = useState<SettingsTab>("general");
  return (
    <>
      <div className="scrim modal-scrim" onClick={onClose}>
        <div className="modal" onClick={(e) => e.stopPropagation()}>
          <div className="modal-head">
            <h2>Settings</h2>
            <span className="modal-x" title="Close" onClick={onClose}><Icon name="x" size={16} /></span>
          </div>
          <div className="modal-tabs">
            {SETTINGS_TABS.map((t) => (
              <div
                key={t.id}
                className={`mtab ${tab === t.id ? "active" : ""}`}
                onClick={() => setTab(t.id)}
              >
                {t.label}
              </div>
            ))}
          </div>
          <div className="modal-body">
          {tab === "general" && (
            <GeneralTab prefs={prefs} setPrefs={setPrefs} helper={helper} setHelper={setHelper} showToast={showToast} />
          )}

          {tab === "network" && (<>
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
          </>)}

          {tab === "mcp" && (<>
            <h3>MCP server</h3>
            <McpCard mcp={mcp} setMcp={setMcp} showToast={showToast} />
          </>)}

          {tab === "setup" && (<>
            <h3>1. Route traffic through the proxy</h3>
            <CodeSnippet text={`curl -x http://127.0.0.1:${port} https://example.com`} />

            <h3>2. Trust the root certificate</h3>
            <p>Open the Certs panel and click “Install &amp; trust” so HTTPS decrypts cleanly.</p>
            <CodeSnippet
              text={`export HTTP_PROXY=http://127.0.0.1:${port}
export HTTPS_PROXY=http://127.0.0.1:${port}
export NODE_EXTRA_CA_CERTS="${ca?.cert_path ?? "<ca.pem path>"}"`}
            />
          </>)}
          </div>
        </div>
      </div>
    </>
  );
}

/**
 * The defaults a session starts from, plus the one piece of machinery that
 * decides whether changing the system proxy costs a password.
 */
const GROUPING_ITEMS: DropdownItem[] = [
  { value: "grouped", label: "Grouped by host", icon: "globe" },
  { value: "flat", label: "Flat", icon: "list" },
];

const LAUNCH_ITEMS: DropdownItem[] = [
  { value: "none", label: "None — leave the OS alone", icon: "circle" },
  { value: "system", label: "System proxy — capture everything", icon: "power" },
];

function GeneralTab({
  prefs, setPrefs, helper, setHelper, showToast,
}: {
  prefs: Prefs;
  setPrefs: (p: Prefs) => void;
  helper: HelperStatus | null;
  setHelper: (h: HelperStatus) => void;
  showToast: (t: string) => void;
}) {
  return (
    <>
      <h3>Flow list</h3>
      <div className="pref-row">
        <span className="k">Default grouping</span>
        <Dropdown
          label="Default grouping"
          value={prefs.flowGrouping}
          items={GROUPING_ITEMS}
          onChange={(v) => setPrefs({ ...prefs, flowGrouping: v as Prefs["flowGrouping"] })}
        />
      </div>
      <p>
        How the list opens. The <b>grouped / flat</b> control above the list still switches the
        current session without changing this default.
      </p>

      <h3>System proxy</h3>
      <div className="pref-row">
        <span className="k">At launch</span>
        <Dropdown
          label="System proxy at launch"
          value={prefs.systemProxyAtLaunch}
          items={LAUNCH_ITEMS}
          onChange={(v) => setPrefs({ ...prefs, systemProxyAtLaunch: v as Prefs["systemProxyAtLaunch"] })}
        />
      </div>
      <p>
        <b>None</b> is the default: pointing the OS at NovaProxy rewrites a setting the whole
        machine depends on for working internet, so it should be a deliberate act.
        {helper?.supported && !helper.running && (
          <> Choosing <b>System proxy</b> without the helper below means macOS asks for your
          password on every launch.</>
        )}
      </p>

      {helper?.supported && <HelperCard helper={helper} setHelper={setHelper} showToast={showToast} />}
    </>
  );
}

/**
 * Install (or remove) the privileged helper.
 *
 * macOS needs root to change the system proxy, and the app used to buy that
 * privilege one `osascript` prompt at a time — including during launch, when
 * recovering from an unclean exit. The helper turns that into a single prompt,
 * once, ever.
 */
function HelperCard({
  helper, setHelper, showToast,
}: {
  helper: HelperStatus;
  setHelper: (h: HelperStatus) => void;
  showToast: (t: string) => void;
}) {
  const [busy, setBusy] = useState(false);
  const stale = helper.running && helper.version !== helper.expected_version;

  async function run(action: "install" | "remove") {
    setBusy(true);
    try {
      const next = action === "install" ? await api.installHelper() : await api.uninstallHelper();
      setHelper(next);
      showToast(next.running ? "Helper installed — no more password prompts" : "Helper removed");
    } catch (e) {
      showToast(String(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <>
      <h3>Privileged helper</h3>
      <div className="field-group">
        <span className={`dot ${helper.running && !stale ? "ok" : "warn"}`} />
        <span style={{ color: "var(--text2)" }}>
          {busy
            ? "Working…"
            : stale
              ? `Installed, but speaks protocol ${helper.version} — reinstall to update`
              : helper.running
                ? "Installed — proxy changes apply silently"
                : "Not installed — every proxy change asks for your password"}
        </span>
      </div>
      <p>
        A small background service that applies system-proxy changes for you. Installing it costs
        one administrator password; after that, turning the proxy on or off — and putting your
        settings back after a crash — happens without a prompt.
      </p>
      <div className="cert-actions">
        {(!helper.running || stale) && (
          <div
            className={`btn-primary ${busy || !helper.installable ? "disabled" : ""}`}
            onClick={() => !busy && helper.installable && void run("install")}
          >
            {stale ? "Reinstall helper" : "Install helper"}
          </div>
        )}
        {helper.running && (
          <div className="btn-neutral" onClick={() => !busy && void run("remove")}>Remove helper</div>
        )}
      </div>
      {!helper.installable && (
        <p className="warn-note">
          No helper binary was found next to the app. In a development tree, build it first:
          <code> cargo build -p nova-helper</code>.
        </p>
      )}
    </>
  );
}

/**
 * Enable/disable the MCP endpoint and hand the user the one command that wires
 * an agent to it. Off by default: it exposes every captured request, so turning
 * it on is a deliberate act.
 */
function McpCard({
  mcp, setMcp, showToast,
}: {
  mcp: McpStatus | null;
  setMcp: (m: McpStatus) => void;
  showToast: (t: string) => void;
}) {
  const [busy, setBusy] = useState(false);
  const running = !!mcp?.running;
  const port = mcp?.port ?? 9091;
  const url = mcp?.url ?? `http://127.0.0.1:${port}/`;

  async function toggle() {
    setBusy(true);
    try {
      const next = await api.setMcpEnabled(!running, port);
      setMcp(next);
      showToast(next.running ? `MCP server listening on ${next.url}` : "MCP server stopped");
    } catch (e) {
      showToast(String(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <>
      <div className="field-group">
        <div className={`switch ${running ? "on" : ""}`} onClick={() => !busy && void toggle()}>
          <span className="knob" />
        </div>
        <span style={{ color: "var(--text2)" }}>
          {busy ? "Working…" : running ? `Serving on ${url}` : "Off"}
        </span>
      </div>
      <p>
        Lets Claude and other MCP clients read the traffic you have captured — list and search
        flows, inspect bodies, replay requests, set rules. Loopback only, and its own calls are
        marked so they stay out of your flow list.
      </p>
      {running && <CodeSnippet text={`claude mcp add --transport http novaproxy ${url}`} />}
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
