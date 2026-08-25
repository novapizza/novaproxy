import type { Flow } from "./api";

/**
 * The sidebar tree: which slice of the capture the table is looking at.
 *
 * A scope is not a query. It answers "where am I" — an app, a host, a path
 * prefix — and it ANDs with the chips and the search box rather than replacing
 * them. That separation is what lets "Google Chrome" stay selected while the
 * status chips change underneath.
 *
 * There is no `saved` scope. A saved filter *is* a whole filter (Proxyman shows
 * it as a chip in the chip row, not as a tree row), so it replaces the filter
 * instead of narrowing one — see `FlowFilter` in `filter.ts`.
 */
export type Scope =
  | { kind: "all" }
  /** Flows the user pinned. Membership lives outside the flow, so it is passed in. */
  | { kind: "pinned" }
  /** Exact `Flow.process`. `""` is the bucket for flows with no attribution. */
  | { kind: "app"; name: string }
  | { kind: "host"; host: string }
  /** One host and a path prefix, e.g. `/v3/shoots`. Matches the prefix or below it. */
  | { kind: "path"; host: string; prefix: string };

export const ALL_TRAFFIC: Scope = { kind: "all" };

/** What the scope needs to know that a `Flow` does not carry. */
export interface ScopeContext {
  /** Ids of pinned flows; only read by the `pinned` scope. */
  pinned?: ReadonlySet<string>;
}

export function matchScope(f: Flow, scope: Scope, ctx: ScopeContext = {}): boolean {
  switch (scope.kind) {
    case "all":
      return true;
    case "pinned":
      return ctx.pinned?.has(f.id) ?? false;
    case "app":
      // An unattributed flow answers to the "no app" bucket, not to every app.
      return (f.process ?? "") === scope.name;
    case "host":
      return f.host === scope.host;
    case "path":
      return f.host === scope.host && isUnder(f.path, scope.prefix);
  }
}

/** True when `path` is the prefix itself or a descendant of it — never a sibling. */
function isUnder(path: string, prefix: string): boolean {
  if (path === prefix) return true;
  if (!path.startsWith(prefix)) return false;
  // `/v3/shoots` must not swallow `/v3/shootsummary`: the next character has to
  // be a boundary.
  const next = path[prefix.length];
  return next === "/" || next === "?";
}

/** A stable string for a scope, for React keys and for comparing selections. */
export function scopeKey(scope: Scope): string {
  switch (scope.kind) {
    case "all": return "all";
    case "pinned": return "pinned";
    case "app": return `app:${scope.name}`;
    case "host": return `host:${scope.host}`;
    case "path": return `path:${scope.host}${scope.prefix}`;
  }
}

export function sameScope(a: Scope, b: Scope): boolean {
  return scopeKey(a) === scopeKey(b);
}

/* --------------------------------- the tree -------------------------------- */

export interface AppNode {
  /** `Flow.process`, or `""` for flows with no attribution. */
  name: string;
  count: number;
  /** Hosts this app talked to, by descending count. */
  hosts: { host: string; count: number }[];
}

export interface PathNode {
  /** The last segment, for display: `shoots`. */
  segment: string;
  /** The whole prefix, for the scope: `/v3/shoots`. */
  prefix: string;
  count: number;
  children: PathNode[];
}

export interface HostNode {
  host: string;
  count: number;
  /** True when at least one flow on this host was HTTPS. */
  tls: boolean;
  children: PathNode[];
}

export interface ScopeTree {
  apps: AppNode[];
  domains: HostNode[];
  /** Flows counted into the tree. */
  total: number;
}

/**
 * How deep the path tree goes.
 *
 * Two levels, not unbounded. The tree is rebuilt whenever the capture changes,
 * so its cost is paid per frame during a heavy capture; a full path tree over
 * 10k flows with eight-segment URLs is tens of thousands of map operations for
 * nodes nobody expanded. Two levels is what a person navigates by (`/v3` →
 * `/v3/shoots`); below that the search box is the better tool.
 */
export const MAX_PATH_DEPTH = 2;

/**
 * Build the sidebar tree in one pass.
 *
 * Counts are of the flows handed in — the caller decides whether NovaProxy's
 * own traffic is among them — and are *not* narrowed by the chips: a count that
 * moved every time a chip changed would stop being a landmark.
 */
export function buildScopeTree(flows: Flow[]): ScopeTree {
  const apps = new Map<string, { count: number; hosts: Map<string, number> }>();
  const hosts = new Map<string, { count: number; tls: boolean; kids: Map<string, PathAcc> }>();

  for (const f of flows) {
    const app = f.process ?? "";
    let a = apps.get(app);
    if (!a) apps.set(app, (a = { count: 0, hosts: new Map() }));
    a.count++;
    a.hosts.set(f.host, (a.hosts.get(f.host) ?? 0) + 1);

    let h = hosts.get(f.host);
    if (!h) hosts.set(f.host, (h = { count: 0, tls: false, kids: new Map() }));
    h.count++;
    if (f.scheme.toLowerCase() === "https") h.tls = true;
    addPath(h.kids, segmentsOf(f.path), 0);
  }

  return {
    total: flows.length,
    apps: [...apps.entries()]
      .map(([name, v]) => ({
        name,
        count: v.count,
        hosts: [...v.hosts.entries()]
          .map(([host, count]) => ({ host, count }))
          .sort(byCountThenName((x) => x.host)),
      }))
      .sort(byAppOrder),
    domains: [...hosts.entries()]
      .map(([host, v]) => ({ host, count: v.count, tls: v.tls, children: freeze(v.kids) }))
      .sort(byCountThenName((x) => x.host)),
  };
}

interface PathAcc {
  segment: string;
  prefix: string;
  count: number;
  kids: Map<string, PathAcc>;
}

/** Path segments, query dropped: `/v3/shoots/4821?page=2` → `["v3","shoots","4821"]`. */
function segmentsOf(path: string): string[] {
  const q = path.indexOf("?");
  const clean = q === -1 ? path : path.slice(0, q);
  return clean.split("/").filter((s) => s !== "");
}

function addPath(into: Map<string, PathAcc>, segs: string[], depth: number, prefix = "") {
  if (depth >= MAX_PATH_DEPTH || depth >= segs.length) return;
  const seg = segs[depth];
  const full = `${prefix}/${seg}`;
  let node = into.get(seg);
  if (!node) into.set(seg, (node = { segment: seg, prefix: full, count: 0, kids: new Map() }));
  node.count++;
  addPath(node.kids, segs, depth + 1, full);
}

function freeze(kids: Map<string, PathAcc>): PathNode[] {
  return [...kids.values()]
    .map((k) => ({ segment: k.segment, prefix: k.prefix, count: k.count, children: freeze(k.kids) }))
    .sort(byCountThenName((x) => x.segment));
}

/**
 * Apps busiest first — except the unattributed bucket, which always sits last.
 *
 * Sorting it by name would float it to the top (its name is `""`), putting "we
 * could not tell" above every app the user recognises. It is a fallback, so it
 * reads as one.
 */
function byAppOrder(a: AppNode, b: AppNode): number {
  if ((a.name === "") !== (b.name === "")) return a.name === "" ? 1 : -1;
  return b.count - a.count || a.name.localeCompare(b.name);
}

/**
 * Busiest first, ties broken by name.
 *
 * Count alone is not a stable order: two hosts with the same count would swap
 * places on every rebuild, and a list that reshuffles while you are reading it
 * is worse than one in the wrong order.
 */
function byCountThenName<T extends { count: number }>(name: (x: T) => string) {
  return (a: T, b: T) => b.count - a.count || name(a).localeCompare(name(b));
}
