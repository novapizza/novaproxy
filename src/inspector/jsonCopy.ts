/**
 * What the Treeview's per-node copy buttons put on the clipboard.
 *
 * A leaf copies the value as it would be used, not as it is displayed: a string
 * without its quotes (a token pasted into a header must not carry them), other
 * scalars as JSON writes them. A branch copies its subtree as indented JSON,
 * the same shape the Body panel shows.
 */
export function nodeValueText(value: unknown): string {
  if (typeof value === "string") return value;
  if (value !== null && typeof value === "object") return JSON.stringify(value, null, 2);
  return String(value);
}

/** A key that can follow a dot in a JavaScript path; anything else is bracketed. */
const IDENTIFIER = /^[A-Za-z_$][A-Za-z0-9_$]*$/;

/**
 * The path of a child below `parent`, in the JavaScript accessor form people
 * paste into code, a filter or `jq` with a leading dot: `data.items[0].id`,
 * `headers["content-type"]`. The root's path is the empty string.
 */
export function childPath(parent: string, key: string, inArray: boolean): string {
  if (inArray) return `${parent}[${key}]`;
  if (IDENTIFIER.test(key)) return parent === "" ? key : `${parent}.${key}`;
  return `${parent}[${JSON.stringify(key)}]`;
}
