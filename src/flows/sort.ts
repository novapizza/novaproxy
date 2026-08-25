import type { Flow } from "../api";
import { num } from "../format";
import type { ColumnId } from "./columns";

/**
 * Sorting the table.
 *
 * Capture order — newest first — is a sort in its own right and the one the
 * table opens in, so `null` is a first-class state rather than "unsorted junk":
 * clicking a header three times returns to it, because a table you cannot put
 * back is a table you stop clicking.
 */

export type SortDir = "asc" | "desc";

export interface Sort {
  by: ColumnId;
  dir: SortDir;
}

/** Columns worth ordering by. `ssl` and `url` sort as text; `seq` is capture order. */
const COMPARATORS: Partial<Record<ColumnId, (a: Flow, b: Flow) => number>> = {
  seq: (a, b) => num(a.seq) - num(b.seq),
  url: (a, b) => a.url.localeCompare(b.url),
  client: (a, b) => (a.process ?? "").localeCompare(b.process ?? ""),
  method: (a, b) => a.method.localeCompare(b.method),
  // A flow with no status yet sorts after every answered one rather than as a
  // zero: "in flight" is not "less than 200".
  status: (a, b) => (a.status ?? Infinity) - (b.status ?? Infinity),
  time: (a, b) => a.started_at - b.started_at,
  duration: (a, b) => (a.duration_ms ?? Infinity) - (b.duration_ms ?? Infinity),
  request: (a, b) => num(a.request_size) - num(b.request_size),
  response: (a, b) => num(a.response_size) - num(b.response_size),
  protocol: (a, b) => a.http_version.localeCompare(b.http_version),
};

export function sortable(id: ColumnId): boolean {
  return id in COMPARATORS;
}

/**
 * Next state for a header click: ascending, then descending, then back to
 * capture order.
 */
export function cycleSort(current: Sort | null, by: ColumnId): Sort | null {
  if (!sortable(by)) return current;
  if (!current || current.by !== by) return { by, dir: "asc" };
  if (current.dir === "asc") return { by, dir: "desc" };
  return null;
}

/**
 * Sort a copy, never in place — the array handed in is the memoised result of
 * filtering, and sorting it under React would mutate a value another render is
 * still holding.
 *
 * Ties break on `seq`, which is unique and monotonic: without that, two flows
 * with the same duration swap places on every re-render and the table shimmers
 * while you read it.
 */
export function sortFlows(flows: Flow[], sort: Sort | null): Flow[] {
  if (!sort) return flows;
  const cmp = COMPARATORS[sort.by];
  if (!cmp) return flows;
  const sign = sort.dir === "asc" ? 1 : -1;
  return [...flows].sort((a, b) => sign * (cmp(a, b) || num(a.seq) - num(b.seq)));
}

/** The glyph a header carries: `▲`, `▼`, or nothing. */
export function sortGlyph(sort: Sort | null, id: ColumnId): string {
  if (!sort || sort.by !== id) return "";
  return sort.dir === "asc" ? "▲" : "▼";
}
