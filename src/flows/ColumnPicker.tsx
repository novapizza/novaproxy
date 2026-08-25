import { useEffect, useRef, useState } from "react";
import { Icon } from "../icons";
import { COLUMNS, COLUMN_ORDER, DEFAULT_COLUMNS, normalizeColumns, type ColumnId } from "./columns";

/**
 * Which columns the table shows.
 *
 * A popover rather than a Settings page: the decision is made while looking at
 * the table that is too wide, and walking to another window to fix it loses the
 * thing you were looking at.
 */
export function ColumnPicker({
  columns,
  setColumns,
}: {
  columns: ColumnId[];
  setColumns: (c: ColumnId[]) => void;
}) {
  const [open, setOpen] = useState(false);
  const box = useRef<HTMLDivElement | null>(null);

  // Click-outside rather than a scrim: the point of a popover is that the table
  // behind it stays readable while you decide.
  useEffect(() => {
    if (!open) return;
    const away = (e: MouseEvent) => {
      if (!box.current?.contains(e.target as Node)) setOpen(false);
    };
    const esc = (e: KeyboardEvent) => e.key === "Escape" && setOpen(false);
    window.addEventListener("mousedown", away);
    window.addEventListener("keydown", esc);
    return () => {
      window.removeEventListener("mousedown", away);
      window.removeEventListener("keydown", esc);
    };
  }, [open]);

  const toggle = (id: ColumnId) => {
    const next = columns.includes(id) ? columns.filter((c) => c !== id) : [...columns, id];
    // Through `normalizeColumns` so the result is in display order and can never
    // lose the URL column, whatever was clicked.
    setColumns(normalizeColumns(next));
  };

  return (
    <div className="colpick" ref={box}>
      <span
        className={`cp-btn ${open ? "on" : ""}`}
        title="Choose columns"
        onClick={() => setOpen(!open)}
      >
        <Icon name="columns" size={13} />
      </span>
      {open && (
        <div className="cp-panel">
          <div className="cp-eyebrow">Columns</div>
          {COLUMN_ORDER.map((id) => {
            const on = columns.includes(id);
            const locked = id === "url";
            return (
              <div
                key={id}
                className={`cp-row ${on ? "on" : ""} ${locked ? "locked" : ""}`}
                onClick={() => !locked && toggle(id)}
                title={locked ? "The URL column cannot be hidden" : undefined}
              >
                <span className="tick">{on && <Icon name="check" size={12} />}</span>
                {COLUMNS[id].label === "#" ? "Index" : COLUMNS[id].label}
              </div>
            );
          })}
          <div className="cp-reset" onClick={() => setColumns([...DEFAULT_COLUMNS])}>
            Reset to default
          </div>
        </div>
      )}
    </div>
  );
}
