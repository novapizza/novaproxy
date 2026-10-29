import type { BodyPreview, Flow } from "../api";
import { bodyToText } from "./BodyPanel";
import { parseCookies, parseQuery, rawHttp, summaryOf, type Pair } from "./parts";
import { formatMs, timingBreakdown } from "../timing";
import type { PaneSide } from "./Inspector";

/**
 * The active tab of a pane as plain text, for the pane's Copy action.
 *
 * Each tab copies in the shape it is read in: headers and summaries as
 * `name: value` lines, query and cookies as `name=value`, Raw as the HTTP text
 * the tab shows. Body and Treeview copy the body itself (JSON pretty-printed,
 * as the tab shows it); the caller passes the body with its bytes, because the
 * list keeps none.
 *
 * `null` means there is nothing to copy as text — no headers, a binary body —
 * and the caller says so rather than putting an empty string on the clipboard.
 */
export function paneText(
  flow: Flow,
  side: PaneSide,
  tab: string,
  body: BodyPreview | null,
): string | null {
  const lines = (rows: Pair[], sep: string) =>
    rows.length ? rows.map((r) => `${r.k}${sep}${r.v}`).join("\n") : null;
  switch (tab) {
    case "Header": {
      const headers = side === "request" ? flow.request_headers : flow.response_headers;
      return lines(headers.map((h) => ({ k: h.name, v: h.value })), ": ");
    }
    case "Query":
      return lines(parseQuery(flow.url), "=");
    case "Cookies":
      return lines(parseCookies(flow, side), "=");
    case "Summary":
      return lines(summaryOf(flow, side), ": ");
    case "Raw":
      return rawHttp(flow, side);
    case "Body":
    case "Treeview":
      return bodyToText(body) || null;
    case "Timing": {
      const b = timingBreakdown(flow);
      if (b.empty) return null;
      const rows = b.phases.map((p) => ({ k: p.label, v: formatMs(p.ms) }));
      if (b.totalMs != null) rows.push({ k: "Total", v: formatMs(b.totalMs) });
      return lines(rows, ": ");
    }
    default:
      return null;
  }
}
