import { useEffect, useState } from "react";
import { api, type WsMessage } from "../api";
import { Icon } from "../icons";
import { formatAgo, formatCellBytes } from "../format";
import { useStore } from "../store";

/**
 * Frames rendered at once. A busy socket fills its retention window in seconds,
 * and every frame is a DOM row — the rest stay one click away rather than being
 * mounted where nobody is looking.
 */
const WS_PAGE = 400;

/** What one row shows: text as is, binary by size, a close by its reason. */
function rowText(m: WsMessage): string {
  if (m.text != null) return m.text;
  if (m.opcode === "Binary") return `[binary ${formatCellBytes(m.size)}]`;
  if (m.opcode === "Close") return "(closed)";
  return "";
}

/**
 * A socket's frames. The list streams in as previews — 200 characters of text,
 * no binary payload — and the engine keeps the rest: opening the socket loads
 * every frame it still holds (including ones captured while recording was
 * paused), and expanding a cut-short frame fetches its whole payload.
 */
export function WsPanel({
  flowId,
  messages,
  dropped,
}: {
  flowId: string;
  messages: WsMessage[] | undefined;
  dropped: number;
}) {
  const [showAll, setShowAll] = useState(false);
  const [open, setOpen] = useState<Record<string, string>>({});

  useEffect(() => {
    let live = true;
    api
      .wsFrames(flowId)
      .then((r) => live && useStore.getState().backfillWs(flowId, r.frames, r.dropped))
      // An imported session has no engine copy; the list is all there is.
      .catch(() => {});
    return () => {
      live = false;
    };
  }, [flowId]);

  async function expand(m: WsMessage) {
    const key = String(m.seq);
    if (key in open) {
      const { [key]: _, ...rest } = open;
      setOpen(rest);
      return;
    }
    try {
      const full = await api.wsFrame(flowId, Number(m.seq));
      setOpen((o) => ({ ...o, [key]: full.text ?? full.base64 ?? "" }));
    } catch (e) {
      setOpen((o) => ({ ...o, [key]: String(e) }));
    }
  }

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
              {dropped.toLocaleString()} earlier frame{dropped === 1 ? " was" : "s were"} dropped to stay
              within memory limits.
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
        const key = String(m.seq);
        const full = open[key];
        return (
          <div
            className={`ws-frame ${sent ? "sent" : "recv"} ${m.truncated ? "expandable" : ""}`}
            key={m.flow_id + "-" + key}
            onClick={m.truncated ? () => void expand(m) : undefined}
            title={m.truncated ? (full != null ? "Collapse" : "Show the whole frame") : undefined}
          >
            <span className={`ws-dir ${sent ? "sent" : "recv"}`}>
              <Icon name={sent ? "arrow-up" : "arrow-down"} size={11} />
              {sent ? "sent" : "recv"}
            </span>
            <span className="ws-op">{m.opcode.toLowerCase()}</span>
            <span className="ws-payload">
              {full ?? rowText(m)}
              {full == null && m.truncated ? " …" : ""}
            </span>
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
