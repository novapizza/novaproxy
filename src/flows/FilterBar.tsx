import { Icon } from "../icons";
import { FLOW_TYPES, PROTOS, STATUS_CLASSES } from "../classify";
import { clauseActive } from "../builder";
import { activeFilterCount, toggleIn, type FlowFilter, type SavedFilter } from "../filter";
import { usePopover } from "../usePopover";
import { ChipMenu } from "./ChipMenu";
import { FilterBuilder } from "./FilterBuilder";

/**
 * One row: search box, the three axes as menus, saved filters, and the
 * conditions toggle.
 *
 * The axes are still three independent things — **OR inside one, AND between
 * them**, an empty one meaning "all of it", which is why there is no `All` chip
 * and why Reset is what clears. What changed is only how they are drawn: as
 * buttons rather than as three rows of chips, because those rows cost ~140px of a
 * window that exists to show the table underneath (design.md §4.1).
 */
export function FilterBar({
  filter,
  patch,
  reset,
  searchRef,
  onChip,
  trailing,
  saved,
  applySaved,
  saveCurrent,
  removeSaved,
  builderOpen,
  toggleBuilder,
}: {
  filter: FlowFilter;
  patch: (p: Partial<FlowFilter>) => void;
  reset: () => void;
  /** Focus target for ⌘F. */
  searchRef?: React.RefObject<HTMLInputElement | null>;
  /** Which chip was pressed — the id only, never the search text. */
  onChip?: (id: string) => void;
  /** View controls that belong at the end of the row — today, the column picker. */
  trailing?: React.ReactNode;
  /**
   * Filters the user kept. Chips rather than a list in the sidebar, the way
   * Proxyman does it: a saved filter *is* a filter, so it belongs among the
   * filters (issues/0002 §8.1).
   */
  saved: SavedFilter[];
  applySaved: (s: SavedFilter) => void;
  saveCurrent: () => void;
  removeSaved: (id: string) => void;
  /** Structured rows shown, and the toggle that shows them. */
  builderOpen: boolean;
  toggleBuilder: () => void;
}) {
  const active = activeFilterCount(filter);
  const rows = filter.clauses.filter(clauseActive).length;

  return (
    <div className="fbar">
      <div className="fbar-row">
        <div className="fbar-search">
          <span className="mag"><Icon name="search" /></span>
          <input
            ref={searchRef}
            value={filter.query}
            onChange={(e) => patch({ query: e.target.value })}
            placeholder="host, path, method:GET, status:401…"
            aria-label="Search the capture"
          />
          {filter.query && (
            <span className="clear" title="Clear the search" onClick={() => patch({ query: "" })}>
              <Icon name="x" />
            </span>
          )}
        </div>

        <ChipMenu
          label="Proto"
          chips={PROTOS}
          on={filter.proto}
          toggle={(id) => {
            patch({ proto: toggleIn(filter.proto, id) });
            onChip?.(id);
          }}
          clear={() => patch({ proto: new Set() })}
        />
        <ChipMenu
          label="Type"
          chips={FLOW_TYPES}
          on={filter.type}
          toggle={(id) => {
            patch({ type: toggleIn(filter.type, id) });
            onChip?.(id);
          }}
          clear={() => patch({ type: new Set() })}
        />
        <ChipMenu
          label="Status"
          chips={STATUS_CLASSES}
          on={filter.status}
          toggle={(id) => {
            patch({ status: toggleIn(filter.status, id) });
            onChip?.(id);
          }}
          clear={() => patch({ status: new Set() })}
        />

        {saved.length > 0 && (
          <SavedMenu saved={saved} apply={applySaved} remove={removeSaved} />
        )}

        <span
          className={`fb-toggle ${builderOpen || rows > 0 ? "on" : ""}`}
          title="Filter by field, operator and value"
          onClick={toggleBuilder}
        >
          <Icon name="sliders" size={13} />
          {rows > 0 ? `${rows} condition${rows === 1 ? "" : "s"}` : "Conditions"}
        </span>

        {/* Save and Reset appear only when there is a filter to save or clear —
            two permanent buttons that usually do nothing is furniture. */}
        {active > 0 && (
          <>
            <span className="fb-act" title="Name and keep this filter" onClick={saveCurrent}>
              Save
            </span>
            <span className="fb-act accent" onClick={reset}>
              Reset ({active})
            </span>
          </>
        )}

        {/* Pushes the view controls to the right edge, so they stay put while
            the menus above change width. */}
        <span className="fbar-gap" />
        {trailing}
      </div>

      {builderOpen && (
        <FilterBuilder clauses={filter.clauses} setClauses={(c) => patch({ clauses: c })} />
      )}
    </div>
  );
}

/**
 * Saved filters, behind one button.
 *
 * Inline chips were fine at two and would have been the widest thing in the row
 * at six — and unlike the axes, this list grows without bound because the user
 * writes it.
 */
function SavedMenu({
  saved,
  apply,
  remove,
}: {
  saved: SavedFilter[];
  apply: (s: SavedFilter) => void;
  remove: (id: string) => void;
}) {
  const pop = usePopover();
  return (
    <div className="chipmenu" ref={pop.ref}>
      <span
        className={`cm-btn ${pop.open ? "on" : ""}`}
        onClick={pop.toggle}
        role="button"
        aria-expanded={pop.open}
      >
        <span className="cm-label">Saved</span>
        <span className="cm-sum">{saved.length}</span>
        <Icon name="chevron-down" size={12} />
      </span>
      {pop.open && (
        <div className="cm-panel wide">
          {saved.map((sf) => (
            <div
              key={sf.id}
              className="cm-row"
              onClick={() => {
                apply(sf);
                pop.setOpen(false);
              }}
            >
              <span className="tick"><Icon name="filter" size={12} /></span>
              <span className="t">{sf.label}</span>
              <span
                className="x"
                title="Forget this filter"
                onClick={(e) => {
                  e.stopPropagation();
                  remove(sf.id);
                }}
              >
                <Icon name="x" size={11} />
              </span>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
