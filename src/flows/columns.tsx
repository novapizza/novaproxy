import { useEffect, useState, type ReactNode } from "react";
import type { Flow } from "../api";
import { appIcon, onIconsChanged } from "../appicons";
import { useStore } from "../store";
import { Icon } from "../icons";
import { methodClass, statusClass, statusText } from "../badges";
import { formatCellBytes, formatClock } from "../format";
import { formatMs } from "../timing";
import { mcpLabel } from "../filter";

/**
 * The flows table's columns.
 *
 * One declaration per column, holding its grid track *and* its cell renderer, so
 * the header and the body cannot disagree about how many tracks there are — the
 * failure mode of hand-written table markup, and the one that shows up only
 * after a column becomes optional.
 */

export type ColumnId =
  | "seq"
  | "url"
  | "client"
  | "method"
  | "status"
  | "time"
  | "duration"
  | "request"
  | "response"
  | "ssl"
  | "protocol"
  | "edited"
  | "comment";

export interface Column {
  id: ColumnId;
  label: string;
  /** A CSS grid track. Exactly one column is allowed to be flexible. */
  track: string;
  align?: "right";
  cell: (f: Flow) => ReactNode;
}

/** Why this flow's transport is (or is not) readable. */
function sslCell(f: Flow) {
  if (f.tunneled) {
    return (
      <span className="ssl tunneled" title="Tunneled — not decrypted, so no body was captured">
        <Icon name="lock" size={12} />
      </span>
    );
  }
  if (f.scheme.toLowerCase() === "https") {
    return (
      <span className="ssl on" title="HTTPS, decrypted">
        <Icon name="lock" size={12} />
      </span>
    );
  }
  return (
    <span className="ssl off" title="Plaintext HTTP">
      <Icon name="lock-open" size={12} />
    </span>
  );
}

export const COLUMNS: Record<ColumnId, Column> = {
  seq: {
    id: "seq",
    label: "#",
    track: "52px",
    cell: (f) => (
      <>
        <span className={`rdot ${statusClass(f.status, f.error)}`} />
        {String(f.seq)}
      </>
    ),
  },
  url: {
    id: "url",
    label: "URL",
    track: "minmax(320px, 1fr)",
    cell: (f) => (
      <>
        <PinMark flow={f} />
        <span className="u">{f.url}</span>
        {f.mcp && <span className="tag mcp" title={mcpLabel(f)}>MCP</span>}
        {f.is_websocket && <span className="tag ws">WS</span>}
        {f.mapped_from && <span className="tag map" title={`mapped from ${f.mapped_from}`}>MAP</span>}
        {f.resent && <span className="tag resent">RESENT</span>}
      </>
    ),
  },
  client: {
    id: "client",
    label: "Client",
    track: "132px",
    cell: (f) => <ClientCell flow={f} />,
  },
  method: {
    id: "method",
    label: "Method",
    track: "62px",
    cell: (f) => <span className={`badge ${methodClass(f.method)}`}>{f.method}</span>,
  },
  status: {
    id: "status",
    label: "Status",
    track: "58px",
    cell: (f) => (
      <span className={`status-pill ${statusClass(f.status, f.error)}`} title={f.error ?? undefined}>
        {statusText(f.status, f.error)}
      </span>
    ),
  },
  time: { id: "time", label: "Time", track: "96px", cell: (f) => formatClock(f.started_at) },
  duration: {
    id: "duration",
    label: "Duration",
    track: "78px",
    align: "right",
    cell: (f) => (f.duration_ms != null ? formatMs(f.duration_ms) : "–"),
  },
  request: {
    id: "request",
    label: "Request",
    track: "78px",
    align: "right",
    cell: (f) => formatCellBytes(f.request_size),
  },
  response: {
    id: "response",
    label: "Response",
    track: "78px",
    align: "right",
    cell: (f) => formatCellBytes(f.response_size),
  },
  ssl: { id: "ssl", label: "SSL", track: "44px", align: "right", cell: sslCell },
  protocol: {
    id: "protocol",
    label: "Protocol",
    track: "84px",
    cell: (f) => f.http_version,
  },
  comment: {
    id: "comment",
    label: "Comment",
    track: "160px",
    cell: (f) => <CommentCell flow={f} />,
  },
  edited: {
    id: "edited",
    label: "Edited",
    track: "70px",
    // Three letters rather than one pencil: "why is this response wrong?" has
    // three different answers, and which one it was is the whole point.
    cell: (f) => (
      <>
        {f.edits.rule && <span className="etag rule" title="A rule rewrote this">R</span>}
        {f.edits.script && <span className="etag script" title="The script hook changed this">S</span>}
        {f.edits.breakpoint && (
          <span className="etag bp" title="Edited at a breakpoint">B</span>
        )}
        {!f.edits.rule && !f.edits.script && !f.edits.breakpoint && "–"}
      </>
    ),
  },
};

/**
 * A pin, on the row itself.
 *
 * Without it the only way to see what is pinned is to switch to the Pinned
 * scope — which is exactly the switch a pin is meant to save you.
 */
function PinMark({ flow }: { flow: Flow }) {
  const pinned = useStore((s) => s.pinned.has(flow.id));
  if (!pinned) return null;
  return (
    <span className="pinmark" title="Pinned">
      <Icon name="pin" size={11} />
    </span>
  );
}

/** The note the user wrote on this row, if any. */
function CommentCell({ flow }: { flow: Flow }) {
  const note = useStore((s) => s.comments[flow.id]);
  return <span className="note">{note ?? "–"}</span>;
}

/**
 * The originating app, with its real icon once macOS has been asked for it.
 *
 * A component rather than a plain cell because the icon arrives after the first
 * paint: the fetch is cached per app (`src/appicons.ts`), so this subscribes to
 * "an icon landed" and repaints the handful of rows that were waiting.
 *
 * No attribution is a real state, not a blank — the proxy saw the socket but
 * could not name the process behind it.
 */
function ClientCell({ flow }: { flow: Flow }) {
  const [, bump] = useState(0);
  useEffect(() => onIconsChanged(() => bump((n) => n + 1)), []);
  const icon = flow.internal ? null : appIcon(flow.process);
  return (
    <>
      {typeof icon === "string" ? (
        <img className="appicon" src={icon} alt="" />
      ) : (
        <Icon name={flow.internal ? "shield-check" : "app-window"} size={13} />
      )}
      <span className="t">{flow.process ?? "unknown"}</span>
    </>
  );
}

/** Display order. A visible set is a subset of this, never a reordering (phase 6). */
export const COLUMN_ORDER: ColumnId[] = [
  "seq",
  "url",
  "client",
  "method",
  "status",
  "time",
  "duration",
  "request",
  "response",
  "protocol",
  "edited",
  "comment",
  "ssl",
];

/**
 * What is shown until the user says otherwise.
 *
 * Seven, not eleven, and the arithmetic is the reason rather than taste: these
 * seven need 746px, all eleven need ~1140px. The window's `minWidth` is 940;
 * the rail takes 78 and the scope tree another 252. So the default set fits with
 * the tree hidden (862px) and the table scrolls sideways with it open (610px),
 * while the full set fits neither.
 *
 * Columns drop rather than squeeze: ten unreadable slivers is worse than seven
 * readable cells (design.md §8).
 */
export const DEFAULT_COLUMNS: ColumnId[] = [
  "seq",
  "url",
  "client",
  "method",
  "status",
  "duration",
  "ssl",
];

/** Keep a stored column list usable: known ids only, in display order, never empty. */
export function normalizeColumns(ids: readonly string[] | null | undefined): ColumnId[] {
  const wanted = new Set(ids ?? []);
  const kept = COLUMN_ORDER.filter((id) => wanted.has(id));
  // URL is not optional: a table of sizes and statuses with no URLs identifies
  // nothing.
  if (!kept.includes("url")) kept.unshift("url");
  return kept.length > 1 ? kept : [...DEFAULT_COLUMNS];
}

/** A dragged width per column; anything absent keeps its declared track. */
export type ColumnWidths = Partial<Record<ColumnId, number>>;

/** Narrower than this and a cell shows nothing but an ellipsis. */
export const MIN_COLUMN_W = 44;
export const MAX_COLUMN_W = 720;

export function clampColumnWidth(px: number): number {
  if (!Number.isFinite(px)) return MIN_COLUMN_W;
  return Math.round(Math.min(MAX_COLUMN_W, Math.max(MIN_COLUMN_W, px)));
}

/** Keep stored widths sane: known columns, in range. */
export function normalizeWidths(raw: unknown): ColumnWidths {
  const out: ColumnWidths = {};
  if (!raw || typeof raw !== "object") return out;
  for (const [k, v] of Object.entries(raw as Record<string, unknown>)) {
    if (!(k in COLUMNS)) continue;
    if (typeof v !== "number" || !Number.isFinite(v)) continue;
    out[k as ColumnId] = clampColumnWidth(v);
  }
  return out;
}

/**
 * The `grid-template-columns` the header row and every body row share.
 *
 * A dragged width replaces the declared track — including the URL column's
 * `1fr`, which is the one case where dragging changes the *kind* of track and
 * not just its size: a user who sized the URL column meant that size.
 */
export function gridTemplate(ids: readonly ColumnId[], widths: ColumnWidths = {}): string {
  return ids.map((id) => (widths[id] ? `${widths[id]}px` : COLUMNS[id].track)).join(" ");
}

/** Minimum width the visible columns need before the table has to scroll sideways. */
export function minTableWidth(ids: readonly ColumnId[], widths: ColumnWidths = {}): number {
  return ids.reduce((sum, id) => {
    if (widths[id]) return sum + widths[id]!;
    const t = COLUMNS[id].track;
    const px = /(\d+)px/.exec(t.includes("minmax") ? t.slice(t.indexOf("minmax")) : t);
    return sum + (px ? Number(px[1]) : 0);
  }, 0);
}
