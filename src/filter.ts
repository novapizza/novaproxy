import type { Flow } from "./api";

/**
 * Match a flow against the search query. Supports `method:`, `status:`,
 * `host:`, `app:` and `mcp:` prefixes; otherwise free-text over host/path/
 * method/status/process, plus the MCP method and tool when present.
 */
export function matchQuery(f: Flow, q: string): boolean {
  q = q.trim().toLowerCase();
  if (!q) return true;
  if (q.startsWith("method:")) return f.method.toLowerCase() === q.slice(7).trim();
  if (q.startsWith("status:")) return String(f.status ?? "") === q.slice(7).trim();
  if (q.startsWith("host:")) return f.host.toLowerCase().includes(q.slice(5).trim());
  if (q.startsWith("app:")) return (f.process ?? "").toLowerCase().includes(q.slice(4).trim());
  // `mcp:` with no term means "any MCP traffic"; with one, match the JSON-RPC
  // method or the tool being called.
  if (q.startsWith("mcp:")) {
    const term = q.slice(4).trim();
    if (!f.mcp) return false;
    return !term || mcpLabel(f).toLowerCase().includes(term);
  }
  const haystack = `${f.host} ${f.path} ${f.method} ${f.status ?? ""} ${f.process ?? ""} ${
    f.mcp ? mcpLabel(f) : ""
  }`;
  return haystack.toLowerCase().includes(q);
}

/** `tools/call → read_file` for an MCP flow; empty for anything else. */
export function mcpLabel(f: Flow): string {
  if (!f.mcp) return "";
  const { method, tool } = f.mcp;
  if (method && tool) return `${method} → ${tool}`;
  return method ?? "mcp";
}

/**
 * The one-click filters above the list. Mutually exclusive — they answer
 * "which slice am I looking at", not "which flags are set".
 */
export type FlowChip = "all" | "errors" | "slow" | "mcp";

export const FLOW_CHIPS: { id: FlowChip; label: string }[] = [
  { id: "all", label: "All" },
  { id: "errors", label: "Errors" },
  { id: "slow", label: "Slow" },
  { id: "mcp", label: "MCP" },
];

/** Above this, a request is worth a second look. */
export const SLOW_MS = 300;

/**
 * A flow still in flight has no duration and no status yet, so it cannot be
 * called slow or failed — it is excluded from both slices rather than assumed
 * healthy.
 */
export function matchChip(f: Flow, chip: FlowChip): boolean {
  switch (chip) {
    case "errors":
      return f.error != null || (f.status != null && f.status >= 400);
    case "slow":
      return f.duration_ms != null && f.duration_ms >= SLOW_MS;
    case "mcp":
      return f.mcp != null;
    default:
      return true;
  }
}

/** Which flows the list shows. */
export interface ViewFilters {
  /** Exact app/process name from the dropdown; empty means all. */
  app?: string;
  /** Active one-click filter; defaults to `all`. */
  chip?: FlowChip;
  /** Show NovaProxy's own traffic (its MCP endpoint and replays it issued). */
  includeInternal?: boolean;
}

/**
 * Apply the view filters plus the search query.
 *
 * NovaProxy's own traffic is dropped unless asked for: with the MCP endpoint
 * enabled, an agent's tool calls would otherwise dominate the list you are
 * trying to read.
 */
export function filterFlows(flows: Flow[], query: string, filters: ViewFilters = {}): Flow[] {
  return flows.filter((f) => {
    if (f.internal && !filters.includeInternal) return false;
    if (filters.chip && !matchChip(f, filters.chip)) return false;
    if (filters.app && f.process !== filters.app) return false;
    return matchQuery(f, query);
  });
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
