import { useState } from "react";
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

const REQUEST_TABS = ["Header", "Query", "Body", "Cookies", "Raw", "Summary"] as const;
const RESPONSE_TABS = ["Header", "Body", "Raw", "Treeview", "Timing", "Summary"] as const;

type RequestTab = (typeof REQUEST_TABS)[number];
type ResponseTab = (typeof RESPONSE_TABS)[number];

export function Inspector({
  flow,
  showToast,
  onTab,
}: {
  flow: Flow;
  showToast: (t: string) => void;
  /** Told which pane tab was opened, for usage counting. */
  onTab?: (pane: "request" | "response", tab: string) => void;
}) {
  const [reqTab, setReqTab] = useState<RequestTab>("Header");
  const [resTab, setResTab] = useState<ResponseTab>("Header");
  /** Which pane, if any, is collapsed to its head. Never both. */
  const [collapsed, setCollapsed] = useState<"request" | "response" | null>(null);
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
        setTab={(t) => {
          setReqTab(t as RequestTab);
          onTab?.("request", t);
        }}
        collapsed={collapsed === "request"}
        toggleCollapse={() => setCollapsed(collapsed === "request" ? null : "request")}
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
        setTab={(t) => {
          setResTab(t as ResponseTab);
          onTab?.("response", t);
        }}
        collapsed={collapsed === "response"}
        toggleCollapse={() => setCollapsed(collapsed === "response" ? null : "response")}
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
function metaFor(flow: Flow, side: "request" | "response"): string {
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
  setTab,
  collapsed,
  toggleCollapse,
  meta,
  children,
}: {
  side: "request" | "response";
  tabs: readonly string[];
  tab: string;
  setTab: (t: string) => void;
  collapsed: boolean;
  toggleCollapse: () => void;
  meta: string;
  children: React.ReactNode;
}) {
  return (
    <div className={`insp-pane ${collapsed ? "collapsed" : ""}`}>
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
