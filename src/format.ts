/**
 * Formatters shared by the table, the inspector and the status bar.
 *
 * Collected here rather than left local to `App.tsx` because the table renders
 * the same values in a much denser place: a size that reads `1.2 KB` in a card
 * and `1.2KB` in a cell is the kind of drift nobody notices until both are on
 * screen at once.
 */

/** `1.2 KB`. Whole bytes below 1 KB, one decimal above it. */
export function formatBytes(n: number | bigint): string {
  const v = Number(n);
  if (!Number.isFinite(v) || v <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  let i = 0;
  let size = v;
  while (size >= 1024 && i < units.length - 1) {
    size /= 1024;
    i++;
  }
  return `${i === 0 ? Math.round(size) : size.toFixed(1)} ${units[i]}`;
}

/**
 * A size for a table cell: `–` when there was no body at all.
 *
 * `0 B` and "no body" are different facts — a 204 sent nothing, a 200 with an
 * empty JSON object sent two bytes — and a table that prints `0 B` for both
 * throws away the distinction the SSL and status columns are read against.
 */
export function formatCellBytes(n: number | bigint | null | undefined): string {
  if (n == null) return "–";
  const v = Number(n);
  if (!Number.isFinite(v)) return "–";
  return v === 0 ? "–" : formatBytes(v);
}

/**
 * Wall-clock time of a capture, `22:40:03.296`.
 *
 * Local time, not UTC: the point of this column is lining a request up against
 * something the user just did.
 */
export function formatClock(epochMs: number): string {
  const d = new Date(epochMs);
  if (Number.isNaN(d.getTime())) return "—";
  const p2 = (n: number) => String(n).padStart(2, "0");
  const ms = String(d.getMilliseconds()).padStart(3, "0");
  return `${p2(d.getHours())}:${p2(d.getMinutes())}:${p2(d.getSeconds())}.${ms}`;
}


/**
 * Coerce a `bigint | number | null` wire value to a number.
 *
 * ts-rs types the wire sizes as `bigint` while the runtime hands over plain
 * numbers, and `null` means "not measured"; every arithmetic site needs the same
 * three-way coercion, so it lives here once.
 */
export const num = (n: number | bigint | null | undefined) => (n == null ? 0 : Number(n));

/** `just now` / `12s ago` / `4m ago` — relative, because absolute is the Time column's job. */
export function formatAgo(ms: number) {
  const s = Math.max(0, (Date.now() - ms) / 1000);
  if (s < 1) return "just now";
  if (s < 60) return `${Math.floor(s)}s ago`;
  if (s < 3600) return `${Math.floor(s / 60)}m ago`;
  return `${Math.floor(s / 3600)}h ago`;
}
