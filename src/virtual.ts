/**
 * Geometry for the windowed ("virtual") flow list.
 *
 * The list holds up to `MAX_FLOWS` rows, and rendering all of them is what made
 * a long recording session collapse: every snapshot re-rendered tens of
 * thousands of DOM nodes, until the webview died and took the UI with it. Only
 * the rows overlapping the viewport are rendered now; spacers stand in for the
 * rest so the scrollbar still describes the whole list.
 *
 * One geometry, because the table's rows are one fixed height: the per-group
 * arithmetic this module used to carry existed only for the old list's sticky
 * host headers, which needed each group to be its own containing block.
 */

const clamp = (v: number, lo: number, hi: number) => Math.min(Math.max(v, lo), hi);

/** What to render for one flat list of fixed-height rows. */
export interface FlatSlice {
  /** Rows to render: `[from, to)`. */
  from: number;
  to: number;
  /** Spacers standing in for the rows above `from` and below `to`. */
  padTop: number;
  padBottom: number;
}

/**
 * The same geometry for a flat list — the flows table.
 *
 * Simpler than [`sliceGroups`] on purpose: with no sticky group headers there is
 * nothing to keep in its own containing block, so one row height and one window
 * describe the whole list. That is the entire reason the table can drop the
 * per-group arithmetic.
 *
 * Before the viewport is measured this renders nothing rather than everything: a
 * first frame that mounts 10,000 rows is the crash this module exists to
 * prevent.
 */
export function sliceFlat(
  count: number,
  rowH: number,
  scrollTop: number,
  viewportH: number,
  overscan: number,
): FlatSlice {
  const total = Math.max(0, count);
  if (!(viewportH > 0) || !(rowH > 0)) {
    return { from: 0, to: 0, padTop: 0, padBottom: total * Math.max(0, rowH) };
  }
  const margin = Math.max(0, overscan) * rowH;
  const from = clamp(Math.floor((scrollTop - margin) / rowH), 0, total);
  const to = clamp(Math.ceil((scrollTop + viewportH + margin) / rowH), from, total);
  return { from, to, padTop: from * rowH, padBottom: (total - to) * rowH };
}

