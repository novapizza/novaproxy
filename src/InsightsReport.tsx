import { useState } from "react";
import type { Flow } from "./api";
import type { StatusClass } from "./classify";
import { formatBytes } from "./format";
import { Icon } from "./icons";
import { formatSpan, safeTarget, STATUS_ORDER, type AppGroup, type TrafficSummary } from "./report";

/**
 * The traffic report above the findings: how the traffic is doing overall,
 * before the list of what is wrong with it.
 *
 * Every number is clickable where clicking has an obvious meaning — a status
 * segment sets that status chip, a chip opens its flow, a host row scopes the
 * table — so the report is a way into the capture, not a screenshot of it.
 */
export function ReportHeader({
  summary: s,
  onSelectFlow,
  onStatus,
}: {
  summary: TrafficSummary;
  onSelectFlow: (f: Flow) => void;
  onStatus: (c: StatusClass) => void;
}) {
  const rate = s.completed ? (s.failed / s.completed) * 100 : 0;
  const b = s.failedBreakdown;
  const cards: { label: string; value: string; sub: string; tone?: "bad" | "warn" }[] = [
    { label: "Requests", value: String(s.requests), sub: `${s.completed} finished` },
    {
      label: "Failed",
      value: s.completed ? `${rate.toFixed(1)}%` : "–",
      sub: `${b.client} 4xx · ${b.server} 5xx · ${b.transport} no reply`,
      tone: rate >= 10 ? "bad" : rate >= 2 ? "warn" : undefined,
    },
    { label: "P50", value: formatSpan(s.p50), sub: `P95 ${formatSpan(s.p95)}` },
    {
      label: "P99",
      value: formatSpan(s.p99),
      sub: `server wait P95 ${formatSpan(s.waitP95)}`,
      tone: s.p99 != null && s.p99 >= 3_000 ? "warn" : undefined,
    },
    { label: "Downloaded", value: formatBytes(s.responseBytes), sub: `${formatBytes(s.requestBytes)} uploaded` },
    { label: "Capture window", value: formatSpan(s.windowMs), sub: "first request → last reply" },
  ];
  const total = STATUS_ORDER.reduce((n, c) => n + s.status[c], 0);

  return (
    <div className="rep">
      <div className="rep-cards">
        {cards.map((c) => (
          <div key={c.label} className="rep-card">
            <div className="rep-label">{c.label}</div>
            <div className={`rep-value ${c.tone ?? ""}`}>{c.value}</div>
            <div className="rep-sub">{c.sub}</div>
          </div>
        ))}
      </div>

      {total > 0 && (
        <div className="rep-status">
          <div className="rep-bar">
            {STATUS_ORDER.filter((c) => s.status[c] > 0).map((c) => (
              <span
                key={c}
                className={`seg s-${c}`}
                style={{ flexGrow: s.status[c] }}
                title={`${s.status[c]} × ${c.toUpperCase()} — show them`}
                onClick={() => onStatus(c)}
              />
            ))}
          </div>
          <div className="rep-legend">
            {STATUS_ORDER.map((c) => (
              <span key={c} className={`lg ${s.status[c] ? "" : "zero"}`} onClick={() => s.status[c] && onStatus(c)}>
                <span className={`dot s-${c}`} />
                {c.toUpperCase()} {s.status[c]}
              </span>
            ))}
          </div>
        </div>
      )}

      <div className="rep-chips">
        {s.slowest && (
          <span className="rep-chip" onClick={() => onSelectFlow(s.slowest!)} title={safeTarget(s.slowest)}>
            <Icon name="turtle" size={12} />Slowest <code>{safeTarget(s.slowest)}</code>
            <b>{formatSpan(s.slowest.duration_ms)}</b>
          </span>
        )}
        {s.largest && (
          <span className="rep-chip" onClick={() => onSelectFlow(s.largest!)} title={safeTarget(s.largest)}>
            <Icon name="download" size={12} />Largest <code>{safeTarget(s.largest)}</code>
            <b>{formatBytes(s.largest.response_size)}</b>
          </span>
        )}
        {s.mostCalled && (
          <span className="rep-chip" onClick={() => onSelectFlow(s.mostCalled!.sample)} title={s.mostCalled.endpoint}>
            <Icon name="repeat" size={12} />Most called <code>{s.mostCalled.endpoint}</code>
            <b>×{s.mostCalled.count}</b>
          </span>
        )}
      </div>
    </div>
  );
}

/** How many apps the table shows before "Show all". */
const APPS_SHOWN = 6;

/** Apps, and the hosts each one talks to, with how those requests went. */
export function HostsTable({
  groups,
  onHost,
}: {
  groups: AppGroup[];
  onHost: (app: string, host: string) => void;
}) {
  const [all, setAll] = useState(false);
  if (groups.length === 0) return null;
  const shown = all ? groups : groups.slice(0, APPS_SHOWN);
  return (
    <div className="rep-hosts">
      <h3 className="rep-h">Hosts</h3>
      <table>
        <thead>
          <tr>
            <th>App / host</th>
            <th className="r">Requests</th>
            <th className="r">Failed</th>
            <th className="r">P50</th>
            <th className="r">P95</th>
            <th className="r">Downloaded</th>
          </tr>
        </thead>
        <tbody>
          {shown.map((g) => (
            <AppRows key={g.app} group={g} onHost={onHost} />
          ))}
        </tbody>
      </table>
      {groups.length > APPS_SHOWN && (
        <div className="ins-muted-toggle" onClick={() => setAll(!all)}>
          <Icon name={all ? "chevron-down" : "chevron-right"} size={13} />
          {all ? "Show fewer apps" : `Show all ${groups.length} apps`}
        </div>
      )}
    </div>
  );
}

function AppRows({ group: g, onHost }: { group: AppGroup; onHost: (app: string, host: string) => void }) {
  return (
    <>
      <tr className="app">
        <td>{g.app || "Unknown app"}</td>
        <td className="r">{g.requests}</td>
        <td className={`r ${g.failed ? "bad" : ""}`}>{g.failed}</td>
        <td className="r" />
        <td className="r" />
        <td className="r">{formatBytes(g.bytes)}</td>
      </tr>
      {g.hosts.map((h) => (
        <tr key={h.host} className="host" onClick={() => onHost(g.app, h.host)} title="Show these flows">
          <td>{h.host}</td>
          <td className="r">{h.requests}</td>
          <td className={`r ${h.failed ? "bad" : ""}`}>{h.failed}</td>
          <td className="r">{formatSpan(h.p50)}</td>
          <td className="r">{formatSpan(h.p95)}</td>
          <td className="r">{formatBytes(h.bytes)}</td>
        </tr>
      ))}
    </>
  );
}
