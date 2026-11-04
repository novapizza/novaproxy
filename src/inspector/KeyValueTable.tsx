import { useEffect, useRef, useState } from "react";
import { api } from "../api";
import { Icon } from "../icons";
import type { Pair } from "./parts";

/** How long a row's copy button shows the check after copying. */
const COPIED_MS = 1200;

/**
 * The key/value grid every text panel in the inspector is made of: headers,
 * query parameters, cookies, summaries.
 *
 * One component rather than four near-identical tables, because the alignment of
 * the key column is what makes two panes readable side by side — the request's
 * headers and the response's have to line up, and four implementations would
 * drift apart the first time one of them needed a wider key.
 *
 * Each row copies its value: selecting a long token or user agent by dragging
 * is error-prone once it wraps, and the pane's Copy takes every row at once.
 */
export function KeyValueTable({ rows, empty }: { rows: Pair[]; empty: string }) {
  const [copied, setCopied] = useState<string | null>(null);
  const timer = useRef<number | undefined>(undefined);
  useEffect(() => () => window.clearTimeout(timer.current), []);

  if (rows.length === 0) return <div className="kv-empty">— {empty} —</div>;

  const copy = (id: string, value: string) => {
    void api.copyText(value).then(() => {
      setCopied(id);
      window.clearTimeout(timer.current);
      timer.current = window.setTimeout(() => setCopied(null), COPIED_MS);
    }, () => {});
  };

  return (
    <div className="kv">
      <div className="kv-head">
        <span>Key</span>
        <span>Value</span>
        <span />
      </div>
      {rows.map((r, i) => {
        const id = `${r.k}:${i}`;
        const done = copied === id;
        return (
          <div className="kv-row" key={id}>
            <span className="k">{r.k}</span>
            <span className="v">{r.v}</span>
            {r.v !== "" && (
              <button
                type="button"
                className={done ? "kv-copy done" : "kv-copy"}
                title={done ? "Copied" : "Copy value"}
                aria-label={`Copy value of ${r.k}`}
                onClick={() => copy(id, r.v)}
              >
                <Icon name={done ? "check" : "copy"} size={12} />
              </button>
            )}
          </div>
        );
      })}
    </div>
  );
}
