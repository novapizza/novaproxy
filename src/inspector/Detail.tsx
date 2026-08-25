import { useEffect, useState } from "react";
import {
  api,
  type BodyPreview,
  type Flow,
  type WsMessage,
} from "../api";
import { Icon } from "../icons";
import { MAX_WS_FRAMES, useStore } from "../store";
import { mcpLabel } from "../filter";
import { methodClass, statusClass, statusText } from "../badges";
import { formatAgo, formatCellBytes, num } from "../format";
import { formatDuration, formatMs, timingBreakdown } from "../timing";

/** Which panel of the inspector is open. */
export type DetailTab = "overview" | "request" | "response" | "timing" | "curl" | "ws";

export function buildCurl(f: Flow): string {
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
export async function withRequestBody(flow: Flow): Promise<Flow> {
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

const DETAIL_TABS: { id: DetailTab; label: string }[] = [
  { id: "overview", label: "Overview" },
  { id: "request", label: "Request" },
  { id: "response", label: "Response" },
  { id: "timing", label: "Timing" },
  { id: "curl", label: "cURL" },
];

export function Detail({
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
    { k: "Size", v: formatCellBytes(totalSize) },
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
              <span className="metaval">{(flow.content_type ?? "—")} · {formatCellBytes(flow.response_size)}</span>
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
          Request body streamed upstream in {formatMs(b.requestMs)} ({formatCellBytes(flow.request_size)}).
        </div>
      )}

      <div className="timing-total">
        <span>Total</span>
        <span className="mono">{b.totalMs != null ? formatMs(b.totalMs) : "in flight"}</span>
      </div>
      <div className="timing-total">
        <span>Transferred</span>
        <span className="mono">{formatCellBytes(num(flow.request_size) + num(flow.response_size))}</span>
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
            ? `[binary ${formatCellBytes(m.size)}]`
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
            <span className="ws-meta">{formatCellBytes(m.size)} · {formatAgo(m.at)}</span>
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
    return <pre className={`code ${kind}`}>Loading body ({formatCellBytes(shown.size)})…</pre>;
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
        {loading ? "Loading…" : `Load full body (${formatCellBytes(body.size)})`}
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
        <pre className={`code ${kind}`}>Binary body — {formatCellBytes(shown.size)} ({shown.media_type ?? "unknown"}){shown.truncated ? ", truncated" : ""}</pre>
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
