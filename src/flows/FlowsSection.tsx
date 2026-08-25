import { forwardRef, useEffect, useImperativeHandle, useMemo, useRef, useState } from "react";
import type { Flow } from "../api";
import type { IconName } from "../icons";
import {
  applyFilter,
  describeFilter,
  filterFromJson,
  filterToJson,
  isFiltering,
  type FlowFilter,
  type SavedFilter,
} from "../filter";
import { buildScopeTree, type Scope } from "../scope";
import { useStore } from "../store";
import {
  INITIAL_PANES,
  Inspector,
  tabsFor,
  type PaneSide,
  type PaneState,
} from "../inspector/Inspector";
import { methodClass, statusClass, statusText } from "../badges";
import { Icon } from "../icons";
import { ScopeTree } from "./ScopeTree";
import { FilterBar } from "./FilterBar";
import { clauseActive, newClause } from "../builder";
import { FlowTable } from "./FlowTable";
import { ColumnPicker } from "./ColumnPicker";
import { Splitter } from "../Splitter";
import { DEFAULT_PREFS, INSPECTOR_PCT, TREE_W } from "../prefs";
import { sortFlows, type Sort } from "./sort";
import { clampColumnWidth, type ColumnId, type ColumnWidths } from "./columns";
import { formatChord, shortcut } from "../shortcuts";

/**
 * What the keyboard can ask of this section from outside it.
 *
 * The global dispatcher lives in `App`, but the inspector's panes and the table's
 * focus are this component's state; a handle is the smallest seam between the
 * two — smaller than lifting four pieces of view state into `App` so that a
 * chord can reach them.
 */
export interface FlowsHandle {
  /** ⌘N / ⇧⌘N: add or drop a structured filter row. */
  addClause: () => void;
  removeClause: () => void;
  focusTable: () => void;
  focusInspector: () => void;
  paneTab: (delta: 1 | -1) => void;
  switchPane: () => void;
  toggleCollapse: () => void;
  /** ⌘⇧A: mark every row the filter is currently showing. */
  markAll: () => void;
}

/**
 * The Flows section: scope tree, filter bar, table, inspector.
 *
 * Replaces the stat-strip-over-two-panes layout (design.md §4.1). The three
 * narrowing mechanisms are independent by construction — the tree sets a scope,
 * the chips set three axes, the search box sets a query, and all of them AND
 * together in `buildPredicate` — so none of them has to know about the others.
 */
export const FlowsSection = forwardRef<FlowsHandle, {
  flows: Flow[];
  filter: FlowFilter;
  patch: (p: Partial<FlowFilter>) => void;
  reset: () => void;
  columns: ColumnId[];
  setColumns: (c: ColumnId[]) => void;
  widths: ColumnWidths;
  setWidths: (w: ColumnWidths) => void;
  recording: boolean;
  selected: Flow | null;
  select: (id: string | null) => void;
  /** Follow the tail: keep the newest row selected as it arrives. */
  autoSelect: boolean;
  onResend: () => void;
  onCopyCurl: () => void;
  showToast: (t: string) => void;
  /**
   * Usage counting, passed in rather than imported: the section is also mounted
   * by the dev harness (`preview.html`), which has no Tauri runtime to invoke.
   */
  track?: (ev: "ui.flow.chip" | "ui.flow.scope" | "ui.detail_tab", name: string) => void;
  /** Focus targets for the keyboard shortcuts (issues/0003). */
  searchRef?: React.RefObject<HTMLInputElement | null>;
  treeFilterRef?: React.RefObject<HTMLInputElement | null>;
  tableRef?: React.RefObject<HTMLDivElement | null>;
  /** Hidden by ⌘0, so a small window can give the table its width back. */
  treeHidden?: boolean;
  /** Rows marked for a bulk action, reported up so the actions can use them. */
  onMarked?: (ids: string[]) => void;
  /** Filters the user kept, and the two ways the list changes. */
  saved: SavedFilter[];
  setSaved: (s: SavedFilter[]) => void;
  /**
   * The three dividers. `set` paints while dragging, `commit` persists once at
   * the end — a drag emits hundreds of moves and would otherwise write hundreds
   * of preferences.
   */
  treeWidth: number;
  setTreeWidth: (px: number) => void;
  commitTreeWidth: (px: number) => void;
  inspectorPct: number;
  setInspectorPct: (pct: number) => void;
  commitInspectorPct: (pct: number) => void;
  requestPct: number;
  setRequestPct: (pct: number) => void;
  commitRequestPct: (pct: number) => void;
}>(function FlowsSection(props, ref) {
  const { flows, filter, selected, select } = props;
  const [panes, setPanes] = useState<PaneState>(INITIAL_PANES);
  const [sort, setSort] = useState<Sort | null>(null);
  /**
   * Open when there is anything to see. A filter whose conditions are hidden is
   * a filter the user cannot read — which matters most for a saved filter that
   * carries rows, since nothing else on screen says they exist.
   */
  const [builderOpen, setBuilderOpen] = useState(() => filter.clauses.some(clauseActive));
  /**
   * Rows marked for a bulk action, as ids rather than flows: a flow object is
   * replaced on every snapshot, and a set of objects would hold the stale ones
   * (and leak them past the retention cap).
   */
  const [marked, setMarked] = useState<ReadonlySet<string>>(new Set());
  const pinned = useStore((s) => s.pinned);
  const comments = useStore((s) => s.comments);
  const togglePin = useStore((s) => s.togglePin);
  const setComment = useStore((s) => s.setComment);
  const ownTableRef = useRef<HTMLDivElement | null>(null);
  const tableRef = props.tableRef ?? ownTableRef;
  const inspRef = useRef<HTMLDivElement | null>(null);

  useImperativeHandle(ref, () => ({
    focusTable: () => tableRef.current?.focus(),
    focusInspector: () => inspRef.current?.focus(),
    paneTab: (delta) => {
      const side: PaneSide = panes.active;
      const tabs = tabsFor(side);
      const at = tabs.indexOf(panes[side]);
      // Wrap: with six panels, walking off the end and stopping there feels like
      // the key stopped working.
      const next = tabs[(at + delta + tabs.length) % tabs.length];
      setPanes({ ...panes, [side]: next });
    },
    switchPane: () =>
      setPanes({ ...panes, active: panes.active === "request" ? "response" : "request" }),
    toggleCollapse: () =>
      setPanes({
        ...panes,
        collapsed: panes.collapsed === panes.active ? null : panes.active,
      }),
    markAll: () => setMarked(new Set(rows.map((f) => f.id))),
    addClause: () => {
      // Adding a row opens the builder: a chord that changes state you cannot
      // see is a chord that looks broken.
      setBuilderOpen(true);
      props.patch({ clauses: [...filter.clauses, newClause()] });
    },
    removeClause: () => props.patch({ clauses: filter.clauses.slice(0, -1) }),
  }));

  /**
   * The tree counts what the table could show, so NovaProxy's own traffic is
   * excluded from it on exactly the same terms as from the rows — otherwise the
   * app's own MCP calls inflate a count for rows that never appear.
   */
  const tree = useMemo(
    () => buildScopeTree(filter.includeInternal ? flows : flows.filter((f) => !f.internal)),
    [flows, filter.includeInternal],
  );

  // `pinned` reaches the predicate as context rather than as part of the filter:
  // it is membership the flow does not carry, and the scope only reads it.
  const filtered = useMemo(() => applyFilter(flows, filter, { pinned }), [flows, filter, pinned]);
  const rows = useMemo(() => sortFlows(filtered, sort), [filtered, sort]);

  // Marks follow the rows: a row the filter hides is not a row a bulk action
  // should still act on, and holding its id would surprise the next Export.
  useEffect(() => {
    if (marked.size === 0) return;
    const visible = new Set(rows.map((f) => f.id));
    const kept = [...marked].filter((id) => visible.has(id));
    if (kept.length !== marked.size) setMarked(new Set(kept));
  }, [rows, marked]);

  useEffect(() => props.onMarked?.([...marked]), [marked, props.onMarked]);

  /**
   * Follow the tail.
   *
   * Keyed on the newest row's id rather than on the array: a snapshot updating
   * an in-flight flow must not re-select anything, only a genuinely new head of
   * the list should.
   */
  // Newest by `seq`, not "first row": once the table is sorted by duration the
  // top row is no longer the newest, and following it would jump around.
  const newestId = useMemo(() => {
    let best: (typeof filtered)[number] | null = null;
    for (const f of filtered) if (!best || Number(f.seq) > Number(best.seq)) best = f;
    return best?.id ?? null;
  }, [filtered]);
  useEffect(() => {
    if (props.autoSelect && newestId) select(newestId);
  }, [props.autoSelect, newestId, select]);

  // Three empty states, not one: telling someone to loosen a filter they never
  // set is worse than saying nothing (design.md §8).
  const empty: { icon: IconName; msg: string; hint: string } | null =
    filtered.length > 0
      ? null
      : flows.length === 0
      ? props.recording
        ? { icon: "activity", msg: "Waiting for traffic…", hint: "Flows land here as your apps make requests." }
        : { icon: "circle-pause", msg: "Recording paused", hint: "Press Recording in the toolbar to start capturing." }
      : isFiltering(filter)
      ? {
          icon: "search-x",
          msg: "No flows match",
          hint: `${flows.length} captured, none matching. Reset the filters, or narrow them differently.`,
        }
      : { icon: "search-x", msg: "Nothing to show", hint: "Every captured flow is hidden." };

  const setScope = (scope: Scope) => {
    props.patch({ scope });
    props.track?.("ui.flow.scope", scope.kind);
  };

  return (
    <div className="flows2">
      {!props.treeHidden && (
      <>
      <ScopeTree
        tree={tree}
        scope={filter.scope}
        setScope={setScope}
        pinnedCount={pinned.size}
        filterRef={props.treeFilterRef}
        width={props.treeWidth}
      />
      <Splitter
        orientation="vertical"
        label="Resize the sidebar"
        value={props.treeWidth}
        min={TREE_W.min}
        max={TREE_W.max}
        reset={DEFAULT_PREFS.treeWidth}
        onDrag={props.setTreeWidth}
        onCommit={props.commitTreeWidth}
        measure={(e, rect) => e.clientX - rect.left}
      />
      </>
      )}

      <div className="flows2-main">
        <FilterBar
          filter={filter}
          patch={props.patch}
          reset={props.reset}
          searchRef={props.searchRef}
          onChip={(id) => props.track?.("ui.flow.chip", id)}
          trailing={<ColumnPicker columns={props.columns} setColumns={props.setColumns} />}
          saved={props.saved}
          applySaved={(sf) => {
            const next = filterFromJson(sf.filter);
            if (next.clauses.some(clauseActive)) setBuilderOpen(true);
            props.patch(next);
          }}
          saveCurrent={() => {
            const label = describeFilter(filter);
            // Saving the same filter twice is a no-op rather than a duplicate
            // chip: the label *is* the filter, so two identical chips would be
            // two identical buttons.
            if (props.saved.some((s) => s.label === label)) return;
            props.setSaved([
              ...props.saved,
              { id: `sf${Date.now()}`, label, filter: filterToJson(filter) },
            ]);
          }}
          removeSaved={(id) => props.setSaved(props.saved.filter((s) => s.id !== id))}
          builderOpen={builderOpen}
          toggleBuilder={() => setBuilderOpen((v) => !v)}
        />

        <FlowTable
          flows={rows}
          columns={props.columns}
          selectedId={selected?.id ?? null}
          select={select}
          empty={empty}
          scrollRef={tableRef}
          onInspect={() => inspRef.current?.focus()}
          sort={sort}
          setSort={setSort}
          widths={props.widths}
          setWidth={(id, px) => props.setWidths({ ...props.widths, [id]: clampColumnWidth(px) })}
          marked={marked}
          toggleMark={(id) =>
            setMarked((prev) => {
              const next = new Set(prev);
              if (!next.delete(id)) next.add(id);
              return next;
            })
          }
          markRange={(toId) => {
            // From the current row to the clicked one, inclusive — the range a
            // person means by shift-clicking, in the order the table is showing.
            const a = rows.findIndex((f) => f.id === (selected?.id ?? toId));
            const b = rows.findIndex((f) => f.id === toId);
            if (a < 0 || b < 0) return;
            const [lo, hi] = a <= b ? [a, b] : [b, a];
            setMarked(new Set(rows.slice(lo, hi + 1).map((f) => f.id)));
          }}
        />

        {/* The summary bar is the inspector's head: the panes below carry tabs
            and bodies only, so the URL is stated once (design.md §4.1). */}
        <Splitter
          orientation="horizontal"
          label="Resize the inspector"
          value={props.inspectorPct}
          min={INSPECTOR_PCT.min}
          max={INSPECTOR_PCT.max}
          reset={DEFAULT_PREFS.inspectorPct}
          onDrag={props.setInspectorPct}
          onCommit={props.commitInspectorPct}
          // Dragging up grows the inspector, so the value is measured from the
          // bottom of the table rather than from its top.
          measure={(e, rect) => ((rect.bottom - e.clientY) / rect.height + 0) * 100 + 0}
        />

        <div className="summary-bar">
          {selected ? (
            <>
              <span className={`badge ${methodClass(selected.method)}`}>{selected.method}</span>
              <span className={`status-pill ${statusClass(selected.status, selected.error)}`}>
                {statusText(selected.status, selected.error)}
              </span>
              <span className="url" title={selected.url}>
                <span className="scheme">{selected.scheme}://</span>
                <span className="host">{selected.host}</span>
                {selected.url.slice(selected.url.indexOf(selected.host) + selected.host.length)}
              </span>
              <span className="spacer" />
              <span className="act" onClick={props.onResend}>
                <Icon name="repeat" size={12} /> Resend
              </span>
              <span className="act" onClick={props.onCopyCurl}>
                <Icon name="copy" size={12} /> cURL
              </span>
              <span
                className={`act ${pinned.has(selected.id) ? "on" : ""}`}
                title={pinned.has(selected.id) ? "Unpin this flow" : "Pin this flow"}
                onClick={() => togglePin(selected.id)}
              >
                <Icon name="pin" size={12} /> {pinned.has(selected.id) ? "Pinned" : "Pin"}
              </span>
              {/* The note is part of the bar rather than a panel: it is written
                  while looking at the row, and it is one line. */}
              <input
                className="note"
                value={comments[selected.id] ?? ""}
                onChange={(e) => setComment(selected.id, e.target.value)}
                placeholder="Add a note…"
                aria-label="Note on this flow"
              />
            </>
          ) : (
            <>
              <span className="none">No row selected</span>
              <span className="spacer" />
            </>
          )}
          <span className="rows">
            {rows.length} row{rows.length === 1 ? "" : "s"}
            {marked.size > 0
              ? ` · ${marked.size} marked`
              : selected
              ? " · 1 selected"
              : ""}
          </span>
        </div>

        <div
          className="insp-strip"
          ref={inspRef}
          tabIndex={-1}
          style={{ height: `${props.inspectorPct}%` }}
        >
          {selected ? (
            <Inspector
              flow={selected}
              showToast={props.showToast}
              panes={panes}
              setPanes={setPanes}
              onTab={(_pane, tab) => props.track?.("ui.detail_tab", tab.toLowerCase())}
              requestPct={props.requestPct}
              setRequestPct={props.setRequestPct}
              commitRequestPct={props.commitRequestPct}
            />
          ) : (
            <div className="detail-empty">
              <div className="big">Select a flow to inspect</div>
              <div className="hint">
                click a row, or press{" "}
                <span className="kbd">{formatChord(shortcut("palette").chord).join("")}</span> for
                commands
              </div>
            </div>
          )}
        </div>
      </div>
    </div>
  );
});
