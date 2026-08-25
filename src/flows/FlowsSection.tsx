import { useEffect, useMemo } from "react";
import type { Flow } from "../api";
import type { IconName } from "../icons";
import { applyFilter, isFiltering, type FlowFilter } from "../filter";
import { buildScopeTree, type Scope } from "../scope";
import { Inspector } from "../inspector/Inspector";
import { methodClass, statusClass, statusText } from "../badges";
import { Icon } from "../icons";
import { ScopeTree } from "./ScopeTree";
import { FilterBar } from "./FilterBar";
import { FlowTable } from "./FlowTable";
import type { ColumnId } from "./columns";

/**
 * The Flows section: scope tree, filter bar, table, inspector.
 *
 * Replaces the stat-strip-over-two-panes layout (design.md §4.1). The three
 * narrowing mechanisms are independent by construction — the tree sets a scope,
 * the chips set three axes, the search box sets a query, and all of them AND
 * together in `buildPredicate` — so none of them has to know about the others.
 */
export function FlowsSection(props: {
  flows: Flow[];
  filter: FlowFilter;
  patch: (p: Partial<FlowFilter>) => void;
  reset: () => void;
  columns: ColumnId[];
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
}) {
  const { flows, filter, selected, select } = props;

  /**
   * The tree counts what the table could show, so NovaProxy's own traffic is
   * excluded from it on exactly the same terms as from the rows — otherwise the
   * app's own MCP calls inflate a count for rows that never appear.
   */
  const tree = useMemo(
    () => buildScopeTree(filter.includeInternal ? flows : flows.filter((f) => !f.internal)),
    [flows, filter.includeInternal],
  );

  const filtered = useMemo(() => applyFilter(flows, filter), [flows, filter]);

  /**
   * Follow the tail.
   *
   * Keyed on the newest row's id rather than on the array: a snapshot updating
   * an in-flight flow must not re-select anything, only a genuinely new head of
   * the list should.
   */
  const newestId = filtered.length > 0 ? filtered[0].id : null;
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
      <ScopeTree
        tree={tree}
        scope={filter.scope}
        setScope={setScope}
        pinnedCount={0}
        savedCount={0}
        filterRef={props.treeFilterRef}
      />

      <div className="flows2-main">
        <FilterBar
          filter={filter}
          patch={props.patch}
          reset={props.reset}
          searchRef={props.searchRef}
          onChip={(id) => props.track?.("ui.flow.chip", id)}
        />

        <FlowTable
          flows={filtered}
          columns={props.columns}
          selectedId={selected?.id ?? null}
          select={select}
          empty={empty}
          scrollRef={props.tableRef}
        />

        {/* The summary bar is the inspector's head: the panes below carry tabs
            and bodies only, so the URL is stated once (design.md §4.1). */}
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
            </>
          ) : (
            <>
              <span className="none">No row selected</span>
              <span className="spacer" />
            </>
          )}
          <span className="rows">
            {filtered.length} row{filtered.length === 1 ? "" : "s"}
            {selected ? " · 1 selected" : ""}
          </span>
        </div>

        <div className="insp-strip">
          {selected ? (
            <Inspector
              flow={selected}
              showToast={props.showToast}
              onTab={(_pane, tab) => props.track?.("ui.detail_tab", tab.toLowerCase())}
            />
          ) : (
            <div className="detail-empty">
              <div className="big">Select a flow to inspect</div>
              <div className="hint">
                click a row, or press <span className="kbd">⌘K</span> for commands
              </div>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
