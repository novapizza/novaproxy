import type { Flow } from "./api";
import { protoOf, statusClassOf, typeOf, type Proto, type FlowType, type StatusClass } from "./classify";
import { ALL_TRAFFIC, matchScope, type Scope, type ScopeContext } from "./scope";

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
 * The one-click filters above the *old* list. Mutually exclusive — they answer
 * "which slice am I looking at", not "which flags are set".
 *
 * Superseded by the three chip groups below (`FlowFilter`), and removed with
 * the list view itself. Kept only so the shipped UI keeps compiling until the
 * table replaces it.
 *
 * @deprecated use `FlowFilter`
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


/* ------------------------------ the filter ------------------------------- */

/**
 * Everything narrowing the flows table, in one value.
 *
 * The three chip sets are **OR inside a set, AND between sets**, and an empty
 * set means "all of it" — which is why there is no `All` chip to switch back
 * to, only Reset. That shape is the point: `type: {json} + status: {4xx, 5xx}`
 * — "which API is failing" — is the query the tool exists for, and it is
 * exactly what one mutually-exclusive chip row cannot say.
 *
 * `scope` comes from the sidebar tree and ANDs with the rest, so picking an app
 * does not disturb the chips.
 */
export interface FlowFilter {
  proto: ReadonlySet<Proto>;
  type: ReadonlySet<FlowType>;
  status: ReadonlySet<StatusClass>;
  scope: Scope;
  /** Free text, with the `method:` / `host:` / `app:` / `mcp:` prefixes. */
  query: string;
  /** Show NovaProxy's own traffic (its MCP endpoint, and replays it issued). */
  includeInternal: boolean;
}

export const EMPTY_FILTER: FlowFilter = {
  proto: new Set(),
  type: new Set(),
  status: new Set(),
  scope: ALL_TRAFFIC,
  query: "",
  includeInternal: false,
};

/**
 * How many things are narrowing the view right now.
 *
 * Counts *groups*, not chips: three status chips are one decision, and "Reset
 * filters (5)" for a single idea reads as a bug. `includeInternal` is not
 * counted — it is a default, not something the user set.
 */
export function activeFilterCount(f: FlowFilter): number {
  let n = 0;
  if (f.proto.size) n++;
  if (f.type.size) n++;
  if (f.status.size) n++;
  if (f.scope.kind !== "all") n++;
  if (f.query.trim()) n++;
  return n;
}

export function isFiltering(f: FlowFilter): boolean {
  return activeFilterCount(f) > 0;
}

/**
 * Compile a filter into one predicate.
 *
 * Compiled once per filter change rather than re-read per flow: the table runs
 * this over up to `MAX_FLOWS` rows on every capture frame, so the set lookups
 * and the empty-set checks are hoisted out of the loop.
 *
 * Order is cheapest-first — the query, which lowercases and concatenates, runs
 * last and only for flows that survived everything else.
 */
export function buildPredicate(f: FlowFilter, ctx: ScopeContext = {}): (flow: Flow) => boolean {
  const { proto, type, status, scope, includeInternal } = f;
  const query = f.query.trim();
  const anyProto = proto.size === 0;
  const anyType = type.size === 0;
  const anyStatus = status.size === 0;
  const anyScope = scope.kind === "all";

  return (flow) => {
    if (flow.internal && !includeInternal) return false;
    if (!anyProto && !proto.has(protoOf(flow))) return false;
    if (!anyStatus) {
      const sc = statusClassOf(flow);
      // A flow still in flight belongs to no status class, so a status filter
      // hides it rather than guessing which one it will land in.
      if (sc == null || !status.has(sc)) return false;
    }
    if (!anyType && !type.has(typeOf(flow))) return false;
    if (!anyScope && !matchScope(flow, scope, ctx)) return false;
    return query === "" || matchQuery(flow, query);
  };
}

/** Apply a whole filter. */
export function applyFilter(flows: Flow[], f: FlowFilter, ctx: ScopeContext = {}): Flow[] {
  return flows.filter(buildPredicate(f, ctx));
}

/** Toggle one chip in a set, returning a new set (empty means "all"). */
export function toggleIn<T>(set: ReadonlySet<T>, value: T): Set<T> {
  const next = new Set(set);
  if (!next.delete(value)) next.add(value);
  return next;
}
