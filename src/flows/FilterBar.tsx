import { Icon } from "../icons";
import { FLOW_TYPES, PROTOS, STATUS_CLASSES } from "../classify";
import { activeFilterCount, toggleIn, type FlowFilter } from "../filter";

/**
 * Search box over three chip groups.
 *
 * The groups are the point: **OR inside a group, AND between groups**, so
 * `JSON` + `4xx` + `5xx` — "which API is failing" — is expressible, which is
 * exactly what a single mutually-exclusive chip row cannot say. An empty group
 * means "all of it", so there is no `All` chip to switch back to; **Reset** does
 * that, and it names how many groups are narrowing the view.
 *
 * Three rows cost ~56px that Proxyman spends on one scrollable row. Deliberate
 * (design.md §4.1): one row cannot show which axis a chip belongs to.
 */
export function FilterBar({
  filter,
  patch,
  reset,
  searchRef,
  onChip,
}: {
  filter: FlowFilter;
  patch: (p: Partial<FlowFilter>) => void;
  reset: () => void;
  /** Focus target for ⌘F. */
  searchRef?: React.RefObject<HTMLInputElement | null>;
  /** Which chip was pressed — the id only, never the search text. */
  onChip?: (id: string) => void;
}) {
  const active = activeFilterCount(filter);

  return (
    <div className="fbar">
      <div className="fbar-search">
        <span className="mag"><Icon name="search" /></span>
        <input
          ref={searchRef}
          value={filter.query}
          onChange={(e) => patch({ query: e.target.value })}
          placeholder="host, path, method:GET, status:401, app:Chrome, mcp:"
          aria-label="Search the capture"
        />
        {filter.query && (
          <span className="clear" title="Clear the search" onClick={() => patch({ query: "" })}>
            <Icon name="x" />
          </span>
        )}
        <span className="spacer" />
        {active > 0 && (
          <span className="reset" onClick={reset}>
            Reset filters ({active})
          </span>
        )}
      </div>

      <ChipGroup
        label="Proto"
        chips={PROTOS}
        on={filter.proto}
        toggle={(id) => {
          patch({ proto: toggleIn(filter.proto, id) });
          onChip?.(id);
        }}
      />
      <ChipGroup
        label="Type"
        chips={FLOW_TYPES}
        on={filter.type}
        toggle={(id) => {
          patch({ type: toggleIn(filter.type, id) });
          onChip?.(id);
        }}
      />
      <ChipGroup
        label="Status"
        chips={STATUS_CLASSES}
        on={filter.status}
        toggle={(id) => {
          patch({ status: toggleIn(filter.status, id) });
          onChip?.(id);
        }}
      />
    </div>
  );
}

function ChipGroup<T extends string>({
  label,
  chips,
  on,
  toggle,
}: {
  label: string;
  chips: { id: T; label: string }[];
  on: ReadonlySet<T>;
  toggle: (id: T) => void;
}) {
  return (
    <div className="chip-group">
      <span className="cg-label">{label}</span>
      {chips.map((c) => (
        <div
          key={c.id}
          className={`fchip ${on.has(c.id) ? "on" : ""}`}
          onClick={() => toggle(c.id)}
          role="checkbox"
          aria-checked={on.has(c.id)}
        >
          {c.label}
        </div>
      ))}
    </div>
  );
}
