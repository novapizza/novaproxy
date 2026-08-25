import type { Pair } from "./parts";

/**
 * The key/value grid every text panel in the inspector is made of: headers,
 * query parameters, cookies, summaries.
 *
 * One component rather than four near-identical tables, because the alignment of
 * the key column is what makes two panes readable side by side — the request's
 * headers and the response's have to line up, and four implementations would
 * drift apart the first time one of them needed a wider key.
 */
export function KeyValueTable({ rows, empty }: { rows: Pair[]; empty: string }) {
  if (rows.length === 0) return <div className="kv-empty">— {empty} —</div>;
  return (
    <div className="kv">
      <div className="kv-head">
        <span>Key</span>
        <span>Value</span>
      </div>
      {rows.map((r, i) => (
        <div className="kv-row" key={`${r.k}:${i}`}>
          <span className="k">{r.k}</span>
          <span className="v">{r.v}</span>
        </div>
      ))}
    </div>
  );
}
