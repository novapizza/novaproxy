import type { Flow } from "./api";
import { protoOf, statusClassOf, typeOf, type Proto, type FlowType, type StatusClass } from "./classify";
import { ALL_TRAFFIC, matchScope, type Scope, type ScopeContext } from "./scope";
import { FLOW_TYPES, PROTOS, STATUS_CLASSES } from "./classify";
import { clauseActive, describeClause, matchClauses, type Clause } from "./builder";

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
 * Whether a toast is reporting something that worked or something that failed.
 *
 * Explicit rather than sniffed out of the text: nothing in a string reliably
 * says "this is an error", and guessing wrong is how every failure came to be
 * announced under a green check mark.
 */
export type ToastKind = "ok" | "error";

/** How a toast is shown: its severity, and how long it stays. */
export interface ToastOptions {
  ms?: number;
  kind?: ToastKind;
}

/** The toast function every section is handed. */
export type ShowToast = (text: string, opts?: ToastOptions) => void;

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
  /**
   * Structured rows: field, operator, value. AND with everything else and with
   * each other (`src/builder.ts`).
   */
  clauses: Clause[];
  /**
   * Whether the narrowing above is in force.
   *
   * `⌘B` flips this rather than clearing anything: "show me everything for a
   * second" and "throw away the filter I just built" are different intentions,
   * and only one of them is undoable.
   */
  enabled: boolean;
}

export const EMPTY_FILTER: FlowFilter = {
  proto: new Set(),
  type: new Set(),
  status: new Set(),
  scope: ALL_TRAFFIC,
  query: "",
  clauses: [],
  includeInternal: false,
  enabled: true,
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
  // Rows count as one decision however many there are, like the chip groups:
  // "Reset filters (7)" for one idea reads as a bug.
  if (f.clauses.some(clauseActive)) n++;
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
  // Only rows that would narrow anything: an empty or switched-off row costs a
  // function call per flow per frame otherwise.
  const clauses = f.clauses.filter(clauseActive);
  // Switched off, everything shows — except NovaProxy's own traffic, which is
  // not a filter the user set but a default about whose capture this is.
  if (!f.enabled) return (flow) => includeInternal || !flow.internal;
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
    if (!matchClauses(flow, clauses)) return false;
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


/* ----------------------------- saved filters ------------------------------ */

/**
 * A filter, flattened for storage.
 *
 * `Set` and a tagged union do not survive `JSON.stringify` in a form that comes
 * back as itself, so a saved filter is stored as this and rebuilt on load —
 * defensively, since the blob on disk was written by an older build.
 */
export interface FilterJson {
  proto?: string[];
  type?: string[];
  status?: string[];
  scope?: Scope;
  query?: string;
  clauses?: Clause[];
  includeInternal?: boolean;
}

export function filterToJson(f: FlowFilter): FilterJson {
  return {
    proto: [...f.proto],
    type: [...f.type],
    status: [...f.status],
    scope: f.scope,
    query: f.query.trim(),
    clauses: f.clauses.filter(clauseActive),
    includeInternal: f.includeInternal,
  };
}

/**
 * Rebuild a filter from storage, dropping anything unrecognised.
 *
 * A chip id that no longer exists (a payload kind that was renamed, say) would
 * otherwise make a saved filter match nothing at all, which reads as "my saved
 * filter is broken" rather than "that chip is gone".
 */
export function filterFromJson(raw: unknown): FlowFilter {
  const j = (raw ?? {}) as FilterJson;
  const known = <T extends string>(list: unknown, valid: readonly { id: T }[]): Set<T> => {
    const ids = new Set(valid.map((v) => v.id as string));
    const arr = Array.isArray(list) ? list : [];
    return new Set(arr.filter((x): x is T => typeof x === "string" && ids.has(x)));
  };
  const scope = j.scope && typeof j.scope === "object" && "kind" in j.scope ? j.scope : ALL_TRAFFIC;
  return {
    proto: known(j.proto, PROTOS),
    type: known(j.type, FLOW_TYPES),
    status: known(j.status, STATUS_CLASSES),
    scope: scope as Scope,
    query: typeof j.query === "string" ? j.query : "",
    clauses: Array.isArray(j.clauses) ? j.clauses.filter(isClause) : [],
    includeInternal: j.includeInternal === true,
    enabled: true,
  };
}

/** A stored row is trusted only as far as its shape can be checked. */
function isClause(raw: unknown): raw is Clause {
  if (!raw || typeof raw !== "object") return false;
  const c = raw as Partial<Clause>;
  return (
    typeof c.id === "string" &&
    typeof c.field === "string" &&
    typeof c.op === "string" &&
    typeof c.value === "string"
  );
}

/**
 * What a filter actually narrows, in words.
 *
 * Groups keep their order (protocol, kind, status, then scope, conditions and
 * query) so two filters built the same way read the same way.
 *
 * An axis with **everything** selected is left out, because it narrows nothing —
 * an empty axis means the same thing. Listing all three protocols under a filter
 * that lets every protocol through would describe a restriction that is not
 * there.
 */
export function describeFilter(f: FlowFilter): string {
  const parts: string[] = [];
  const label = <T extends string>(
    set: ReadonlySet<T>,
    defs: readonly { id: T; label: string }[],
  ) => (set.size === defs.length ? [] : defs.filter((d) => set.has(d.id)).map((d) => d.label));
  parts.push(...label(f.proto, PROTOS));
  parts.push(...label(f.type, FLOW_TYPES));
  parts.push(...label(f.status, STATUS_CLASSES));
  if (f.scope.kind === "app") parts.push(f.scope.name || "unknown app");
  if (f.scope.kind === "host") parts.push(f.scope.host);
  if (f.scope.kind === "path") parts.push(`${f.scope.host}${f.scope.prefix}`);
  if (f.scope.kind === "pinned") parts.push("pinned");
  for (const c of f.clauses.filter(clauseActive)) parts.push(describeClause(c));
  const q = f.query.trim();
  if (q) parts.push(`“${q}”`);
  return parts.length > 0 ? parts.join(" · ") : "everything";
}

/**
 * A short name to pre-fill the Save dialog with.
 *
 * `describeFilter` spells out every part, which is right for a tooltip and far
 * too long for a chip — "HTTPS · JSON · 4xx · 5xx · api.example.com · “shoots”"
 * is not a name. This keeps the two most *identifying* parts instead of the
 * first two: a host or a search term says which piece of work this is, where
 * `HTTPS` says almost nothing, so the order here is deliberately not
 * `describeFilter`'s.
 */
export function suggestFilterName(f: FlowFilter): string {
  const parts: string[] = [];
  const q = f.query.trim();
  if (q) parts.push(q);
  if (f.scope.kind === "app") parts.push(f.scope.name || "unknown app");
  if (f.scope.kind === "host") parts.push(f.scope.host);
  if (f.scope.kind === "path") parts.push(f.scope.prefix);
  if (f.scope.kind === "pinned") parts.push("pinned");
  const pick = <T extends string>(set: ReadonlySet<T>, defs: readonly { id: T; label: string }[]) =>
    defs.filter((d) => set.has(d.id)).map((d) => d.label);
  // A full axis is not a distinguishing feature — it filters nothing — so it
  // never contributes to the name.
  if (f.type.size > 0 && f.type.size < FLOW_TYPES.length) parts.push(...pick(f.type, FLOW_TYPES));
  if (f.status.size > 0 && f.status.size < STATUS_CLASSES.length) {
    parts.push(...pick(f.status, STATUS_CLASSES));
  }
  if (f.proto.size > 0 && f.proto.size < PROTOS.length) parts.push(...pick(f.proto, PROTOS));
  if (f.clauses.filter(clauseActive).length > 0 && parts.length === 0) parts.push("conditions");

  const name = parts.slice(0, 2).join(" · ");
  if (name === "") return "Filter";
  return name.length > 28 ? `${name.slice(0, 27)}…` : name;
}

/** A named filter, kept across launches. */
export interface SavedFilter {
  id: string;
  label: string;
  filter: FilterJson;
}
