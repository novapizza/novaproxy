import { memo, useLayoutEffect, useRef, useState } from "react";
import type { Flow } from "../api";
import { Icon, type IconName } from "../icons";
import { sliceFlat } from "../virtual";
import {
  clampColumnWidth,
  COLUMNS,
  gridTemplate,
  minTableWidth,
  type ColumnId,
  type ColumnWidths,
} from "./columns";
import { cycleSort, sortable, sortGlyph, type Sort } from "./sort";

/** Rows kept mounted beyond each viewport edge, to cover a fast flick. */
const OVERSCAN = 10;

/** First-frame estimate only; the real height is measured from the DOM. */
const ROW_H_GUESS = 34;

/**
 * The flows table, windowed.
 *
 * Retention allows `MAX_FLOWS` rows and mounting them all is what killed the
 * webview before `src/virtual.ts` existed. Rows here are one fixed height, which
 * is what lets this use the flat `sliceFlat` geometry instead of the per-group
 * arithmetic the old list needed for its sticky host headers.
 *
 * The header row and every body row share one `grid-template-columns`, taken
 * from the column declarations — so a column that appears or disappears cannot
 * leave the header describing tracks the body does not have.
 */
export function FlowTable({
  flows,
  columns,
  selectedId,
  select,
  empty,
  scrollRef,
  onInspect,
  sort,
  setSort,
  widths,
  setWidth,
  marked,
  toggleMark,
  markRange,
}: {
  flows: Flow[];
  columns: ColumnId[];
  selectedId: string | null;
  select: (id: string) => void;
  empty: { icon: IconName; msg: string; hint: string } | null;
  scrollRef?: React.RefObject<HTMLDivElement | null>;
  /** Enter: hand focus to the inspector. */
  onInspect?: () => void;
  sort: Sort | null;
  setSort: (s: Sort | null) => void;
  widths: ColumnWidths;
  setWidth: (id: ColumnId, px: number) => void;
  /** Rows marked for a bulk action. The *current* row is `selectedId`. */
  marked: ReadonlySet<string>;
  toggleMark: (id: string) => void;
  markRange: (toId: string) => void;
}) {
  const ownRef = useRef<HTMLDivElement | null>(null);
  const ref = scrollRef ?? ownRef;
  const [scrollTop, setScrollTop] = useState(0);
  const [viewportH, setViewportH] = useState(0);
  const [rowH, setRowH] = useState(ROW_H_GUESS);

  // Measured in a layout effect so the very first paint is already windowed, and
  // observed because a window resize changes how many rows fit.
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const sync = () => {
      setViewportH(el.clientHeight);
      setScrollTop(el.scrollTop);
    };
    sync();
    const ro = new ResizeObserver(sync);
    ro.observe(el);
    return () => ro.disconnect();
  }, [ref]);

  const measure = (el: HTMLElement | null) => {
    if (!el) return;
    const h = el.getBoundingClientRect().height;
    if (h > 0 && Math.abs(h - rowH) > 0.5) setRowH(h);
  };

  /**
   * Row navigation is handled here rather than by the global dispatcher: arrow
   * keys belong to the element that has focus, and a window-level listener would
   * fight every scrollable panel in the app. The chords are still declared in
   * `src/shortcuts.ts` so the dialog can list them.
   */
  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.metaKey || e.ctrlKey || e.altKey) return;
    const at = flows.findIndex((f) => f.id === selectedId);
    const page = Math.max(1, Math.floor(viewportH / rowH) - 1);
    const go = (i: number) => {
      const next = flows[Math.min(Math.max(i, 0), flows.length - 1)];
      if (next) {
        select(next.id);
        scrollTo(Math.min(Math.max(i, 0), flows.length - 1));
      }
    };
    switch (e.key) {
      case "ArrowDown": e.preventDefault(); go(at < 0 ? 0 : at + 1); break;
      case "ArrowUp": e.preventDefault(); go(at < 0 ? 0 : at - 1); break;
      case "Home": e.preventDefault(); go(0); break;
      case "End": e.preventDefault(); go(flows.length - 1); break;
      case "PageDown": e.preventDefault(); go((at < 0 ? 0 : at) + page); break;
      case "PageUp": e.preventDefault(); go((at < 0 ? 0 : at) - page); break;
      case "Enter": e.preventDefault(); onInspect?.(); break;
    }
  };

  /** Keep the row that just became current inside the window. */
  const scrollTo = (index: number) => {
    const el = ref.current;
    if (!el) return;
    const top = index * rowH;
    const headH = rowH; // the sticky header covers the first row's worth
    if (top < el.scrollTop + headH) el.scrollTop = Math.max(0, top - headH);
    else if (top + rowH > el.scrollTop + el.clientHeight) el.scrollTop = top + rowH - el.clientHeight;
  };

  const slice = sliceFlat(flows.length, rowH, scrollTop, viewportH, OVERSCAN);
  const template = gridTemplate(columns, widths);
  const minWidth = minTableWidth(columns, widths);

  /**
   * Drag a column edge.
   *
   * Pointer capture rather than window listeners: it keeps the drag alive when
   * the cursor outruns the 6px grip, and releases itself if the pointer is lost.
   */
  const startResize = (id: ColumnId) => (e: React.PointerEvent<HTMLSpanElement>) => {
    e.preventDefault();
    e.stopPropagation();
    const grip = e.currentTarget;
    const cell = grip.parentElement as HTMLElement | null;
    if (!cell) return;
    const startX = e.clientX;
    const startW = cell.getBoundingClientRect().width;
    grip.setPointerCapture(e.pointerId);
    const move = (ev: PointerEvent) => setWidth(id, clampColumnWidth(startW + ev.clientX - startX));
    const up = (ev: PointerEvent) => {
      grip.removeEventListener("pointermove", move);
      grip.removeEventListener("pointerup", up);
      grip.removeEventListener("pointercancel", up);
      grip.releasePointerCapture(ev.pointerId);
    };
    grip.addEventListener("pointermove", move);
    grip.addEventListener("pointerup", up);
    grip.addEventListener("pointercancel", up);
  };

  /**
   * A click means one of three things, and the modifier says which: plain picks
   * one row, ⌘/Ctrl adds or removes one, Shift takes everything between.
   */
  const onRowClick = (e: React.MouseEvent, id: string) => {
    if (e.metaKey || e.ctrlKey) toggleMark(id);
    else if (e.shiftKey) markRange(id);
    else select(id);
  };

  return (
    <div
      className="ftable"
      ref={ref}
      onScroll={(e) => setScrollTop(e.currentTarget.scrollTop)}
      onKeyDown={onKeyDown}
      tabIndex={0}
      role="grid"
      aria-label="Captured flows"
    >
      <div className="ft-head" style={{ gridTemplateColumns: template, minWidth }}>
        {columns.map((id) => (
          <span
            key={id}
            className={`${COLUMNS[id].align === "right" ? "r" : ""} ${sortable(id) ? "sortable" : ""} ${
              sort?.by === id ? "sorted" : ""
            }`}
            onClick={() => setSort(cycleSort(sort, id))}
            title={sortable(id) ? "Sort by this column" : undefined}
          >
            {COLUMNS[id].label}
            {sort?.by === id && <span className="sg">{sortGlyph(sort, id)}</span>}
            <span className="grip" onPointerDown={startResize(id)} />
          </span>
        ))}
      </div>

      {empty ? (
        <div className="list-empty">
          <div className="icon"><Icon name={empty.icon} size={26} /></div>
          <div className="big">{empty.msg}</div>
          <div>{empty.hint}</div>
        </div>
      ) : (
        <div className="ft-body" style={{ minWidth }}>
          {slice.padTop > 0 && <div style={{ height: slice.padTop }} />}
          {flows.slice(slice.from, slice.to).map((f, i) => (
            <Row
              key={f.id}
              flow={f}
              columns={columns}
              template={template}
              even={(slice.from + i) % 2 === 0}
              selected={f.id === selectedId}
              marked={marked.has(f.id)}
              onClick={onRowClick}
              measure={i === 0 ? measure : undefined}
            />
          ))}
          {slice.padBottom > 0 && <div style={{ height: slice.padBottom }} />}
        </div>
      )}
    </div>
  );
}

/** One row. Memoised: a snapshot for one flow must not re-render its neighbours. */
const Row = memo(function Row({
  flow,
  columns,
  template,
  even,
  selected,
  marked,
  onClick,
  measure,
}: {
  flow: Flow;
  columns: ColumnId[];
  template: string;
  even: boolean;
  selected: boolean;
  marked: boolean;
  onClick: (e: React.MouseEvent, id: string) => void;
  measure?: (el: HTMLElement | null) => void;
}) {
  return (
    <div
      ref={measure}
      role="row"
      aria-selected={selected}
      className={`ft-row ${even ? "even" : "odd"} ${selected ? "sel" : ""} ${
        marked ? "marked" : ""
      } ${flow.status == null && flow.error == null ? "pending" : ""}`}
      style={{ gridTemplateColumns: template }}
      onClick={(e) => onClick(e, flow.id)}
    >
      {columns.map((id) => (
        <span key={id} className={`c-${id}${COLUMNS[id].align === "right" ? " r" : ""}`}>
          {COLUMNS[id].cell(flow)}
        </span>
      ))}
    </div>
  );
});
