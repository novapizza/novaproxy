import type { Flow } from "./api";
import type { Insight } from "./bindings/Insight";
import type { InsightSeverity } from "./bindings/InsightSeverity";
import { statusClassOf, type StatusClass } from "./classify";
import { formatBytes } from "./format";
import { INSIGHT_DOCS } from "./insightDocs";
import { serverWaitMs } from "./timing";

/**
 * The traffic report at the top of the Insights page, and its Markdown export.
 *
 * Computed here from the flow list (no bodies needed) rather than in the
 * engine, because it follows whatever the table's filter shows — the same list
 * the user is looking at.
 *
 * Percentiles, not averages, lead: one request that hung for 30 s drags a mean
 * far from what a typical call feels like, and the tail is the question anyway.
 */

/** The status classes the breakdown bar shows, in order. */
export const STATUS_ORDER: StatusClass[] = ["2xx", "3xx", "4xx", "5xx", "err"];

export interface TrafficSummary {
  requests: number;
  /** Finished, with a status or a transport error — what the rates are over. */
  completed: number;
  failed: number;
  failedBreakdown: { client: number; server: number; transport: number };
  status: Record<StatusClass, number>;
  /** Over completed request/response exchanges only (see `timed`). */
  p50: number | null;
  p95: number | null;
  p99: number | null;
  /** 95th percentile of the server's own wait (TTFB minus connection setup). */
  waitP95: number | null;
  responseBytes: number;
  requestBytes: number;
  /** First request seen → last one finished. */
  windowMs: number;
  slowest: Flow | null;
  largest: Flow | null;
  mostCalled: { endpoint: string; count: number; sample: Flow } | null;
}

const num = (n: number | bigint | null | undefined) => (n == null ? 0 : Number(n));

/**
 * Whether a flow's duration measures a request, not a connection.
 *
 * A WebSocket's or a tunnel's duration is how long the socket stayed open, and
 * an event stream's is how long someone listened: folding those into P95 would
 * make every capture with one open socket look catastrophically slow.
 */
function timed(f: Flow): boolean {
  if (f.duration_ms == null || f.is_websocket || f.tunneled) return false;
  return !(f.content_type ?? "").toLowerCase().startsWith("text/event-stream");
}

/** Nearest-rank percentile of an ascending list. */
export function percentile(sorted: readonly number[], p: number): number | null {
  if (sorted.length === 0) return null;
  const rank = Math.ceil((p / 100) * sorted.length);
  return sorted[Math.min(sorted.length - 1, Math.max(0, rank - 1))];
}

/**
 * `/users/42/posts?x=1` → `/users/:id/posts`. A copy of the engine's
 * `normalize_path`, kept to the same cases, so "Most called" groups calls the
 * way the findings do.
 */
export function normalizePath(path: string): string {
  const bare = path.split(/[?#]/)[0];
  const out = bare
    .split("/")
    .map((seg) => {
      if (seg === "") return seg;
      if (/^\d+$/.test(seg)) return ":id";
      if (/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(seg)) return ":uuid";
      if (seg.length >= 12 && /^[0-9a-f]+$/i.test(seg) && /\d/.test(seg)) return ":hash";
      if (seg.length >= 20 && /^[A-Za-z0-9_-]+$/.test(seg) && /\d/.test(seg) && /[A-Za-z]/.test(seg)) return ":token";
      return seg;
    })
    .join("/");
  return out || "/";
}

function failedClass(f: Flow): "client" | "server" | "transport" | null {
  if (f.error != null || f.state === "Error") return "transport";
  if (f.status == null) return null;
  if (f.status >= 500) return "server";
  if (f.status >= 400) return "client";
  return null;
}

export function summarize(flows: readonly Flow[]): TrafficSummary {
  const status: Record<StatusClass, number> = { "1xx": 0, "2xx": 0, "3xx": 0, "4xx": 0, "5xx": 0, err: 0 };
  const breakdown = { client: 0, server: 0, transport: 0 };
  const durations: number[] = [];
  const waits: number[] = [];
  const calls = new Map<string, { count: number; sample: Flow }>();
  let completed = 0;
  let responseBytes = 0;
  let requestBytes = 0;
  let first = Infinity;
  let last = -Infinity;
  let slowest: Flow | null = null;
  let largest: Flow | null = null;

  for (const f of flows) {
    responseBytes += num(f.response_size);
    requestBytes += num(f.request_size);
    first = Math.min(first, f.started_at);
    last = Math.max(last, f.started_at + (f.duration_ms ?? 0));

    const sc = statusClassOf(f);
    if (sc) {
      completed++;
      status[sc]++;
      const fc = failedClass(f);
      if (fc) breakdown[fc]++;
    }
    if (timed(f)) {
      durations.push(f.duration_ms!);
      if (!slowest || f.duration_ms! > slowest.duration_ms!) slowest = f;
    }
    const wait = serverWaitMs(f);
    if (wait != null && timed(f)) waits.push(wait);
    if (!largest || num(f.response_size) > num(largest.response_size)) largest = f;

    if (!f.is_websocket && !f.tunneled) {
      const endpoint = `${f.method} ${f.host}${normalizePath(f.path)}`;
      const c = calls.get(endpoint);
      if (c) c.count++;
      else calls.set(endpoint, { count: 1, sample: f });
    }
  }

  durations.sort((a, b) => a - b);
  waits.sort((a, b) => a - b);
  let mostCalled: TrafficSummary["mostCalled"] = null;
  for (const [endpoint, c] of calls) {
    if (!mostCalled || c.count > mostCalled.count) mostCalled = { endpoint, count: c.count, sample: c.sample };
  }
  const failed = breakdown.client + breakdown.server + breakdown.transport;

  return {
    requests: flows.length,
    completed,
    failed,
    failedBreakdown: breakdown,
    status,
    p50: percentile(durations, 50),
    p95: percentile(durations, 95),
    p99: percentile(durations, 99),
    waitP95: percentile(waits, 95),
    responseBytes,
    requestBytes,
    windowMs: flows.length ? Math.max(0, last - first) : 0,
    slowest,
    largest: largest && num(largest.response_size) > 0 ? largest : null,
    mostCalled: mostCalled && mostCalled.count > 1 ? mostCalled : null,
  };
}

/* --------------------------------- hosts --------------------------------- */

export interface HostRow {
  host: string;
  requests: number;
  failed: number;
  p50: number | null;
  p95: number | null;
  bytes: number;
}

export interface AppGroup {
  /** `Flow.process`; `""` for flows with no attribution. */
  app: string;
  requests: number;
  failed: number;
  bytes: number;
  hosts: HostRow[];
}

/** Apps by request count, each with its hosts by request count. */
export function hostTable(flows: readonly Flow[]): AppGroup[] {
  const apps = new Map<string, Map<string, { flows: Flow[] }>>();
  for (const f of flows) {
    const app = f.process ?? "";
    let hosts = apps.get(app);
    if (!hosts) apps.set(app, (hosts = new Map()));
    let h = hosts.get(f.host);
    if (!h) hosts.set(f.host, (h = { flows: [] }));
    h.flows.push(f);
  }
  const groups: AppGroup[] = [];
  for (const [app, hosts] of apps) {
    const rows: HostRow[] = [];
    for (const [host, { flows: hf }] of hosts) {
      const d = hf.filter(timed).map((f) => f.duration_ms!).sort((a, b) => a - b);
      rows.push({
        host,
        requests: hf.length,
        failed: hf.filter((f) => failedClass(f) != null).length,
        p50: percentile(d, 50),
        p95: percentile(d, 95),
        bytes: hf.reduce((s, f) => s + num(f.response_size), 0),
      });
    }
    rows.sort((a, b) => b.requests - a.requests || a.host.localeCompare(b.host));
    groups.push({
      app,
      requests: rows.reduce((s, r) => s + r.requests, 0),
      failed: rows.reduce((s, r) => s + r.failed, 0),
      bytes: rows.reduce((s, r) => s + r.bytes, 0),
      hosts: rows,
    });
  }
  return groups.sort((a, b) => b.requests - a.requests || a.app.localeCompare(b.app));
}

/* --------------------------------- export -------------------------------- */

export function formatSpan(ms: number | null): string {
  if (ms == null) return "–";
  if (ms >= 60_000) return `${(ms / 60_000).toFixed(1)} min`;
  if (ms >= 1_000) return `${(ms / 1_000).toFixed(2)} s`;
  return `${Math.round(ms)} ms`;
}

/** `host/path` without the query string, which is where tokens hide. */
export function safeTarget(f: Flow): string {
  return `${f.method} ${f.host}${f.path.split(/[?#]/)[0]}`;
}

const SEVERITIES: InsightSeverity[] = ["High", "Warning", "Info"];

/** Markdown table cells cannot hold a bare pipe or a line break. */
const cell = (s: string) => s.replace(/\|/g, "\\|").replace(/\n/g, " ");

/**
 * The report as Markdown, for a ticket or a PR.
 *
 * Built only from aggregates, finding text and query-less URLs: findings name
 * the parameters and cookies they are about, never their values, and no
 * header value or body byte is in here. That is what makes it safe to paste
 * where a HAR file is not.
 */
export function reportMarkdown(opts: {
  scope: string;
  generatedAt: Date;
  summary: TrafficSummary;
  insights: Insight[];
  mutedCount: number;
  hosts: AppGroup[];
}): string {
  const { summary: s, insights, hosts } = opts;
  const out: string[] = [];
  out.push("# NovaProxy traffic report", "");
  out.push(`Generated ${opts.generatedAt.toISOString().replace("T", " ").slice(0, 16)} UTC · Scope: ${opts.scope}`, "");

  out.push("## Summary", "");
  out.push("| Requests | Failed | P50 | P95 | P99 | Server wait P95 | Downloaded | Capture window |");
  out.push("|---:|---:|---:|---:|---:|---:|---:|---:|");
  const rate = s.completed ? ` (${((s.failed / s.completed) * 100).toFixed(1)}%)` : "";
  out.push(
    `| ${s.requests} | ${s.failed}${rate} | ${formatSpan(s.p50)} | ${formatSpan(s.p95)} | ${formatSpan(s.p99)} | ${formatSpan(s.waitP95)} | ${formatBytes(s.responseBytes)} | ${formatSpan(s.windowMs)} |`,
    "",
  );
  out.push(`Status: ${STATUS_ORDER.map((c) => `${c.toUpperCase()} ${s.status[c]}`).join(" · ")}`);
  out.push(
    `Failures: ${s.failedBreakdown.client} client (4xx) · ${s.failedBreakdown.server} server (5xx) · ${s.failedBreakdown.transport} no response`,
    "",
  );
  if (s.slowest) out.push(`- Slowest: \`${safeTarget(s.slowest)}\` — ${formatSpan(s.slowest.duration_ms)}`);
  if (s.largest) out.push(`- Largest: \`${safeTarget(s.largest)}\` — ${formatBytes(s.largest.response_size)}`);
  if (s.mostCalled) out.push(`- Most called: \`${s.mostCalled.endpoint}\` — ${s.mostCalled.count}×`);
  out.push("");

  out.push(`## Findings (${insights.length})`, "");
  if (insights.length === 0) out.push("Nothing to flag.", "");
  for (const sev of SEVERITIES) {
    const group = insights.filter((i) => i.severity === sev);
    if (group.length === 0) continue;
    out.push(`### ${sev} (${group.length})`, "");
    for (const i of group) {
      const where = [i.app, i.host].filter(Boolean).join(" · ");
      out.push(`#### ${i.title}`, "");
      out.push(`${where} · \`${i.rule}\` · ${i.count} ${i.count === 1 ? "flow" : "flows"}`, "");
      out.push(i.detail, "");
      const doc = INSIGHT_DOCS[i.rule as keyof typeof INSIGHT_DOCS];
      if (doc) out.push(`*Why it matters:* ${doc.why}`, "");
      out.push(`**Fix:** ${i.advice}`, "");
    }
  }
  if (opts.mutedCount > 0) out.push(`_${opts.mutedCount} muted ${opts.mutedCount === 1 ? "finding" : "findings"} left out._`, "");

  out.push("## Hosts", "");
  out.push("| App | Host | Requests | Failed | P50 | P95 | Downloaded |");
  out.push("|---|---|---:|---:|---:|---:|---:|");
  for (const g of hosts) {
    for (const h of g.hosts) {
      out.push(
        `| ${cell(g.app || "unknown app")} | ${cell(h.host)} | ${h.requests} | ${h.failed} | ${formatSpan(h.p50)} | ${formatSpan(h.p95)} | ${formatBytes(h.bytes)} |`,
      );
    }
  }
  out.push("", "---", "_Query strings, header values and bodies are not included in this report._", "");
  return out.join("\n");
}
