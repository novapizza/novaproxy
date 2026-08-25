import type { Flow } from "./api";
import { protoOf, typeOf } from "./classify";

/**
 * The structured filter: field, operator, value — stackable, each row switchable.
 *
 * The search box answers "find me this string"; this answers "show me the rows
 * where *this field* does *this thing*", which is the question you cannot ask
 * with a substring. Proxyman's filter rows are the model (issues/0002 §8.1), and
 * the prefix syntax the search box already supports (`method:`, `host:`) is the
 * poorer version of the same idea — kept, because typing is faster than clicking
 * when you know what you want.
 *
 * Rows AND together. OR is deliberately absent: two rows that mean OR are two
 * saved filters, and an expression tree in a debugging toolbar is a language
 * nobody asked to learn.
 */

export type ClauseField =
  | "url"
  | "host"
  | "path"
  | "method"
  | "status"
  | "client"
  | "kind"
  | "proto"
  | "header";

export type ClauseOp =
  | "contains"
  | "notContains"
  | "is"
  | "isNot"
  | "startsWith"
  | "endsWith"
  | "matches"
  | "gt"
  | "lt";

export interface Clause {
  id: string;
  /** Off keeps the row and its value, and stops it narrowing anything. */
  enabled: boolean;
  field: ClauseField;
  op: ClauseOp;
  value: string;
}

export const FIELDS: { id: ClauseField; label: string; numeric?: boolean }[] = [
  { id: "url", label: "URL" },
  { id: "host", label: "Host" },
  { id: "path", label: "Path" },
  { id: "method", label: "Method" },
  { id: "status", label: "Status", numeric: true },
  { id: "client", label: "Client" },
  { id: "kind", label: "Kind" },
  { id: "proto", label: "Protocol" },
  { id: "header", label: "Header" },
];

const TEXT_OPS: ClauseOp[] = ["contains", "notContains", "is", "isNot", "startsWith", "endsWith", "matches"];
const NUM_OPS: ClauseOp[] = ["is", "isNot", "gt", "lt"];

export const OP_LABEL: Record<ClauseOp, string> = {
  contains: "contains",
  notContains: "does not contain",
  is: "is",
  isNot: "is not",
  startsWith: "begins with",
  endsWith: "ends with",
  matches: "matches regex",
  gt: "is over",
  lt: "is under",
};

/** Which operators a field can take. A numeric field has no `contains`. */
export function opsFor(field: ClauseField): ClauseOp[] {
  return FIELDS.find((f) => f.id === field)?.numeric ? NUM_OPS : TEXT_OPS;
}

export function newClause(): Clause {
  return { id: `c${Math.random().toString(36).slice(2, 8)}`, enabled: true, field: "url", op: "contains", value: "" };
}

/**
 * Keep a clause coherent after its field changes: an operator the new field
 * cannot take falls back to that field's first one, rather than silently
 * matching nothing.
 */
export function withField(clause: Clause, field: ClauseField): Clause {
  const ops = opsFor(field);
  return { ...clause, field, op: ops.includes(clause.op) ? clause.op : ops[0] };
}

/**
 * The value of a field, as text.
 *
 * `header` is `name: value` over every header on both sides, so one clause can
 * ask "did anything carry this token" — the question that sends people to the
 * Raw panel otherwise. The body is deliberately not a field: the flow list holds
 * no body bytes (see `withoutBytes` in `store.ts`), so a body clause would match
 * on whatever preview happened to survive and silently disagree with itself.
 */
function fieldText(f: Flow, field: ClauseField): string {
  switch (field) {
    case "url": return f.url;
    case "host": return f.host;
    case "path": return f.path;
    case "method": return f.method;
    case "status": return f.error ? "error" : String(f.status ?? "");
    case "client": return f.process ?? "";
    case "kind": return typeOf(f);
    case "proto": return protoOf(f);
    case "header":
      return [...f.request_headers, ...f.response_headers]
        .map((h) => `${h.name}: ${h.value}`)
        .join("\n");
  }
}

/** Does one clause hold for this flow? A blank value never narrows anything. */
export function matchClause(f: Flow, c: Clause): boolean {
  if (!c.enabled) return true;
  const needle = c.value.trim();
  if (needle === "") return true;
  const hay = fieldText(f, c.field);

  switch (c.op) {
    case "contains": return hay.toLowerCase().includes(needle.toLowerCase());
    case "notContains": return !hay.toLowerCase().includes(needle.toLowerCase());
    case "is": return hay.toLowerCase() === needle.toLowerCase();
    case "isNot": return hay.toLowerCase() !== needle.toLowerCase();
    case "startsWith": return hay.toLowerCase().startsWith(needle.toLowerCase());
    case "endsWith": return hay.toLowerCase().endsWith(needle.toLowerCase());
    case "matches": {
      // A half-typed regex is the normal state of a regex field, so an invalid
      // pattern matches nothing rather than throwing into the render.
      const re = compile(needle);
      return re ? re.test(hay) : false;
    }
    case "gt":
    case "lt": {
      const a = Number(hay);
      const b = Number(needle);
      if (!Number.isFinite(a) || !Number.isFinite(b)) return false;
      return c.op === "gt" ? a > b : a < b;
    }
  }
}

const cache = new Map<string, RegExp | null>();

function compile(pattern: string): RegExp | null {
  if (cache.has(pattern)) return cache.get(pattern)!;
  let re: RegExp | null = null;
  try {
    re = new RegExp(pattern, "i");
  } catch {
    re = null;
  }
  // Bounded: a user typing a regex produces one entry per keystroke, and this
  // map would otherwise grow for the life of the session.
  if (cache.size > 100) cache.clear();
  cache.set(pattern, re);
  return re;
}

/** True when a clause would actually narrow the view. */
export function clauseActive(c: Clause): boolean {
  return c.enabled && c.value.trim() !== "";
}

/** `Host is api.example.com` — for the saved-filter label and the tooltip. */
export function describeClause(c: Clause): string {
  const field = FIELDS.find((f) => f.id === c.field)?.label ?? c.field;
  return `${field} ${OP_LABEL[c.op]} ${c.value.trim()}`;
}

/** Rows AND together; a row that is off or empty is not a row. */
export function matchClauses(f: Flow, clauses: readonly Clause[]): boolean {
  for (const c of clauses) if (!matchClause(f, c)) return false;
  return true;
}
