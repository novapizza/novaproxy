/**
 * Geometry for the windowed ("virtual") flow list.
 *
 * The list holds up to `MAX_FLOWS` rows, and rendering all of them is what made
 * a long recording session collapse: every snapshot re-rendered tens of
 * thousands of DOM nodes, until the webview died and took the UI with it. Only
 * the rows overlapping the viewport are rendered now; spacers stand in for the
 * rest so the scrollbar still describes the whole list.
 *
 * Groups are sliced one by one rather than flattened into a single row list:
 * host headers are `position: sticky`, which only sticks per host while each
 * group remains its own containing block.
 */

export interface ListMetrics {
  /** Height of one flow row, in px. Measured from the DOM, not assumed. */
  rowH: number;
  /** Height of a group header, in px; 0 when the list is flat. */
  headerH: number;
  /** Rows kept rendered beyond each viewport edge, to cover a fast flick. */
  overscan: number;
}

/** How much of one group to render, and how much space to hold open for the rest. */
export interface GroupSlice {
  /** Height the whole group occupies, header included. */
  height: number;
  /** True when the group overlaps the overscanned viewport at all. */
  onScreen: boolean;
  /** Rows to render: `[from, to)`. Empty when the group is off-screen. */
  from: number;
  to: number;
  /** Spacers standing in for the rows before `from` and after `to`. */
  padTop: number;
  padBottom: number;
}

const offScreen = (height: number): GroupSlice => ({
  height,
  onScreen: false,
  from: 0,
  to: 0,
  padTop: 0,
  padBottom: 0,
});

const clamp = (v: number, lo: number, hi: number) => Math.min(Math.max(v, lo), hi);

/**
 * Decide what to render for each group, given how many rows it holds and where
 * the scroll position is. `rowCounts` is in display order, top to bottom.
 */
export function sliceGroups(
  rowCounts: number[],
  m: ListMetrics,
  scrollTop: number,
  viewportH: number,
): GroupSlice[] {
  const margin = Math.max(0, m.overscan) * m.rowH;
  const top = scrollTop - margin;
  const bottom = scrollTop + viewportH + margin;
  // Before the viewport has been measured there is no window to compute, and a
  // zero row height would make the arithmetic below meaningless.
  const measured = viewportH > 0 && m.rowH > 0;

  const slices: GroupSlice[] = [];
  let offset = 0;
  for (const raw of rowCounts) {
    const count = Math.max(0, raw);
    const height = m.headerH + count * m.rowH;
    const start = offset;
    offset += height;

    if (!measured || start + height <= top || start >= bottom) {
      slices.push(offScreen(height));
      continue;
    }
    // Rows begin below the group's own header.
    const rowsTop = start + m.headerH;
    const from = clamp(Math.floor((top - rowsTop) / m.rowH), 0, count);
    const to = clamp(Math.ceil((bottom - rowsTop) / m.rowH), from, count);
    slices.push({
      height,
      onScreen: true,
      from,
      to,
      padTop: from * m.rowH,
      padBottom: (count - to) * m.rowH,
    });
  }
  return slices;
}

/** Rows actually rendered across every group — what the DOM node count follows. */
export function renderedRows(slices: GroupSlice[]): number {
  return slices.reduce((n, s) => n + (s.to - s.from), 0);
}
