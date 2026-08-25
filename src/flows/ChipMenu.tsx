import { Icon } from "../icons";
import { usePopover } from "../usePopover";

/**
 * One filter axis as a button plus a checkbox popover.
 *
 * The axes used to be three rows of chips, which read beautifully and cost ~140px
 * of a window whose whole point is the table below it. Collapsed to buttons, the
 * label keeps doing the job the rows were there for — saying *which* axis a
 * selection belongs to — and the button states what is on, so the filter is still
 * readable without opening anything.
 *
 * The cost, stated plainly: setting one chip is two clicks where it used to be
 * one. That is the trade for one row instead of four.
 */
export function ChipMenu<T extends string>({
  label,
  chips,
  on,
  toggle,
  clear,
}: {
  label: string;
  chips: readonly { id: T; label: string }[];
  on: ReadonlySet<T>;
  toggle: (id: T) => void;
  clear: () => void;
}) {
  const pop = usePopover();

  /**
   * What the button says.
   *
   * Up to two selected labels are spelled out, because "Status: 4xx, 5xx" is the
   * whole filter and reading it beats counting it. Beyond two there is no room,
   * so it falls back to a count.
   */
  const picked = chips.filter((c) => on.has(c.id));
  const summary =
    picked.length === 0
      ? null
      : picked.length <= 2
      ? picked.map((c) => c.label).join(", ")
      : `${picked.length} of ${chips.length}`;

  return (
    <div className="chipmenu" ref={pop.ref}>
      <span
        className={`cm-btn ${picked.length > 0 ? "on" : ""}`}
        onClick={pop.toggle}
        role="button"
        aria-expanded={pop.open}
      >
        <span className="cm-label">{label}</span>
        {summary && <span className="cm-sum">{summary}</span>}
        <Icon name="chevron-down" size={12} />
      </span>

      {pop.open && (
        <div className="cm-panel">
          {chips.map((c) => (
            <div
              key={c.id}
              className={`cm-row ${on.has(c.id) ? "on" : ""}`}
              onClick={() => toggle(c.id)}
              role="checkbox"
              aria-checked={on.has(c.id)}
            >
              <span className="tick">{on.has(c.id) && <Icon name="check" size={12} />}</span>
              {c.label}
            </div>
          ))}
          {/* Only when there is something to clear: an empty axis already means
              "all of it", so the row would do nothing. */}
          {picked.length > 0 && (
            <div className="cm-clear" onClick={clear}>
              Clear {label.toLowerCase()}
            </div>
          )}
        </div>
      )}
    </div>
  );
}
