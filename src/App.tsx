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
  type McpStatus,
  type HelperStatus,
  type UpdateProgress,
} from "./api";
import {
  INITIAL_UPDATE_STATE,
  afterCheck,
  afterProgress,
  canAct as canActOnUpdate,
  progressPercent,
  updateSummary,
  type UpdateState,
} from "./update";
import { useStore } from "./store";
import { exportSession, exportHar, importSession } from "./session";
import { EMPTY_FILTER, toastDuration, type FlowFilter } from "./filter";
import { Brandmark } from "./Brandmark";
import { Dropdown, type DropdownItem } from "./Dropdown";
import { Icon, type IconName } from "./icons";
import { formatRate, SPARK_WINDOW_MS, throughputRate, throughputSeries } from "./stats";
import { methodClass } from "./badges";
import { buildCurl, withRequestBody } from "./inspector/curl";
import { FlowsSection, type FlowsHandle } from "./flows/FlowsSection";
import { useShortcuts } from "./useShortcuts";
import { ShortcutsDialog } from "./ShortcutsDialog";
import { formatChord, shortcut } from "./shortcuts";
import { trustHint, trustLabel } from "./trust";
import { launchDecision } from "./onboarding";
import { Coachmark, OnboardingWizard, type CoachTarget } from "./Walkthrough";
import { loadPrefs, savePrefs, type Prefs } from "./prefs";

/* ------------------------------- helpers ------------------------------- */

const num = (n: number | bigint | null | undefined) => (n == null ? 0 : Number(n));

function formatBytes(n: number | bigint) {
  const v = num(n);
  if (!v) return "—";
  if (v < 1024) return `${v} B`;
  if (v < 1024 * 1024) return `${(v / 1024).toFixed(1)} KB`;
  return `${(v / 1024 / 1024).toFixed(2)} MB`;
}


type Section = "flows" | "rules" | "break" | "scripts" | "certs";

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
  /**
   * Everything narrowing the flows table, in one value — the tree's scope, the
   * three chip groups and the search box. One state atom rather than five: they
   * are read together on every capture frame, and a `Reset filters` that has to
   * remember to clear five setters is a bug waiting to happen.
   */
  const [filter, setFilter] = useState<FlowFilter>(EMPTY_FILTER);
  const patchFilter = (p: Partial<FlowFilter>) => setFilter((f) => ({ ...f, ...p }));
  /**
   * The table's own persisted view choices. Held in prefs rather than in state
   * because both outlive the session, and `Auto Select` is reachable from two
   * places (the status bar and Settings) that must not disagree.
   */
  const columns = prefs.columns;
  const autoSelect = prefs.autoSelect;
  /**
   * Rows marked in the table, mirrored up here because the two actions that can
   * act on more than one flow — Copy as cURL and Export as HAR — live at this
   * level. Empty means "act on the current row", which is what they did before
   * multi-select existed.
   */
  const [markedIds, setMarkedIds] = useState<string[]>([]);
  // Which slice of the capture the list shows, and (separately) whether
  // NovaProxy's own MCP/replay traffic is part of it.

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
  const [shortcutsOpen, setShortcutsOpen] = useState(false);
  const [mcp, setMcp] = useState<McpStatus | null>(null);
  const [helper, setHelper] = useState<HelperStatus | null>(null);
  const [update, setUpdate] = useState<UpdateState>(INITIAL_UPDATE_STATE);
  // True when Settings was opened *for* the Updates card — from the menu item,
  // which is otherwise a click that appears to do nothing, because the card sits
  // below three others. Reset when Settings is opened the ordinary way.
  const [revealUpdates, setRevealUpdates] = useState(false);
  const [restoreHidden, setRestoreHidden] = useState(false);

  // First-run walkthrough. `coach` runs after it closes and points at the two
  // controls the wizard just talked about; both anchors have to be refs because
  // one of them lives two components down.
  const [onboardingOpen, setOnboardingOpen] = useState(false);
  const [coach, setCoach] = useState<CoachTarget>(null);
  // Set once the CA and helper statuses have settled. The walkthrough decides
  // from them, and deciding early would flash a wizard over a working install
  // or open it on a step that is already done.
  const [statusProbed, setStatusProbed] = useState(false);
  const recBtnRef = useRef<HTMLDivElement | null>(null);
  /**
   * Focus targets. The table anchors the walkthrough's second coachmark as well
   * as taking focus for row navigation; the two inputs are what ⌘F and ⌘⇧F
   * reach (issues/0003).
   */
  const flowsRef = useRef<FlowsHandle | null>(null);
  const flowListRef = useRef<HTMLDivElement | null>(null);
  const searchRef = useRef<HTMLInputElement | null>(null);
  const treeFilterRef = useRef<HTMLInputElement | null>(null);
  const onboardingDecided = useRef(false);

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
      const all = await flowsForExport();
      // Marked rows narrow the export; nothing marked exports the capture, which
      // is what the command did before there was a way to mark anything.
      const wanted = markedIds.length > 0 ? new Set(markedIds) : null;
      const flows = wanted ? all.filter((f) => wanted.has(f.id)) : all;
      if (await exportHar(flows)) {
        showToast(wanted ? `HAR exported (${flows.length} marked flows)` : "HAR exported");
      }
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
    // These five hydrate the UI from what the backend persisted. A failure used
    // to be swallowed outright, which showed the user default rules and an
    // empty script as though that were their configuration — the worst kind of
    // silent failure. They still must not toast (nothing is actionable during
    // launch) but they no longer vanish.
    const hydrate = (what: string) => (e: unknown) =>
      api.logUi("warn", "command", `could not load ${what}: ${String(e)}`);
    api.getRules().then(setRulesState).catch(hydrate("rules"));
    api.getScript().then((s) => { if (s.trim()) setScriptSource(s); }).catch(hydrate("script"));
    api.getNetworkConditions().then(setNet).catch(hydrate("network conditions"));
    api.mcpStatus().then(setMcp).catch(hydrate("MCP status"));
    // These two together decide the walkthrough, so they are awaited as a pair
    // — a rejection still counts as settled, since a CA that cannot be read is
    // exactly the install that needs the walkthrough most.
    void Promise.allSettled([
      api.caStatus().then((c) => useStore.getState().setCa(c)),
      api.helperStatus().then(setHelper),
    ]).then(() => setStatusProbed(true));

    return () => {
      if (frame) cancelAnimationFrame(frame);
    };
  }, []);

  // Look for a new version once, at launch. Reports only: a found update waits
  // in Settings until the user clicks install, because replacing the binary ends
  // in a relaunch that would drop whatever is being captured.
  useEffect(() => {
    if (!prefs.autoCheckUpdates) return;
    let cancelled = false;
    api
      .checkUpdate()
      .then((status) => {
        if (cancelled) return;
        setUpdate(afterCheck(status));
        if (status.available) {
          showToast(`NovaProxy ${status.version} is available — install it in Settings › General`);
        }
      })
      // A failed check at launch is not worth a toast: the network is often not
      // up yet, and the user did not ask for this. Settings shows the reason.
      .catch((e) => !cancelled && setUpdate({ ...INITIAL_UPDATE_STATE, phase: "error", error: String(e) }));
    return () => {
      cancelled = true;
    };
    // Launch-only: re-running on every pref flip would check again mid-session.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  /**
   * The check the user asked for — from the card's button or from the menu.
   *
   * Distinct from the launch check above: this one shows that it is working and
   * says so when there is nothing to report, because a request with no visible
   * answer reads as a broken button.
   */
  const checkForUpdates = async () => {
    setUpdate((prev) => ({ ...prev, phase: "checking", error: null }));
    try {
      const status = await api.checkUpdate();
      setUpdate(afterCheck(status));
      if (status.configured && !status.available) showToast("NovaProxy is up to date");
    } catch (e) {
      setUpdate((prev) => ({ ...prev, phase: "error", error: String(e) }));
    }
  };

  // The native menu's "Check for Updates…". The menu only asks; the answer is
  // the Updates card, so Settings opens on it and the check runs from here.
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    void api
      .onMenuCheckUpdates(() => {
        setSettingsOpen(true);
        setRevealUpdates(true);
        void checkForUpdates();
      })
      .then((off) => (cancelled ? off() : (unlisten = off)))
      .catch(() => {
        // No listener means the menu item cannot reach the card. Settings still
        // has its own button, so this is a degraded menu, not a broken app.
      });
    return () => {
      cancelled = true;
      unlisten?.();
    };
    // Subscribe once: the handler only calls setters, which are stable.
    // eslint-disable-next-line react-hooks/exhaustive-deps
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

  // Show the walkthrough to someone who has not seen it — unless their CA is
  // already trusted, which means they were using NovaProxy before this existed
  // and do not need to be taught it. That case records the flag silently rather
  // than greeting every upgrading user with a wizard.
  useEffect(() => {
    if (!statusProbed || onboardingDecided.current) return;
    onboardingDecided.current = true;
    const decision = launchDecision(prefs, useStore.getState().ca);
    if (decision === "open") openOnboarding();
    else if (decision === "mark-done") setPrefs({ ...prefs, onboardingDone: true });
    // Launch-only, and `prefs` is read once at mount by design (see above).
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [statusProbed]);

  // Walk the coachmarks forward on their own: once recording is armed the
  // "press Record" bubble has nothing left to say, and once a flow lands the
  // list has explained itself better than a bubble could.
  useEffect(() => {
    if (coach === "record" && recording) setCoach(flows.length === 0 ? "list" : null);
    else if (coach === "list" && flows.length > 0) setCoach(null);
  }, [coach, recording, flows.length]);

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
    api.trackUi("ui.flow.action", "resend");
    try {
      await api.resendFlow(selected);
      showToast("Request resent through the proxy");
    } catch (e) {
      showToast(String(e));
    }
  }

  const selected = useMemo(() => flows.find((f) => f.id === selectedId) ?? null, [flows, selectedId]);

  /**
   * The flows an action should act on: the marked rows if there are any,
   * otherwise the current one. Marking is additive to the old behaviour rather
   * than a mode — nothing has to be marked for the actions to work.
   */
  function actionTargets(): Flow[] {
    if (markedIds.length === 0) return selected ? [selected] : [];
    const byId = new Map(flows.map((f) => [f.id, f]));
    return markedIds.map((id) => byId.get(id)).filter((f): f is Flow => f != null);
  }

  async function copyCurl() {
    const targets = actionTargets();
    if (targets.length === 0) return showToast("No flow selected");
    api.trackUi("ui.flow.action", "copy_curl");
    // The list holds no body bytes, so the request body is fetched before the
    // command is written out — a cURL without its `--data` is not the request.
    const cmds = await Promise.all(targets.map(async (f) => buildCurl(await withRequestBody(f))));
    navigator.clipboard.writeText(cmds.join("\n\n"));
    showToast(targets.length === 1 ? "cURL copied to clipboard" : `${targets.length} cURLs copied`);
  }

  /* command palette */
  /** True when the type group holds exactly the MCP chip — what the palette toggles. */
  const mcpOnly = filter.type.size === 1 && filter.type.has("mcp");

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
      { id: "mcponly", icon: "plug", label: mcpOnly ? "Show all traffic (clear MCP filter)" : "Show only MCP traffic", run: () => { patchFilter({ type: mcpOnly ? new Set() : new Set(["mcp"]) }); goSection("flows"); } },
      { id: "bp", icon: "circle-pause", label: "Arm breakpoint on next request", run: () => { armBreakpoint(true); goSection("break"); showToast("Breakpoint armed"); } },
      { id: "rules", icon: "git-branch", label: "Open Rules", run: () => goSection("rules") },
      { id: "scripts", icon: "braces", label: "Open Scripts", run: () => goSection("scripts") },
      { id: "certs", icon: "shield-check", label: "Open Certificate", run: () => goSection("certs") },
      { id: "walkthrough", icon: "play", label: "Show the getting-started walkthrough", run: () => { setSettingsOpen(false); setCoach(null); openOnboarding(); } },
    ],
    [recording, proxy.running, proxy.system_proxy, mcpOnly, selected],
  );
  const palFiltered = useMemo(() => {
    const q = paletteQuery.toLowerCase();
    return commands.filter((c) => c.label.toLowerCase().includes(q));
  }, [commands, paletteQuery]);

  /* usage counting — one wrapper per thing worth counting, so the tracking
     lives in a single place instead of at every click that reaches it. Only
     fixed identifiers are ever passed; see `api.trackUi`. */
  const goSection = (id: Section) => { setSection(id); api.trackUi("ui.section", id); };
  const openSettings = () => { setSettingsOpen(true); setRevealUpdates(false); api.trackUi("ui.settings.open"); };
  const openOnboarding = () => { setOnboardingOpen(true); api.trackUi("ui.onboarding", "open"); };

  const openPalette = () => { setPaletteOpen(true); setPaletteQuery(""); setPalIndex(0); api.trackUi("ui.palette.open"); };
  const closePalette = () => setPaletteOpen(false);
  const runCommand = (c: (typeof commands)[number]) => { setPaletteOpen(false); setTimeout(() => c.run(), 0); };

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      // Only the palette's own navigation lives here. Opening it is a chord like
      // any other and belongs to the registry (`src/shortcuts.ts`); this listener
      // exists because ↑/↓/↵ mean something different while the palette is up.
      if (!paletteOpen) return;
      if (e.key === "Escape") { e.preventDefault(); closePalette(); }
      else if (e.key === "ArrowDown") { e.preventDefault(); setPalIndex((i) => Math.min(palFiltered.length - 1, i + 1)); }
      else if (e.key === "ArrowUp") { e.preventDefault(); setPalIndex((i) => Math.max(0, i - 1)); }
      else if (e.key === "Enter") { e.preventDefault(); const c = palFiltered[palIndex]; if (c) runCommand(c); }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [paletteOpen, palFiltered, palIndex]);

  /**
   * Every global chord, in one place, reading its keys from the registry.
   *
   * A modal swallows the lot: while Settings or the palette is up, ⌘1 must not
   * change the section behind it.
   */
  useShortcuts(
    {
      palette: () => (paletteOpen ? closePalette() : openPalette()),
      clear: () => void clearAll(),
      settings: openSettings,
      record: () => setRecording(!recording),
      "section.flows": () => goSection("flows"),
      "section.rules": () => goSection("rules"),
      "section.break": () => goSection("break"),
      "section.scripts": () => goSection("scripts"),
      "section.certs": () => goSection("certs"),
      "session.save": () => void doExportSession(),
      "session.open": () => void doImportSession(),
      "session.har": () => void doExportHar(),
      "filter.search": () => { goSection("flows"); searchRef.current?.focus(); },
      "filter.tree": () => { goSection("flows"); treeFilterRef.current?.focus(); },
      "filter.toggle": () => {
        patchFilter({ enabled: !filter.enabled });
        showToast(filter.enabled ? "Filters off" : "Filters on");
      },
      "tree.toggle": () => setPrefs({ ...prefs, treeHidden: !prefs.treeHidden }),
      "flow.resend": () => void resendSelected(),
      "flow.curl": copyCurl,
      "row.selectAll": () => flowsRef.current?.markAll(),
      "pane.prevTab": () => flowsRef.current?.paneTab(-1),
      "pane.nextTab": () => flowsRef.current?.paneTab(1),
      "pane.switch": () => flowsRef.current?.switchPane(),
      "pane.collapse": () => flowsRef.current?.toggleCollapse(),
    },
    { modalOpen: paletteOpen || settingsOpen || shortcutsOpen || onboardingOpen || intercept != null },
  );

  /**
   * Escape closes whatever is on top.
   *
   * Separate from `useShortcuts`, which stops dispatching while a modal is open —
   * that is the rule that keeps ⌘1 from moving the section behind a dialog, and
   * this is the one exception to it.
   */
  useEffect(() => {
    if (!(settingsOpen || shortcutsOpen)) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      e.preventDefault();
      if (shortcutsOpen) setShortcutsOpen(false);
      else setSettingsOpen(false);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [settingsOpen, shortcutsOpen]);

  /** The menu's ⌘/ item, which the OS consumes before the webview sees it. */
  useEffect(() => {
    const un = api.onMenuShortcuts(() => setShortcutsOpen(true));
    return () => void un.then((f) => f());
  }, []);

  const hostCount = useMemo(() => new Set(flows.map((f) => f.host)).size, [flows]);

  /**
   * Throughput for the status bar.
   *
   * Recomputed when the capture changes rather than on a timer: an idle proxy
   * should not repaint this once a second forever. It lives in the status bar
   * because the flows section spends its height on rows (design.md §3).
   */
  const throughput = useMemo(
    () => formatRate(throughputRate(throughputSeries(flows, Date.now()), SPARK_WINDOW_MS)),
    [flows],
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
              onClick={() => goSection(r.id)}
            >
              <span className="icon"><Icon name={r.icon} size={19} /></span>
              <span className="label">{r.label}</span>
            </div>
          ))}
          <div className="spacer" />
          <div className="rail-gear" title="Settings" onClick={openSettings}>
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
            <div
              ref={recBtnRef}
              className={`tool-btn rec-btn ${recording ? "on" : ""}`}
              onClick={() => setRecording(!recording)}
            >
              <span className="rec-dot" />
              {recording ? "Recording" : "Paused"}
            </div>
            <div className="tool-btn" onClick={() => void clearAll()}>
              <Icon name="eraser" />
              Clear
            </div>
            <div className="spacer" />
            <div className="cmd-btn" onClick={openPalette}>
              <Icon name="command" size={13} />
              <span>Commands</span>
              <span className="kbd">{formatChord(shortcut("palette").chord).join("")}</span>
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
              ref={flowsRef}
              treeHidden={prefs.treeHidden}
              setColumns={(c) => setPrefs({ ...prefs, columns: c })}
              widths={prefs.columnWidths}
              setWidths={(w) => setPrefs({ ...prefs, columnWidths: w })}
              onMarked={setMarkedIds}
              saved={prefs.savedFilters}
              setSaved={(sf) => setPrefs({ ...prefs, savedFilters: sf })}
              flows={flows}
              filter={filter}
              patch={patchFilter}
              reset={() => setFilter(EMPTY_FILTER)}
              columns={columns}
              recording={recording}
              selected={selected}
              select={select}
              autoSelect={autoSelect}
              onResend={() => void resendSelected()}
              onCopyCurl={copyCurl}
              showToast={showToast}
              track={(ev, name) => api.trackUi(ev, name as Parameters<typeof api.trackUi>[1])}
              searchRef={searchRef}
              treeFilterRef={treeFilterRef}
              tableRef={flowListRef}
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
        <span
          className={`autosel ${autoSelect ? "on" : ""}`}
          title="Keep the newest row selected as it arrives"
          onClick={() => setPrefs({ ...prefs, autoSelect: !autoSelect })}
        >
          <Icon name={autoSelect ? "circle-dot" : "circle"} size={11} />
          Auto Select
        </span>
        <span className="spacer" />
        <span title="Throughput over the last minute">{throughput}</span>
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

      {/* keyboard shortcuts */}
      {shortcutsOpen && <ShortcutsDialog onClose={() => setShortcutsOpen(false)} />}

      {/* settings modal */}
      {settingsOpen && (
        <SettingsModal
          port={proxy.port ?? 9090} ca={ca}
          net={net} setNet={saveNet}
          mcp={mcp} setMcp={setMcp}
          helper={helper} setHelper={setHelper}
          update={update} setUpdate={setUpdate}
          onCheckUpdates={checkForUpdates} revealUpdates={revealUpdates}
          prefs={prefs} setPrefs={setPrefs}
          showToast={showToast}
          onClose={() => setSettingsOpen(false)}
          onWalkthrough={() => { setSettingsOpen(false); setCoach(null); openOnboarding(); }}
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

      {/* first-run walkthrough */}
      {onboardingOpen && (
        <OnboardingWizard
          helper={helper} setHelper={setHelper}
          ca={ca} proxy={proxy}
          recording={recording} flowCount={flows.length}
          setRecording={setRecording}
          showToast={showToast}
          onDismiss={(withCoach) => {
            setOnboardingOpen(false);
            // Whether they got to the end of it is the only interesting thing
            // about a walkthrough, so the two exits are counted apart.
            api.trackUi("ui.onboarding", prefs.onboardingDone ? "done" : "skip");
            if (!prefs.onboardingDone) setPrefs({ ...prefs, onboardingDone: true });
            // Only coach someone who still has nothing captured — pointing at an
            // empty list is help; pointing at a full one is noise.
            if (withCoach && flows.length === 0) setCoach(recording ? "list" : "record");
          }}
        />
      )}

      {/* walkthrough coachmarks — no scrim: the point is that the control below
          them stays clickable */}
      {coach === "record" && !recording && (
        <Coachmark
          anchor={recBtnRef}
          text="Capture is paused. Press here and NovaProxy starts recording what your apps request."
          cta="Got it"
          onDismiss={() => setCoach(null)}
        />
      )}
      {coach === "list" && flows.length === 0 && (
        <Coachmark
          anchor={flowListRef}
          placement="right"
          text="Requests land here as they happen. Click one to read its headers, body and timings."
          cta="Got it"
          onDismiss={() => setCoach(null)}
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
    api.getTlsScope()
      .then(setScope)
      .catch((e) => api.logUi("warn", "command", `could not load TLS scope: ${String(e)}`));
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
  helper, setHelper, update, setUpdate, onCheckUpdates, revealUpdates,
  prefs, setPrefs, showToast, onClose, onWalkthrough,
}: {
  port: number;
  ca: CaStatus | null;
  net: NetworkConditions;
  setNet: (n: NetworkConditions) => void;
  mcp: McpStatus | null;
  setMcp: (m: McpStatus) => void;
  helper: HelperStatus | null;
  setHelper: (h: HelperStatus) => void;
  update: UpdateState;
  setUpdate: (u: UpdateState) => void;
  /** Run a check on demand; owned by App so the menu can run the same one. */
  onCheckUpdates: () => Promise<void>;
  /** Settings was opened for the Updates card — scroll it into view. */
  revealUpdates: boolean;
  prefs: Prefs;
  setPrefs: (p: Prefs) => void;
  showToast: (t: string) => void;
  onClose: () => void;
  /** Re-open the first-run walkthrough; closes Settings on the way. */
  onWalkthrough: () => void;
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
            <GeneralTab
              prefs={prefs} setPrefs={setPrefs}
              helper={helper} setHelper={setHelper}
              update={update} setUpdate={setUpdate}
              onCheckUpdates={onCheckUpdates} revealUpdates={revealUpdates}
              showToast={showToast}
            />
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
            <h3>Guided setup</h3>
            <p>
              The four things a new install needs — the privileged helper, a trusted root
              certificate, the system proxy, and a first captured request — in order.
            </p>
            <div className="cert-actions">
              <div className="btn-primary" onClick={onWalkthrough}>
                <Icon name="play" />Run the walkthrough
              </div>
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
const LAUNCH_ITEMS: DropdownItem[] = [
  { value: "none", label: "None — leave the OS alone", icon: "circle" },
  { value: "system", label: "System proxy — capture everything", icon: "power" },
];

function GeneralTab({
  prefs, setPrefs, helper, setHelper, update, setUpdate,
  onCheckUpdates, revealUpdates, showToast,
}: {
  prefs: Prefs;
  setPrefs: (p: Prefs) => void;
  helper: HelperStatus | null;
  setHelper: (h: HelperStatus) => void;
  update: UpdateState;
  setUpdate: (u: UpdateState) => void;
  onCheckUpdates: () => Promise<void>;
  revealUpdates: boolean;
  showToast: (t: string) => void;
}) {
  return (
    <>
      <h3>Flows table</h3>
      <div className="pref-row">
        <span className="k">Follow the tail</span>
        <span
          className={`switch sm ${prefs.autoSelect ? "on" : ""}`}
          onClick={() => setPrefs({ ...prefs, autoSelect: !prefs.autoSelect })}
        >
          <span className="knob" />
        </span>
      </div>
      <p>
        Keep the newest row selected as it arrives — the same <b>Auto Select</b> toggle the status
        bar carries, remembered across launches. Off by default: a selection that moves while you
        are reading a body is worse than one click.
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

      <UpdateCard
        update={update} setUpdate={setUpdate}
        onCheck={onCheckUpdates} reveal={revealUpdates}
        prefs={prefs} setPrefs={setPrefs}
        showToast={showToast}
      />
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
 * Check for a new version, and install it.
 *
 * Installing is always a click, never automatic: replacing a binary that holds
 * a root CA and proxies the machine's traffic is not something to do behind the
 * user's back, and it ends with the window going away — a relaunch on macOS, an
 * exit into the installer on Windows — which drops the capture session either
 * way. The launch check only ever reports.
 *
 * The check itself belongs to App, because the native menu can ask for one too
 * and both routes have to end in this card rather than in two descriptions of
 * the same state.
 */
function UpdateCard({
  update, setUpdate, onCheck, reveal, prefs, setPrefs, showToast,
}: {
  update: UpdateState;
  setUpdate: (u: UpdateState) => void;
  onCheck: () => Promise<void>;
  /** Settings was opened by the menu item; bring the card to the user's eyes. */
  reveal: boolean;
  prefs: Prefs;
  setPrefs: (p: Prefs) => void;
  showToast: (t: string) => void;
}) {
  const pct = progressPercent(update.progress);
  const acting = !canActOnUpdate(update);
  const headingRef = useRef<HTMLHeadingElement | null>(null);

  // The card is the last of four in this tab, so opening Settings from the menu
  // would otherwise land above the fold on the thing that was just asked for.
  useEffect(() => {
    if (!reveal) return;
    headingRef.current?.scrollIntoView({ block: "start", behavior: "smooth" });
  }, [reveal]);

  async function install() {
    // A channel rather than a promise chain: the download is the one operation
    // here long enough that silence reads as a hang.
    const channel = new Channel<UpdateProgress>();
    let state: UpdateState = { ...update, phase: "downloading", progress: null, error: null };
    setUpdate(state);
    channel.onmessage = (p) => {
      state = afterProgress(state, p);
      setUpdate(state);
    };
    try {
      await api.installUpdate(channel);
      // Reached only if the process is still alive: on Windows the command
      // returns after handing off to the installer, which closes this build.
      showToast("Installer running — NovaProxy will close to finish");
    } catch (e) {
      setUpdate({ ...state, phase: "error", error: String(e) });
    }
  }

  return (
    <>
      <h3 ref={headingRef}>Updates</h3>
      <div className="field-group">
        <span
          className={`dot ${
            update.phase === "error"
              ? "warn"
              : update.phase === "available"
                ? "warn"
                : update.phase === "current"
                  ? "ok"
                  : ""
          }`}
        />
        <span style={{ color: "var(--text2)" }}>{updateSummary(update)}</span>
      </div>

      {(update.phase === "downloading" || update.phase === "installing") && (
        <div className="upd-bar" title={pct == null ? "Downloading" : `${pct}%`}>
          <div
            className={`upd-fill ${pct == null ? "indeterminate" : ""}`}
            style={pct == null ? undefined : { width: `${pct}%` }}
          />
        </div>
      )}
      {update.phase === "downloading" && update.progress?.total != null && (
        <p>
          {formatBytes(update.progress.downloaded)} of {formatBytes(update.progress.total)}
        </p>
      )}

      {update.phase === "available" && update.status?.notes && (
        <p style={{ whiteSpace: "pre-wrap" }}>{update.status.notes}</p>
      )}

      <div className="pref-row">
        <span className="k">Check at launch</span>
        <div
          className={`switch ${prefs.autoCheckUpdates ? "on" : ""}`}
          onClick={() => setPrefs({ ...prefs, autoCheckUpdates: !prefs.autoCheckUpdates })}
        >
          <span className="knob" />
        </div>
      </div>
      <p>
        A debugging proxy carries its own root CA and TLS stack, so staying current matters more
        here than in most apps. Found versions are only ever reported — installing is this card's
        button, and nothing else.
      </p>

      <div className="cert-actions">
        <div
          className={`btn-neutral ${acting ? "disabled" : ""}`}
          onClick={() => !acting && void onCheck()}
        >
          Check now
        </div>
        {update.phase === "available" && (
          <div
            className={`btn-primary ${acting ? "disabled" : ""}`}
            onClick={() => !acting && void install()}
          >
            Install {update.status?.version ?? "update"}
          </div>
        )}
      </div>

      {update.phase === "unconfigured" && (
        <p className="warn-note">
          Development builds have no update endpoint. Released builds check
          <code> latest.json</code> published alongside the installers.
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
