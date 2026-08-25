import type { Flow } from "../api";
import { Icon } from "../icons";
import { useStore } from "../store";
import { formatCellBytes } from "../format";
import { headerValue } from "../classify";
import { KeyValueTable } from "./KeyValueTable";
import { BodyPanel, bodyToText } from "./BodyPanel";
import { TimingPanel } from "./TimingPanel";
import { TreeviewPanel } from "./TreeviewPanel";
import { WsPanel } from "./WsPanel";
import { parseCookies, parseQuery, rawHttp, summaryOf } from "./parts";

/**
 * Request and response, side by side.
 *
 * Two panes rather than one tabbed panel because the question being asked is
 * almost always about the *pair* — this header went out, that status came back —
 * and a single panel makes you flip between them from memory.
 *
 * Each pane keeps its own tab, and the two tab sets are deliberately different:
 * `Query`/`Cookies` are properties of a request, `Treeview`/`Timing` of a
 * response. cURL is not a tab at all — it is an action on the summary bar
 * (issues/0003 §6).
 */

export const REQUEST_TABS = ["Header", "Query", "Body", "Cookies", "Raw", "Summary"] as const;
export const RESPONSE_TABS = ["Header", "Body", "Raw", "Treeview", "Timing", "Summary"] as const;

export type PaneSide = "request" | "response";

/**
 * Which panel each pane is showing, which pane the keyboard acts on, and which
 * one is collapsed.
 *
 * Held by the caller rather than inside this component so the keyboard shortcuts
 * (`⌘[`, `⌘]`, `⌘E`, `⌘⇧→`) have something to act on: a chord arrives at the
 * window, and state buried in a child is unreachable from there without a ref
 * dance that would be harder to read than one prop.
 */
export interface PaneState {
  active: PaneSide;
  request: string;
  response: string;
  collapsed: PaneSide | null;
}

export const INITIAL_PANES: PaneState = {
  active: "response",
  request: "Header",
  response: "Header",
  collapsed: null,
};

/** The tab list for one side. */
export function tabsFor(side: PaneSide): readonly string[] {
  return side === "request" ? REQUEST_TABS : RESPONSE_TABS;
}

export function Inspector({
  flow,
  showToast,
  onTab,
  panes,
  setPanes,
}: {
  flow: Flow;
  showToast: (t: string) => void;
  /** Told which pane tab was opened, for usage counting. */
  onTab?: (pane: PaneSide, tab: string) => void;
  panes: PaneState;
  setPanes: (p: PaneState) => void;
}) {
  const reqTab = panes.request;
  const resTab = panes.response;
  const collapsed = panes.collapsed;
  const wsMessages = useStore((s) => s.wsMessages[flow.id]);
  const wsDropped = useStore((s) => s.wsDropped[flow.id] ?? 0);

  /**
   * A WebSocket takes the whole strip.
   *
   * Its upgrade request and 101 response are read once; the frames are the
   * point, and they need the width. Proxyman does the same.
   */
  if (flow.is_websocket) {
    return (
      <div className="insp">
        <div className="insp-pane">
          <div className="insp-head">
            <span className="title">WebSocket</span>
            <span className="tabs">
              <span className="itab active">
                Messages{wsMessages ? ` (${wsMessages.length})` : ""}
              </span>
            </span>
          </div>
          <div className="insp-body">
            <WsPanel key={flow.id} messages={wsMessages} dropped={wsDropped} />
          </div>
        </div>
      </div>
    );
  }

  return (
    <div className="insp">
      <Pane
        side="request"
        tabs={REQUEST_TABS}
        tab={reqTab}
        active={panes.active === "request"}
        setTab={(t) => {
          setPanes({ ...panes, active: "request", request: t });
          onTab?.("request", t);
        }}
        collapsed={collapsed === "request"}
        toggleCollapse={() =>
          setPanes({ ...panes, collapsed: collapsed === "request" ? null : "request" })
        }
        meta={metaFor(flow, "request")}
      >
        {reqTab === "Header" && (
          <KeyValueTable
            rows={flow.request_headers.map((h) => ({ k: h.name, v: h.value }))}
            empty="no request headers"
          />
        )}
        {reqTab === "Query" && (
          <KeyValueTable rows={parseQuery(flow.url)} empty="no query parameters" />
        )}
        {reqTab === "Body" && (
          <BodyPanel body={flow.request_body} kind="req" flowId={flow.id} showToast={showToast} />
        )}
        {reqTab === "Cookies" && (
          <KeyValueTable rows={parseCookies(flow, "request")} empty="no cookies sent" />
        )}
        {reqTab === "Raw" && <RawView flow={flow} side="request" />}
        {reqTab === "Summary" && (
          <KeyValueTable rows={summaryOf(flow, "request")} empty="nothing captured" />
        )}
      </Pane>

      <Pane
        side="response"
        tabs={RESPONSE_TABS}
        tab={resTab}
        active={panes.active === "response"}
        setTab={(t) => {
          setPanes({ ...panes, active: "response", response: t });
          onTab?.("response", t);
        }}
        collapsed={collapsed === "response"}
        toggleCollapse={() =>
          setPanes({ ...panes, collapsed: collapsed === "response" ? null : "response" })
        }
        meta={metaFor(flow, "response")}
      >
        {resTab === "Header" && (
          <KeyValueTable
            rows={flow.response_headers.map((h) => ({ k: h.name, v: h.value }))}
            empty={flow.tunneled ? "tunneled — nothing was decrypted" : "no response headers"}
          />
        )}
        {resTab === "Body" && (
          <BodyPanel
            body={flow.response_body}
            kind="res"
            status={flow.status}
            flowId={flow.id}
            showToast={showToast}
          />
        )}
        {resTab === "Raw" && <RawView flow={flow} side="response" />}
        {resTab === "Treeview" && <TreeviewPanel text={bodyToText(flow.response_body)} />}
        {resTab === "Timing" && <TimingPanel flow={flow} />}
        {resTab === "Summary" && (
          <KeyValueTable rows={summaryOf(flow, "response")} empty="nothing captured" />
        )}
      </Pane>
    </div>
  );
}

/** The one line of context a pane head can carry: what this side weighed. */
function metaFor(flow: Flow, side: PaneSide): string {
  const type =
    side === "request" ? headerValue(flow.request_headers, "content-type") : flow.content_type;
  const size = side === "request" ? flow.request_size : flow.response_size;
  const media = type ? type.split(";")[0] : null;
  return [media, formatCellBytes(size)].filter((x) => x && x !== "–").join(" · ");
}

function Pane({
  side,
  tabs,
  tab,
  active,
  setTab,
  collapsed,
  toggleCollapse,
  meta,
  children,
}: {
  side: PaneSide;
  tabs: readonly string[];
  tab: string;
  /** True for the pane the keyboard is acting on. */
  active: boolean;
  setTab: (t: string) => void;
  collapsed: boolean;
  toggleCollapse: () => void;
  meta: string;
  children: React.ReactNode;
}) {
  return (
    <div className={`insp-pane ${collapsed ? "collapsed" : ""} ${active ? "active" : ""}`}>
      <div className="insp-head">
        <span className="title">{side === "request" ? "Request" : "Response"}</span>
        <span className="tabs">
          {tabs.map((t) => (
            <span
              key={t}
              className={`itab ${tab === t ? "active" : ""}`}
              onClick={() => setTab(t)}
            >
              {t}
            </span>
          ))}
        </span>
        <span className="spacer" />
        {meta && <span className="meta">{meta}</span>}
        <span
          className="collapse"
          title={collapsed ? "Expand this pane" : "Collapse this pane"}
          onClick={toggleCollapse}
        >
          <Icon name={collapsed ? "plus" : "minus-circle"} size={14} />
        </span>
      </div>
      {!collapsed && <div className="insp-body">{children}</div>}
    </div>
  );
}

/**
 * Raw HTTP text, labelled for what it is.
 *
 * `rawHttp` assembles this from the parsed flow — the engine does not retain wire
 * bytes — so the panel says "reconstructed" rather than letting the reader
 * assume byte-for-byte fidelity it does not have.
 */
function RawView({ flow, side }: { flow: Flow; side: "request" | "response" }) {
  return (
    <>
      <div className="raw-note">
        reconstructed from the captured flow — header order and casing are real, framing is not
      </div>
      <pre className="code">{rawHttp(flow, side)}</pre>
    </>
  );
}
