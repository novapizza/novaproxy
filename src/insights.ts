import type { Insight } from "./bindings/Insight";
import type { InsightReport } from "./bindings/InsightReport";
import type { InsightSeverity } from "./bindings/InsightSeverity";
import type { Flow } from "./api";
import type { FlowFilter } from "./filter";
import { ALL_TRAFFIC, type Scope } from "./scope";

/**
 * The Insights page: what the engine's anti-pattern checks found
 * (`nova_core::insights`), and the view decisions layered on top.
 *
 * The checks run in Rust so MCP's `get_insights` and this page cannot
 * disagree. What lives here is only what the user decided — which findings are
 * muted — and how that changes what the rail badge counts.
 */

/**
 * How often the page asks for a fresh report while recording.
 *
 * The engine answers from a cache until the flow store changes, so this is the
 * debounce: often enough that a burst shows up while you are still looking at
 * the app that caused it, rarely enough that rows do not churn under the cursor.
 */
export const INSIGHT_POLL_MS = 3_000;

export const SEVERITY_LABEL: Record<InsightSeverity, string> = {
  High: "High",
  Warning: "Warning",
  Info: "Info",
};

const RANK: Record<InsightSeverity, number> = { High: 2, Warning: 1, Info: 0 };

/** The report split into what to show and what the user muted. Order is kept. */
export function partition(
  report: InsightReport | null,
  muted: ReadonlySet<string>,
): { shown: Insight[]; muted: Insight[] } {
  const shown: Insight[] = [];
  const hidden: Insight[] = [];
  for (const i of report?.insights ?? []) (muted.has(i.id) ? hidden : shown).push(i);
  return { shown, muted: hidden };
}

/**
 * What the rail badge says: how many unmuted findings, and the worst of them.
 *
 * `null` when there is nothing to say, so the rail shows no badge at all rather
 * than a zero — a permanent "0" next to an icon reads as a broken counter.
 */
export function badge(
  report: InsightReport | null,
  muted: ReadonlySet<string>,
): { count: number; severity: InsightSeverity } | null {
  const { shown } = partition(report, muted);
  if (shown.length === 0) return null;
  const severity = shown.reduce<InsightSeverity>(
    (worst, i) => (RANK[i.severity] > RANK[worst] ? i.severity : worst),
    "Info",
  );
  return { count: shown.length, severity };
}

/** Per-severity counts of the shown findings, for the page header. */
export function severityCounts(insights: Insight[]): Record<InsightSeverity, number> {
  const out: Record<InsightSeverity, number> = { High: 0, Warning: 0, Info: 0 };
  for (const i of insights) out[i.severity]++;
  return out;
}

/** The flow-table scope that shows exactly this finding's flows. */
export function insightScope(i: Insight): Scope {
  return { kind: "insight", id: i.id, title: i.title, flowIds: new Set(i.flow_ids) };
}

/** Mute or unmute one finding, returning the new list for prefs. */
export function toggleMuted(muted: readonly string[], id: string): string[] {
  return muted.includes(id) ? muted.filter((m) => m !== id) : [...muted, id];
}

/**
 * The filter as the Insights page applies it.
 *
 * An insight scope is set by this page's own "Show flows", so following it
 * back would collapse the page to the one finding just opened; it counts as
 * all traffic here.
 */
export function filterForInsights(f: FlowFilter): FlowFilter {
  return f.scope.kind === "insight" ? { ...f, scope: ALL_TRAFFIC } : f;
}

/**
 * The report narrowed to findings with at least one flow the filter lets
 * through — "Chrome" shows Chrome's problems — while every check still ran over
 * the whole capture, so a pattern is never missed for want of context.
 *
 * `flows` is the list the table filters; a finding whose flows have all left
 * it (evicted) is not in view either. Each kept finding carries how many of its
 * flows matched, for "3 of 12 in view".
 */
export function insightsInView(
  report: InsightReport | null,
  flows: readonly Flow[],
  pass: (f: Flow) => boolean,
): { report: InsightReport | null; inView: Map<string, number>; hidden: number } {
  const inView = new Map<string, number>();
  if (!report) return { report, inView, hidden: 0 };
  const wanted = new Set<string>();
  for (const i of report.insights) for (const id of i.flow_ids) wanted.add(id);
  const passing = new Set<string>();
  for (const f of flows) if (wanted.has(f.id) && pass(f)) passing.add(f.id);
  const kept = report.insights.filter((i) => {
    const n = i.flow_ids.reduce((acc, id) => acc + (passing.has(id) ? 1 : 0), 0);
    if (n > 0) inView.set(i.id, n);
    return n > 0;
  });
  return { report: { ...report, insights: kept }, inView, hidden: report.insights.length - kept.length };
}
