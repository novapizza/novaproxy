import type { Flow } from "../api";
import { formatAgo, formatCellBytes, num } from "../format";
import { formatMs, timingBreakdown } from "../timing";

/**
 * Waterfall of the phases the engine actually measured. Every bar here comes
 * from an instrumented timer — phases that did not happen (no DNS lookup for an
 * IP literal, no handshake on plain HTTP) or could not be attributed (a reused
 * connection) are stated as such instead of being drawn.
 */
export function TimingPanel({ flow }: { flow: Flow }) {
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
