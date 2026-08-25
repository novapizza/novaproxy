/**
 * The two visual scales that classify a flow at a glance: method and status.
 *
 * Shared rather than local to a view because the flow table, the inspector head,
 * the summary bar and the intercept modal all render them, and a method badge
 * that is green in one place and violet in another stops being a scale.
 */

export const KNOWN_METHODS = ["GET", "POST", "PUT", "DELETE", "PATCH"];

/** `m-GET` … `m-OTHER`; the tints live in `styles.css` (see design.md §5). */
export const methodClass = (m: string) =>
  KNOWN_METHODS.includes(m) ? `m-${m}` : "m-OTHER";

export function statusClass(status: number | null, error: string | null) {
  if (error) return "s-err";
  if (!status) return "s-pending";
  const b = Math.floor(status / 100);
  return b === 1 ? "s-1xx" : b === 2 ? "s-2xx" : b === 3 ? "s-3xx" : b === 4 ? "s-4xx" : "s-5xx";
}

export const statusText = (status: number | null, error: string | null) => (error ? "ERR" : status ?? "···");
