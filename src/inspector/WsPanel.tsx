import { useState } from "react";
import type { WsMessage } from "../api";
import { Icon } from "../icons";
import { formatAgo, formatCellBytes } from "../format";
import { MAX_WS_FRAMES } from "../store";

/**
 * Frames rendered at once. A busy socket fills its retention window in seconds,
 * and every frame is a DOM row — the rest stay one click away rather than being
 * mounted where nobody is looking.
 */
const WS_PAGE = 400;

export function WsPanel({ messages, dropped }: { messages: WsMessage[] | undefined; dropped: number }) {
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
