import type { Flow } from "./api";

/**
 * Match a flow against the search query. Supports `method:`, `status:`,
 * `host:` and `app:` prefixes; otherwise free-text over host/path/method/
 * status/process.
 */
export function matchQuery(f: Flow, q: string): boolean {
  q = q.trim().toLowerCase();
  if (!q) return true;
  if (q.startsWith("method:")) return f.method.toLowerCase() === q.slice(7).trim();
  if (q.startsWith("status:")) return String(f.status ?? "") === q.slice(7).trim();
  if (q.startsWith("host:")) return f.host.toLowerCase().includes(q.slice(5).trim());
  if (q.startsWith("app:")) return (f.process ?? "").toLowerCase().includes(q.slice(4).trim());
  return `${f.host} ${f.path} ${f.method} ${f.status ?? ""} ${f.process ?? ""}`.toLowerCase().includes(q);
}

/** Apply the app dropdown filter (exact match) plus the search query. */
export function filterFlows(flows: Flow[], query: string, appFilter: string): Flow[] {
  return flows.filter((f) => (!appFilter || f.process === appFilter) && matchQuery(f, query));
}

/** Distinct originating apps observed in captured traffic, sorted for display. */
export function distinctApps(flows: Flow[]): string[] {
  const set = new Set<string>();
  for (const f of flows) if (f.process) set.add(f.process);
  return [...set].sort((a, b) => a.localeCompare(b));
}

/**
 * How long a toast stays visible. Longer messages (typically errors) need
 * more reading time: scale with length, clamped to a sane range, unless an
 * explicit duration is given.
 */
export function toastDuration(text: string, ms?: number): number {
  return ms ?? Math.min(9000, Math.max(2600, 2000 + text.length * 55));
}
